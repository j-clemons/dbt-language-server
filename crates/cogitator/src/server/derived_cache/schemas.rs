//! The project-schema job's slot: one `Arc` snapshot per project root,
//! published by a background job after each refresh. Requests only read
//! the current snapshot; nothing on a request path runs the job.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cogitator_analysis::features::plan::schemas::{
    run_schema_job, JobInput, JobLimits, JobState, SavedFiles, SchemaSnapshot,
};
use cogitator_analysis::DbtContext;

use super::{stamp, DerivedCache};

/// Files larger than this are not read; the model degrades to unreadable.
/// The front end's own cap is smaller, so it stays the one that speaks.
const MAX_MODEL_BYTES: u64 = 1 << 20;

#[derive(Default)]
struct RootSlot {
    /// Held for the whole job: a superseded job finishes quickly.
    job: Mutex<JobState>,
    generation: AtomicU64,
    current: Mutex<Option<Arc<SchemaSnapshot>>>,
}

#[derive(Default)]
pub(crate) struct SchemaSlot {
    roots: Mutex<HashMap<PathBuf, Arc<RootSlot>>>,
}

struct Disk;

impl SavedFiles for Disk {
    fn stamp(&self, path: &Path) -> Option<u64> {
        let mut hasher = DefaultHasher::new();
        stamp(path)?.hash(&mut hasher);
        Some(hasher.finish())
    }

    fn read(&self, path: &Path) -> Option<String> {
        if std::fs::metadata(path).ok()?.len() > MAX_MODEL_BYTES {
            return None;
        }
        std::fs::read_to_string(path).ok()
    }
}

impl DerivedCache {
    /// The latest snapshot for `root`, if a job has finished.
    pub fn project_schemas(&self, root: &Path) -> Option<Arc<SchemaSnapshot>> {
        let slot = self.root_slot(root);
        let current = slot.current.lock().expect("schema snapshot lock");
        current.clone()
    }

    fn root_slot(&self, root: &Path) -> Arc<RootSlot> {
        let mut roots = self.schemas.roots.lock().expect("schema roots lock");
        roots.entry(root.to_path_buf()).or_default().clone()
    }

    /// Starts a background job for this context, superseding one in flight.
    pub fn spawn_schema_job(self: &Arc<Self>, ctx: Arc<DbtContext>, artifact_cap: u64) {
        let Some(root) = ctx.project_root.clone() else {
            return;
        };
        let slot = self.root_slot(&root);
        let generation = slot.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let derived = self.clone();
        tokio::task::spawn_blocking(move || {
            let started = std::time::Instant::now();
            let artifacts = derived.artifacts(&root, artifact_cap);
            let mut state = slot.job.lock().expect("schema job lock");
            let stale = || slot.generation.load(Ordering::SeqCst) != generation;
            if stale() {
                return;
            }
            let input = JobInput {
                dialect: &ctx.dialect,
                models: &ctx.model_detail_map,
                sources: &ctx.source_detail_map,
                graph: &ctx.graph,
                manifest: artifacts.manifest.as_deref(),
                catalog: artifacts.catalog.as_deref(),
                files: &Disk,
                limits: JobLimits::default(),
                stale: &stale,
            };
            let Some(snapshot) = run_schema_job(&input, &mut state) else {
                return;
            };
            tracing::info!(
                models = snapshot.report.models,
                rebuilt = snapshot.report.rebuilt,
                reused = snapshot.report.reused,
                degraded = snapshot.report.degraded.len(),
                revision = snapshot.revision,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "project schemas published"
            );
            *slot.current.lock().expect("schema snapshot lock") = Some(snapshot);
        });
    }
}
