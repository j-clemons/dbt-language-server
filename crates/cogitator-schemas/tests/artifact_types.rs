//! The generated artifact types (08 §4): the leniency the preprocess
//! artifact mode manufactures — unknown fields pass, every field is
//! optional, statuses are plain strings, object unions read merged.

use cogitator_schemas::artifacts::{catalog, manifest, run_results, sources};

#[test]
fn run_results_tolerate_unknowns_and_foreign_statuses() {
    let rr: run_results::RunResultsArtifact = serde_json::from_str(
        r#"{"results": [{"unique_id": "model.p.m", "status": "flaked",
                         "execution_time": 1.5, "future_field": {"x": 1}}],
            "metadata": {"dbt_schema_version": "https://schemas.getdbt.com/dbt/run-results/v5.json",
                         "generated_at": "g", "env": {"CI": "1"}},
            "elapsed_time": 2.0, "args": {"which": "build"}, "extra": 1}"#,
    )
    .unwrap();
    let result = &rr.results[0];
    assert_eq!(result.unique_id.as_deref(), Some("model.p.m"));
    assert_eq!(
        result.status.as_deref(),
        Some("flaked"),
        "status stays a String"
    );
    assert_eq!(result.execution_time, Some(serde_json::json!(1.5)));
    assert!(result.message.is_none());
    // a result missing every field but unique_id still parses
    let sparse: run_results::RunResultsArtifact =
        serde_json::from_str(r#"{"results": [{"unique_id": "model.p.m"}]}"#).unwrap();
    assert_eq!(sparse.results.len(), 1);
}

#[test]
fn run_results_accept_any_execution_time_shape() {
    for raw in [
        r#"{"execution_time": "fast"}"#,
        r#"{"execution_time": null}"#,
        "{}",
    ] {
        let text = format!(r#"{{"results": [{raw}]}}"#);
        let parsed: run_results::RunResultsArtifact = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.results.len(), 1, "{raw}");
    }
}

#[test]
fn sources_read_both_union_variants_through_the_merge() {
    // rule (g): RuntimeError and Output rows land in ONE merged shape
    let fr: sources::FreshnessExecutionResultArtifact = serde_json::from_str(
        r#"{"results": [
            {"unique_id": "source.p.s.ok", "status": "pass", "snapshotted_at": "t1",
             "max_loaded_at": "t0"},
            {"unique_id": "source.p.s.boom", "status": "runtime error", "error": "db gone"}
        ]}"#,
    )
    .unwrap();
    assert_eq!(fr.results.len(), 2);
    assert_eq!(fr.results[0].snapshotted_at.as_deref(), Some("t1"));
    assert_eq!(fr.results[1].status.as_deref(), Some("runtime error"));
}

#[test]
fn catalog_tables_and_columns_parse() {
    let cat: catalog::CatalogArtifact = serde_json::from_str(
        r#"{"nodes": {}, "sources": {"source.p.stripe.payments": {
            "metadata": {"type": "BASE TABLE", "schema": "main", "name": "payments"},
            "columns": {"amount": {"type": "INTEGER", "index": 2, "name": "amount"},
                        "id": {"type": "BIGINT", "index": 1, "name": "id", "comment": "pk"}},
            "stats": {"row_count": {"id": "row_count", "label": "rows", "value": 9,
                                     "include": true}}}}}"#,
    )
    .unwrap();
    let table = &cat.sources["source.p.stripe.payments"];
    assert_eq!(table.columns.len(), 2);
    assert_eq!(table.columns["id"].comment.as_deref(), Some("pk"));
    let stats = table.stats.as_ref().expect("stats are retained");
    assert_eq!(stats["row_count"]["value"], 9);
}

#[test]
fn catalog_accepts_any_stats_and_generated_at_shape() {
    let cat: catalog::CatalogArtifact = serde_json::from_str(
        r#"{"metadata": {"generated_at": 17},
            "nodes": {"model.p.a": {"stats": "none"},
                      "model.p.b": {"stats": null},
                      "model.p.c": {"stats": {"row_count": {"value": {"odd": true}}}},
                      "model.p.d": {}}}"#,
    )
    .unwrap();
    assert_eq!(cat.nodes.len(), 4);
    assert!(cat.nodes["model.p.d"].stats.is_none());
}

#[test]
fn manifest_projection_reads_the_two_lookups() {
    let mf: manifest::ManifestProjection = serde_json::from_str(
        r#"{"metadata": {"dbt_schema_version": "v12"},
            "nodes": {
              "model.p.customers": {"resource_type": "model", "name": "customers",
                                     "original_file_path": "models/customers.sql",
                                     "checksum": {"ignored": true}},
              "test.p.not_null.abc": {"resource_type": "test",
                                       "attached_node": "model.p.customers"}
            }}"#,
    )
    .unwrap();
    assert_eq!(mf.nodes.len(), 2);
    assert_eq!(
        mf.nodes["test.p.not_null.abc"].attached_node.as_deref(),
        Some("model.p.customers")
    );
}

#[test]
fn manifest_projection_keeps_relation_name_and_config() {
    let mf: manifest::ManifestProjection = serde_json::from_str(
        r#"{"nodes": {
              "model.p.a": {"relation_name": "\"db\".\"s\".\"a\"",
                             "config": {"materialized": "table",
                                        "partition_by": {"field": "ts"}}},
              "model.p.b": {"relation_name": null, "config": null},
              "model.p.c": {"relation_name": 3, "config": "table"},
              "model.p.d": {}
            }}"#,
    )
    .unwrap();
    assert_eq!(mf.nodes.len(), 4, "no shape rejects the manifest");
    let config = mf.nodes["model.p.a"].config.as_ref().expect("config");
    assert_eq!(config["partition_by"]["field"], "ts");
    assert!(mf.nodes["model.p.d"].config.is_none());
}
