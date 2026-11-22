//! The `date(ts) = d` quick fix of `filter.non_sargable`, verified against
//! the document text.

mod physical_support;

use cogitator_ir::lir::{Backend, KeyKind};
use cogitator_ir::origin::{SourceOrigin, SpanMap, Staleness};
use cogitator_ir::types::SqlType;
use cogitator_rules::FixSafety;
use physical_support::{hir_of, only, run, run_hir, run_with, Case, Options, Run};

const NON_SARGABLE: &str = "filter.non_sargable";
const PARTITION_TS: &[(&str, KeyKind)] = &[("ts", KeyKind::Partition)];
const DATE_RANGE: &str = "ts >= TIMESTAMP(DATE '2024-01-01') AND \
     ts < TIMESTAMP(DATE_ADD(DATE '2024-01-01', INTERVAL 1 DAY))";

fn date_sql(predicate: &str) -> String {
    format!("select id from {{{{ ref('events') }}}} where {predicate} and a = 1")
}

fn apply(sql: &str, r: &Run) -> String {
    let f = only(r, NON_SARGABLE);
    let [fix] = f.fixes.as_slice() else {
        panic!("one fix: {f:?}");
    };
    assert_eq!(fix.safety, FixSafety::Preserving);
    let [edit] = fix.edits.as_slice() else {
        panic!("one edit: {fix:?}");
    };
    let (start, end) = (edit.span.start as usize, edit.span.end as usize);
    format!("{}{}{}", &sql[..start], edit.new_text, &sql[end..])
}

#[test]
fn the_fix_replaces_exactly_the_predicate() {
    for predicate in [
        "date(ts) = DATE '2024-01-01'",
        "DATE(ts)   =   date '2024-01-01'",
        "DATE '2024-01-01' = date(ts)",
        "cast(ts as date) = DATE '2024-01-01'",
    ] {
        let sql = date_sql(predicate);
        let r = run(Case::new(&sql, Backend::BigQuery, PARTITION_TS));
        assert_eq!(apply(&sql, &r), date_sql(DATE_RANGE), "{predicate}");
    }
}

#[test]
fn the_fix_writes_the_constructor_for_the_column_type() {
    let sql = date_sql("date(ts) = DATE '2024-01-01'");
    let datetime = physical_support::base::datetime();
    let mut case = Case::new(&sql, Backend::BigQuery, PARTITION_TS);
    case.ts_type = &datetime;
    assert!(apply(&sql, &run(case)).contains("ts >= DATETIME(DATE '2024-01-01')"));
}

#[test]
fn the_fix_is_withheld_when_the_text_does_not_verify() {
    for predicate in [
        "date(/*c*/ts) = DATE '2024-01-01'",
        "(date(ts)) = DATE '2024-01-01'",
        "date(ts) = (DATE '2024-01-01')",
        "date(ts) /*c*/ = DATE '2024-01-01'",
        "date(ts) = DATE \"2024-01-01\"",
    ] {
        let sql = date_sql(predicate);
        let r = run(Case::new(&sql, Backend::BigQuery, PARTITION_TS));
        assert!(only(&r, NON_SARGABLE).fixes.is_empty(), "{predicate}");
    }
}

#[test]
fn the_fix_needs_the_document_text() {
    let sql = date_sql("date(ts) = DATE '2024-01-01'");
    let case = Case::new(&sql, Backend::BigQuery, PARTITION_TS);
    let options = Options {
        with_text: false,
        ..Options::default()
    };
    assert!(only(&run_with(case, options), NON_SARGABLE)
        .fixes
        .is_empty());
}

#[test]
fn the_fix_needs_a_known_timestamp_and_a_date_literal() {
    let sql = date_sql("date(ts) = DATE '2024-01-01'");
    for ty in [SqlType::Unknown, SqlType::Date, SqlType::Text] {
        let mut case = Case::new(&sql, Backend::BigQuery, PARTITION_TS);
        case.ts_type = &ty;
        assert!(only(&run(case), NON_SARGABLE).fixes.is_empty(), "{ty:?}");
    }
    let string_value = date_sql("date(ts) = '2024-01-01'");
    let r = run(Case::new(&string_value, Backend::BigQuery, PARTITION_TS));
    assert!(only(&r, NON_SARGABLE).fixes.is_empty());
}

#[test]
fn the_fix_is_withheld_on_compiled_sql_and_other_dialects() {
    let compiled = SourceOrigin::Compiled {
        staleness: Staleness::Fresh,
        map: SpanMap::default(),
    };
    let sql = date_sql("date(ts) = DATE '2024-01-01'");
    let case = Case::new(&sql, Backend::BigQuery, PARTITION_TS);
    let mut hir = hir_of(&case);
    hir.origin = compiled;
    let r = run_hir(&case, &hir, Options::default());
    assert!(only(&r, NON_SARGABLE).fixes.is_empty());

    let keys = [("ts", KeyKind::Cluster)];
    let r = run(Case::new(&sql, Backend::Snowflake, &keys));
    assert!(only(&r, NON_SARGABLE).fixes.is_empty());
}
