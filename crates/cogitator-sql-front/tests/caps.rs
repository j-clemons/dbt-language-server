//! Caps degrade explicitly and nothing overflows the stack. These run on
//! test threads with the default 2 MiB stack on purpose.

mod common;

use cogitator_ir::common::HoleKind;
use cogitator_ir::hir::{Body, ExprKind, Hir, SelectItem};
use cogitator_ir::origin::TemplateVariant;
use cogitator_ir::{DegradationKind, Fidelity};
use cogitator_sql_front::{parse_sql_type, SchemaEnv};

use common::{build, build_in, hir_of};

fn root_hole(hir: &Hir) -> Option<HoleKind> {
    match hir.queries[hir.root.expect("root")].body {
        Body::Hole(kind) => Some(kind),
        _ => None,
    }
}

#[test]
fn text_cap() {
    let text = format!("select 1 -- {}", "x".repeat(600 * 1024));
    let lowered = build(&text);
    assert_eq!(lowered.fidelity, Fidelity::Partial);
    assert_eq!(lowered.degraded[0].kind, DegradationKind::CapExceeded);
    assert_eq!(
        root_hole(lowered.value.as_ref().expect("hir")),
        Some(HoleKind::Truncated)
    );
}

#[test]
fn token_cap() {
    let text = format!("select {}", vec!["1"; 41_000].join(", "));
    let lowered = build(&text);
    assert_eq!(lowered.degraded[0].kind, DegradationKind::CapExceeded);
    assert_eq!(
        root_hole(lowered.value.as_ref().expect("hir")),
        Some(HoleKind::Truncated)
    );
}

#[test]
fn deep_left_chain_degrades_without_overflow() {
    let chain = vec!["a"; 100_000].join("+");
    let text = format!("select {chain} from t");
    let lowered = build(&text[..text.len().min(500_000)]);
    assert!(lowered
        .degraded
        .iter()
        .any(|d| d.kind == DegradationKind::CapExceeded));
}

#[test]
fn chain_under_the_cap_lowers() {
    let chain = vec!["a"; 200].join(" + ");
    let hir = hir_of(&format!("select {chain} from t"));
    assert_eq!(hir.cols.len(), 200);
}

#[test]
fn chain_at_the_cap_fits_the_default_stack() {
    let chain = vec!["a"; 257].join(" + ");
    let hir = hir_of(&format!("select {chain} from t"));
    assert_eq!(hir.cols.len(), 257);
}

#[test]
fn chain_just_over_the_cap_degrades() {
    let chain = vec!["a"; 258].join(" + ");
    let lowered = build(&format!("select {chain} from t"));
    assert!(lowered
        .degraded
        .iter()
        .any(|d| d.kind == DegradationKind::CapExceeded));
}

#[test]
fn deep_parens_become_a_hole() {
    let text = format!("select {}a{} from t", "(".repeat(5000), ")".repeat(5000));
    let lowered = build(&text);
    assert_eq!(lowered.fidelity, Fidelity::Partial);
}

#[test]
fn deep_case_nesting_does_not_overflow() {
    let text = format!(
        "select {} 1 {} from t",
        "case when a then ".repeat(400),
        " end".repeat(400)
    );
    let lowered = build(&text);
    assert_eq!(lowered.fidelity, Fidelity::Partial);
}

#[test]
fn long_union_chain_degrades() {
    let text = vec!["select 1"; 2000].join(" union all ");
    let lowered = build(&text);
    assert!(lowered
        .degraded
        .iter()
        .any(|d| d.kind == DegradationKind::CapExceeded));
}

#[test]
fn star_expansion_bomb_hits_the_node_cap() {
    let mut text = String::from("with c0 as (select 1 as a0, 2 as a1, 3 as a2, 4 as a3)");
    for i in 1..40 {
        text.push_str(&format!(
            ", c{i} as (select * from c{p} l, c{p} r)",
            p = i - 1
        ));
    }
    text.push_str(" select * from c39");
    let lowered = build(&text);
    assert!(lowered
        .degraded
        .iter()
        .any(|d| d.kind == DegradationKind::CapExceeded));
}

#[test]
fn deep_derived_nesting_stays_within_limits() {
    let text = format!(
        "select * from {}t{} x",
        "(select * from ".repeat(200),
        ") a".repeat(200)
    );
    let lowered = build(&text);
    lowered.assert_clean();
}

#[test]
fn hostile_bytes_do_not_panic() {
    for text in [
        "\u{0}select\u{1}",
        "select '",
        "select \"",
        "/* unterminated",
        "{{",
        "{% if",
        "{#",
        "select {{ 'é' }}\u{2028}",
        "with",
        "with a",
        "with a as",
        "with a as (",
        "with a as ()",
        "with a(b",
        ";;;",
        "select 1;;select 2",
        "",
    ] {
        build(text);
    }
}

#[test]
fn every_dialect_handles_the_same_edge_cases() {
    for dialect in [
        "snowflake",
        "bigquery",
        "postgres",
        "duckdb",
        "redshift",
        "",
    ] {
        build_in(
            "select a::int, b from t",
            dialect,
            TemplateVariant::Plain,
            &SchemaEnv::default(),
        );
        build_in(
            "select `a` from t",
            dialect,
            TemplateVariant::Plain,
            &SchemaEnv::default(),
        );
        let _ = parse_sql_type(
            "array<struct<a int64>>",
            &cogitator_dialects::Dialect::new(dialect),
        );
    }
}

#[test]
fn opaque_expressions_are_not_lost() {
    let hir = hir_of("select extract(year from d) from t");
    let Body::Select(select) = &hir.queries[hir.root.expect("root")].body else {
        panic!("select");
    };
    let SelectItem::Expr { expr, .. } = &select.items[0] else {
        panic!("item");
    };
    let ExprKind::Opaque { children } = &hir.exprs[*expr].kind else {
        panic!("opaque: {:?}", hir.exprs[*expr].kind);
    };
    assert_eq!(children.len(), 1);
}
