//! The `join.*`, `set.*` and `model.*` rules through the real pipeline.

mod join_support;

use cogitator_ir::mir::print_mir;
use cogitator_rules::meta::FixSafety;
use cogitator_rules::passes::Outcome;
use cogitator_rules::{Evidence, Finding, Need, Needs, Severity};
use join_support::{all_needs, apply, findings, model, run, run_raw, schemas};

const RIGHT: &str = "{{ ref('a') }} as x left join {{ ref('b') }} as y on x.id = y.k";

fn left(sql: &str) -> Vec<Finding> {
    findings(sql, "join.left_nullified")
}

fn cartesian(sql: &str) -> Vec<Finding> {
    findings(sql, "join.cartesian")
}

fn fanout(sql: &str) -> Vec<Finding> {
    findings(sql, "join.distinct_fanout")
}

fn cte(sql: &str) -> Vec<Finding> {
    findings(sql, "model.unused_cte")
}

fn union(sql: &str) -> Vec<Finding> {
    findings(sql, "set.union_distinct")
}

fn only(mut found: Vec<Finding>) -> Finding {
    assert_eq!(found.len(), 1, "{found:#?}");
    found.remove(0)
}

fn fix_of(finding: &Finding, safety: FixSafety) -> &cogitator_rules::Fix {
    finding
        .fixes
        .iter()
        .find(|f| f.safety == safety)
        .unwrap_or_else(|| panic!("no {safety:?} fix: {finding:#?}"))
}

// join.left_nullified

#[test]
fn left_nullified_flags_a_null_rejecting_where_and_offers_both_fixes() {
    let sql = format!("select x.id from {RIGHT} where y.state = 1");
    let finding = only(left(&sql));
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.evidence, Evidence::Exact);
    assert_eq!(
        &sql[finding.span.start as usize..finding.span.end as usize],
        "y.state = 1"
    );
    assert_eq!(finding.fixes.len(), 2, "{finding:#?}");
    assert_eq!(
        finding.fixes[0].safety,
        FixSafety::Preserving,
        "P comes first"
    );
    assert_eq!(
        apply(&sql, fix_of(&finding, FixSafety::Preserving)),
        "select x.id from {{ ref('a') }} as x inner join {{ ref('b') }} as y on x.id = y.k where y.state = 1"
    );
    assert_eq!(
        apply(&sql, fix_of(&finding, FixSafety::ChangesIntent)),
        "select x.id from {{ ref('a') }} as x left join {{ ref('b') }} as y on x.id = y.k and y.state = 1 "
    );
}

#[test]
fn left_nullified_moves_one_conjunct_and_keeps_the_rest() {
    let sql = format!("select x.id from {RIGHT} where x.flag = 2 and y.state = 1");
    let finding = only(left(&sql));
    assert_eq!(
        apply(&sql, fix_of(&finding, FixSafety::ChangesIntent)),
        "select x.id from {{ ref('a') }} as x left join {{ ref('b') }} as y on x.id = y.k and y.state = 1 where x.flag = 2"
    );
}

#[test]
fn left_nullified_is_silent_on_filters_that_keep_null_rows() {
    for filter in [
        "y.state is null",
        "coalesce(y.state, 0) = 0",
        "my_udf(y.state) = 1",
        "x.flag = 1",
        "y.state = 1 or x.flag = 2",
        "case when y.state = 1 then true else false end",
    ] {
        let sql = format!("select x.id from {RIGHT} where {filter}");
        assert!(left(&sql).is_empty(), "{filter}");
    }
}

#[test]
fn left_nullified_recognises_three_valued_logic() {
    for filter in [
        "y.state is not null",
        "y.state = 1 or y.flag = 2",
        "not (y.state = 1)",
        "lower(y.state) = 'a'",
        "y.state in (1, 2)",
    ] {
        let sql = format!("select x.id from {RIGHT} where {filter}");
        assert_eq!(left(&sql).len(), 1, "{filter}");
    }
}

#[test]
fn left_nullified_covers_right_joins_and_their_keyword() {
    let sql = "select y.id from {{ ref('a') }} as x right outer join {{ ref('b') }} as y on x.id = y.k where x.state = 1";
    let finding = only(left(sql));
    assert_eq!(
        apply(sql, fix_of(&finding, FixSafety::Preserving)),
        "select y.id from {{ ref('a') }} as x inner join {{ ref('b') }} as y on x.id = y.k where x.state = 1"
    );
}

#[test]
fn left_nullified_points_at_the_authored_filter_after_pushdown() {
    let sql = format!("select * from (select x.id, y.state as s from {RIGHT}) as t where t.s = 1");
    let finding = only(left(&sql));
    assert_eq!(
        &sql[finding.span.start as usize..finding.span.end as usize],
        "t.s = 1"
    );
    // The pushed filter names the outer alias, so it cannot be moved textually.
    assert!(
        finding
            .fixes
            .iter()
            .all(|f| f.safety != FixSafety::ChangesIntent),
        "{finding:#?}"
    );
}

#[test]
fn left_nullified_with_jinja_values_stays_honest() {
    let opaque = format!("select x.id from {RIGHT} where y.state = {{{{ var('s') }}}}");
    assert!(
        left(&opaque).is_empty(),
        "an opaque comparison proves nothing"
    );
    let mixed =
        format!("select x.id from {RIGHT} where y.state = 1 and x.flag = {{{{ var('s') }}}}");
    assert_eq!(left(&mixed).len(), 1);
}

#[test]
fn left_nullified_claims_a_qualified_column_of_an_open_from_item() {
    let sql =
        "select o.id from orders as o left join customers as c on o.cid = c.id where c.state = 1";
    assert_eq!(left(sql).len(), 1, "c.state names its FROM item");
}

#[test]
fn left_nullified_stays_silent_on_a_name_that_could_be_either_side() {
    let sql =
        "select o.id from orders as o left join customers as c on o.cid = c.id where state = 1";
    assert!(left(sql).is_empty(), "either open side could supply it");
}

#[test]
fn left_nullified_withholds_fixes_it_cannot_prove() {
    let sql = "select x.id from {{ ref('a') }} as x left /* why */ join {{ ref('b') }} as y on x.id = y.k where y.state = 1";
    let finding = only(left(sql));
    assert!(
        finding
            .fixes
            .iter()
            .all(|f| f.safety != FixSafety::Preserving),
        "{finding:#?}"
    );
    let derived = "select x.id from {{ ref('a') }} as x left join (select id, k, state from {{ ref('b') }}) as y on x.id = y.k where y.state = 1";
    assert!(only(left(derived))
        .fixes
        .iter()
        .all(|f| f.safety != FixSafety::Preserving));
}

// join.cartesian

#[test]
fn cartesian_flags_a_comma_join_with_no_relation() {
    let sql = "select x.id from {{ ref('a') }} as x, {{ ref('b') }} as y";
    let finding = only(cartesian(sql));
    assert_eq!(finding.severity, Severity::Warning);
}

/// Canonicalisation moves `y.flag = 1` into `y`, leaving `on` empty on a
/// LEFT join. Only the written ON can say whether a key was ever there.
#[test]
fn cartesian_reads_the_written_on_when_canonicalisation_emptied_it() {
    let moved = "select x.id from {{ ref('a') }} as x left join {{ ref('b') }} as y on y.flag = 1";
    let plan = run_raw(moved, schemas(), all_needs());
    assert!(
        print_mir(&plan.mir).contains("Left"),
        "expected a left join in:\n{}",
        print_mir(&plan.mir)
    );
    let finding = only(cartesian(moved));
    assert_eq!(
        &moved[finding.span.start as usize..finding.span.start as usize + 2],
        "{{"
    );
    let keyed = "select x.id from {{ ref('a') }} as x left join {{ ref('b') }} as y on x.id = y.k and y.flag = 1";
    assert!(cartesian(keyed).is_empty());
    let intentional = "select x.id from {{ ref('a') }} as x left join {{ ref('b') }} as y on true";
    assert!(
        cartesian(intentional).is_empty(),
        "ON TRUE is a deliberate cross product"
    );
}

#[test]
fn cartesian_is_silent_when_the_inputs_are_related() {
    for sql in [
        "select x.id from {{ ref('a') }} as x join {{ ref('b') }} as y on x.id = y.k",
        "select x.id from {{ ref('a') }} as x, {{ ref('b') }} as y where x.id = y.k",
        "select x.id from {{ ref('a') }} as x join {{ ref('b') }} as y on x.flag < y.flag",
        "select x.id from {{ ref('a') }} as x, lateral (select y.id from {{ ref('b') }} as y where y.k = x.id) as s",
    ] {
        assert!(cartesian(sql).is_empty(), "{sql}");
    }
}

#[test]
fn cartesian_exempts_explicit_cross_joins_and_one_row_inputs() {
    for sql in [
        "select x.id from {{ ref('a') }} as x cross join {{ ref('b') }} as y",
        "select x.id from {{ ref('a') }} as x, (select count(*) as n from {{ ref('b') }}) as s",
        "select x.id from {{ ref('a') }} as x join (select id from {{ ref('b') }} limit 1) as s on true",
    ] {
        assert!(cartesian(sql).is_empty(), "{sql}");
    }
}

#[test]
fn cartesian_does_not_claim_through_holes_or_open_schemas() {
    let jinja_on = "select x.id from {{ ref('a') }} as x left join {{ ref('b') }} as y on {{ my_condition() }}";
    assert!(cartesian(jinja_on).is_empty());
    let hole = "select x.id from {{ ref('a') }} as x, generate_series(1, 3) as g";
    assert!(
        cartesian(hole).is_empty(),
        "a table function hides what it reads"
    );
    let open_related = "select x.id from orders as x, customers as y where x.id = y.oid";
    assert!(
        cartesian(open_related).is_empty(),
        "qualified keys relate them"
    );
    let open_ambiguous = "select x.id from orders as x, customers as y where id = oid";
    assert!(
        cartesian(open_ambiguous).is_empty(),
        "unqualified: could relate them"
    );
}

#[test]
fn cartesian_reports_a_comma_join_over_open_schemas() {
    let open = "select x.id from {{ ref('a') }} as x, some_table as y";
    assert_eq!(
        cartesian(open).len(),
        1,
        "keyless as written; no schema needed"
    );
    let one_sided = "select x.id from orders as x join customers as y on x.state = 1";
    assert_eq!(cartesian(one_sided).len(), 1, "qualified: one side only");
}

// join.distinct_fanout

const JOINED: &str =
    "select distinct x.state from {{ ref('a') }} as x join {{ ref('b') }} as y on x.id = y.k";

#[test]
fn distinct_fanout_hints_without_uniqueness_evidence() {
    let finding = only(fanout(JOINED));
    assert_eq!(finding.severity, Severity::Hint);
    assert_eq!(finding.evidence, Evidence::Heuristic);
}

#[test]
fn distinct_fanout_is_info_when_a_test_covers_one_side() {
    let mut schemas = schemas();
    schemas.models.insert(
        "a".into(),
        model(&[
            ("id", true),
            ("k", false),
            ("state", false),
            ("flag", false),
        ]),
    );
    let finding = only(run(JOINED, schemas, all_needs(), "join.distinct_fanout"));
    assert_eq!(finding.severity, Severity::Info);
    assert!(
        matches!(finding.evidence, Evidence::Tested { .. }),
        "{finding:#?}"
    );
}

#[test]
fn distinct_fanout_stays_silent_when_a_star_could_cross_the_join() {
    // An open star names no columns, so "only one side is kept" cannot be
    // shown: this claim genuinely needs closed schemas.
    assert!(
        fanout("select distinct * from orders as x join customers as y on x.id = y.k").is_empty()
    );
}

#[test]
fn distinct_fanout_is_silent_when_both_keys_are_unique() {
    let mut schemas = schemas();
    for name in ["a", "b"] {
        schemas.models.insert(
            name.into(),
            model(&[("id", true), ("k", true), ("state", false), ("flag", false)]),
        );
    }
    assert!(run(JOINED, schemas, all_needs(), "join.distinct_fanout").is_empty());
}

/// `Tests` is evidence, not a precondition: without it the rule still
/// reports, and a `unique` flag present in the schema is not trusted.
#[test]
fn distinct_fanout_does_not_need_tests_to_report() {
    let mut schemas = schemas();
    schemas.models.insert(
        "a".into(),
        model(&[
            ("id", true),
            ("k", false),
            ("state", false),
            ("flag", false),
        ]),
    );
    let finding = only(run(JOINED, schemas, Needs::NONE, "join.distinct_fanout"));
    assert_eq!(finding.severity, Severity::Hint);
}

#[test]
fn distinct_fanout_is_silent_on_shapes_that_do_not_fan_out() {
    for sql in [
        "select distinct x.state from {{ ref('a') }} as x",
        "select distinct x.state, y.state from {{ ref('a') }} as x join {{ ref('b') }} as y on x.id = y.k",
        "select distinct x.state from {{ ref('a') }} as x cross join {{ ref('b') }} as y",
        "select x.state from {{ ref('a') }} as x join {{ ref('b') }} as y on x.id = y.k",
    ] {
        assert!(fanout(sql).is_empty(), "{sql}");
    }
}

#[test]
fn distinct_fanout_with_a_dynamic_relation_is_silent() {
    let sql =
        "select distinct x.state from {{ ref('a') }} as x join {{ my_rel() }} as y on x.id = y.k";
    assert!(fanout(sql).is_empty());
}

// model.unused_cte

#[test]
fn unused_cte_is_deleted_with_its_comma() {
    let sql = "with dead as (select 1 as v), live as (select 2 as v) select v from live";
    let finding = only(cte(sql));
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.span, cogitator_ir::Span::new(5, 9));
    let fixed = apply(sql, fix_of(&finding, FixSafety::Preserving));
    assert_eq!(fixed, "with live as (select 2 as v) select v from live");
    assert!(cte(&fixed).is_empty());
}

#[test]
fn unused_cte_flags_a_chain_but_only_the_outermost_can_be_deleted() {
    let sql = "with inner_one as (select 1 as v), outer_one as (select v from inner_one), live as (select 2 as v) select v from live";
    let found = cte(sql);
    assert_eq!(found.len(), 2, "{found:#?}");
    let of = |name: &str| {
        found
            .iter()
            .find(|f| f.message.contains(name))
            .expect("finding")
    };
    assert!(of("inner_one").fixes.is_empty(), "outer_one still names it");
    assert!(!of("outer_one").fixes.is_empty());
}

#[test]
fn unused_cte_does_not_fix_the_last_or_only_cte() {
    let sql = "with live as (select 1 as v), dead as (select 2 as v) select v from live";
    assert!(only(cte(sql)).fixes.is_empty());
    let sole = "with dead as (select 1 as v) select 2 as v";
    assert!(only(cte(sole)).fixes.is_empty());
}

#[test]
fn unused_cte_keeps_a_comment_it_would_otherwise_delete() {
    let sql = "with dead as (select 1 as v),\n-- this one is live\nlive as (select 2 as v) select v from live";
    assert!(only(cte(sql)).fixes.is_empty());
}

#[test]
fn unused_cte_is_silent_on_used_ctes_and_nested_scopes() {
    for sql in [
        "with a1 as (select 1 as v), b1 as (select v from a1) select v from b1",
        "with a1 as (select 1 as v) select (select max(v) from a1) as m",
        "select * from (with a1 as (select 1 as v) select v from a1) as t",
        "with a1 as (select 1 as v) select 1 as x union all select v from a1",
    ] {
        assert!(cte(sql).is_empty(), "{sql}");
    }
}

#[test]
fn unused_cte_with_a_ref_body_reports_and_fixes() {
    let sql = "with dead as (select * from {{ ref('a') }}), live as (select id from {{ ref('b') }}) select id from live";
    let finding = only(cte(sql));
    assert!(!finding.fixes.is_empty());
}

#[test]
fn unused_cte_stays_silent_when_a_reference_could_be_hidden() {
    for sql in [
        "with dead as (select 1 as v) select * from {{ my_macro() }}",
        "with dead as (select 1 as v) select v from t {{ extra_clause() }}",
        "with dead as (select 1 as v), live as (select 1 as v) select v from live {% if is_incremental() %} where v > 0 {% endif %}",
    ] {
        assert!(cte(sql).is_empty(), "{sql}");
    }
}

#[test]
fn unused_cte_lowers_its_wording_around_a_scalar_macro() {
    let sql = "with dead as (select 1 as v), live as (select 2 as v) select {{ cents('v') }} as d from live";
    let finding = only(cte(sql));
    assert_eq!(finding.severity, Severity::Hint);
    assert_eq!(finding.evidence, Evidence::Heuristic);
    assert!(finding.fixes.is_empty());
    // A macro in WHERE sits in predicate position, so the same applies.
    let predicate = "with dead as (select 1 as v) select v from t where {{ my_filter() }}";
    assert_eq!(only(cte(predicate)).severity, Severity::Hint);
}

#[test]
fn unused_cte_is_skipped_when_not_every_branch_is_visible() {
    let sql = "with dead as (select 1 as v) select 2 as x";
    let out = run_raw(sql, schemas(), Needs::NONE.with(Need::Tests));
    assert!(out
        .findings
        .iter()
        .all(|f| f.id.as_str() != "model.unused_cte"));
    let skipped = out
        .trace
        .entries
        .iter()
        .find(|e| e.rule.as_str() == "model.unused_cte");
    assert!(
        matches!(skipped.map(|e| &e.outcome), Some(Outcome::Skipped { .. })),
        "{skipped:?}"
    );
}

// set.union_distinct

#[test]
fn union_distinct_offers_an_intent_changing_fix_by_default() {
    let sql = "select id from {{ ref('a') }} union select id from {{ ref('b') }}";
    let finding = only(union(sql));
    assert_eq!(finding.severity, Severity::Info);
    let fix = fix_of(&finding, FixSafety::ChangesIntent);
    assert_eq!(
        apply(sql, fix),
        "select id from {{ ref('a') }} union all select id from {{ ref('b') }}"
    );
}

#[test]
fn union_distinct_preserves_only_with_disjoint_duplicate_free_arms() {
    let sql = "select distinct 'a' as src, id from {{ ref('a') }} UNION select distinct 'b' as src, id from {{ ref('b') }}";
    let finding = only(union(sql));
    assert_eq!(finding.evidence, Evidence::Exact);
    let fix = fix_of(&finding, FixSafety::Preserving);
    assert!(apply(sql, fix).contains("UNION ALL"), "{fix:?}");
    assert!(finding
        .fixes
        .iter()
        .all(|f| f.safety != FixSafety::ChangesIntent));
}

#[test]
fn union_distinct_literals_without_arm_dedupe_are_not_preserving() {
    let sql =
        "select 'a' as src, id from {{ ref('a') }} union select 'b' as src, id from {{ ref('b') }}";
    assert_eq!(only(union(sql)).fixes[0].safety, FixSafety::ChangesIntent);
}

#[test]
fn union_distinct_group_by_arms_count_as_duplicate_free() {
    let sql = "select 'a' as src, id, count(*) as n from {{ ref('a') }} group by id union select 'b' as src, id, count(*) as n from {{ ref('b') }} group by id";
    assert!(!only(union(sql)).fixes.is_empty());
    assert_eq!(only(union(sql)).fixes[0].safety, FixSafety::Preserving);
}

#[test]
fn union_distinct_is_silent_when_dedupe_may_be_needed() {
    for sql in [
        "select id from {{ ref('a') }} where id > 1 union select id from {{ ref('a') }} where id < 5",
        "select id from {{ ref('a') }} union all select id from {{ ref('b') }}",
    ] {
        assert!(union(sql).is_empty(), "{sql}");
    }
}

#[test]
fn union_distinct_does_not_need_closed_arms() {
    let sql = "select id from orders union select id from customers";
    let finding = only(union(sql));
    assert_eq!(finding.evidence, Evidence::Heuristic);
    assert_eq!(finding.fixes[0].safety, FixSafety::ChangesIntent);
}

#[test]
fn union_distinct_with_an_opaque_arm_stays_silent() {
    let sql = "select id from generate_series(1, 2) union select id from {{ ref('b') }}";
    assert!(
        union(sql).is_empty(),
        "a table function hides what it reads"
    );
}

#[test]
fn union_distinct_discriminators_need_closed_arms_to_be_preserving() {
    let closed = "select 'a' as src, id from {{ ref('a') }} group by id union select 'b' as src, id from {{ ref('b') }} group by id";
    assert_eq!(only(union(closed)).fixes[0].safety, FixSafety::Preserving);
    // The same shape over open tables keeps the P proof off: it needs positions.
    let open = "select 'a' as src, id from orders group by id union select 'b' as src, id from customers group by id";
    assert_eq!(only(union(open)).fixes[0].safety, FixSafety::ChangesIntent);
}

#[test]
fn union_distinct_withholds_the_edit_it_cannot_place() {
    let commented = "select id from {{ ref('a') }} union /* keep */ select id from {{ ref('b') }}";
    assert!(only(union(commented)).fixes.is_empty());
}

#[test]
fn union_distinct_covers_a_chain_of_arms_with_every_keyword() {
    let sql = "select id from {{ ref('a') }} union select id from {{ ref('b') }} union select id from {{ ref('c') }}";
    let finding = only(union(sql));
    let fixed = apply(sql, fix_of(&finding, FixSafety::ChangesIntent));
    assert_eq!(fixed.matches("union all").count(), 2, "{fixed}");
}

#[test]
fn union_distinct_with_literal_arms_of_mixed_type_is_not_disjoint() {
    let sql = "select distinct 1 as src, id from {{ ref('a') }} union select distinct '1' as src, id from {{ ref('b') }}";
    assert_eq!(only(union(sql)).fixes[0].safety, FixSafety::ChangesIntent);
}
