//! The physical-design rules and the date-range quick fix over real SQL.

mod physical_support;

use cogitator_ir::common::RuleId;
use cogitator_ir::lir::{Backend, KeyKind};
use cogitator_lower::backends::bigquery;
use cogitator_rules::{Evidence, Need, Outcome, Severity, SkipReason};
use physical_support::{hir_of, no_findings, only, run, run_hir, run_with, Case, Options, Run};

const PARTITION_OR: &str = "filter.partition_or";
const CLUSTER_ORDER: &str = "filter.cluster_order";
const NON_SARGABLE: &str = "filter.non_sargable";
const LIMIT: &str = "limit.no_scan_reduction";
const UNIQUE_KEY: &str = "incr.no_unique_key";

const PARTITION_TS: &[(&str, KeyKind)] = &[("ts", KeyKind::Partition)];
const CLUSTER_AB: &[(&str, KeyKind)] = &[("a", KeyKind::Cluster), ("b", KeyKind::Cluster)];
const CLUSTER_A: &[(&str, KeyKind)] = &[("a", KeyKind::Cluster)];
const COSTS_DOC: &str = "https://docs.cloud.google.com/bigquery/docs/best-practices-costs";
const STRATEGY_DOC: &str = "https://docs.getdbt.com/docs/build/incremental-strategy";

const NAMED: [Backend; 4] = [
    Backend::Snowflake,
    Backend::DuckDb,
    Backend::Postgres,
    Backend::BigQuery,
];

fn bq(sql: &str, keys: &'static [(&'static str, KeyKind)]) -> Run {
    run(Case::new(sql, Backend::BigQuery, keys))
}

fn skipped(run: &Run, id: &'static str, need: Need) {
    let entry = run
        .out
        .trace
        .entries
        .iter()
        .find(|e| e.rule == RuleId(id))
        .expect("traced");
    assert_eq!(
        entry.outcome,
        Outcome::Skipped {
            reason: SkipReason::Unmet(need)
        }
    );
}

// filter.partition_or

const OR_ACROSS: &str =
    "select id from {{ ref('events') }} where ts > TIMESTAMP '2024-01-01' or a = 1";

#[test]
fn or_across_columns_on_a_partition_key_warns_with_the_doc() {
    let r = bq(OR_ACROSS, PARTITION_TS);
    let f = only(&r, PARTITION_OR);
    assert_eq!(f.severity, Severity::Warning);
    assert_eq!(f.evidence, Evidence::Exact);
    assert!(
        f.message.contains("partition pruning won't work"),
        "{}",
        f.message
    );
    assert!(f.message.contains(bigquery::PARTITION_DOC), "{}", f.message);
    assert!(f.fixes.is_empty());
}

#[test]
fn or_rule_stays_silent_when_the_partition_key_is_still_usable() {
    for sql in [
        "select id from {{ ref('events') }} where ts > TIMESTAMP '2024-01-01' and a = 1",
        "select id from {{ ref('events') }} \
         where ts > TIMESTAMP '2024-01-01' or ts < TIMESTAMP '2020-01-01'",
    ] {
        no_findings(&bq(sql, PARTITION_TS), PARTITION_OR);
    }
}

#[test]
fn or_rule_makes_no_claim_on_jinja_or_an_open_schema() {
    let jinja = "select id from {{ ref('events') }} where ts > {{ var('d') }} or {{ var('c') }}";
    no_findings(&bq(jinja, PARTITION_TS), PARTITION_OR);

    let mut open = Case::new(OR_ACROSS, Backend::BigQuery, PARTITION_TS);
    open.open_schema = true;
    no_findings(&run(open), PARTITION_OR);
}

#[test]
fn or_rule_claims_nothing_for_cluster_keys_or_other_dialects() {
    no_findings(&bq(OR_ACROSS, &[("ts", KeyKind::Cluster)]), PARTITION_OR);
    for backend in [
        Backend::Snowflake,
        Backend::DuckDb,
        Backend::Postgres,
        Backend::Generic,
    ] {
        no_findings(
            &run(Case::new(OR_ACROSS, backend, PARTITION_TS)),
            PARTITION_OR,
        );
    }
}

// filter.cluster_order

const SKIPS_A: &str = "select id from {{ ref('events') }} where b = 1";

#[test]
fn a_filter_skipping_the_leading_cluster_key_is_a_hint_with_the_doc() {
    let r = bq(SKIPS_A, CLUSTER_AB);
    let f = only(&r, CLUSTER_ORDER);
    assert_eq!(f.severity, Severity::Hint);
    assert_eq!(f.evidence, Evidence::Unknown);
    assert!(
        f.message.contains("probably won't be as effective"),
        "{}",
        f.message
    );
    assert!(f.message.contains(bigquery::CLUSTER_DOC), "{}", f.message);
}

#[test]
fn a_skipped_cluster_key_is_reported_once_across_both_rules() {
    let r = bq(SKIPS_A, CLUSTER_AB);
    only(&r, CLUSTER_ORDER);
    no_findings(&r, NON_SARGABLE);
    // A wrapped later key is non-sargable, not an ordering problem.
    let wrapped = "select id from {{ ref('events') }} where a = 1 and my_fn(b) = 1";
    let r = bq(wrapped, CLUSTER_AB);
    no_findings(&r, CLUSTER_ORDER);
    only(&r, NON_SARGABLE);
}

#[test]
fn filtering_clustering_columns_in_order_is_silent() {
    for sql in [
        "select id from {{ ref('events') }} where a = 1",
        "select id from {{ ref('events') }} where a = 1 and b = 2",
    ] {
        no_findings(&bq(sql, CLUSTER_AB), CLUSTER_ORDER);
    }
}

#[test]
fn cluster_order_makes_no_claim_on_a_jinja_value() {
    let sql = "select id from {{ ref('events') }} where b = {{ var('x') }}";
    no_findings(&bq(sql, CLUSTER_AB), CLUSTER_ORDER);
}

#[test]
fn cluster_order_is_bigquery_only() {
    for backend in [
        Backend::Snowflake,
        Backend::DuckDb,
        Backend::Postgres,
        Backend::Generic,
    ] {
        no_findings(&run(Case::new(SKIPS_A, backend, CLUSTER_AB)), CLUSTER_ORDER);
    }
}

// limit.no_scan_reduction

const LIMITED: &str = "select id from {{ ref('events') }} limit 10";

#[test]
fn limit_on_a_non_clustered_table_is_info_with_the_doc() {
    for keys in [PARTITION_TS, &[]] {
        let r = bq(LIMITED, keys);
        let f = only(&r, LIMIT);
        assert_eq!(f.severity, Severity::Info);
        assert_eq!(f.evidence, Evidence::Exact);
        assert!(f.message.contains(COSTS_DOC), "{}", f.message);
    }
}

#[test]
fn limit_on_a_clustered_table_is_silent() {
    no_findings(&bq(LIMITED, CLUSTER_A), LIMIT);
}

#[test]
fn limit_with_no_known_facts_is_silent() {
    let case = Case::new(LIMITED, Backend::BigQuery, PARTITION_TS);
    let options = Options {
        with_facts: false,
        ..Options::default()
    };
    no_findings(&run_with(case, options), LIMIT);
}

#[test]
fn limit_makes_no_claim_on_jinja_zero_or_open_schemas() {
    let jinja = "select id from {{ ref('events') }} limit {{ var('n') }}";
    no_findings(&bq(jinja, PARTITION_TS), LIMIT);
    let zero = "select id from {{ ref('events') }} limit 0";
    no_findings(&bq(zero, PARTITION_TS), LIMIT);

    let mut open = Case::new(LIMITED, Backend::BigQuery, PARTITION_TS);
    open.open_schema = true;
    no_findings(&run(open), LIMIT);
}

#[test]
fn limit_is_quiet_without_a_limit_and_on_other_dialects() {
    let no_limit = "select id from {{ ref('events') }}";
    no_findings(&bq(no_limit, PARTITION_TS), LIMIT);
    for backend in [
        Backend::Snowflake,
        Backend::DuckDb,
        Backend::Postgres,
        Backend::Generic,
    ] {
        no_findings(&run(Case::new(LIMITED, backend, PARTITION_TS)), LIMIT);
    }
}

// incr.no_unique_key

fn incremental(args: &str) -> String {
    format!(
        "{{{{ config(materialized='incremental'{args}) }}}}\n\
         select id, ts from {{{{ ref('events') }}}}\n\
         {{% if is_incremental() %}}where ts > TIMESTAMP '2024-01-01'{{% endif %}}"
    )
}

#[test]
fn default_strategy_per_adapter() {
    let sql = incremental("");
    let rows = [
        (Backend::BigQuery, true),
        (Backend::Snowflake, true),
        (Backend::Postgres, false),
        (Backend::DuckDb, false),
        (Backend::Generic, false),
    ];
    for (backend, fires) in rows {
        let r = run(Case::new(&sql, backend, &[]));
        if fires {
            let f = only(&r, UNIQUE_KEY);
            assert_eq!(f.severity, Severity::Warning, "{backend:?}");
            assert_eq!(f.evidence, Evidence::Heuristic, "{backend:?}");
            assert!(f.message.contains(STRATEGY_DOC), "{}", f.message);
            assert!(f.message.contains("default"), "{}", f.message);
        } else {
            no_findings(&r, UNIQUE_KEY);
        }
    }
}

#[test]
fn an_explicit_merge_style_strategy_fires_on_every_named_adapter() {
    for strategy in ["merge", "delete+insert"] {
        let sql = incremental(&format!(", incremental_strategy='{strategy}'"));
        for backend in NAMED {
            let r = run(Case::new(&sql, backend, &[]));
            let f = only(&r, UNIQUE_KEY);
            assert_eq!(f.severity, Severity::Warning, "{backend:?}");
            assert_eq!(f.evidence, Evidence::Exact, "{backend:?}");
            assert!(f.message.contains(strategy), "{}", f.message);
        }
        no_findings(&run(Case::new(&sql, Backend::Generic, &[])), UNIQUE_KEY);
    }
}

#[test]
fn append_and_other_strategies_stay_silent() {
    for strategy in ["append", "insert_overwrite", "microbatch"] {
        let sql = incremental(&format!(", incremental_strategy='{strategy}'"));
        for backend in NAMED {
            no_findings(&run(Case::new(&sql, backend, &[])), UNIQUE_KEY);
        }
    }
}

#[test]
fn a_unique_key_silences_the_rule() {
    let sql = incremental(", incremental_strategy='merge', unique_key='id'");
    for backend in NAMED {
        no_findings(&run(Case::new(&sql, backend, &[])), UNIQUE_KEY);
    }
}

#[test]
fn config_this_pipeline_cannot_read_is_no_claim() {
    for args in [
        ", unique_key=var('key')",
        ", incremental_strategy=var('s')",
        ", **shared_config()",
    ] {
        let sql = incremental(args);
        no_findings(&run(Case::new(&sql, Backend::BigQuery, &[])), UNIQUE_KEY);
    }
}

#[test]
fn the_manifest_can_supply_the_unique_key() {
    let sql = incremental("");
    let case = Case::new(&sql, Backend::BigQuery, &[]);
    let mut hir = hir_of(&case);
    let key = hir.syms.intern("id");
    hir.config.unique_key = vec![key];
    no_findings(&run_hir(&case, &hir, Options::default()), UNIQUE_KEY);
}

#[test]
fn a_table_model_is_silent() {
    let sql = "{{ config(materialized='table', incremental_strategy='merge') }}\n\
               select id from {{ ref('events') }}";
    no_findings(&run(Case::new(sql, Backend::BigQuery, &[])), UNIQUE_KEY);
}

#[test]
fn an_unmet_need_is_skipped() {
    let sql = incremental("");
    let mut case = Case::new(&sql, Backend::BigQuery, &[]);
    case.without = Some(Need::Mat);
    let r = run(case);
    no_findings(&r, UNIQUE_KEY);
    skipped(&r, UNIQUE_KEY, Need::Mat);
}
