//! dbt.doctor (05 §9): one typed health report merging what the
//! editors used to half-compute client-side — server, project,
//! profiles trace, artifact freshness, caches, graph, lint, jobs.

use std::path::Path;

use cogitator_analysis::artifacts::{status::manifest_drift, ArtifactPaths};
use cogitator_analysis::config_schema::lint_flags;
use serde::Serialize;

use super::super::derived_cache::{CacheReport, DerivedCache};
use super::super::reply::Reply;
use super::super::snapshot::Snapshot;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub server_version: &'static str,
    pub project_root: Option<String>,
    pub project_name: String,
    pub dialect: String,
    /// The binary dbt.run/... would spawn, and whether PATH has it.
    pub dbt_binary: String,
    pub dbt_binary_found: bool,
    pub profiles: DoctorProfiles,
    pub artifacts: DoctorArtifacts,
    pub caches: CacheReport,
    pub scan_cache_persisted: bool,
    pub graph: DoctorGraph,
    pub lint_enabled: Vec<&'static str>,
    pub running_jobs: Vec<DoctorJob>,
    pub config: super::super::config_watcher::ConfigStatus,
    pub perf: super::super::status::PerfStatus,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorProfiles {
    pub path: Option<String>,
    pub active_profile: String,
    pub active_target: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorArtifacts {
    pub run_results_age_s: Option<u64>,
    pub manifest_age_s: Option<u64>,
    pub sources_age_s: Option<u64>,
    pub catalog_age_s: Option<u64>,
    pub drift_count: usize,
}

#[derive(Debug, Serialize)]
pub struct DoctorGraph {
    pub nodes: usize,
    pub edges: usize,
    pub dangling: usize,
}

#[derive(Debug, Serialize)]
pub struct DoctorJob {
    pub token: u64,
    pub label: String,
}

#[allow(clippy::too_many_arguments)] // one assembly point, spelled out
pub fn doctor(
    snap: &Snapshot,
    derived: &DerivedCache,
    lints: u32,
    jobs: Vec<(u64, String)>,
    config: super::super::config_watcher::ConfigStatus,
    artifact_cap: u64,
    perf: super::super::status::PerfStatus,
) -> Reply {
    let ctx = &snap.ctx;
    let binary = if snap.fusion_path.is_empty() {
        "dbt".to_string()
    } else {
        snap.fusion_path.clone()
    };
    let profiles_path =
        cogitator_analysis::util::profiles_candidates(&snap.profiles.dir_override, &snap.root_path)
            .into_iter()
            .find(|p| p.is_file());
    let artifacts = ctx
        .project_root
        .as_deref()
        .map(|root| {
            let paths = ArtifactPaths::under(root);
            let set = derived.artifacts(root, artifact_cap);
            DoctorArtifacts {
                run_results_age_s: age_of(&paths.run_results),
                manifest_age_s: age_of(&paths.manifest),
                sources_age_s: age_of(&paths.sources),
                catalog_age_s: age_of(&paths.catalog),
                drift_count: set
                    .manifest
                    .as_deref()
                    .map(|m| manifest_drift(ctx, m).len())
                    .unwrap_or(0),
            }
        })
        .unwrap_or(DoctorArtifacts {
            run_results_age_s: None,
            manifest_age_s: None,
            sources_age_s: None,
            catalog_age_s: None,
            drift_count: 0,
        });
    let report = DoctorReport {
        server_version: crate::VERSION,
        project_root: ctx.project_root.as_ref().map(|p| p.display().to_string()),
        project_name: ctx.project.name.clone(),
        dialect: ctx.dialect.as_str().to_string(),
        dbt_binary_found: on_path(&binary),
        dbt_binary: binary,
        profiles: DoctorProfiles {
            path: profiles_path.map(|p| p.display().to_string()),
            active_profile: if snap.profiles.profile.is_empty() {
                ctx.project.profile.clone()
            } else {
                snap.profiles.profile.clone()
            },
            active_target: snap.profiles.target.clone(),
        },
        artifacts,
        caches: derived.report(),
        scan_cache_persisted: ctx
            .project_root
            .as_deref()
            .is_some_and(|root| super::super::cache_persist::cache_path(root).is_file()),
        graph: DoctorGraph {
            nodes: ctx.graph.node_count(),
            edges: ctx.graph.edge_count(),
            dangling: ctx.graph.dangling_refs().count(),
        },
        lint_enabled: enabled_lints(lints),
        running_jobs: jobs
            .into_iter()
            .map(|(token, label)| DoctorJob { token, label })
            .collect(),
        config,
        perf,
    };
    Reply::result(&report)
}

fn enabled_lints(flags: u32) -> Vec<&'static str> {
    [
        (lint_flags::SELECT_STAR, "select_star"),
        (lint_flags::UNALIASED_COLUMNS, "unaliased_columns"),
        (lint_flags::TRAILING_COMMA, "trailing_comma"),
        (lint_flags::UNDOCUMENTED_COLUMNS, "undocumented_columns"),
        (lint_flags::DEAD_MODELS, "dead_models"),
    ]
    .into_iter()
    .filter(|(bit, _)| flags & bit != 0)
    .map(|(_, name)| name)
    .collect()
}

fn age_of(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified.elapsed().ok().map(|age| age.as_secs())
}

/// Absolute/relative paths check directly; bare names walk PATH.
fn on_path(binary: &str) -> bool {
    let candidate = Path::new(binary);
    if candidate.components().count() > 1 {
        return candidate.is_file();
    }
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            let joined = dir.join(binary);
            joined.is_file() || joined.with_extension("exe").is_file()
        })
    })
}
