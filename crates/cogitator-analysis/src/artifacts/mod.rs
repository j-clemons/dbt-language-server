//! dbt artifact readers (03 §10, 08 §4): target/run_results.json, a
//! minimal manifest.json index and sources.json, deserialized through
//! the types generated from the vendored schemas.getdbt.com
//! definitions — with the leniency manufactured by the preprocess
//! artifact mode (every field optional, unknown fields ignored,
//! statuses plain strings) plus adapter-side row dropping. Version
//! mismatches trace, never reject; artifacts must degrade feature-off,
//! never break core diagnostics.

pub mod catalog;
mod catalog_stats;
pub mod freshness;
pub mod manifest;
mod manifest_config;
pub mod run_results;
pub mod status;

use std::path::{Path, PathBuf};

/// Where the artifacts live under a project root. `target-path`
/// overrides are not read today (the scan does not keep them); the
/// dbt default is what the rest of the server assumes too.
#[derive(Debug, Clone)]
pub struct ArtifactPaths {
    pub run_results: PathBuf,
    pub manifest: PathBuf,
    pub sources: PathBuf,
    pub catalog: PathBuf,
}

impl ArtifactPaths {
    pub fn under(root: &Path) -> Self {
        let target = root.join("target");
        ArtifactPaths {
            run_results: target.join("run_results.json"),
            manifest: target.join("manifest.json"),
            sources: target.join("sources.json"),
            catalog: target.join("catalog.json"),
        }
    }
}

/// Everything the shell managed to read this round; any member may be
/// absent (missing file, over the size cap, or unparseable).
#[derive(Debug, Default, Clone)]
pub struct ArtifactSet {
    pub run_results: Option<std::sync::Arc<run_results::RunResults>>,
    pub manifest: Option<std::sync::Arc<manifest::ManifestIndex>>,
    pub freshness: Option<std::sync::Arc<freshness::SourcesFreshness>>,
    pub catalog: Option<std::sync::Arc<catalog::CatalogIndex>>,
}

/// Version mismatch is a TRACE, never a rejection and never a user-
/// facing message (03 §10: read, trace, never reject).
pub(crate) fn trace_schema_version(
    path: &std::path::Path,
    found: Option<&str>,
    expected: &'static str,
) {
    if let Some(found) = found {
        if found != expected {
            tracing::debug!(
                path = %path.display(),
                found,
                expected,
                "artifact schema version differs; reading leniently"
            );
        }
    }
}
