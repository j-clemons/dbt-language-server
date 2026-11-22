//! run_results.json: per-node status, timing and adapter message.

use std::path::Path;

use cogitator_error::guidance::Guidance;
#[derive(Debug, Default)]
pub struct RunResults {
    pub metadata: ArtifactMeta,
    pub results: Vec<RunResult>,
}

#[derive(Debug, Default)]
pub struct ArtifactMeta {
    pub generated_at: Option<String>,
    pub dbt_version: Option<String>,
}

#[derive(Debug)]
pub struct RunResult {
    /// The only hard requirement: "model.<project>.<name>",
    /// "test.<project>.<name>.<hash>", "seed.…", "source.…".
    pub unique_id: String,
    /// "success" | "error" | "fail" | "skipped" | "pass" | "warn" — or
    /// whatever a future dbt says; consumers match loosely.
    pub status: String,
    /// Seconds; `None` when absent, negative or not a number.
    pub execution_time: Option<f64>,
    pub message: Option<String>,
}

impl RunResults {
    /// Seconds the node with this `unique_id` took in the last run.
    pub fn execution_time(&self, unique_id: &str) -> Option<f64> {
        self.results
            .iter()
            .find(|result| result.unique_id == unique_id)?
            .execution_time
    }
}

pub fn parse(text: &str, path: &Path) -> Result<RunResults, Guidance> {
    use cogitator_schemas::artifacts::run_results as gen;
    let raw: gen::RunResultsArtifact = serde_json::from_str(text).map_err(|err| {
        Guidance::new("could not read run_results.json.")
            .at(path.display().to_string())
            .because(err.to_string())
            .fix("re-run dbt to regenerate target/; run-status features stay off until then.")
    })?;
    super::trace_schema_version(
        path,
        raw.metadata
            .as_ref()
            .and_then(|m| m.dbt_schema_version.as_deref()),
        cogitator_schemas::artifacts::RUN_RESULTS_SCHEMA_V6,
    );
    let metadata = raw.metadata.unwrap_or_default();
    Ok(RunResults {
        metadata: ArtifactMeta {
            generated_at: metadata.generated_at,
            dbt_version: metadata.dbt_version,
        },
        results: raw
            .results
            .into_iter()
            .filter_map(|result| {
                // the ONE hard requirement, re-imposed per ROW: a result
                // without a unique_id drops, its siblings survive
                let Some(unique_id) = result.unique_id else {
                    tracing::debug!("run result without unique_id skipped");
                    return None;
                };
                Some(RunResult {
                    unique_id,
                    status: result.status.unwrap_or_default(),
                    execution_time: result
                        .execution_time
                        .and_then(|value| value.as_f64())
                        .filter(|seconds| *seconds >= 0.0),
                    message: result.message,
                })
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let rr = parse(
            r#"{"metadata": {"generated_at": "2026-11-03T10:00:00Z", "env": {"x": 1}},
                "results": [
                  {"unique_id": "model.jaffle_shop.customers", "status": "success",
                   "execution_time": 3.2, "adapter_response": {"rows": 9}},
                  {"unique_id": "test.jaffle_shop.not_null_customers_customer_id.abc",
                   "status": "fail", "message": "Got 3 results"}
                ],
                "elapsed_time": 5.0, "args": {"which": "build"}}"#,
            Path::new("run_results.json"),
        )
        .unwrap();
        assert_eq!(rr.results.len(), 2);
        assert_eq!(rr.results[0].status, "success");
        assert_eq!(rr.results[1].execution_time, None);
        assert_eq!(rr.metadata.dbt_version, None);
    }

    #[test]
    fn execution_time_is_looked_up_by_unique_id() {
        let rr = parse(
            r#"{"results": [
                {"unique_id": "model.p.ran", "execution_time": 3.25},
                {"unique_id": "model.p.instant", "execution_time": 0},
                {"unique_id": "model.p.absent"},
                {"unique_id": "model.p.null", "execution_time": null},
                {"unique_id": "model.p.text", "execution_time": "fast"},
                {"unique_id": "model.p.negative", "execution_time": -1.0}
            ]}"#,
            Path::new("x"),
        )
        .unwrap();
        assert_eq!(rr.results.len(), 6, "a malformed time keeps its row");
        assert_eq!(rr.execution_time("model.p.ran"), Some(3.25));
        assert_eq!(rr.execution_time("model.p.instant"), Some(0.0));
        for missing in ["absent", "null", "text", "negative", "never_ran"] {
            assert_eq!(rr.execution_time(&format!("model.p.{missing}")), None);
        }
    }

    #[test]
    fn broken_json_is_guidance_not_a_panic() {
        let err = parse("{not json", Path::new("target/run_results.json")).unwrap_err();
        let message = err.to_message();
        assert!(message.contains("run_results.json"), "{message}");
        assert!(message.contains("re-run dbt"), "{message}");
    }

    #[test]
    fn rows_without_unique_id_drop_while_siblings_survive() {
        let rr = parse(
            r#"{"results": [
                {"status": "success"},
                {"unique_id": "model.p.kept", "status": "success"}
            ]}"#,
            Path::new("x"),
        )
        .unwrap();
        assert_eq!(rr.results.len(), 1);
        assert_eq!(rr.results[0].unique_id, "model.p.kept");
    }

    #[test]
    fn older_versions_and_foreign_statuses_read_leniently() {
        // a v5-flavored payload: old version string, fields v6 lacks,
        // a status outside every v6 enum set — all must parse
        let rr = parse(
            r#"{"metadata": {"dbt_schema_version": "https://schemas.getdbt.com/dbt/run-results/v5.json",
                             "mystery": true},
                "results": [{"unique_id": "model.p.m", "status": "partial success"},
                            {"unique_id": "model.p.n", "status": "flaked"}]}"#,
            Path::new("x"),
        )
        .unwrap();
        assert_eq!(rr.results[0].status, "partial success");
        assert_eq!(rr.results[1].status, "flaked");
        // and a result with no status at all defaults to empty
        let sparse = parse(
            r#"{"results": [{"unique_id": "model.p.m"}]}"#,
            Path::new("x"),
        )
        .unwrap();
        assert_eq!(sparse.results[0].status, "");
    }
}
