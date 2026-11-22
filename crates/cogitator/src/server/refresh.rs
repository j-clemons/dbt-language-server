//! The debounced context-refresh task (05 §6.2): coalesces triggers,
//! rebuilds the DbtContext off the async workers (scan-cache warm
//! start, optional warehouse enrichment, snapshot persistence), then
//! publishes over the watch channel with one Guidance warning per
//! distinct degradation cause.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cogitator_analysis::{ContextBuilder, DbtContext};
use tokio::sync::{mpsc, watch};

use super::client_link::ClientLink;
use super::derived_cache::DerivedCache;
use super::{cache_persist, derived_cache, ProfilesConfig, SharedLimits, StatusInfo};

pub(super) struct RefreshTask {
    pub link: ClientLink,
    pub profiles: Arc<std::sync::Mutex<ProfilesConfig>>,
    pub derived: Arc<DerivedCache>,
    pub limits: Arc<SharedLimits>,
    #[cfg(feature = "duckdb-parser")]
    pub toggles: Arc<super::SharedToggles>,
    pub status: Arc<std::sync::Mutex<StatusInfo>>,
    pub revision: Arc<AtomicU64>,
    pub builder: Arc<std::sync::Mutex<ContextBuilder>>,
    pub ctx_tx: watch::Sender<Arc<DbtContext>>,
    /// Secondary project contexts (multi-root, 01 §3 addendum).
    pub extra: Arc<super::multi_root::ExtraContexts>,
    /// The first workspace folder: its refreshes publish on the watch
    /// channel; every other root lands in `extra`.
    pub primary_root: Arc<std::sync::Mutex<String>>,
}

impl RefreshTask {
    pub async fn run(self, mut refresh_rx: mpsc::UnboundedReceiver<String>) {
        let refresh_link = self.link;
        let refresh_profiles = self.profiles;
        let refresh_derived = self.derived;
        let refresh_limits = self.limits;
        #[cfg(feature = "duckdb-parser")]
        let refresh_toggles = self.toggles;
        let refresh_status = self.status;
        let refresh_revision = self.revision;
        let builder = self.builder;
        let ctx_tx = self.ctx_tx;
        let extra = self.extra;
        let primary_root = self.primary_root;
        // one builder per secondary project root, keyed by resolved root
        let secondary_builders: Arc<
            std::sync::Mutex<std::collections::HashMap<String, ContextBuilder>>,
        > = Arc::default();
        let mut warned: Option<String> = None;
        // scan-cache totals are cumulative across builds: keep the last
        // observation so the metrics see per-refresh deltas
        let (mut prev_hits, mut prev_misses) = (0u64, 0u64);
        while let Some(first) = refresh_rx.recv().await {
            // debounce: coalesce triggers within the (hot-tunable) window —
            // per DISTINCT root, so a secondary refresh never swallows a
            // primary one (multi-root)
            let debounce = Duration::from_millis(
                refresh_limits
                    .refresh_debounce_ms
                    .load(std::sync::atomic::Ordering::Relaxed),
            );
            let mut roots = vec![first];
            while let Ok(Some(next)) = tokio::time::timeout(debounce, refresh_rx.recv()).await {
                if !roots.contains(&next) {
                    roots.push(next);
                }
            }
            for root in roots {
                let is_primary = {
                    let primary = primary_root.lock().expect("primary root lock");
                    primary.is_empty() || *primary == root
                };
                if !is_primary {
                    secondary_refresh(
                        &root,
                        &refresh_profiles,
                        &secondary_builders,
                        &extra,
                        &refresh_link,
                        &refresh_status,
                        &refresh_revision,
                    )
                    .await;
                    continue;
                }
                let cfg = refresh_profiles
                    .lock()
                    .expect("profiles config lock")
                    .clone();
                let progress = refresh_link.progress_begin("dbt: refreshing project");
                let started = std::time::Instant::now();
                let built = tokio::task::spawn_blocking({
                    let root = root.clone();
                    let builder = builder.clone();
                    let cfg = cfg.clone();
                    #[cfg(feature = "duckdb-parser")]
                    let warehouse_toggles = refresh_toggles.clone();
                    let enrich_derived = refresh_derived.clone();
                    let enrich_limits = refresh_limits.clone();
                    move || {
                        let span = tracing::info_span!("context_refresh", root = %root);
                        let _enter = span.enter();
                        let mut builder = builder.lock().expect("context builder lock");
                        builder.set_profiles_dir(cfg.dir_override.clone());
                        builder.set_profile_override(cfg.profile.clone());
                        builder.set_target_override(cfg.target.clone());
                        // cold process + persisted snapshot → warm start
                        if builder.scan_is_cold() {
                            if let Some(project_root) = cogitator_analysis::util::get_project_root(
                                "dbt_project.yml",
                                std::path::Path::new(&root),
                            ) {
                                if let Some(scan) = server_cache_load(&project_root) {
                                    builder.seed_scan(scan);
                                }
                            }
                        }
                        #[allow(unused_mut)]
                        let mut ctx = builder.build(&root);
                        let stats = builder.cache_stats();
                        tracing::debug!(hits = stats.0, misses = stats.1, "scan cache totals");
                        #[cfg(feature = "duckdb-parser")]
                        if warehouse_toggles.warehouse_columns.load(Ordering::Relaxed) {
                            enrich_warehouse_columns(&mut ctx, &root, &cfg, &enrich_derived);
                        }
                        // catalog.json columns fill whatever is still empty —
                        // always on: one stamp-cached local file the user made
                        // with `dbt docs generate`, adapter-agnostic (03 §10)
                        if let Some(project_root) = ctx.project_root.clone() {
                            let cap = enrich_limits
                                .artifact_cap_bytes
                                .load(std::sync::atomic::Ordering::Relaxed);
                            let set = enrich_derived.artifacts(&project_root, cap);
                            if let Some(catalog) = &set.catalog {
                                enrich_catalog_columns(&mut ctx, catalog);
                            }
                        }
                        if let Some(project_root) = ctx.project_root.as_deref() {
                            cache_persist::save(project_root, builder.export_scan());
                        }
                        (ctx, stats)
                    }
                })
                .await;
                match built {
                    Ok((ctx, (hits, misses))) => {
                        let elapsed_ms = started.elapsed().as_millis() as u64;
                        {
                            let mut info = refresh_status.lock().expect("status lock");
                            info.refreshes += 1;
                            info.last_refresh_ms = elapsed_ms;
                            info.cache_hits = hits;
                            info.cache_misses = misses;
                        }
                        crate::metrics::record_refresh(
                            elapsed_ms as f64,
                            hits.saturating_sub(prev_hits),
                            misses.saturating_sub(prev_misses),
                        );
                        (prev_hits, prev_misses) = (hits, misses);
                        progress.end(format!(
                            "{} models, {} sources ({} ms)",
                            ctx.model_detail_map.len(),
                            ctx.source_detail_map.len(),
                            elapsed_ms
                        ));
                        tracing::info!(
                            models = ctx.model_detail_map.len(),
                            sources = ctx.source_detail_map.len(),
                            elapsed_ms,
                            "context refreshed"
                        );
                        // proactive feedback: explain *why* features are degraded
                        // and what fixes it — at most once per distinct cause
                        let warning = if ctx.project_root.is_none() && !root.is_empty() {
                            Some(
                                cogitator_error::guidance::Guidance::new("No dbt project found.")
                                    .because(format!(
                                        "dbt_project.yml was not located from '{root}' \
                                     (searched upward and downward)."
                                    ))
                                    .fix(
                                        "open the folder containing dbt_project.yml; \
                                     hover/completion for refs stays empty until then.",
                                    )
                                    .to_message(),
                            )
                        } else if ctx.project_root.is_some()
                            && ctx.dialect.as_str().is_empty()
                            && !ctx.project.profile.is_empty()
                        {
                            let searched: Vec<String> =
                                cogitator_analysis::util::profiles_candidates(
                                    &cfg.dir_override,
                                    &root,
                                )
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect();
                            Some(
                                cogitator_error::guidance::Guidance::new(format!(
                                    "SQL dialect not resolved for profile '{}'.",
                                    ctx.project.profile
                                ))
                                .because(format!(
                                    "no profiles.yml with a matching target was found. \
                                 Searched: {}.",
                                    searched.join(", ")
                                ))
                                .fix(
                                    "create profiles.yml with this profile in one of those \
                                 places, or point --profiles-dir, \
                                 initializationOptions.profilesDir or DBT_PROFILES_DIR \
                                 at its directory. Function docs and dialect keywords \
                                 are generic until resolved.",
                                )
                                .to_message(),
                            )
                        } else {
                            // dialect fine: check the active target's auth next
                            auth_warning(&cfg, &root, &ctx, &refresh_derived)
                        };
                        if warning != warned {
                            if let Some(text) = &warning {
                                refresh_link.show_message(
                                    cogitator_lsp::message_type::WARNING,
                                    text.clone(),
                                );
                            }
                            warned = warning;
                        }
                        refresh_revision.fetch_add(1, Ordering::Relaxed);
                        let ctx = Arc::new(ctx);
                        refresh_derived.spawn_schema_job(
                            ctx.clone(),
                            refresh_limits
                                .artifact_cap_bytes
                                .load(std::sync::atomic::Ordering::Relaxed),
                        );
                        let _ = ctx_tx.send(ctx);
                    }
                    Err(err) => {
                        progress.end("refresh failed");
                        tracing::warn!(%err, "context refresh panicked");
                    }
                }
            } // per-root loop
        }
    }
}

/// One secondary-root rebuild (multi-root): same scan-cache warm start
/// and persistence as the primary, no watch publish, no degradation
/// warnings (the primary owns the message stream), result into `extra`
/// keyed by the resolved project root.
#[allow(clippy::type_complexity)]
async fn secondary_refresh(
    root: &str,
    profiles: &Arc<std::sync::Mutex<ProfilesConfig>>,
    builders: &Arc<std::sync::Mutex<std::collections::HashMap<String, ContextBuilder>>>,
    extra: &Arc<super::multi_root::ExtraContexts>,
    link: &ClientLink,
    status: &Arc<std::sync::Mutex<StatusInfo>>,
    revision: &Arc<AtomicU64>,
) {
    let cfg = profiles.lock().expect("profiles config lock").clone();
    let progress = link.progress_begin(format!("dbt: refreshing secondary project ({root})"));
    let started = std::time::Instant::now();
    let built = tokio::task::spawn_blocking({
        let root = root.to_string();
        let builders = builders.clone();
        move || {
            let span = tracing::info_span!("context_refresh", root = %root, secondary = true);
            let _enter = span.enter();
            let resolved = cogitator_analysis::util::get_project_root(
                "dbt_project.yml",
                std::path::Path::new(&root),
            )
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| root.clone());
            let mut map = builders.lock().expect("secondary builders lock");
            let builder = map.entry(resolved).or_default();
            builder.set_profiles_dir(cfg.dir_override.clone());
            builder.set_profile_override(cfg.profile.clone());
            builder.set_target_override(cfg.target.clone());
            if builder.scan_is_cold() {
                if let Some(project_root) = cogitator_analysis::util::get_project_root(
                    "dbt_project.yml",
                    std::path::Path::new(&root),
                ) {
                    if let Some(scan) = server_cache_load(&project_root) {
                        builder.seed_scan(scan);
                    }
                }
            }
            let ctx = builder.build(&root);
            if let Some(project_root) = ctx.project_root.as_deref() {
                cache_persist::save(project_root, builder.export_scan());
            }
            ctx
        }
    })
    .await;
    match built {
        Ok(ctx) => {
            let elapsed_ms = started.elapsed().as_millis() as u64;
            {
                let mut info = status.lock().expect("status lock");
                info.refreshes += 1;
                info.last_refresh_ms = elapsed_ms;
            }
            progress.end(format!(
                "{} models, {} sources ({} ms, secondary)",
                ctx.model_detail_map.len(),
                ctx.source_detail_map.len(),
                elapsed_ms
            ));
            tracing::info!(
                models = ctx.model_detail_map.len(),
                sources = ctx.source_detail_map.len(),
                elapsed_ms,
                secondary = true,
                "context refreshed"
            );
            if let Some(project_root) = ctx.project_root.clone() {
                revision.fetch_add(1, Ordering::Relaxed);
                extra.insert(project_root.display().to_string(), Arc::new(ctx));
            }
            // a folder without dbt_project.yml stays silent: the primary
            // root owns user-facing degradation messages
        }
        Err(err) => {
            progress.end("secondary refresh failed");
            tracing::warn!(%err, "secondary context refresh panicked");
        }
    }
}

/// Auth problems of the *active* target, as one Reason/Solution warning.
fn auth_warning(
    cfg: &ProfilesConfig,
    root: &str,
    ctx: &DbtContext,
    derived: &derived_cache::DerivedCache,
) -> Option<String> {
    if ctx.project.profile.is_empty() {
        return None;
    }
    let path = cogitator_analysis::util::profiles_candidates(&cfg.dir_override, root)
        .into_iter()
        .find(|p| p.is_file())?;
    let inventory = derived.profiles_inventory(&path, &ctx.project.profile, &cfg.allow);
    let profile_name = if cfg.profile.is_empty() {
        ctx.project.profile.as_str()
    } else {
        cfg.profile.as_str()
    };
    let profile = inventory.profile(profile_name)?;
    let target_name = if cfg.target.is_empty() {
        profile.default_target.as_str()
    } else {
        cfg.target.as_str()
    };
    let target = profile.targets.iter().find(|t| t.name == target_name)?;
    if target.auth.issues.is_empty() {
        return None;
    }
    let list: Vec<String> = target
        .auth
        .issues
        .iter()
        .map(|i| format!("• {}", i.message))
        .collect();
    Some(
        cogitator_error::guidance::Guidance::new(format!(
            "Connection auth for {profile_name}/{target_name} ({}, {}) has problems:\n{}",
            target.adapter_type,
            target.auth.method.as_str(),
            list.join("\n")
        ))
        .at(inventory.path.display().to_string())
        .fix("fix the profile — dbt run/test/compile will fail until then.")
        .to_message(),
    )
}

/// duckdb builds, `warehouse.columns: true` only (03 §10): source
/// tables whose YAML declares no columns get them from the warehouse's
/// information_schema, read-only, during the context refresh — never
/// on a request path. Any open/query failure switches the pass off for
/// this refresh with a trace.
#[cfg(feature = "duckdb-parser")]
fn enrich_warehouse_columns(
    ctx: &mut DbtContext,
    root: &str,
    cfg: &ProfilesConfig,
    derived: &derived_cache::DerivedCache,
) {
    if ctx.dialect.as_str() != "duckdb" || ctx.project.profile.is_empty() {
        return;
    }
    let Some(path) = cogitator_analysis::util::profiles_candidates(&cfg.dir_override, root)
        .into_iter()
        .find(|p| p.is_file())
    else {
        return;
    };
    let inventory = derived.profiles_inventory(&path, &ctx.project.profile, &cfg.allow);
    let profile_name = if cfg.profile.is_empty() {
        ctx.project.profile.as_str()
    } else {
        cfg.profile.as_str()
    };
    let Some(profile) = inventory.profile(profile_name) else {
        return;
    };
    let target_name = if cfg.target.is_empty() {
        profile.default_target.as_str()
    } else {
        cfg.target.as_str()
    };
    let Some(target) = profile.targets.iter().find(|t| t.name == target_name) else {
        return;
    };
    let Some(warehouse) = &target.warehouse_path else {
        return;
    };
    let mut warehouse = std::path::PathBuf::from(warehouse);
    if warehouse.is_relative() {
        if let Some(project_root) = ctx.project_root.as_deref() {
            warehouse = project_root.join(warehouse);
        }
    }
    if !warehouse.is_file() {
        return;
    }
    for source in ctx.source_detail_map.values_mut() {
        let schema = source.schema.clone().unwrap_or_else(|| source.name.clone());
        for table in source.tables.values_mut() {
            if !table.columns.is_empty() {
                continue;
            }
            let identifier = table
                .identifier
                .clone()
                .unwrap_or_else(|| table.name.clone());
            match cogitator_duckdb::catalog::table_columns(&warehouse, &schema, &identifier) {
                Ok(columns) => {
                    table.columns = columns
                        .into_iter()
                        .map(|c| cogitator_schemas::ColumnEntry {
                            name: c.name,
                            description: format!("{} (from warehouse)", c.data_type),
                            name_pos: Default::default(),
                            data_type: None,
                            tests: Vec::new(),
                        })
                        .collect();
                }
                Err(guidance) => {
                    tracing::debug!(
                        error = %guidance.to_message(),
                        "warehouse introspection off for this refresh"
                    );
                    return;
                }
            }
        }
    }
}

/// Thin indirection so the refresh closure reads cleanly.
fn server_cache_load(
    project_root: &std::path::Path,
) -> Option<cogitator_analysis::context::persist::PersistedScan> {
    cache_persist::load(project_root)
}

/// Source tables whose YAML declares no columns get them from
/// target/catalog.json (dbt docs generate) — the adapter-agnostic
/// sibling of the warehouse pass; a live warehouse wins, the catalog
/// fills the rest. Model columns stay YAML-only on purpose: filling
/// them here would mute the undocumented_columns lint.
fn enrich_catalog_columns(
    ctx: &mut DbtContext,
    catalog: &cogitator_analysis::artifacts::catalog::CatalogIndex,
) {
    for source in ctx.source_detail_map.values_mut() {
        for table in source.tables.values_mut() {
            if !table.columns.is_empty() {
                continue;
            }
            let key = (source.name.clone(), table.name.clone());
            let Some(entry) = catalog.sources.get(&key) else {
                continue;
            };
            table.columns = entry
                .columns
                .iter()
                .map(|column| cogitator_schemas::ColumnEntry {
                    name: column.name.clone(),
                    description: format!("{} (from catalog)", column.data_type),
                    name_pos: Default::default(),
                    data_type: None,
                    tests: Vec::new(),
                })
                .collect();
        }
    }
}
