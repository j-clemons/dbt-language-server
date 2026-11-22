//! Task wiring (specs/rust-port/05 §6.2):
//! reader → dispatcher (owns Documents) → writer; debounced refresh task
//! publishing `Arc<DbtContext>` snapshots over a watch channel.

mod cache_persist;
mod client_link;
mod commands;
mod compiled;
pub(crate) mod config_file;
mod config_watcher;
mod derived_cache;
mod diagnostics;
mod dispatcher;
mod function_docs;
mod fusion;
mod handlers;
mod jobs;
mod multi_root;
mod offload;
mod refresh;
mod reply;
mod snapshot;
mod status;
mod test_hooks;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use cogitator_analysis::{ContextBuilder, DbtContext};
use cogitator_lsp::rpc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

/// What the server has been doing — read by `dbt.status`, written by the
/// startup wiring and the refresh task.
#[derive(Debug, Default, Clone)]
pub struct StatusInfo {
    pub backend: &'static str,
    pub refreshes: u64,
    pub last_refresh_ms: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

/// Hot-tunable numeric settings (cogitator.yml `limits:` / `refresh:`),
/// read lock-free at their use sites.
#[derive(Debug)]
pub struct SharedLimits {
    pub compiled_cap_bytes: std::sync::atomic::AtomicU64,
    pub refresh_debounce_ms: std::sync::atomic::AtomicU64,
    /// target/ artifact files past this are skipped (03 §10).
    pub artifact_cap_bytes: std::sync::atomic::AtomicU64,
}

impl Default for SharedLimits {
    fn default() -> Self {
        SharedLimits {
            compiled_cap_bytes: std::sync::atomic::AtomicU64::new(2 * 1024 * 1024),
            refresh_debounce_ms: std::sync::atomic::AtomicU64::new(200),
            artifact_cap_bytes: std::sync::atomic::AtomicU64::new(64 * 1024 * 1024),
        }
    }
}

/// Hot-togglable feature switches (cogitator.yml `lint:`/`warehouse:`),
/// read lock-free where diagnostics and completion assemble.
#[derive(Debug, Default)]
pub struct SharedToggles {
    /// `config_file::lint_flags` bits; 0 = every check off.
    pub lints: std::sync::atomic::AtomicU32,
    pub warehouse_columns: std::sync::atomic::AtomicBool,
    /// `perf`/`perf_rules`: an atomic can't hold a severity per rule, so
    /// readers clone this snapshot (cheap) and a config change swaps it.
    pub perf: std::sync::RwLock<
        Arc<cogitator_analysis::features::plan::diagnostics_config::PerfSettings>,
    >,
}

/// Shared profiles configuration: seeded from launch flags, replaced by
/// initializationOptions and `dbt.setTarget`, read by every context refresh.
#[derive(Debug, Default, Clone)]
pub struct ProfilesConfig {
    /// `--profiles-dir` / initializationOptions.profilesDir.
    pub dir_override: String,
    /// Profiles readable besides the project's own (`--profiles-allow`).
    pub allow: Vec<String>,
    /// Active profile override; empty → the project's profile.
    pub profile: String,
    /// Active target override; empty → the profile's default target.
    pub target: String,
}

/// Input fingerprint for the modelInfo/modelTests result cache: the
/// model's schema-YAML stamp plus its compiled-file stamp. `None` when
/// the model cannot be resolved cheaply (versioned indirections) —
/// those calls skip the cache rather than risk a stale answer.
pub(crate) fn schema_fingerprint(
    ctx: &DbtContext,
    uri: &str,
    model: Option<&str>,
) -> Option<String> {
    let requested = match model {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => cogitator_analysis::features::model_name_from_uri(uri)?,
    };
    let details = ctx.model_detail_map.get(&requested)?;
    let schema = details.schema_uri.as_deref().and_then(derived_cache::stamp);
    let compiled = ctx.project_root.as_deref().map(|root| {
        derived_cache::stamp(&cogitator_analysis::features::compiled_sql_path(
            root,
            &ctx.project.name,
            &details.uri,
        ))
    });
    Some(format!("{schema:?}|{compiled:?}"))
}

pub async fn run(
    fusion_path: String,
    use_tree_sitter: bool,
    profiles: ProfilesConfig,
    launch: config_file::FileConfig,
) -> Result<()> {
    // Backend selection (06 §3.5): tree-sitter is the default; phase-1 stays
    // compiled as the fallback and behind --no-tree-sitter. The status
    // backend string reports what actually loaded, not what was asked for.
    let (documents, backend) = if use_tree_sitter {
        match cogitator_treesitter::TreeSitterParser::new() {
            Ok(parser) => (dbt_ls_documents(Box::new(parser)), "tree-sitter"),
            Err(err) => {
                tracing::warn!(%err, "tree-sitter unavailable; using phase-1 pipeline");
                (
                    dbt_ls_documents(Box::new(cogitator_parser::JinjaSqlParser)),
                    "minijinja+sqlparser",
                )
            }
        }
    } else {
        (
            dbt_ls_documents(Box::new(cogitator_parser::JinjaSqlParser)),
            "minijinja+sqlparser",
        )
    };

    // writer task: sole owner of stdout (05 §6.2 / 01 §4)
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    // from here on, INFO+ events reach the editor's log view too
    crate::client_log::set_sink(write_tx.clone());
    let request_ids = Arc::new(std::sync::atomic::AtomicI64::new(1));
    // set at initialize from the client's window.workDoneProgress capability
    let progress_supported = Arc::new(AtomicBool::new(false));
    let status = Arc::new(std::sync::Mutex::new(StatusInfo {
        backend,
        ..StatusInfo::default()
    }));

    let fusion_enabled = Arc::new(AtomicBool::new(false));
    if !fusion_path.is_empty() {
        let flag = fusion_enabled.clone();
        let path = fusion_path.clone();
        let link = client_link::ClientLink::new(
            write_tx.clone(),
            request_ids.clone(),
            progress_supported.clone(),
        );
        tokio::spawn(async move {
            let valid = fusion::validate_fusion(&path).await;
            flag.store(valid, Ordering::Relaxed);
            if !valid {
                link.show_message(
                    cogitator_lsp::message_type::ERROR,
                    cogitator_error::guidance::Guidance::new("dbt Fusion features are disabled.")
                        .because(format!(
                            "'{path}' was not found or is not a dbt-fusion binary \
                             (its --version must report dbt-fusion)."
                        ))
                        .fix("install dbt Fusion, or pass the correct path via --fusion <path>.")
                        .to_message(),
                );
            }
        });
    }
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(frame) = write_rx.recv().await {
            tracing::trace!(target: "cogitator::wire", bytes = frame.len(), "frame write");
            if stdout.write_all(&frame).await.is_err() {
                break;
            }
            let _ = stdout.flush().await;
        }
    });

    // refresh task: debounced context rebuilds → watch snapshots
    let (ctx_tx, ctx_rx) = watch::channel(Arc::new(DbtContext::default()));
    let (refresh_tx, refresh_rx) = mpsc::unbounded_channel::<String>();
    let builder = Arc::new(std::sync::Mutex::new(ContextBuilder::default()));
    // launch flags; initializationOptions and dbt.setTarget replace fields
    let profiles = Arc::new(std::sync::Mutex::new(profiles));
    let refresh_profiles = profiles.clone();
    let derived = Arc::new(derived_cache::DerivedCache::default());
    let refresh_derived = derived.clone();
    let limits = Arc::new(SharedLimits::default());
    // --config seeds the hot-tunable numbers; the file at the root may
    // re-tune them later (the watcher's baseline is captured post-seed)
    if let Some(mib) = launch.limits.compiled_sql_mib {
        limits
            .compiled_cap_bytes
            .store(mib * 1024 * 1024, Ordering::Relaxed);
    }
    if let Some(ms) = launch.refresh.debounce_ms {
        limits.refresh_debounce_ms.store(ms, Ordering::Relaxed);
    }
    if let Some(mib) = launch.limits.artifact_mib {
        limits
            .artifact_cap_bytes
            .store(mib * 1024 * 1024, Ordering::Relaxed);
    }
    let jobs = Arc::new(jobs::DbtJobs::default());
    // multi-root: secondary project contexts + the primary root marker
    let extra_contexts = Arc::new(multi_root::ExtraContexts::default());
    let refresh_extra = extra_contexts.clone();
    let primary_root = Arc::new(std::sync::Mutex::new(String::new()));
    let refresh_primary = primary_root.clone();
    // workspace/diagnostic result ids: one global revision per rebuild
    let context_revision = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let refresh_revision = context_revision.clone();
    let toggles = Arc::new(SharedToggles::default());
    toggles.lints.store(launch.lint.flags(0), Ordering::Relaxed);
    if let Some(on) = launch.warehouse.columns {
        toggles.warehouse_columns.store(on, Ordering::Relaxed);
    }
    *toggles.perf.write().expect("perf settings lock") =
        Arc::new(launch.perf_settings(&Default::default()));
    let refresh_limits = limits.clone();
    #[cfg(feature = "duckdb-parser")]
    let refresh_toggles = toggles.clone();
    let refresh_link = client_link::ClientLink::new(
        write_tx.clone(),
        request_ids.clone(),
        progress_supported.clone(),
    );
    let refresh_status = status.clone();
    tokio::spawn(
        refresh::RefreshTask {
            link: refresh_link,
            profiles: refresh_profiles,
            derived: refresh_derived,
            limits: refresh_limits,
            #[cfg(feature = "duckdb-parser")]
            toggles: refresh_toggles,
            status: refresh_status,
            revision: refresh_revision,
            builder,
            ctx_tx,
            extra: refresh_extra,
            primary_root: refresh_primary,
        }
        .run(refresh_rx),
    );

    // config watcher: <root>/cogitator.yml hot-reloads into the shared
    // state above; the dispatcher hands it the root at initialize
    let (config_root_tx, config_root_rx) = watch::channel::<Option<String>>(None);
    let (config_poke_tx, config_poke_rx) = mpsc::unbounded_channel::<()>();
    let config_status = Arc::new(std::sync::Mutex::new(
        config_watcher::ConfigStatus::default(),
    ));
    tokio::spawn(
        config_watcher::ConfigWatcher {
            root_rx: config_root_rx,
            poke_rx: config_poke_rx,
            link: client_link::ClientLink::new(
                write_tx.clone(),
                request_ids.clone(),
                progress_supported.clone(),
            ),
            refresh_tx: refresh_tx.clone(),
            profiles: profiles.clone(),
            limits: limits.clone(),
            toggles: toggles.clone(),
            status: config_status.clone(),
        }
        .run(),
    );

    // reader loop feeding the dispatcher (same task: ordering preserved)
    let dispatcher_link =
        client_link::ClientLink::new(write_tx, request_ids, progress_supported.clone());
    let mut dispatcher = dispatcher::Dispatcher::new(
        documents,
        dispatcher_link,
        ctx_rx,
        refresh_tx,
        fusion_path,
        fusion_enabled,
        profiles,
        progress_supported,
        status,
        derived,
        limits,
        config_root_tx,
        config_poke_tx,
        config_status,
        toggles,
        jobs,
        context_revision,
        extra_contexts,
        primary_root,
    );

    let mut stdin = tokio::io::stdin();
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 16 * 1024];
    loop {
        // drain complete frames before reading more
        loop {
            match rpc::parse_frame(&buf) {
                Ok(Some(frame)) => {
                    let data: Vec<u8> = buf.drain(..frame.total_len).collect();
                    let content =
                        &data[frame.content_start..frame.content_start + frame.content_len];
                    dispatcher.handle(content).await;
                }
                Ok(None) => break,
                Err(err) => {
                    // Malformed stream: Go's scanner stopped; mirror that.
                    tracing::error!(%err, "malformed frame; shutting down reader");
                    drop(dispatcher);
                    writer.await.ok();
                    crate::observability::flush(); // batched spans survive this exit path too
                    return Ok(());
                }
            }
        }
        let n = stdin.read(&mut chunk).await?;
        if n == 0 {
            break; // EOF → clean exit like Go's scanner loop
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    drop(dispatcher);
    writer.await.ok();
    crate::observability::flush(); // stdin EOF used to lose batched spans
    Ok(())
}

fn dbt_ls_documents(
    parser: Box<dyn cogitator_parser::DocumentParser + Send>,
) -> cogitator_analysis::Documents {
    cogitator_analysis::Documents::with_parser(parser)
}
