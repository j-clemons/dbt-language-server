//! Name-resolution cases: one test per construct the binder must handle.

mod common;

use cogitator_ir::hir::{Body, FromKind, RelTarget, Resolution, SelectItem};
use cogitator_ir::origin::TemplateVariant;
use cogitator_sql_front::SchemaEnv;

use common::{all_res_of, build, build_in, env_with, hir_of, report, res_of};

fn is_bound(res: &Resolution) -> bool {
    matches!(res, Resolution::Bound { .. })
}

#[test]
fn cte_columns_bind() {
    let hir = hir_of("with a as (select 1 as x, 2 as y) select x, a.y from a");
    assert!(is_bound(&res_of(&hir, "x")));
    assert!(is_bound(&res_of(&hir, "y")));
}

#[test]
fn nested_with_shadows_outer_cte() {
    let hir = hir_of(
        "with a as (select 1 as x) \
         select x from (with a as (select 2 as y) select y from a) as t, a",
    );
    let refs = all_res_of(&hir, "y");
    assert!(is_bound(&refs[0]));
    assert!(is_bound(&res_of(&hir, "x")));
}

#[test]
fn nested_cte_inside_cte_body() {
    let hir =
        hir_of("with a as (with inner_a as (select 1 as v) select v from inner_a) select v from a");
    for res in all_res_of(&hir, "v") {
        assert!(is_bound(&res), "{res:?}");
    }
}

#[test]
fn derived_table_alias_columns() {
    let hir = hir_of("select t.a from (select 1 as x) as t(a)");
    assert!(is_bound(&res_of(&hir, "a")));
}

#[test]
fn unnest_exposes_alias_and_correlates() {
    let hir = hir_of("with t as (select [1] as arr) select v from t, unnest(t.arr) as u(v)");
    assert!(is_bound(&res_of(&hir, "v")));
    assert!(all_res_of(&hir, "arr").iter().all(is_bound));
    // Beside an open relation the element column is only a candidate.
    let hir = hir_of("select v from t, unnest(t.arr) as u(v)");
    assert!(matches!(res_of(&hir, "v"), Resolution::Candidates(c) if c.len() == 2));
    let hir = hir_of("select a.x from t as a, lateral (select a.x as y) as s");
    assert!(matches!(res_of(&hir, "x"), Resolution::OpenSchema(_)));
}

#[test]
fn table_function_columns_from_alias() {
    let hir = hir_of("select g from generate_series(1, 3) as s(g)");
    assert!(is_bound(&res_of(&hir, "g")));
}

#[test]
fn set_ops_keep_left_names_and_order_by_outputs() {
    let hir = hir_of("select 1 as a union all select 2 as a order by a");
    let root = hir.root.expect("root");
    assert!(matches!(hir.queries[root].body, Body::SetOp { .. }));
    let refs = all_res_of(&hir, "a");
    assert!(matches!(
        refs.last().expect("order ref"),
        Resolution::LateralAlias(0)
    ));
}

#[test]
fn order_by_prefers_alias_over_input() {
    let hir = hir_of("select x + 1 as y, y as z from t order by y");
    assert!(all_res_of(&hir, "y")
        .iter()
        .any(|r| matches!(r, Resolution::LateralAlias(0))));
}

#[test]
fn window_over_and_filter_bind() {
    let hir = hir_of("select sum(a) filter (where b > 0) over (partition by c order by d) from t");
    for name in ["a", "b", "c", "d"] {
        assert!(
            matches!(res_of(&hir, name), Resolution::OpenSchema(_)),
            "{name}"
        );
    }
}

#[test]
fn lateral_alias_is_dialect_gated() {
    let duck = hir_of("select 1 as a, a + 1 as b from (select 2 as z) t");
    assert_eq!(res_of(&duck, "a"), Resolution::LateralAlias(0));
    // An open input may have the column, and input columns win.
    let open = hir_of("select 1 as a, a + 1 as b from t");
    assert!(matches!(res_of(&open, "a"), Resolution::OpenSchema(_)));
    let env = SchemaEnv::default();
    let bq = build_in(
        "select 1 as a, a + 1 as b from (select 2 as z) t",
        "bigquery",
        TemplateVariant::Plain,
        &env,
    );
    let hir = bq.value.expect("hir");
    assert_eq!(res_of(&hir, "a"), Resolution::Unresolved);
}

#[test]
fn star_exclude_and_replace() {
    let hir = hir_of("with a as (select 1 as x, 2 as y) select * exclude (y) from a");
    let root = hir.root.expect("root");
    assert_eq!(hir.queries[root].outputs.len(), 1);
    let hir = hir_of("with a as (select 1 as x) select * replace (x + 1 as x) from a");
    let Body::Select(select) = &hir.queries[hir.root.expect("root")].body else {
        panic!("select");
    };
    let SelectItem::Star { replace, .. } = &select.items[0] else {
        panic!("star");
    };
    assert_eq!(replace.len(), 1);
}

#[test]
fn qualified_star_resolves_its_from() {
    let hir = hir_of("with a as (select 1 as x) select a.* from a");
    let Body::Select(select) = &hir.queries[hir.root.expect("root")].body else {
        panic!("select");
    };
    assert!(matches!(
        &select.items[0],
        SelectItem::Star { from: Some(_), .. }
    ));
}

#[test]
fn correlated_subquery_binds_outer() {
    let hir = hir_of(
        "with a as (select 1 as id), b as (select 1 as id2, 2 as v) \
         select (select max(v) from b where b.id2 = a.id) from a",
    );
    assert!(matches!(
        res_of(&hir, "id"),
        Resolution::Outer { depth: 1, .. }
    ));
}

#[test]
fn exists_correlation() {
    let hir = hir_of(
        "with a as (select 1 as k), b as (select 1 as k2) \
         select 1 from a where exists (select 1 from b where b.k2 = k)",
    );
    assert!(all_res_of(&hir, "k")
        .iter()
        .any(|r| matches!(r, Resolution::Outer { depth: 1, .. })));
}

#[test]
fn cte_body_cannot_see_outer_from() {
    let hir = hir_of("with a as (select missing from b) select 1 from (select 1 as missing) x, a");
    assert_eq!(
        res_of(&hir, "missing"),
        Resolution::OpenSchema(_b_item(&hir))
    );
}

fn _b_item(hir: &cogitator_ir::hir::Hir) -> cogitator_ir::hir::FromId {
    hir.from
        .iter()
        .find(|(_, f)| {
            matches!(&f.kind, FromKind::Base { target: RelTarget::Physical(p), .. }
                if hir.syms.text(p[0].folded) == "b")
        })
        .map(|(id, _)| id)
        .expect("physical b")
}

#[test]
fn folding_per_dialect() {
    let env = SchemaEnv::default();
    let snowflake = build_in(
        "with a as (select 1 as Foo) select foo, FOO, \"Foo\" from a",
        "snowflake",
        TemplateVariant::Plain,
        &env,
    );
    let hir = snowflake.value.expect("hir");
    let refs = all_res_of(&hir, "FOO");
    assert_eq!(refs.len(), 2, "the quoted reference keeps its case");
    assert!(is_bound(&refs[0]) && is_bound(&refs[1]));
    assert_eq!(res_of(&hir, "Foo"), Resolution::Unresolved);
}

#[test]
fn recursive_cte_references_itself() {
    let text = "with recursive r as (select 1 as n union all select n + 1 from r where n < 5) select n from r";
    let hir = hir_of(text);
    let cte = hir.ctes.values().next().expect("cte");
    assert!(cte.recursive);
    let self_ref = hir
        .from
        .values()
        .find(|f| {
            matches!(
                &f.kind,
                FromKind::Base {
                    target: RelTarget::Cte(_),
                    ..
                }
            )
        })
        .expect("self reference");
    assert!(self_ref.open, "the body is not known at its own reference");
    insta::assert_snapshot!(report(text, &build(text)));
}

#[test]
fn with_recursive_only_marks_actual_recursion() {
    let hir = hir_of("with recursive a as (select 1 as n) select n from a");
    assert!(!hir.ctes.values().next().expect("cte").recursive);
}

#[test]
fn closed_schema_reports_unresolved_columns() {
    let env = env_with("orders", &["id"], true);
    let lowered = build_in(
        "select id, nope from {{ ref('orders') }}",
        "duckdb",
        TemplateVariant::Plain,
        &env,
    );
    let hir = lowered.value.expect("hir");
    assert!(is_bound(&res_of(&hir, "id")));
    assert_eq!(res_of(&hir, "nope"), Resolution::Unresolved);
}

#[test]
fn open_schema_binds_quietly() {
    let env = env_with("orders", &["id"], false);
    let text = "select id, nope from {{ ref('orders') }}";
    let lowered = build_in(text, "duckdb", TemplateVariant::Plain, &env);
    assert!(lowered.degraded.is_empty(), "{:?}", lowered.degraded);
    let hir = lowered.value.expect("hir");
    assert!(is_bound(&res_of(&hir, "id")));
    assert!(matches!(res_of(&hir, "nope"), Resolution::OpenSchema(_)));
}

#[test]
fn ambiguity_lists_candidates() {
    let hir =
        hir_of("with a as (select 1 as x), b as (select 2 as x) select x from a join b on true");
    assert!(matches!(res_of(&hir, "x"), Resolution::Candidates(v) if v.len() == 2));
}

#[test]
fn using_merges_the_column() {
    let hir =
        hir_of("with a as (select 1 as x), b as (select 2 as x) select x from a join b using (x)");
    assert!(is_bound(&res_of(&hir, "x")));
}

#[test]
fn struct_field_binds_the_head_column() {
    let hir = hir_of("with a as (select 1 as s) select s.field from a");
    assert!(is_bound(&res_of(&hir, "s")));
}

#[test]
fn lambda_parameters_are_not_columns() {
    let hir = hir_of("select list_transform([1,2], x -> x + 1) from t");
    assert!(all_res_of(&hir, "x").is_empty());
}

#[test]
fn unknown_expressions_keep_children_as_opaque() {
    let hir = hir_of("with a as (select [1] as arr, 2 as i) select arr[i] from a");
    assert!(is_bound(&res_of(&hir, "arr")));
    assert!(is_bound(&res_of(&hir, "i")));
}
