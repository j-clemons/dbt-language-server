//! `incr.missing_guard` and `incr.subquery_watermark` over real SQL.

mod filter_support;

use cogitator_ir::lir::{Backend, KeyKind};
use cogitator_ir::origin::{SourceOrigin, SpanMap, Staleness, TemplateVariant};
use cogitator_lower::backends::{bigquery, snowflake};
use cogitator_rules::{Evidence, Severity};
use filter_support::{hir_of, no_findings, only, run, run_on, Case};

const MISSING_GUARD: &str = "incr.missing_guard";
const WATERMARK: &str = "incr.subquery_watermark";
const PARTITION_TS: &[(&str, KeyKind)] = &[("ts", KeyKind::Partition)];

const INCREMENTAL: &str = "{{ config(materialized='incremental') }}\n";

fn incr(body: &str) -> String {
    format!("{INCREMENTAL}{body}")
}

#[test]
fn incremental_model_without_a_guard_is_warned() {
    let sql = incr("select id from {{ ref('events') }}");
    let r = run(Case::new(&sql, Backend::DuckDb, &[]));
    let f = only(&r, MISSING_GUARD);
    assert_eq!(f.severity, Severity::Warning);
    assert_eq!(f.evidence, Evidence::Exact);
    assert_eq!((f.span.start, f.span.end), (0, 40));
}

#[test]
fn a_guard_or_another_materialization_is_silent() {
    let guarded = incr(
        "select id from {{ ref('events') }}\n\
         {% if is_incremental() %}where id > 1{% endif %}",
    );
    no_findings(
        &run(Case::new(&guarded, Backend::DuckDb, &[])),
        MISSING_GUARD,
    );
    let table = "{{ config(materialized='table') }}\nselect id from {{ ref('events') }}";
    no_findings(&run(Case::new(table, Backend::DuckDb, &[])), MISSING_GUARD);
}

#[test]
fn jinja_the_rule_cannot_see_into_is_silent() {
    let macro_where = incr("select id from {{ ref('events') }} where {{ my_filter() }}");
    no_findings(
        &run(Case::new(&macro_where, Backend::DuckDb, &[])),
        MISSING_GUARD,
    );
    let macro_clause = incr("select id from {{ ref('events') }} {{ incremental_clause() }}");
    no_findings(
        &run(Case::new(&macro_clause, Backend::DuckDb, &[])),
        MISSING_GUARD,
    );
    let scalar_var = incr("select id, {{ var('x') }} as v from {{ ref('events') }}");
    only(
        &run(Case::new(&scalar_var, Backend::DuckDb, &[])),
        MISSING_GUARD,
    );
}

#[test]
fn other_control_flow_declines_the_hir_and_compiled_sql_is_silent() {
    let looped = incr("{% for x in [1] %}select id from {{ ref('events') }}{% endfor %}");
    assert!(hir_of(&Case::new(&looped, Backend::DuckDb, &[])).is_none());

    let compiled = SourceOrigin::Compiled {
        staleness: Staleness::Fresh,
        map: SpanMap::default(),
    };
    let sql = "select id from analytics.events";
    let mut case = Case::new(sql, Backend::DuckDb, &[]);
    case.origin = Some(&compiled);
    let mut hir = hir_of(&case).expect("a HIR");
    hir.config.materialized = Some(cogitator_ir::common::Materialization::Incremental);
    no_findings(&run_on(&case, &hir), MISSING_GUARD);
}

#[test]
fn full_refresh_variant_is_silent() {
    let sql = incr("select id from {{ ref('events') }}");
    let mut case = Case::new(&sql, Backend::DuckDb, &[]);
    case.variant = TemplateVariant::FullRefresh;
    no_findings(&run(case), MISSING_GUARD);
}

const WATERMARK_SQL: &str = "{{ config(materialized='incremental') }}\n\
    select id, ts from {{ ref('events') }}\n\
    {% if is_incremental() %}\n\
    where ts > (select max(ts) from {{ this }})\n\
    {% endif %}";

#[test]
fn watermark_on_bigquery_warns_once_and_names_dbt_max_partition() {
    let r = run(Case::new(WATERMARK_SQL, Backend::BigQuery, PARTITION_TS));
    let f = only(&r, WATERMARK);
    assert_eq!(f.severity, Severity::Warning);
    assert_eq!(f.evidence, Evidence::Exact);
    assert!(f.message.contains("_dbt_max_partition"), "{}", f.message);
    assert!(f.message.contains(bigquery::PARTITION_DOC), "{}", f.message);
    assert!(f.fixes.is_empty());
}

#[test]
fn watermark_on_snowflake_is_info_with_the_clustering_doc() {
    let keys = [("ts", KeyKind::Cluster)];
    let r = run(Case::new(WATERMARK_SQL, Backend::Snowflake, &keys));
    let f = only(&r, WATERMARK);
    assert_eq!(f.severity, Severity::Info);
    assert!(f.message.contains(snowflake::PRUNING_DOC), "{}", f.message);
    assert!(!f.message.contains("_dbt_max_partition"), "{}", f.message);
}

#[test]
fn watermark_makes_no_claim_where_the_docs_say_nothing() {
    let cluster = [("ts", KeyKind::Cluster)];
    no_findings(
        &run(Case::new(WATERMARK_SQL, Backend::BigQuery, &cluster)),
        WATERMARK,
    );
    no_findings(
        &run(Case::new(
            WATERMARK_SQL,
            Backend::DuckDb,
            &[("ts", KeyKind::Sort)],
        )),
        WATERMARK,
    );
    no_findings(
        &run(Case::new(WATERMARK_SQL, Backend::Generic, PARTITION_TS)),
        WATERMARK,
    );
}

#[test]
fn watermark_needs_the_incremental_variant_and_a_subquery() {
    let mut full = Case::new(WATERMARK_SQL, Backend::BigQuery, PARTITION_TS);
    full.variant = TemplateVariant::FullRefresh;
    no_findings(&run(full), WATERMARK);

    let constant = incr(
        "select id, ts from {{ ref('events') }}\n\
         {% if is_incremental() %}where ts > TIMESTAMP '2024-01-01'{% endif %}",
    );
    no_findings(
        &run(Case::new(&constant, Backend::BigQuery, PARTITION_TS)),
        WATERMARK,
    );
}

#[test]
fn watermark_ignores_subqueries_that_are_not_a_max_of_this() {
    let sql = incr(
        "select id, ts from {{ ref('events') }}\n\
         {% if is_incremental() %}\
         where ts > (select max(ts) from {{ ref('events') }})\
         {% endif %}",
    );
    no_findings(
        &run(Case::new(&sql, Backend::BigQuery, PARTITION_TS)),
        WATERMARK,
    );
}
