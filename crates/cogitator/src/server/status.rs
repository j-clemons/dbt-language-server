//! The dbt.status and dbt.setTarget wire payloads: pure assembly over the
//! dispatcher's snapshots, so the report shape lives in one typed place.

use cogitator_analysis::features::plan::diagnostics_config::PerfSettings;
use cogitator_analysis::DbtContext;
use serde::Serialize;

use super::config_watcher::ConfigStatus;
use super::derived_cache::CacheReport;
use super::{ProfilesConfig, StatusInfo};

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub server: ServerStatus,
    pub project: ProjectStatus,
    pub profiles: ProfilesStatus,
    pub refresh: RefreshStatus,
    /// Derived-data cache counters (also the dbt.cache report shape).
    pub caches: CacheReport,
    /// The cogitator.yml watcher's view: path, loaded, last error.
    pub config: ConfigStatus,
    /// The active perf diagnostics configuration (also in dbt.doctor).
    pub perf: PerfStatus,
}

/// The perf diagnostics summary shared by dbt.status and dbt.doctor.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfStatus {
    pub profile: &'static str,
    pub catalog_stats: bool,
    pub enabled_rules: usize,
}

impl PerfStatus {
    pub fn of(settings: &PerfSettings) -> Self {
        PerfStatus {
            profile: settings.profile.as_str(),
            catalog_stats: settings.catalog_stats,
            enabled_rules: settings.enabled_rule_count(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub name: &'static str,
    pub version: &'static str,
    pub backend: &'static str,
    pub fusion: &'static str,
    pub progress_supported: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    /// `None` → `"root": null` (no dbt_project.yml found).
    pub root: Option<String>,
    pub name: String,
    pub dialect: String,
    pub models: usize,
    pub sources: usize,
    pub macro_packages: usize,
    pub graph_nodes: usize,
    pub graph_edges: usize,
    /// Secondary project roots with a live context (multi-root).
    pub extra_roots: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesStatus {
    pub active_profile: String,
    pub active_target: String,
    pub dir_override: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshStatus {
    pub count: u64,
    pub last_ms: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

/// The dbt.setTarget confirmation.
#[derive(Debug, Serialize)]
pub struct SetTargetResult {
    pub profile: String,
    pub target: String,
}

/// The dbt.setProfilesDir confirmation.
#[derive(Debug, Serialize)]
pub struct SetProfilesDirResult {
    pub dir: String,
}

// one assembly point for the whole report: splitting the argument
// list into a one-off struct would only rename the problem
#[allow(clippy::too_many_arguments)]
pub fn status_report(
    info: &StatusInfo,
    cfg: &ProfilesConfig,
    ctx: &DbtContext,
    fusion: &'static str,
    progress_supported: bool,
    caches: CacheReport,
    config: ConfigStatus,
    extra_roots: Vec<String>,
    perf: PerfStatus,
) -> StatusReport {
    let active_profile = if cfg.profile.is_empty() {
        ctx.project.profile.clone()
    } else {
        cfg.profile.clone()
    };
    let dialect = ctx.dialect.as_str();
    StatusReport {
        server: ServerStatus {
            name: "cogitator",
            version: crate::VERSION,
            backend: info.backend,
            fusion,
            progress_supported,
        },
        project: ProjectStatus {
            root: ctx.project_root.as_ref().map(|p| p.display().to_string()),
            name: ctx.project.name.clone(),
            dialect: if dialect.is_empty() {
                "generic (unresolved)".to_string()
            } else {
                dialect.to_string()
            },
            models: ctx.model_detail_map.len(),
            sources: ctx.source_detail_map.len(),
            macro_packages: ctx.macro_detail_map.len(),
            graph_nodes: ctx.graph.node_count(),
            graph_edges: ctx.graph.edge_count(),
            extra_roots,
        },
        profiles: ProfilesStatus {
            active_profile,
            active_target: cfg.target.clone(),
            dir_override: cfg.dir_override.clone(),
        },
        refresh: RefreshStatus {
            count: info.refreshes,
            last_ms: info.last_refresh_ms,
            cache_hits: info.cache_hits,
            cache_misses: info.cache_misses,
        },
        caches,
        config,
        perf,
    }
}
