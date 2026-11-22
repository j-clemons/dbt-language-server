//! `order.in_cte` and the `subq.*` rules through the real pipeline.

mod logical_support;

use cogitator_ir::lir::Backend;
use cogitator_rules::meta::FixSafety;
use cogitator_rules::{Evidence, Severity};
use logical_support::{
    all_needs, apply, at, col, default_schemas, findings, only, run_rule, schemas, Options,
};

fn order(sql: &str) -> Vec<cogitator_rules::Finding> {
    findings(sql, "order.in_cte")
}

fn not_in(sql: &str) -> Vec<cogitator_rules::Finding> {
    findings(sql, "subq.not_in_nullable")
}

fn correlated(sql: &str) -> Vec<cogitator_rules::Finding> {
    findings(sql, "subq.correlated")
}

// order.in_cte

const SORTED_CTE: &str = "with s as (select id from {{ ref('a') }} order by id) select id from s";

#[test]
fn order_flags_a_sort_inside_a_cte_and_spans_the_keys() {
    let finding = only(order(SORTED_CTE));
    assert_eq!(finding.severity, Severity::Info);
    assert_eq!(finding.evidence, Evidence::Exact);
    assert_eq!(at(SORTED_CTE, &finding), "id");
    assert_eq!(finding.fixes.len(), 1);
    assert_eq!(finding.fixes[0].safety, FixSafety::ChangesIntent);
    assert_eq!(
        apply(SORTED_CTE, &finding.fixes[0]),
        "with s as (select id from {{ ref('a') }}) select id from s"
    );
}

#[test]
fn order_fix_is_preserving_only_where_order_is_unobservable() {
    let run = |dialect| {
        only(run_rule(
            SORTED_CTE,
            default_schemas(),
            all_needs(),
            Options {
                dialect: Some(dialect),
                no_text: false,
            },
            "order.in_cte",
        ))
    };
    assert_eq!(
        run(Backend::Snowflake).fixes[0].safety,
        FixSafety::Preserving
    );
    assert_eq!(
        run(Backend::Postgres).fixes[0].safety,
        FixSafety::Preserving
    );
    assert_eq!(
        run(Backend::DuckDb).fixes[0].safety,
        FixSafety::ChangesIntent,
        "DuckDB preserves insertion order"
    );
    assert_eq!(
        run(Backend::Generic).fixes[0].safety,
        FixSafety::ChangesIntent
    );
}

#[test]
fn order_flags_derived_and_in_subqueries() {
    let derived = "select id from (select id from {{ ref('a') }} order by id desc nulls last) d";
    let finding = only(order(derived));
    assert_eq!(
        apply(derived, &finding.fixes[0]),
        "select id from (select id from {{ ref('a') }}) d"
    );
    assert_eq!(
        order(
            "select id from {{ ref('a') }} where id in (select k from {{ ref('b') }} order by k)"
        )
        .len(),
        1
    );
}

#[test]
fn order_ignores_the_outermost_sort_of_the_model() {
    assert!(order("select id from {{ ref('a') }} order by id").is_empty());
    assert!(order(
        "select id from {{ ref('a') }} union all select id from {{ ref('b') }} order by id"
    )
    .is_empty());
}

#[test]
fn order_is_silent_when_something_observes_the_order() {
    for sql in [
        // LIMIT in the same query, directly and over a hidden sort key
        "with s as (select id from {{ ref('a') }} order by id limit 5) select id from s",
        "with s as (select id from {{ ref('a') }} order by k limit 5) select id from s",
        // a consumer LIMIT with no sort of its own
        "with s as (select id from {{ ref('a') }} order by id) select id from s limit 5",
        // a window reads input order for ties, or outright with OVER ()
        "with s as (select id from {{ ref('a') }} order by id) select row_number() over () as n from s",
        // order-sensitive aggregates
        "with s as (select id from {{ ref('a') }} order by id) select array_agg(id) as x from s",
        "with s as (select id from {{ ref('a') }} order by id) select first(id) as x from s",
        // an arm of a set operation
        "(select id from {{ ref('a') }} order by id) union all (select id from {{ ref('b') }})",
        // array(subquery) keeps the order
        "select array(select id from {{ ref('b') }} order by id) as ids from {{ ref('a') }}",
        // OFFSET disappears from HIR but still reads the order
        "with s as (select id from {{ ref('a') }} order by id offset 5) select id from s",
        "with s as (select id from {{ ref('a') }} order by lower(id) offset 5) select id from s",
    ] {
        assert!(order(sql).is_empty(), "{sql}");
    }
}

#[test]
fn order_is_silent_over_an_opaque_key() {
    assert!(order(
        "with s as (select id from {{ ref('a') }} order by {{ var('key') }}) select id from s"
    )
    .is_empty());
}

#[test]
fn order_is_skipped_on_a_plan_that_cannot_see_every_branch() {
    // a compiled plan of a template with control flow could hide a LIMIT
    let needs = cogitator_rules::Needs::NONE.with(cogitator_rules::Need::Tests);
    assert!(run_rule(
        SORTED_CTE,
        default_schemas(),
        needs,
        Options::default(),
        "order.in_cte"
    )
    .is_empty());
}

#[test]
fn order_flags_when_a_later_sort_or_plain_aggregate_erases_it() {
    assert_eq!(
        order("with s as (select id from {{ ref('a') }} order by id) select id from s order by id limit 3").len(),
        1
    );
    assert_eq!(
        order("with s as (select id from {{ ref('a') }} order by id) select sum(id) as x from s")
            .len(),
        1
    );
}

#[test]
fn order_withholds_the_fix_over_comments_and_inexact_spans() {
    let commented =
        "with s as (select id from {{ ref('a') }} order by /* why */ id) select id from s";
    assert!(only(order(commented)).fixes.is_empty());
    let expression =
        "with s as (select id from {{ ref('a') }} order by lower(id)) select id from s";
    assert!(only(order(expression)).fixes.is_empty());
}

#[test]
fn order_keeps_a_line_comment_from_swallowing_the_paren() {
    let sql =
        "with s as (\n  select id from {{ ref('a') }} -- all\n  order by id\n) select id from s";
    let finding = only(order(sql));
    assert_eq!(
        apply(sql, &finding.fixes[0]),
        "with s as (\n  select id from {{ ref('a') }} -- all\n  \n) select id from s"
    );
}

#[test]
fn order_needs_the_text_to_claim_anything() {
    assert!(run_rule(
        SORTED_CTE,
        default_schemas(),
        all_needs(),
        Options {
            dialect: None,
            no_text: true,
        },
        "order.in_cte",
    )
    .is_empty());
}

// subq.not_in_nullable

fn not_in_with(
    a: &[logical_support::Col],
    b: &[logical_support::Col],
) -> Vec<cogitator_rules::Finding> {
    let sql = "select x.id from {{ ref('a') }} as x where x.id not in (select k from {{ ref('b') }} as y)";
    run_rule(
        sql,
        schemas(&[("a", a), ("b", b)]),
        all_needs(),
        Options::default(),
        "subq.not_in_nullable",
    )
}

#[test]
fn not_in_warns_when_the_subquery_column_is_nullable() {
    let found = only(not_in_with(&[col("id")], &[col("k").nullable()]));
    assert_eq!(found.severity, Severity::Warning);
    assert_eq!(found.evidence, Evidence::Exact);
    assert!(found.message.contains("never true"), "{}", found.message);
}

#[test]
fn not_in_on_unknown_nullability_is_a_hint_worded_may() {
    let sql = "select x.id from {{ ref('a') }} as x where x.id not in (select k from {{ ref('b') }} as y)";
    let found = only(not_in(sql));
    assert_eq!(found.severity, Severity::Hint);
    assert_eq!(found.evidence, Evidence::Heuristic);
    assert!(found.message.contains("may"), "{}", found.message);
    assert_eq!(at(sql, &found), "x.id");
}

#[test]
fn not_in_is_silent_on_proven_not_null() {
    assert!(not_in_with(&[col("id")], &[col("k").not_null()]).is_empty());
    for filter in ["k is not null", "k > 0", "k = 1"] {
        let sql = format!(
            "select x.id from {{{{ ref('a') }}}} as x where x.id not in (select k from {{{{ ref('b') }}}} where {filter})"
        );
        assert!(not_in(&sql).is_empty(), "{filter}");
    }
}

#[test]
fn not_in_nullable_from_an_outer_join_warns() {
    let sql = "select x.id from {{ ref('a') }} as x where x.id not in (select u.k from {{ ref('a') }} as t left join {{ ref('b') }} as u on t.id = u.id)";
    let found = run_rule(
        sql,
        schemas(&[
            ("a", &[col("id").not_null()]),
            ("b", &[col("id").not_null(), col("k").not_null()]),
        ]),
        all_needs(),
        Options::default(),
        "subq.not_in_nullable",
    );
    let found = only(found);
    assert_eq!(found.severity, Severity::Warning, "{found:#?}");
}

#[test]
fn not_in_flags_a_literal_null_in_a_list() {
    let sql = "select id from {{ ref('a') }} where id not in (1, null)";
    let found = only(not_in(sql));
    assert_eq!(found.severity, Severity::Warning);
    assert!(found.fixes.is_empty());
    assert!(not_in("select id from {{ ref('a') }} where id in (1, null)").is_empty());
    assert!(not_in("select id from {{ ref('a') }} where id not in (1, 2)").is_empty());
}

#[test]
fn not_in_fix_rewrites_to_not_exists_on_exact_spans() {
    let sql = "select x.id from {{ ref('a') }} as x where x.id not in (select k from {{ ref('b') }} as y)";
    let found = only(not_in(sql));
    assert_eq!(found.fixes.len(), 1, "{found:#?}");
    assert_eq!(found.fixes[0].safety, FixSafety::ChangesIntent);
    assert_eq!(
        apply(sql, &found.fixes[0]),
        "select x.id from {{ ref('a') }} as x where not exists (select k from {{ ref('b') }} as y where x.id = k)"
    );
}

#[test]
fn not_in_fix_extends_an_existing_where() {
    let sql = "select x.id from {{ ref('a') }} as x where x.id NOT IN (select y.k from {{ ref('b') }} as y where y.v = 1 and y.id > 2)";
    let found = only(not_in(sql));
    assert_eq!(
        apply(sql, &found.fixes[0]),
        "select x.id from {{ ref('a') }} as x where NOT EXISTS (select y.k from {{ ref('b') }} as y where x.id = y.k AND y.v = 1 and y.id > 2)"
    );
}

#[test]
fn not_in_withholds_the_fix_when_the_rewrite_is_unsafe() {
    for sql in [
        // an unqualified outer column could rebind inside the subquery
        "select id from {{ ref('a') }} where id not in (select k from {{ ref('b') }} as y)",
        // an OR would regroup under the new AND
        "select x.id from {{ ref('a') }} as x where x.id not in (select k from {{ ref('b') }} as y where k = 1 or v = 2)",
        // the inner alias shadows the outer qualifier
        "select x.id from {{ ref('a') }} as x where x.id not in (select k from {{ ref('b') }} as x)",
        // a group, a union, an expression item
        "select x.id from {{ ref('a') }} as x where x.id not in (select k from {{ ref('b') }} as y group by k)",
        "select x.id from {{ ref('a') }} as x where x.id not in (select k + 1 from {{ ref('b') }} as y)",
    ] {
        let found = only(not_in(sql));
        assert!(found.fixes.is_empty(), "{sql}: {found:#?}");
    }
}

#[test]
fn not_in_handles_the_prefix_not_form_without_a_fix() {
    let sql = "select x.id from {{ ref('a') }} as x where not x.id in (select k from {{ ref('b') }} as y)";
    let found = only(not_in(sql));
    assert!(found.fixes.is_empty());
}

#[test]
fn not_in_is_silent_over_opaque_subqueries() {
    let sql = "select x.id from {{ ref('a') }} as x where x.id not in ({{ my_subquery() }})";
    assert!(not_in(sql).is_empty());
}

// subq.correlated

#[test]
fn correlated_hints_on_a_correlated_scalar_subquery() {
    let sql = "select x.id, (select max(y.v) from {{ ref('b') }} as y where y.k = x.k) as m from {{ ref('a') }} as x";
    let found = only(correlated(sql));
    assert_eq!(found.severity, Severity::Hint);
    assert!(found.message.contains("most engines"), "{}", found.message);
}

#[test]
fn correlated_flags_exists_with_a_non_equality() {
    let sql = "select x.id from {{ ref('a') }} as x where exists (select 1 from {{ ref('b') }} as y where y.k > x.k)";
    assert_eq!(correlated(sql).len(), 1);
}

#[test]
fn correlated_is_silent_on_plain_exists_semi_joins() {
    for sql in [
        "select x.id from {{ ref('a') }} as x where exists (select 1 from {{ ref('b') }} as y where y.k = x.k)",
        "select x.id from {{ ref('a') }} as x where not exists (select 1 from {{ ref('b') }} as y where x.k = y.k and y.v = 1)",
    ] {
        assert!(correlated(sql).is_empty(), "{sql}");
    }
}

#[test]
fn correlated_is_silent_when_not_correlated_or_opaque() {
    assert!(
        correlated("select id from {{ ref('a') }} where id in (select k from {{ ref('b') }})")
            .is_empty()
    );
    assert!(
        correlated("select x.id from {{ ref('a') }} as x where x.id in ({{ my_subquery() }})")
            .is_empty()
    );
}
