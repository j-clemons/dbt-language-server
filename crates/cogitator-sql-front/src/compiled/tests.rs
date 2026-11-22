use cogitator_dialects::Dialect;
use cogitator_ir::hir::{print_hir, FromKind, Hir, RelTarget};
use cogitator_ir::origin::{Mapped, SourceOrigin, SpanMap, Staleness};
use cogitator_ir::{Fidelity, Span};

use super::*;
use crate::{HirInput, SchemaEnv};

fn duck() -> Dialect {
    Dialect::new("duckdb")
}

fn aligned(template: &str, compiled: &str, relations: &RelationTable) -> Alignment {
    let prepared = prepare(compiled, &duck(), relations, Some("m")).expect("prepared");
    align(template, &prepared, compiled)
}

fn plain(template: &str, compiled: &str) -> Alignment {
    aligned(template, compiled, &RelationTable::new(&duck()))
}

fn span_of(text: &str, needle: &str) -> Span {
    let start = text.find(needle).expect("needle") as u32;
    Span::new(start, start + needle.len() as u32)
}

fn hir(compiled: &str, relations: &RelationTable) -> Hir {
    let dialect = duck();
    let prepared = prepare(compiled, &dialect, relations, Some("m")).expect("prepared");
    let input = HirInput {
        text: compiled,
        dialect: &dialect,
        origin: SourceOrigin::Compiled {
            staleness: Staleness::Fresh,
            map: SpanMap::default(),
        },
        config: Default::default(),
    };
    let lowered = build_compiled_hir(&input, &SchemaEnv::default(), &prepared);
    assert_eq!(lowered.fidelity, Fidelity::Complete, "{lowered:?}");
    lowered.value.expect("hir")
}

fn targets(hir: &Hir) -> Vec<String> {
    hir.from
        .iter()
        .filter_map(|(_, from)| match &from.kind {
            FromKind::Base { target, .. } => Some(match target {
                RelTarget::Model { name, .. } => format!("model:{}", hir.syms.text(*name)),
                RelTarget::Source { source, table } => {
                    format!(
                        "source:{}.{}",
                        hir.syms.text(*source),
                        hir.syms.text(*table)
                    )
                }
                RelTarget::This => "this".to_string(),
                RelTarget::Cte(_) => "cte".to_string(),
                RelTarget::Physical(_) => "physical".to_string(),
                RelTarget::Jinja(_) => "jinja".to_string(),
            }),
            _ => None,
        })
        .collect()
}

#[test]
fn trimmed_whitespace_still_anchors_and_translates_exactly() {
    // `{%-` removes the newline and indent before `from`.
    let template = "select a, b\n    {%- if x %}\n    , c\n    {%- endif %}\nfrom t where a > 1\n";
    let compiled = "select a, b\n    , c\nfrom t where a > 1\n";
    let result = plain(template, compiled);
    assert_eq!(result.literal_coverage, 10_000, "{result:?}");
    let from = span_of(compiled, "from t");
    assert_eq!(
        result.map.map(from),
        Mapped::Exact(span_of(template, "from t"))
    );
    let c = span_of(compiled, "c");
    assert_eq!(result.map.map(c), Mapped::Exact(span_of(template, "c")));
}

#[test]
fn inner_whitespace_differences_split_the_anchor_without_losing_offsets() {
    let template = "select a,\r\n  b\nfrom t\n{%- if x %}{% endif %}";
    let compiled = "select a,\n  b\nfrom t\n";
    let result = plain(template, compiled);
    let b = span_of(compiled, "b");
    assert_eq!(result.map.map(b), Mapped::Exact(span_of(template, "b")));
    let a = span_of(compiled, "a,");
    assert_eq!(result.map.map(a), Mapped::Exact(span_of(template, "a,")));
    // a span crossing the differing whitespace has no exact image
    assert_eq!(result.map.map(Span::new(a.start, b.end)), Mapped::Unmapped);
}

#[test]
fn loop_output_maps_to_the_loop_and_never_exactly() {
    let template = "select\n  {% for c in cols %}\n  sum({{ c }}) as {{ c }}_sum,\n  {% endfor %}\n  count(*) as n\nfrom t";
    let compiled =
        "select\n  \n  sum(a) as a_sum,\n  \n  sum(b) as b_sum,\n  \n  count(*) as n\nfrom t";
    let result = plain(template, compiled);
    assert_eq!(result.literal_coverage, 10_000, "{result:?}");
    let in_loop = span_of(compiled, "sum(b) as b_sum");
    match result.map.map(in_loop) {
        Mapped::InJinja(region) => {
            assert!(
                template[region.start as usize..region.end as usize].contains("{% for"),
                "{region:?}"
            );
            assert!(template[region.start as usize..region.end as usize].contains("{% endfor"));
        }
        other => panic!("loop body must not map exactly: {other:?}"),
    }
    let first_iteration = span_of(compiled, "sum(a)");
    assert!(matches!(
        result.map.map(first_iteration),
        Mapped::InJinja(_)
    ));
    assert!(matches!(
        result.map.map(span_of(compiled, "count(*)")),
        Mapped::Exact(_)
    ));
}

#[test]
fn an_untaken_branch_is_skipped_even_when_its_text_appears_later() {
    let template = "select a{% if x %}, b from t where z{% else %}\n{% endif %} from t where z";
    // the else branch ran; the then-branch's text also occurs elsewhere
    let compiled = "select a\n from t where z";
    let result = plain(template, compiled);
    assert_eq!(result.literal_coverage, 10_000, "{result:?}");
    let from = span_of(compiled, "from t");
    assert_eq!(
        result.map.map(from),
        Mapped::Exact(Span::new(
            template.rfind("from t").expect("from") as u32,
            template.rfind("from t").expect("from") as u32 + 6
        ))
    );
}

#[test]
fn set_blocks_and_macros_are_not_expected_in_compiled_output() {
    let template =
        "{% set q %}select 1{% endset %}{% macro m() %}junk{% endmacro %}select a from t";
    let compiled = "select a from t";
    let result = plain(template, compiled);
    assert_eq!(result.literal_coverage, 10_000, "{result:?}");
    assert_eq!(result.map.coverage, 10_000);
}

#[test]
fn edits_the_compile_has_not_seen_lower_literal_coverage() {
    let template = "select a, b from t {% if x %}{% endif %} where a > 1";
    let compiled = "select a, c from t where a > 1";
    let result = plain(template, compiled);
    assert!(result.literal_coverage < MIN_LITERAL_COVERAGE, "{result:?}");
}

#[test]
fn a_model_of_only_macros_counts_as_covered() {
    let result = plain("{{ m() }}", "select a from t");
    assert_eq!(result.literal_coverage, 10_000);
    assert_eq!(result.map.coverage, 0);
    assert!(matches!(
        result.map.map(span_of("select a from t", "select")),
        Mapped::InJinja(_)
    ));
}

#[test]
fn injected_ephemeral_ctes_are_blanked_and_become_refs() {
    let template = "with orders as (select * from {{ ref('eph') }}) select * from orders";
    let compiled = "with __dbt__cte__eph as (\n  select 1 as id\n), orders as (select * from __dbt__cte__eph) select * from orders";
    let relations = RelationTable::new(&duck());
    let built = hir(compiled, &relations);
    assert_eq!(
        targets(&built),
        vec!["model:eph", "cte"],
        "{}",
        print_hir(&built)
    );

    let result = aligned(template, compiled, &relations);
    assert_eq!(result.literal_coverage, 10_000, "{result:?}");
    let ref_target = span_of(compiled, "from __dbt__cte__eph");
    assert_eq!(
        result
            .map
            .map(Span::new(ref_target.start + 5, ref_target.end)),
        Mapped::InJinja(span_of(template, "{{ ref('eph') }}"))
    );
}

#[test]
fn a_lone_injected_cte_takes_its_with_along() {
    let template = "select * from {{ ref('eph') }}";
    let compiled = "with __dbt__cte__eph as (select 1 as id) select * from __dbt__cte__eph";
    let relations = RelationTable::new(&duck());
    let built = hir(compiled, &relations);
    assert_eq!(targets(&built), vec!["model:eph"], "{}", print_hir(&built));
    assert_eq!(
        aligned(template, compiled, &relations).literal_coverage,
        10_000
    );
}

fn manifest_relations() -> RelationTable {
    let mut relations = RelationTable::new(&duck());
    relations.add_model("orders", Some("\"db\".\"main\".\"orders\""));
    relations.add_model("stg", None);
    relations.add_source("raw", "events", Some("raw_schema"), Some("events_v2"));
    relations
}

#[test]
fn physical_relations_map_to_refs_and_sources() {
    let relations = manifest_relations();
    let built = hir(
        "select * from \"db\".\"main\".\"orders\" o, stg as t join raw_schema.events_v2 e on o.id = e.id join stg s on s.id = o.id",
        &relations,
    );
    assert_eq!(
        targets(&built),
        vec![
            "model:orders",
            "model:stg",
            "source:raw.events",
            "model:stg"
        ],
        "{}",
        print_hir(&built)
    );
}

#[test]
fn snowflake_case_folds_when_comparing_relation_names() {
    let dialect = Dialect::new("snowflake");
    let mut relations = RelationTable::new(&dialect);
    relations.add_model("orders", Some("DB.MAIN.ORDERS"));
    let compiled = "select * from db.main.orders";
    let prepared = prepare(compiled, &dialect, &relations, None).expect("prepared");
    assert_eq!(prepared.regions().len(), 1);
}

#[test]
fn a_cte_shadowing_a_model_stays_a_cte() {
    let relations = manifest_relations();
    let built = hir(
        "with stg as (select 1 as id) select id from stg",
        &relations,
    );
    assert_eq!(targets(&built), vec!["cte"], "{}", print_hir(&built));
}

#[test]
fn the_models_own_relation_is_this() {
    let mut relations = RelationTable::new(&duck());
    relations.add_model("m", None);
    let built = hir("select * from m", &relations);
    assert_eq!(targets(&built), vec!["this"], "{}", print_hir(&built));
}

#[test]
fn a_physical_name_that_is_not_a_relation_stays_physical() {
    let relations = manifest_relations();
    let built = hir("select * from elsewhere.t", &relations);
    assert_eq!(targets(&built), vec!["physical"], "{}", print_hir(&built));
}

#[test]
fn a_table_function_is_not_a_relation() {
    let mut relations = RelationTable::new(&duck());
    relations.add_model("read_csv", None);
    let prepared = prepare("select * from read_csv('x')", &duck(), &relations, None).expect("ok");
    assert!(prepared.regions().is_empty());
}

#[test]
fn oversized_compiled_text_is_reported_not_lexed() {
    let compiled = "x".repeat(600 * 1024);
    let err =
        prepare(&compiled, &duck(), &RelationTable::new(&duck()), None).expect_err("over the cap");
    assert!(err.contains("limit"), "{err}");
}
