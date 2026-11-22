//! Jinja participation: roles, relation targets, control flow, variants,
//! config extraction, and recovery of broken SQL around Jinja.

mod common;

use cogitator_ir::common::{HoleKind, Materialization};
use cogitator_ir::hir::{
    Body, ExprKind, FromKind, Hir, JinjaKind, JinjaRole, RelTarget, Resolution, SelectItem,
};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::{DegradationKind, Fidelity};
use cogitator_sql_front::SchemaEnv;

use common::{build, build_in, hir_of, report, res_of};

fn role_of(hir: &Hir, needle: &str, text: &str) -> JinjaRole {
    hir.jinja
        .values()
        .find(|j| text[j.span.start as usize..j.span.end as usize].contains(needle))
        .map(|j| j.role)
        .unwrap_or_else(|| panic!("no region containing {needle}"))
}

fn base_target(hir: &Hir) -> &RelTarget {
    hir.from
        .values()
        .find_map(|f| match &f.kind {
            FromKind::Base { target, .. } => Some(target),
            _ => None,
        })
        .expect("a base relation")
}

#[test]
fn ref_and_source_become_typed_targets() {
    let hir = hir_of("select * from {{ ref('stg') }}");
    assert!(matches!(base_target(&hir), RelTarget::Model { .. }));
    let text = "select * from {{ source('s', 't') }} as a";
    let hir = hir_of(text);
    match base_target(&hir) {
        RelTarget::Source { source, table } => {
            assert_eq!(hir.syms.text(*source), "s");
            assert_eq!(hir.syms.text(*table), "t");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(role_of(&hir, "source", text), JinjaRole::Blank);
}

#[test]
fn versioned_and_packaged_refs() {
    let hir = hir_of("select 1 from {{ ref('pkg', 'm', v=3) }}");
    let RelTarget::Model {
        package,
        name,
        version,
    } = base_target(&hir)
    else {
        panic!("model");
    };
    assert_eq!(package.map(|p| hir.syms.text(p)), Some("pkg"));
    assert_eq!(hir.syms.text(*name), "m");
    assert_eq!(version.map(|v| hir.syms.text(v)), Some("3"));
}

#[test]
fn this_and_dynamic_relations() {
    let hir = hir_of("select * from {{ this }}");
    assert!(matches!(base_target(&hir), RelTarget::This));
    let text = "select * from {{ ref(var('x')) }}";
    let hir = hir_of(text);
    assert!(matches!(base_target(&hir), RelTarget::Jinja(_)));
    assert_eq!(role_of(&hir, "ref", text), JinjaRole::Relation);
}

#[test]
fn macro_in_from_is_a_relation_region() {
    let text = "select * from {{ my_rel() }} as x";
    let hir = hir_of(text);
    assert!(matches!(base_target(&hir), RelTarget::Jinja(_)));
    assert_eq!(role_of(&hir, "my_rel", text), JinjaRole::Relation);
}

#[test]
fn scalar_positions_consume_as_expressions() {
    let text = "select {{ var('v') }} as a, x + {{ m() }} from t where y = {{ n() }}";
    let hir = hir_of(text);
    assert_eq!(role_of(&hir, "var", text), JinjaRole::Scalar);
    assert_eq!(role_of(&hir, "m()", text), JinjaRole::Scalar);
    assert_eq!(role_of(&hir, "n()", text), JinjaRole::Scalar);
}

#[test]
fn a_lone_macro_select_item_is_composite() {
    let text = "select {{ dbt_utils.star(x) }} from t";
    let hir = hir_of(text);
    assert_eq!(role_of(&hir, "star", text), JinjaRole::Composite);
    let root = &hir.queries[hir.root.expect("root")];
    let Body::Select(select) = &root.body else {
        panic!("select")
    };
    assert!(matches!(select.items[0], SelectItem::Jinja(_)));
    assert!(root.outputs.is_empty());
}

#[test]
fn clause_position_after_from_item() {
    let text = "select a from t {{ limit_macro() }}";
    let hir = hir_of(text);
    assert_eq!(
        role_of(&hir, "limit_macro", text),
        JinjaRole::ClausePosition
    );
    assert!(matches!(res_of(&hir, "a"), Resolution::OpenSchema(_)));
    let FromKind::Base { alias, .. } = &hir.from.values().next().expect("from").kind else {
        panic!("base")
    };
    assert!(alias.is_none(), "the macro must not parse as an alias");
}

#[test]
fn config_is_blank_and_read() {
    let text = "{{ config(materialized='incremental', unique_key='id') }}\nselect 1 as x";
    let hir = hir_of(text);
    assert_eq!(role_of(&hir, "config", text), JinjaRole::Blank);
    assert_eq!(hir.config.materialized, Some(Materialization::Incremental));
    assert_eq!(hir.config.unique_key.len(), 1);
    assert_eq!(hir.syms.text(hir.config.unique_key[0]), "id");
}

#[test]
fn string_and_sql_comment_jinja_is_blank() {
    let text = "select '{{ x }}' as a -- {{ y }}\nfrom t";
    let hir = hir_of(text);
    assert_eq!(role_of(&hir, "x }}", text), JinjaRole::Blank);
    assert_eq!(role_of(&hir, "y }}", text), JinjaRole::Blank);
}

#[test]
fn pasted_jinja_is_consumed_once() {
    let text = "select 1 from analytics_{{ var('s') }}.orders";
    let hir = hir_of(text);
    assert!(matches!(base_target(&hir), RelTarget::Jinja(_)));
    assert_eq!(role_of(&hir, "var", text), JinjaRole::Relation);
    let text = "select {{ a() }}_{{ b() }} from t";
    let lowered = build(text);
    lowered.value.expect("hir");
}

#[test]
fn jinja_as_alias_and_column_in_path() {
    let text = "select t.{{ c() }}, x as {{ n() }} from t";
    let hir = hir_of(text);
    assert_eq!(role_of(&hir, "c()", text), JinjaRole::Scalar);
    assert_eq!(role_of(&hir, "n()", text), JinjaRole::Blank);
}

#[test]
fn a_whole_model_macro_is_a_jinja_hole() {
    let text = "{{ dbt_utils.date_spine(a, b) }}";
    let lowered = build(text);
    assert_eq!(lowered.fidelity, Fidelity::Partial);
    let hir = lowered.value.expect("hir");
    let root = &hir.queries[hir.root.expect("root")];
    assert!(matches!(root.body, Body::Hole(HoleKind::Jinja)));
    assert_eq!(
        hir.jinja.values().next().expect("node").role,
        JinjaRole::ClausePosition
    );
}

#[test]
fn incremental_variants_lower_both_ways() {
    let text = "select a from {{ ref('m') }}\n{% if is_incremental() %}\nwhere a > (select max(a) from {{ this }})\n{% endif %}";
    let env = SchemaEnv::default();
    let inc = build_in(text, "duckdb", TemplateVariant::Incremental, &env);
    let inc_hir = inc.value.as_ref().expect("hir");
    let Body::Select(select) = &inc_hir.queries[inc_hir.root.expect("root")].body else {
        panic!("select");
    };
    assert!(select.filter.is_some());
    assert!(matches!(
        inc_hir.origin,
        SourceOrigin::Template {
            variant: TemplateVariant::Incremental
        }
    ));
    let tags = inc_hir
        .jinja
        .values()
        .filter(|j| j.kind == JinjaKind::IsIncremental);
    assert_eq!(tags.count(), 2);

    let full = build_in(text, "duckdb", TemplateVariant::FullRefresh, &env);
    let full_hir = full.value.as_ref().expect("hir");
    let Body::Select(select) = &full_hir.queries[full_hir.root.expect("root")].body else {
        panic!("select");
    };
    assert!(select.filter.is_none());
    assert!(full_hir.jinja.values().all(|j| j.role == JinjaRole::Blank));
    insta::assert_snapshot!("incremental_variant", report(text, &inc));
    insta::assert_snapshot!("full_refresh_variant", report(text, &full));
}

#[test]
fn other_control_flow_declines_with_a_reason() {
    let lowered = build("{% for x in y %}select {{ x }}{% endfor %}");
    assert_eq!(lowered.fidelity, Fidelity::Declined);
    assert!(lowered.value.is_none());
    assert_eq!(lowered.degraded[0].kind, DegradationKind::JinjaControlFlow);
}

#[test]
fn broken_sql_keeps_good_parts() {
    let text = "with a as (select x from {{ ref('m') }}), b as (select from where), c as (select 1 as y from a) select y from c";
    let lowered = build(text);
    assert_eq!(lowered.fidelity, Fidelity::Partial);
    let hir = lowered.value.as_ref().expect("hir");
    let holes = hir
        .queries
        .values()
        .filter(|q| matches!(q.body, Body::Hole(HoleKind::ParseError)))
        .count();
    assert_eq!(holes, 1);
    assert!(matches!(res_of(hir, "y"), Resolution::Bound { .. }));
    let hole = hir
        .queries
        .values()
        .find(|q| matches!(q.body, Body::Hole(HoleKind::ParseError)))
        .expect("hole");
    assert_eq!(
        &text[hole.span.start as usize..hole.span.end as usize],
        "select from where"
    );
    assert!(lowered.degraded[0].span.is_some());
    // The ref inside the good CTE is still typed.
    assert!(matches!(base_target(hir), RelTarget::Model { .. }));
    insta::assert_snapshot!(report(text, &lowered));
}

#[test]
fn jinja_inside_a_hole_is_blank() {
    let text = "with a as (select {{ m() }} from t where) select 1 as x";
    let lowered = build(text);
    assert_eq!(lowered.fidelity, Fidelity::Partial);
    let hir = lowered.value.expect("hir");
    assert_eq!(role_of(&hir, "m()", text), JinjaRole::Blank);
}

#[test]
fn malformed_jinja_is_a_partial_hole() {
    let lowered = build("select {{ broken");
    assert_eq!(lowered.fidelity, Fidelity::Partial);
    let hir = lowered.value.expect("hir");
    assert!(matches!(
        hir.queries[hir.root.expect("root")].body,
        Body::Hole(HoleKind::Jinja)
    ));
}

#[test]
fn jinja_comments_are_blank_nodes() {
    let text = "{# note #}\nselect 1 as x";
    let hir = hir_of(text);
    assert_eq!(role_of(&hir, "note", text), JinjaRole::Blank);
    assert_eq!(
        hir.jinja.values().next().expect("node").kind,
        JinjaKind::Comment
    );
}

#[test]
fn newlines_in_jinja_keep_later_columns_right() {
    let text = "select {{\n  m()\n}} as a,\n  b\nfrom t";
    let hir = hir_of(text);
    let b = hir
        .cols
        .values()
        .find(|c| hir.syms.text(c.name.folded) == "b")
        .expect("b");
    assert_eq!(&text[b.span.start as usize..b.span.end as usize], "b");
}

#[test]
fn multibyte_text_keeps_spans_on_boundaries() {
    let text = "select 'é' as a, {{ m('é') }} as é2, é3 from t";
    let hir = hir_of(text);
    let c = hir.cols.values().next().expect("col");
    assert_eq!(&text[c.span.start as usize..c.span.end as usize], "é3");
    let root = &hir.queries[hir.root.expect("root")];
    let Body::Select(select) = &root.body else {
        panic!("select")
    };
    assert!(matches!(
        hir.exprs[match &select.items[1] {
            SelectItem::Expr { expr, .. } => *expr,
            other => panic!("{other:?}"),
        }]
        .kind,
        ExprKind::Jinja(_)
    ));
}
