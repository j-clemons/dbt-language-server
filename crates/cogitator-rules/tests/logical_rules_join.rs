//! `join.redundant_left`, `join.key_type_mismatch` and
//! `filter.constant_false` through the real pipeline.

mod logical_support;

use cogitator_ir::schema::ProjectSchemas;
use cogitator_rules::{Evidence, Finding, Need, Needs, Severity};
use logical_support::{all_needs, at, col, findings, only, run_rule, schemas, Options, Type};

fn with_unique_b() -> ProjectSchemas {
    schemas(&[
        ("a", &[col("id"), col("bid"), col("v")]),
        ("b", &[col("id").unique(), col("name"), col("v")]),
    ])
}

fn redundant(sql: &str) -> Vec<Finding> {
    run_rule(
        sql,
        with_unique_b(),
        all_needs(),
        Options::default(),
        "join.redundant_left",
    )
}

const LEFT: &str = "{{ ref('a') }} as a left join {{ ref('b') }} as b on a.bid = b.id";

#[test]
fn redundant_left_flags_a_tested_unique_join_with_no_right_reads() {
    let sql = format!("select a.id, a.v from {LEFT}");
    let found = only(redundant(&sql));
    assert_eq!(found.severity, Severity::Info);
    assert_eq!(found.evidence, Evidence::Tested { schema_revision: 0 });
    assert_eq!(at(&sql, &found), "{{ ref('b') }}");
    assert!(found.message.contains("`id`"), "{}", found.message);
}

#[test]
fn redundant_left_stays_silent_when_a_right_column_is_read() {
    for tail in [
        "select a.id, b.name from {LEFT}",
        "select a.id from {LEFT} where b.name is null",
        "select a.id from {LEFT} order by b.name",
        "select a.id, count(b.name) as c from {LEFT} group by a.id",
        "select a.id, (select max(c.v) from {{ ref('a') }} as c where c.id = b.id) as m from {LEFT}",
    ] {
        let sql = tail.replace("{LEFT}", LEFT);
        assert!(redundant(&sql).is_empty(), "{sql}");
    }
}

#[test]
fn redundant_left_stays_silent_without_a_unique_test() {
    let sql = format!("select a.id from {LEFT}");
    let plain = schemas(&[
        ("a", &[col("id"), col("bid")]),
        ("b", &[col("id"), col("name")]),
    ]);
    assert!(run_rule(
        &sql,
        plain,
        all_needs(),
        Options::default(),
        "join.redundant_left"
    )
    .is_empty());
}

#[test]
fn redundant_left_does_not_read_a_unique_key_through_a_join() {
    let sql = "select a.id from {{ ref('a') }} as a left join ({{ ref('b') }} as b join {{ ref('a') }} as c on b.id = c.id) on a.bid = b.id";
    assert!(redundant(sql).is_empty());
}

#[test]
fn redundant_left_requires_tests_to_be_available() {
    let sql = format!("select a.id from {LEFT}");
    let needs = Needs::NONE.with(Need::AllBranches).with(Need::Types);
    let found = run_rule(
        &sql,
        with_unique_b(),
        needs,
        Options::default(),
        "join.redundant_left",
    );
    assert!(found.is_empty());
}

#[test]
fn redundant_left_is_silent_on_an_open_star_and_jinja_reads() {
    let star_open =
        "select * from {{ ref('open_model') }} as a left join {{ ref('b') }} as b on a.bid = b.id";
    assert!(redundant(star_open).is_empty(), "open star");
    let jinja = format!("select a.id, {{{{ dbt_utils.star(ref('b')) }}}} from {LEFT}");
    assert!(redundant(&jinja).is_empty(), "jinja select item");
}

#[test]
fn redundant_left_ignores_inner_joins_and_keeps_a_filtered_right_side() {
    let inner = "select a.id from {{ ref('a') }} as a join {{ ref('b') }} as b on a.bid = b.id";
    assert!(redundant(inner).is_empty());
    let filtered = "select a.id from {{ ref('a') }} as a left join {{ ref('b') }} as b on a.bid = b.id and b.v = 1";
    assert_eq!(
        redundant(filtered).len(),
        1,
        "extra ON keeps at most one match"
    );
}

#[test]
fn redundant_left_follows_a_cte_import() {
    let sql = "with b_rows as (select id from {{ ref('b') }}) select a.id from {{ ref('a') }} as a left join b_rows as r on a.bid = r.id";
    assert_eq!(redundant(sql).len(), 1);
}

// join.key_type_mismatch

fn types(a: Type, b: Type) -> Vec<Finding> {
    let sql = "select x.id from {{ ref('a') }} as x join {{ ref('b') }} as y on x.key = y.key";
    run_rule(
        sql,
        schemas(&[
            ("a", &[col("id"), col("key").typed(a)]),
            ("b", &[col("id"), col("key").typed(b)]),
        ]),
        all_needs(),
        Options::default(),
        "join.key_type_mismatch",
    )
}

#[test]
fn key_mismatch_flags_incompatible_families_as_a_risk() {
    let found = only(types(Type::Int, Type::Text));
    assert_eq!(found.severity, Severity::Info);
    assert!(found.message.contains("prun"), "{}", found.message);
}

#[test]
fn key_mismatch_is_silent_on_same_family_or_unknown() {
    assert!(types(Type::Int, Type::Decimal).is_empty());
    assert!(types(Type::Text, Type::Text).is_empty());
    assert!(types(Type::Int, Type::Unknown).is_empty());
    assert!(types(Type::Unknown, Type::Text).is_empty());
}

#[test]
fn key_mismatch_ignores_a_cast_side() {
    let sql = "select x.id from {{ ref('a') }} as x join {{ ref('b') }} as y on cast(x.key as varchar) = y.key";
    let schema = schemas(&[
        ("a", &[col("id"), col("key").typed(Type::Int)]),
        ("b", &[col("id"), col("key").typed(Type::Text)]),
    ]);
    assert!(run_rule(
        sql,
        schema,
        all_needs(),
        Options::default(),
        "join.key_type_mismatch"
    )
    .is_empty());
}

#[test]
fn key_mismatch_is_skipped_without_types() {
    let needs = Needs::NONE.with(Need::AllBranches).with(Need::Tests);
    let sql = "select x.id from {{ ref('a') }} as x join {{ ref('b') }} as y on x.key = y.key";
    let found = run_rule(
        sql,
        schemas(&[
            ("a", &[col("id"), col("key").typed(Type::Int)]),
            ("b", &[col("id"), col("key").typed(Type::Text)]),
        ]),
        needs,
        Options::default(),
        "join.key_type_mismatch",
    );
    assert!(found.is_empty());
}

// filter.constant_false

fn constant(sql: &str) -> Vec<Finding> {
    findings(sql, "filter.constant_false")
}

#[test]
fn constant_false_exempts_the_schema_only_idioms() {
    for sql in [
        "select id from {{ ref('a') }} where false",
        "select id from {{ ref('a') }} where 1 = 0",
        "select id from {{ ref('a') }} where 0 = 1",
        "select id from {{ ref('a') }} limit 0",
        "select id from {{ ref('a') }} where false limit 0",
    ] {
        assert!(constant(sql).is_empty(), "{sql}");
    }
}

#[test]
fn constant_false_flags_contradictions_on_one_column() {
    let sql = "select id from {{ ref('a') }} where k = 1 and k = 2";
    let found = only(constant(sql));
    assert_eq!(found.severity, Severity::Hint);
    assert_eq!(at(sql, &found), "k = 2");
    assert!(found.message.contains("`k`"), "{}", found.message);
}

#[test]
fn constant_false_flags_false_conjuncts_alongside_other_filters() {
    for sql in [
        "select id from {{ ref('a') }} where k = 1 and false",
        "select id from {{ ref('a') }} where k = 1 and 1 = 0",
    ] {
        assert_eq!(constant(sql).len(), 1, "{sql}");
    }
}

#[test]
fn constant_false_flags_a_comparison_with_null() {
    let sql = "select id from {{ ref('a') }} where k = null";
    let found = only(constant(sql));
    assert!(found.message.contains("NULL"), "{}", found.message);
}

#[test]
fn constant_false_is_silent_on_satisfiable_or_unprovable_filters() {
    for sql in [
        "select id from {{ ref('a') }} where k = 1",
        "select id from {{ ref('a') }} where k = 1 and k = 1.0",
        "select id from {{ ref('a') }} where k = 1 and v = 2",
        "select id from {{ ref('a') }} where k = 1 or k = 2",
        "select id from {{ ref('a') }} where k in (1, 2)",
        "select id from {{ ref('a') }} where v = 'A' and v = 'a'",
        "select id from {{ ref('a') }} where v = 'a ' and v = 'a'",
        "select id from {{ ref('a') }} where {{ var('flag') }} and false",
    ] {
        assert!(constant(sql).is_empty(), "{sql}");
    }
}

#[test]
fn constant_false_reports_strings_that_no_collation_equates() {
    assert_eq!(
        constant("select id from {{ ref('a') }} where v = 'a' and v = 'b'").len(),
        1
    );
}
