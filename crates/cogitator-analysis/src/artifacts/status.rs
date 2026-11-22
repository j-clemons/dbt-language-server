//! Pure joins between the scanned context and the artifact payloads:
//! run status per model, failed tests mapped onto models, scan-vs-
//! manifest drift and source freshness verdicts.

use std::collections::HashMap;

use super::freshness::SourcesFreshness;
use super::manifest::ManifestIndex;
use super::run_results::RunResults;
use crate::context::DbtContext;

#[derive(Debug, Clone)]
pub struct RunStatus {
    pub status: String,
    pub execution_time: f64,
    pub message: Option<String>,
    pub generated_at: Option<String>,
}

/// Last-run status per MODEL name ("model.<proj>.<name>" results only).
pub fn run_status_by_model(rr: &RunResults) -> HashMap<String, RunStatus> {
    let mut out = HashMap::new();
    for result in &rr.results {
        let mut parts = result.unique_id.split('.');
        if parts.next() != Some("model") {
            continue;
        }
        let Some(name) = result.unique_id.rsplit('.').next() else {
            continue;
        };
        out.insert(
            name.to_string(),
            RunStatus {
                status: result.status.clone(),
                execution_time: result.execution_time.unwrap_or_default(),
                message: result.message.clone(),
                generated_at: rr.metadata.generated_at.clone(),
            },
        );
    }
    out
}

#[derive(Debug, PartialEq)]
pub struct TestFailure {
    pub model: String,
    pub test_name: String,
    pub message: Option<String>,
}

/// Failed/errored tests attributed to their model: the manifest's
/// attached_node first, a conservative name-substring fallback without
/// one (skipped when ambiguous).
pub fn test_failures(
    ctx: &DbtContext,
    rr: &RunResults,
    manifest: Option<&ManifestIndex>,
) -> Vec<TestFailure> {
    let mut out = Vec::new();
    for result in &rr.results {
        if !result.unique_id.starts_with("test.") {
            continue;
        }
        if !matches!(result.status.as_str(), "fail" | "error" | "warn") {
            continue;
        }
        let test_name = result
            .unique_id
            .split('.')
            .nth(2)
            .unwrap_or(&result.unique_id)
            .to_string();
        let attached = manifest
            .and_then(|m| m.tests.get(&result.unique_id))
            .and_then(|t| t.attached_node.as_deref())
            .and_then(|node| node.rsplit('.').next())
            .map(str::to_string);
        let model = attached.or_else(|| {
            // fallback: exactly one scanned model name appears in the
            // test name — anything else stays unattributed
            let mut hits = ctx
                .model_detail_map
                .keys()
                .filter(|name| test_name.contains(name.as_str()));
            match (hits.next(), hits.next()) {
                (Some(one), None) => Some(one.clone()),
                _ => None,
            }
        });
        if let Some(model) = model {
            out.push(TestFailure {
                model,
                test_name,
                message: result.message.clone(),
            });
        }
    }
    out
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum DriftKind {
    /// The manifest knows it; the scan does not (stale scan? odd layout?).
    OnlyInManifest,
    /// The scan knows it; the manifest does not (stale target/).
    OnlyInScan,
}

#[derive(Debug, PartialEq)]
pub struct Drift {
    pub model: String,
    pub kind: DriftKind,
}

/// Scan-vs-manifest disagreement. Quiet when either side is empty —
/// a fresh checkout or a half-written manifest is not drift.
pub fn manifest_drift(ctx: &DbtContext, manifest: &ManifestIndex) -> Vec<Drift> {
    if ctx.model_detail_map.is_empty() || manifest.models.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for name in manifest.models.keys() {
        if !ctx.model_detail_map.contains_key(name) {
            out.push(Drift {
                model: name.clone(),
                kind: DriftKind::OnlyInManifest,
            });
        }
    }
    for name in ctx.model_detail_map.keys() {
        if !manifest.models.contains_key(name) {
            out.push(Drift {
                model: name.clone(),
                kind: DriftKind::OnlyInScan,
            });
        }
    }
    out.sort_by(|a, b| a.model.cmp(&b.model));
    out
}

#[derive(Debug, Clone)]
pub struct FreshnessVerdict {
    pub status: String,
    pub snapshotted_at: Option<String>,
}

/// Freshness per (source, table).
pub fn freshness_by_source(fr: &SourcesFreshness) -> HashMap<(String, String), FreshnessVerdict> {
    let mut out = HashMap::new();
    for result in &fr.results {
        let parts: Vec<&str> = result.unique_id.split('.').collect();
        let ["source", _, source, table] = parts.as_slice() else {
            continue;
        };
        out.insert(
            (source.to_string(), table.to_string()),
            FreshnessVerdict {
                status: result.status.clone(),
                snapshotted_at: result.snapshotted_at.clone(),
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::run_results;
    use super::*;
    use std::path::Path;

    fn rr() -> RunResults {
        run_results::parse(
            r#"{"metadata": {"generated_at": "2026-11-03T10:00:00Z"},
                "results": [
                  {"unique_id": "model.jaffle_shop.customers", "status": "success", "execution_time": 3.2},
                  {"unique_id": "model.jaffle_shop.orders", "status": "error", "message": "boom"},
                  {"unique_id": "test.jaffle_shop.not_null_customers_customer_id.abc",
                   "status": "fail", "message": "Got 3 results"},
                  {"unique_id": "seed.jaffle_shop.raw_customers", "status": "success"}
                ]}"#,
            Path::new("x"),
        )
        .unwrap()
    }

    #[test]
    fn run_status_keys_by_model_name() {
        let map = run_status_by_model(&rr());
        assert_eq!(map.len(), 2);
        assert_eq!(map["customers"].status, "success");
        assert_eq!(map["customers"].execution_time, 3.2);
        assert_eq!(map["orders"].message.as_deref(), Some("boom"));
    }

    #[test]
    fn test_failures_attach_via_manifest_then_fallback() {
        let manifest = super::super::manifest::parse(
            r#"{"nodes": {"test.jaffle_shop.not_null_customers_customer_id.abc":
                {"resource_type": "test", "attached_node": "model.jaffle_shop.customers"}}}"#,
            Path::new("m"),
        )
        .unwrap();
        let ctx = DbtContext::default();
        let with_manifest = test_failures(&ctx, &rr(), Some(&manifest));
        assert_eq!(with_manifest.len(), 1);
        assert_eq!(with_manifest[0].model, "customers");

        // without a manifest and an empty scan: unattributable → dropped
        assert!(test_failures(&ctx, &rr(), None).is_empty());
    }

    #[test]
    fn drift_is_quiet_on_empty_sides() {
        let manifest = super::super::manifest::parse(
            r#"{"nodes": {"model.jaffle_shop.customers": {"resource_type": "model", "name": "customers"}}}"#,
            Path::new("m"),
        )
        .unwrap();
        assert!(manifest_drift(&DbtContext::default(), &manifest).is_empty());
    }

    #[test]
    fn freshness_keys_by_source_and_table() {
        let fr = super::super::freshness::parse(
            r#"{"results": [{"unique_id": "source.jaffle_shop.stripe.payments",
                             "status": "pass", "snapshotted_at": "2026-11-03T09:00:00Z"}]}"#,
            Path::new("s"),
        )
        .unwrap();
        let map = freshness_by_source(&fr);
        assert_eq!(
            map[&("stripe".to_string(), "payments".to_string())].status,
            "pass"
        );
    }
}
