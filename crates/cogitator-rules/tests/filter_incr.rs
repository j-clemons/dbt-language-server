//! `filter.non_sargable` over real SQL, one row per claim the rule may
//! or may not make.

mod filter_support;

use cogitator_ir::common::RuleId;
use cogitator_ir::lir::{Backend, KeyKind, PhysOp, PruneUse, Pruning};
use cogitator_ir::mir::{ColId, RelOp, ScalarId};
use cogitator_ir::types::SqlType;
use cogitator_lower::backends::{bigquery, for_backend, postgres};
use cogitator_lower::decide_pruning;
use cogitator_rules::{Evidence, Need, Outcome, Severity, SkipReason};
use filter_support::{datetime, no_findings, only, run, Case, Run};

const NON_SARGABLE: &str = "filter.non_sargable";

const PARTITION_TS: &[(&str, KeyKind)] = &[("ts", KeyKind::Partition)];
const DATE_FILTER: &str = "select id from {{ ref('events') }} where date(ts) = DATE '2024-01-01'";

fn usage(run: &Run, name: &str) -> PruneUse {
    let lir = run.lir();
    let mir = &run.out.mir;
    lir.nodes
        .values()
        .find_map(|node| match &node.op {
            PhysOp::TableScan { pruning, .. } => pruning
                .iter()
                .find(|k| mir.cols[k.col].name.map(|n| mir.syms.text(n)) == Some(name))
                .map(|k| k.usage),
            _ => None,
        })
        .expect("a key of that name")
}

#[test]
fn blocked_function_on_bigquery_partition_is_a_warning_with_the_doc() {
    let sql = "select id from {{ ref('events') }} where unix_seconds(ts) > 100";
    let r = run(Case::new(sql, Backend::BigQuery, PARTITION_TS));
    assert!(matches!(usage(&r, "ts"), PruneUse::Blocked { .. }));
    let f = only(&r, NON_SARGABLE);
    assert_eq!(f.severity, Severity::Warning);
    assert_eq!(f.evidence, Evidence::Exact);
    assert!(f.message.contains("unix_seconds"), "{}", f.message);
    assert!(f.message.contains(bigquery::PARTITION_DOC), "{}", f.message);
    assert!(f.fixes.is_empty());
}

#[test]
fn a_bare_range_makes_no_claim() {
    let sql = "select id from {{ ref('events') }} where ts >= TIMESTAMP '2024-01-01'";
    let r = run(Case::new(sql, Backend::BigQuery, PARTITION_TS));
    assert!(matches!(usage(&r, "ts"), PruneUse::Used(_)));
    no_findings(&r, NON_SARGABLE);
}

#[test]
fn jinja_inputs_keep_the_claim_and_jinja_values_drop_it() {
    let wrapped = "select id from {{ ref('events') }} \
                   where unix_seconds(ts) > 100 and id = {{ var('x') }}";
    only(
        &run(Case::new(wrapped, Backend::BigQuery, PARTITION_TS)),
        NON_SARGABLE,
    );
    let jinja_value = "select id from {{ ref('events') }} where date(ts) = {{ var('d') }}";
    no_findings(
        &run(Case::new(jinja_value, Backend::BigQuery, PARTITION_TS)),
        NON_SARGABLE,
    );
}

#[test]
fn per_dialect_table() {
    const SQL: &str = "select id from {{ ref('events') }} where my_fn(ts) = 1";
    let rows: [(Backend, KeyKind, Option<Severity>, &str); 5] = [
        (
            Backend::BigQuery,
            KeyKind::Partition,
            Some(Severity::Warning),
            "full table scan",
        ),
        (
            Backend::Snowflake,
            KeyKind::Cluster,
            Some(Severity::Hint),
            "may reduce pruning",
        ),
        (
            Backend::DuckDb,
            KeyKind::Sort,
            Some(Severity::Hint),
            "may reduce pruning",
        ),
        (
            Backend::Postgres,
            KeyKind::Sort,
            Some(Severity::Hint),
            postgres::EXPRESSION_DOC,
        ),
        (Backend::Generic, KeyKind::Partition, None, ""),
    ];
    for (backend, kind, severity, wording) in rows {
        let keys = [("ts", kind)];
        let r = run(Case::new(SQL, backend, &keys));
        match severity {
            Some(severity) => {
                let f = only(&r, NON_SARGABLE);
                assert_eq!(f.severity, severity, "{backend:?}");
                assert!(f.message.contains(wording), "{backend:?}: {}", f.message);
            }
            None => no_findings(&r, NON_SARGABLE),
        }
    }
}

#[test]
fn snowflake_stays_at_hint_with_no_blocking_claim() {
    let sql = "select id from {{ ref('events') }} where to_date(ts) = '2024-01-01'";
    let keys = [("ts", KeyKind::Cluster)];
    let r = run(Case::new(sql, Backend::Snowflake, &keys));
    let f = only(&r, NON_SARGABLE);
    assert_eq!(f.severity, Severity::Hint);
    assert_eq!(f.evidence, Evidence::Unknown);
    assert!(f.message.contains("may reduce pruning"), "{}", f.message);
    assert!(!f.message.to_lowercase().contains("block"), "{}", f.message);
}

/// The conjunct over `ts`, and the verdict `decide_pruning` gives it.
fn verdict(r: &Run, backend: Backend) -> Pruning {
    let mir = &r.out.mir;
    let (scalar, col): (ScalarId, ColId) = mir
        .reachable()
        .into_iter()
        .find_map(|id| match &mir.rels[id].op {
            RelOp::Filter { input, conjuncts } => {
                let cols = &mir.rels[*input].out;
                let ts = cols
                    .iter()
                    .copied()
                    .find(|c| mir.cols[*c].name.map(|n| mir.syms.text(n)) == Some("ts"))?;
                Some((*conjuncts.first()?, ts))
            }
            _ => None,
        })
        .expect("a filter over ts");
    let column = cogitator_ir::lir::PhysColumn {
        name: mir.cols[col].name.expect("named"),
        ty: SqlType::Unknown,
        key: Some(KeyKind::Partition),
    };
    let backend = for_backend(backend).expect("registered");
    decide_pruning(backend, mir, scalar, col, &column)
        .expect("the predicate reads ts")
        .1
}

#[test]
fn date_of_ts_is_never_reported_as_blocked_on_bigquery() {
    let casts = "select id from {{ ref('events') }} where cast(ts as date) = DATE '2024-01-01'";
    for sql in [DATE_FILTER, casts] {
        let r = run(Case::new(sql, Backend::BigQuery, PARTITION_TS));
        assert_eq!(usage(&r, "ts"), PruneUse::Unknown, "{sql}");
        assert_eq!(verdict(&r, Backend::BigQuery), Pruning::Unknown, "{sql}");
        let f = only(&r, NON_SARGABLE);
        assert_eq!(f.severity, Severity::Hint, "{sql}");
        assert_eq!(f.evidence, Evidence::Unknown, "{sql}");
        assert!(f.message.contains("may reduce pruning"), "{}", f.message);
        assert!(!f.message.contains("full table scan"), "{}", f.message);
    }
}

#[test]
fn documented_trunc_forms_stay_unknown() {
    for sql in [
        "select id from {{ ref('events') }} where date_trunc(ts, day) = DATE '2024-01-01'",
        "select id from {{ ref('events') }} where timestamp_trunc(ts, day) = TIMESTAMP '2024-01-01'",
    ] {
        let r = run(Case::new(sql, Backend::BigQuery, PARTITION_TS));
        assert_eq!(verdict(&r, Backend::BigQuery), Pruning::Unknown, "{sql}");
        for finding in r.findings(NON_SARGABLE) {
            assert_eq!(finding.severity, Severity::Hint, "{sql}");
        }
    }
}

#[test]
fn the_range_rewrite_requires_a_known_timestamp() {
    const REWRITE: &str = "Comparing the column directly";
    let with = |ty: SqlType| {
        let mut case = Case::new(DATE_FILTER, Backend::BigQuery, PARTITION_TS);
        case.ts_type = &ty;
        run(case).out.findings
    };
    let timestamp = with(filter_support::timestamp());
    assert!(
        timestamp[0].message.contains(
            "ts >= TIMESTAMP(DATE '2024-01-01') AND \
             ts < TIMESTAMP(DATE_ADD(DATE '2024-01-01', INTERVAL 1 DAY))"
        ),
        "{}",
        timestamp[0].message
    );
    assert!(with(datetime())[0].message.contains("ts >= DATETIME("));
    for ty in [SqlType::Unknown, SqlType::Date, SqlType::Text] {
        let findings = with(ty.clone());
        assert!(!findings[0].message.contains(REWRITE), "{ty:?}");
    }
}

#[test]
fn no_fix_is_offered_without_an_exact_span() {
    let r = run(Case::new(DATE_FILTER, Backend::BigQuery, PARTITION_TS));
    assert!(only(&r, NON_SARGABLE).fixes.is_empty());
}

#[test]
fn skipping_a_leading_cluster_key_is_reported_by_cluster_order() {
    let keys = [("a", KeyKind::Cluster), ("b", KeyKind::Cluster)];
    // `a` read or not: required-column pruning removes an unread `a` from
    // the scan, which must not hide that it was skipped.
    for sql in [
        "select id from {{ ref('events') }} where b = 1",
        "select a, id from {{ ref('events') }} where b = 1",
    ] {
        let r = run(Case::new(sql, Backend::BigQuery, &keys));
        no_findings(&r, NON_SARGABLE);
        let f = only(&r, "filter.cluster_order");
        assert_eq!(f.severity, Severity::Hint, "{sql}");
        assert_eq!(f.evidence, Evidence::Unknown, "{sql}");
        assert!(
            f.message.contains("probably won't be as effective"),
            "{}",
            f.message
        );
        assert!(f.message.contains(bigquery::CLUSTER_DOC), "{}", f.message);
    }
    let read = "select a, id from {{ ref('events') }} where b = 1";
    assert_eq!(
        usage(&run(Case::new(read, Backend::BigQuery, &keys)), "b"),
        PruneUse::Unknown
    );

    let sql = "select id from {{ ref('events') }} where a = 1 and b = 1";
    no_findings(&run(Case::new(sql, Backend::BigQuery, &keys)), NON_SARGABLE);
}

#[test]
fn opaque_forms_and_open_schemas_make_no_claim() {
    let extract = "select id from {{ ref('events') }} where extract(year from ts) = 2024";
    no_findings(
        &run(Case::new(extract, Backend::BigQuery, PARTITION_TS)),
        NON_SARGABLE,
    );
    let arith_over_opaque =
        "select id from {{ ref('events') }} where extract(year from ts) + 1 = 2025";
    no_findings(
        &run(Case::new(
            arith_over_opaque,
            Backend::BigQuery,
            PARTITION_TS,
        )),
        NON_SARGABLE,
    );
    let mut open = Case::new(DATE_FILTER, Backend::BigQuery, PARTITION_TS);
    open.open_schema = true;
    no_findings(&run(open), NON_SARGABLE);
}

#[test]
fn unmet_needs_are_skipped_through_the_pass_manager() {
    let mut case = Case::new(DATE_FILTER, Backend::BigQuery, PARTITION_TS);
    case.without = Some(Need::Types);
    let r = run(case);
    no_findings(&r, NON_SARGABLE);
    let entry = r
        .out
        .trace
        .entries
        .iter()
        .find(|e| e.rule == RuleId(NON_SARGABLE))
        .expect("traced");
    assert_eq!(
        entry.outcome,
        Outcome::Skipped {
            reason: SkipReason::Unmet(Need::Types)
        }
    );
}
