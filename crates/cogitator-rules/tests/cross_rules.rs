//! `cross.unused_column`, `cross.filter_late`, `cross.ephemeral_repeat`
//! and `cross.duplicate_logic` through the real front end and pass
//! manager, with hand-written `DagFacts`.

#[allow(dead_code)] // shared with the other rule test crates
mod join_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use cogitator_ir::common::Materialization;
use cogitator_ir::schema::ProjectSchemas;
use cogitator_rules::rules::cross::subplan::subplans_of;
use cogitator_rules::{ColumnFilter, ConsumerFacts, DagFacts, Finding, Need, Needs, SharedSubplan};
use join_support::{model, run_raw_with, Extras};

fn needs() -> Needs {
    Needs::NONE
        .with(Need::Schema)
        .with(Need::Dag)
        .with(Need::Mat)
}

fn up() -> ProjectSchemas {
    let mut out = ProjectSchemas::default();
    for name in ["up", "other"] {
        out.models.insert(
            Arc::from(name),
            model(&[("a", false), ("b", false), ("c", false), ("d", false)]),
        );
    }
    out
}

fn run_rule(
    rule: &str,
    sql: &str,
    mat: Option<Materialization>,
    facts: Option<&DagFacts>,
    available: Needs,
) -> Vec<Finding> {
    run_raw_with(
        sql,
        up(),
        available,
        Extras {
            mat,
            dag: facts,
            no_text: false,
        },
    )
    .findings
    .into_iter()
    .filter(|f| f.id.as_str() == rule)
    .collect()
}

fn run(rule: &str, sql: &str, mat: Materialization, facts: &DagFacts) -> Vec<Finding> {
    run_rule(rule, sql, Some(mat), Some(facts), needs())
}

fn downstream(columns: &[&str]) -> DagFacts {
    DagFacts {
        downstream_columns: BTreeMap::from([(
            Arc::from("orders"),
            columns.iter().map(|c| Arc::from(*c)).collect(),
        )]),
        ..DagFacts::default()
    }
}

const UNUSED: &str = "cross.unused_column";
const NAMED: &str = "select a, b, c from {{ ref('up') }}";

#[test]
fn a_stored_column_nothing_reads_is_named() {
    let found = run(
        UNUSED,
        NAMED,
        Materialization::Table,
        &downstream(&["a", "b"]),
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("`c`"), "{}", found[0].message);
    assert!(found[0].message.contains("in this project"));
    assert!(found[0].fixes.is_empty(), "no autofix");
}

#[test]
fn a_column_read_or_tested_is_not_unused() {
    let facts = DagFacts {
        tested_columns: vec![Arc::from("c")],
        ..downstream(&["a", "b"])
    };
    assert!(run(UNUSED, NAMED, Materialization::Table, &facts).is_empty());
}

#[test]
fn case_does_not_make_a_used_column_unused() {
    let found = run(
        UNUSED,
        NAMED,
        Materialization::Table,
        &downstream(&["A", "B", "C"]),
    );
    assert!(found.is_empty());
}

#[test]
fn views_and_ephemerals_are_silent_because_engines_prune_them() {
    for mat in [Materialization::View, Materialization::Ephemeral] {
        assert!(
            run(UNUSED, NAMED, mat, &downstream(&["a"])).is_empty(),
            "{mat:?}"
        );
    }
}

#[test]
fn a_model_with_no_downstream_is_not_claimed_unused() {
    assert!(run(UNUSED, NAMED, Materialization::Table, &DagFacts::default()).is_empty());
}

#[test]
fn an_unmet_dag_need_keeps_the_rule_silent() {
    let without_dag = Needs::NONE.with(Need::Schema).with(Need::Mat);
    let found = run_rule(
        UNUSED,
        NAMED,
        Some(Materialization::Table),
        None,
        without_dag,
    );
    assert!(
        found.is_empty(),
        "an exposure or unreadable consumer withholds Need::Dag"
    );
}

#[test]
fn a_star_is_left_to_select_star_chain() {
    let facts = downstream(&["a"]);
    let star = "select * from {{ ref('up') }}";
    assert!(run(UNUSED, star, Materialization::Table, &facts).is_empty());
    let mixed = "select *, a + 1 as extra from {{ ref('up') }}";
    assert!(
        run(UNUSED, mixed, Materialization::Table, &facts).is_empty(),
        "select_star_chain reports the whole list"
    );
}

#[test]
fn a_star_on_a_view_still_has_no_column_to_point_at() {
    let facts = downstream(&["a"]);
    let star = "select * from {{ ref('up') }}";
    assert!(run(UNUSED, star, Materialization::View, &facts).is_empty());
}

#[test]
fn a_jinja_expanded_select_list_cannot_be_called_unused() {
    let sql = "select {{ dbt_utils.star(ref('up')) }}, a from {{ ref('up') }}";
    assert!(run(UNUSED, sql, Materialization::Table, &downstream(&["a"])).is_empty());
}

#[test]
fn a_jinja_expression_output_is_named_and_can_be_unused() {
    let sql = "select a, {{ var('x') }} as b from {{ ref('up') }}";
    let found = run(UNUSED, sql, Materialization::Table, &downstream(&["a"]));
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("`b`"));
}

#[test]
fn the_models_own_config_keys_are_read_by_the_build() {
    let sql = "{{ config(materialized='incremental', unique_key='b') }}\
               \nselect a, b, c from {{ ref('up') }}";
    let found = run(
        UNUSED,
        sql,
        Materialization::Incremental,
        &downstream(&["a"]),
    );
    let names: Vec<&str> = found.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(found.len(), 1, "only c: {names:?}");
    assert!(found[0].message.contains("`c`"));
}

#[test]
fn a_contract_keeps_the_column_list_as_published() {
    let sql = "{{ config(materialized='table', contract={'enforced': true}) }}\n\
               select a, b from {{ ref('up') }}";
    assert!(run(UNUSED, sql, Materialization::Table, &downstream(&["a"])).is_empty());
}

const FILTER_LATE: &str = "cross.filter_late";

fn consumers(rows: &[(&str, &[(&str, &str)])]) -> DagFacts {
    DagFacts {
        consumers: rows
            .iter()
            .map(|(name, filters)| {
                let filters: Vec<ColumnFilter> = filters
                    .iter()
                    .map(|(c, p)| ColumnFilter {
                        column: Arc::from(*c),
                        predicate: Arc::from(*p),
                    })
                    .collect();
                (
                    Arc::from(*name),
                    ConsumerFacts {
                        filters: Arc::from(filters),
                    },
                )
            })
            .collect(),
        ..DagFacts::default()
    }
}

const SAME: &[(&str, &str)] = &[("a", "($ Eq n1)")];
const TABLE_SQL: &str = "select a, b from {{ ref('up') }}";

#[test]
fn two_consumers_filtering_alike_on_a_table_is_a_finding() {
    let facts = consumers(&[("c1", SAME), ("c2", SAME)]);
    for mat in [Materialization::Table, Materialization::Incremental] {
        let found = run(FILTER_LATE, TABLE_SQL, mat, &facts);
        assert_eq!(found.len(), 1, "{mat:?}");
        assert!(found[0].message.contains("in this project"));
        assert!(found[0].message.contains("`a`"));
        assert!(found[0].fixes.is_empty());
    }
}

#[test]
fn a_view_gets_pushdown_from_the_engine_so_no_finding() {
    let facts = consumers(&[("c1", SAME), ("c2", SAME)]);
    assert!(run(FILTER_LATE, TABLE_SQL, Materialization::View, &facts).is_empty());
}

#[test]
fn one_consumer_is_not_agreement() {
    let facts = consumers(&[("c1", SAME)]);
    assert!(run(FILTER_LATE, TABLE_SQL, Materialization::Table, &facts).is_empty());
}

#[test]
fn a_consumer_without_the_filter_blocks_the_finding() {
    let facts = consumers(&[("c1", SAME), ("c2", &[])]);
    assert!(run(FILTER_LATE, TABLE_SQL, Materialization::Table, &facts).is_empty());
}

#[test]
fn different_predicates_on_the_same_column_are_silent() {
    let facts = consumers(&[("c1", SAME), ("c2", &[("a", "($ Eq n2)")])]);
    assert!(run(FILTER_LATE, TABLE_SQL, Materialization::Table, &facts).is_empty());
}

#[test]
fn jinja_in_the_consumer_reaches_the_rule_only_as_absent_facts() {
    // The schema job records no filter for it, which is what this states.
    let facts = consumers(&[("c1", SAME), ("c2", &[])]);
    let sql = "select a, {{ var('x') }} as b from {{ ref('up') }}";
    assert!(run(FILTER_LATE, sql, Materialization::Table, &facts).is_empty());
}

const EPHEMERAL: &str = "cross.ephemeral_repeat";
const WORK: &str = "select a, count(*) as n from {{ ref('up') }} group by a";

fn inlined(sites: &[&str]) -> DagFacts {
    DagFacts {
        inlined_into: sites.iter().map(|s| Arc::from(*s)).collect(),
        ..DagFacts::default()
    }
}

#[test]
fn three_compile_sites_with_real_work_is_a_finding() {
    let found = run(
        EPHEMERAL,
        WORK,
        Materialization::Ephemeral,
        &inlined(&["a", "b", "c"]),
    );
    assert_eq!(found.len(), 1);
    assert!(
        found[0].message.contains("3 models in this project"),
        "{}",
        found[0].message
    );
}

#[test]
fn two_sites_is_below_the_threshold() {
    let found = run(
        EPHEMERAL,
        WORK,
        Materialization::Ephemeral,
        &inlined(&["a", "b"]),
    );
    assert!(found.is_empty());
}

#[test]
fn a_trivial_body_is_not_worth_the_hint() {
    let found = run(
        EPHEMERAL,
        "select a from {{ ref('up') }}",
        Materialization::Ephemeral,
        &inlined(&["a", "b", "c"]),
    );
    assert!(found.is_empty());
}

#[test]
fn only_an_ephemeral_model_can_repeat() {
    let found = run(
        EPHEMERAL,
        WORK,
        Materialization::Table,
        &inlined(&["a", "b", "c"]),
    );
    assert!(found.is_empty());
}

#[test]
fn jinja_in_an_ephemeral_body_does_not_change_the_count() {
    let sql = "select a, count(*) as n from {{ ref('up') }} where b = {{ var('x') }} group by a";
    let found = run(
        EPHEMERAL,
        sql,
        Materialization::Ephemeral,
        &inlined(&["a", "b", "c"]),
    );
    assert_eq!(found.len(), 1, "the compile-site count is about the graph");
}

const DUPLICATE: &str = "cross.duplicate_logic";
const JOINED: &str = "select u.a, sum(o.b) as total from {{ ref('up') }} u \
                      join {{ ref('other') }} o on u.a = o.a group by u.a";

/// The canonical subplans of a real query, as the schema job would see.
fn shared_for(sql: &str) -> DagFacts {
    let output = run_raw_with(sql, up(), needs(), Extras::default());
    let subplans = subplans_of(&output.mir);
    assert!(!subplans.is_empty(), "{sql}");
    DagFacts {
        shared_subplans: subplans
            .iter()
            .map(|s| {
                (
                    s.hash,
                    SharedSubplan {
                        text: s.text.clone(),
                        size: s.size,
                        models: vec![Arc::from("twin")],
                        total: 1,
                    },
                )
            })
            .collect(),
        ..DagFacts::default()
    }
}

#[test]
fn a_shared_subplan_reports_the_largest_once() {
    let facts = shared_for(JOINED);
    let found = run_rule(
        DUPLICATE,
        JOINED,
        None,
        Some(&facts),
        Needs::NONE.with(Need::Dag),
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("twin"));
    assert!(found[0].message.contains("in this project"));
    assert!(found[0].fixes.is_empty());
}

#[test]
fn a_hash_match_alone_is_not_a_finding() {
    let mut facts = shared_for(JOINED);
    for shared in facts.shared_subplans.values_mut() {
        shared.text = Arc::from("a different plan with the same hash");
    }
    let found = run_rule(
        DUPLICATE,
        JOINED,
        None,
        Some(&facts),
        Needs::NONE.with(Need::Dag),
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_trivial_star_has_no_comparable_subplan() {
    let output = run_raw_with(
        "select * from {{ ref('up') }}",
        up(),
        needs(),
        Extras::default(),
    );
    assert!(subplans_of(&output.mir).is_empty());
}

#[test]
fn a_cte_and_a_subquery_with_the_same_plan_compare_equal() {
    let cte = "with j as (select u.a, o.b from {{ ref('up') }} u \
               join {{ ref('other') }} o on u.a = o.a) \
               select a, sum(b) as s from j group by a";
    let sub = "select a, sum(b) as s from (select u.a, o.b from {{ ref('up') }} u \
               join {{ ref('other') }} o on u.a = o.a) j group by a";
    let text = |sql| subplans_of(&run_raw_with(sql, up(), needs(), Extras::default()).mir);
    let (cte, sub) = (text(cte), text(sub));
    assert_eq!(
        cte.first().map(|s| s.text.clone()),
        sub.first().map(|s| s.text.clone()),
        "the plan, not the spelling"
    );
}

#[test]
fn a_different_grouping_key_is_different_logic() {
    let one = subplans_of(&run_raw_with(JOINED, up(), needs(), Extras::default()).mir);
    let other = "select u.b, sum(o.b) as total from {{ ref('up') }} u \
                 join {{ ref('other') }} o on u.a = o.a group by u.b";
    let two = subplans_of(&run_raw_with(other, up(), needs(), Extras::default()).mir);
    assert_ne!(one.first().map(|s| &s.text), two.first().map(|s| &s.text));
}

#[test]
fn jinja_bearing_subplans_never_compare() {
    let scalar = "select u.a, sum(o.b) as total from {{ ref('up') }} u \
                  join {{ ref('other') }} o on u.a = o.a \
                  where u.b = {{ var('x') }} group by u.a";
    let literal = "select u.a, sum(o.b) as total from {{ ref('up') }} u \
                   join {{ ref('other') }} o on u.a = o.a \
                   where u.b = '{{ var(\"x\") }}' group by u.a";
    for sql in [scalar, literal] {
        let output = run_raw_with(sql, up(), needs(), Extras::default());
        let with_masked_text = subplans_of(&output.mir)
            .iter()
            .any(|s| s.text.contains("_jj"));
        assert!(!with_masked_text, "{sql}");
    }
}
