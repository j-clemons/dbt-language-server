//! The driver over real facts: YAML, manifest, catalog and the fixture
//! corpus.

use super::tests::Facts;
use crate::artifacts::catalog::{CatalogColumn, CatalogIndex, CatalogTable};
use crate::artifacts::manifest::ManifestConfig;

#[test]
fn yaml_columns_expand_a_star_and_stay_open() {
    let facts = Facts::new("duckdb").yaml("up", &[("id", Some("integer")), ("name", None)]);
    let plan = facts.plan("select * from {{ ref('up') }}");
    let mir = plan.levels.mir.text.as_deref().expect("mir");
    assert!(mir.contains("id") && mir.contains("name"), "{mir}");
    assert!(plan.needs.available.contains(&"schema"));
    assert!(plan.needs.available.contains(&"types"));
}

#[test]
fn a_partly_known_upstream_set_is_not_a_known_schema() {
    let facts = Facts::new("duckdb").yaml("up", &[("id", None)]);
    let plan =
        facts.plan("select u.id from {{ ref('up') }} u join {{ ref('other') }} o on u.id = o.id");
    assert!(plan.needs.missing.contains(&"schema"), "{:#?}", plan.needs);
    assert!(plan.needs.missing.contains(&"types"));
}

#[test]
fn manifest_config_fills_gaps_the_template_left() {
    let facts = Facts::new("bigquery").manifest(
        "m",
        ManifestConfig {
            materialized: Some("incremental".to_string()),
            partition_by: Some(serde_json::json!({"field": "day"})),
            ..ManifestConfig::default()
        },
    );
    let plan = facts.plan("select day, id from {{ ref('up') }}");
    assert!(plan.needs.available.contains(&"mat"));
    let lir = plan.levels.lir.text.as_deref().expect("lir");
    assert!(
        lir.contains("incremental") || lir.contains("Incremental"),
        "{lir}"
    );
    assert!(lir.contains("partition=[day"), "{lir}");
}

#[test]
fn the_templates_config_beats_the_manifest() {
    let facts = Facts::new("duckdb").manifest(
        "m",
        ManifestConfig {
            materialized: Some("table".to_string()),
            ..ManifestConfig::default()
        },
    );
    let plan = facts.plan("{{ config(materialized='view') }} select 1 as x");
    let lir = plan.levels.lir.text.as_deref().expect("lir");
    assert!(
        lir.to_lowercase().contains("view"),
        "the file's own config wins: {lir}"
    );
}

#[test]
fn physical_facts_come_from_the_manifest_and_catalog() {
    let mut facts = Facts::new("snowflake").manifest(
        "up",
        ManifestConfig {
            cluster_by: vec!["to_date(ts)".to_string()],
            ..ManifestConfig::default()
        },
    );
    let mut catalog = CatalogIndex::default();
    let mut table = CatalogTable {
        columns: vec![CatalogColumn {
            name: "A".to_string(),
            data_type: "NUMBER".to_string(),
            comment: None,
            index: Some(1),
        }],
        ..CatalogTable::default()
    };
    table.stats.row_count = Some(10);
    table.stats.clustering = Some("LINEAR(a, b)".to_string());
    catalog.models.insert("src_up".to_string(), table);
    facts.catalog = Some(catalog);

    let plan = facts.plan("select ts from {{ ref('up') }} where ts > '2024-01-01'");
    assert!(plan.needs.available.contains(&"phys"), "{:#?}", plan.needs);
    let lir = plan.levels.lir.text.as_deref().expect("lir");
    assert!(lir.contains("TS") && lir.contains("Cluster"), "{lir}");

    let stats = facts.plan("select a, b from {{ ref('src_up') }} where a > 1");
    assert!(
        stats.needs.available.contains(&"stats"),
        "{:#?}",
        stats.needs
    );
    assert!(
        stats.needs.available.contains(&"phys"),
        "{:#?}",
        stats.needs
    );

    // perf.catalog_stats off: the row count no longer counts as a fact,
    // while the clustering keys (physical layout, not statistics) stay
    let text = "select a, b from {{ ref('src_up') }} where a > 1";
    let mut input = facts.input(text, Some("m"));
    input.catalog_stats = false;
    let gated = crate::features::plan::query_plan(&input);
    assert!(gated.needs.missing.contains(&"stats"), "{:#?}", gated.needs);
    assert!(
        gated.needs.available.contains(&"phys"),
        "{:#?}",
        gated.needs
    );
}

/// Named columns of an open upstream, read through a `SELECT *` CTE, must
/// survive as columns rather than becoming opaque (demand tracking).
#[test]
fn bound_columns_flow_through_an_open_star_cte() {
    let facts = Facts::new("duckdb").yaml("up", &[("day", None), ("id", None)]);
    let plan = facts.plan("with u as (select * from {{ ref('up') }}) select id, day from u");
    assert!(
        plan.levels
            .mir
            .degradations
            .iter()
            .all(|d| !d.detail.contains("no such column")),
        "{:#?}",
        plan.levels.mir
    );
    let mir = plan.levels.mir.text.as_deref().expect("mir");
    assert!(!mir.contains("opaque["), "{mir}");
    assert!(
        mir.contains("cte#0 u out=[") && mir.contains("day"),
        "{mir}"
    );
}

/// Fixtures and awkward shapes run the whole pipeline with no verifier
/// violation and no rollback, whatever fidelity they land on.
#[test]
fn every_fixture_and_shape_plans_without_a_violation() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let mut corpus: Vec<(String, String)> = walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "sql"))
        .map(|e| {
            let text = std::fs::read_to_string(e.path()).expect("fixture");
            (e.path().display().to_string(), text)
        })
        .collect();
    assert!(!corpus.is_empty());
    corpus.sort();
    for sql in [
        "select a, sum(b) over (partition by c order by d) as s from {{ ref('t') }} qualify s > 1",
        "select * from {{ ref('a') }} union all select * from {{ ref('b') }}",
        "select a from {{ ref('a') }} except select a from {{ ref('b') }} order by 1 limit 3",
        "select x.a, (select max(b) from {{ ref('b') }} b where b.id = x.id) m from {{ ref('a') }} x",
        "select a, a + 1 as c, c * 2 as d from {{ ref('t') }} group by all",
        "select u.value from {{ ref('t') }} t, unnest(t.arr) as u(value)",
        "select * exclude (a) replace (b + 1 as b) from {{ ref('t') }}",
        "select count(*) filter (where a > 1) from {{ source('s', 't') }} left join {{ ref('x') }} using (id)",
        "with recursive r as (select 1 as n union all select n + 1 from r where n < 5) select n from r",
        "with a as (select 1 x), b as (select * from a), c as (select x from a join b using (x)) select x from c",
        "select distinct a from t1 cross join t2 where t1.id in (select id from t3) and exists (select 1 from t4)",
        "select {{ my_macro('a') }} as x, {{ var('y') }} from {{ dbt_utils.star(ref('t')) }}",
        "select {{ col }} from {{ this }} where {{ cond }}",
        "{{ config(materialized='incremental', unique_key='id') }} select id from {{ ref('t') }} \
         {% if is_incremental() %} where id > (select max(id) from {{ this }}) {% endif %}",
        "select a from (select a from (select 1 as a) s1) s2 where a = 1 and 1 = 1",
        "select from",
        "",
        "-- comment only\n",
        "select a,, b from t",
        "with a as (selec 1) select * from a",
    ] {
        corpus.push(("shape".to_string(), sql.to_string()));
    }
    let facts = Facts::new("duckdb").yaml("a", &[("a", None), ("b", None)]);
    for (name, text) in &corpus {
        let plan = facts.plan(text);
        for level in [&plan.levels.hir, &plan.levels.mir, &plan.levels.lir] {
            assert!(level.violations.is_empty(), "{name}: {level:#?}");
        }
        assert!(
            plan.trace.entries.iter().all(|r| r.outcome != "rolledBack"),
            "{name}: {:#?}",
            plan.trace
        );
    }
}
