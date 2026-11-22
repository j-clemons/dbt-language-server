//! `dbt.queryPlan` (specs/ir-pipeline/09): the shell around the pure
//! driver — it reads text and artifacts, keys the cache, and serializes.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use cogitator_analysis::artifacts::ArtifactSet;
use cogitator_analysis::features::model_name_from_uri;
use cogitator_analysis::features::plan::schemas::SchemaSnapshot;
use cogitator_analysis::features::plan::{query_plan as drive_plan, suppress, PlanInput};
use cogitator_analysis::DbtContext;
use cogitator_error::guidance::Guidance;
use cogitator_lsp::error_codes;

use super::super::derived_cache::{CachedPlan, DerivedCache};
use super::super::reply::Reply;
use super::super::snapshot::Snapshot;
use super::navigation::short_uri;
use super::plan_compiled::CompiledProbe;

/// The plan for `uri`, cached per (uri, generation, plan key). A closed
/// file is read from disk and planned per call.
#[tracing::instrument(skip_all, fields(uri = %short_uri(uri)))]
pub fn query_plan(
    snap: &Snapshot,
    derived: &DerivedCache,
    uri: &str,
    artifact_cap: u64,
    catalog_stats: bool,
) -> Reply {
    let Some(model) = model_name_from_uri(uri) else {
        return invalid(
            Guidance::new("dbt.queryPlan needs a model's .sql file.")
                .at(uri)
                .fix("pass the uri of a dbt model."),
        );
    };
    let planner = Planner::new(snap, derived, uri, &model, artifact_cap, catalog_stats);
    let plan = match snap.doc.as_deref() {
        Some(doc) => derived.query_plan(uri, doc.generation, planner.key, || {
            planner.compute(&doc.text)
        }),
        None => {
            let path = cogitator_lsp::uri_to_fs_path(uri).unwrap_or_else(|| uri.to_string());
            let Ok(text) = std::fs::read_to_string(&path) else {
                return invalid(
                    Guidance::new(format!("Could not read model '{model}'."))
                        .at(path)
                        .fix("open the model in the editor, or check the uri."),
                );
            };
            planner.compute(&text).map(Arc::new)
        }
    };
    match plan {
        Some(plan) => Reply::raw(&plan.json),
        None => Reply::null(),
    }
}

/// The plan of an OPEN model, computed on a cache miss: the pull
/// diagnostics path, already offloaded. Absent for anything that is not
/// a model the project knows.
pub fn document_plan(
    snap: &Snapshot,
    derived: &DerivedCache,
    uri: &str,
    artifact_cap: u64,
    catalog_stats: bool,
) -> Option<Arc<CachedPlan>> {
    let doc = snap.doc.as_deref()?;
    let model = model_name_from_uri(uri)?;
    if !snap.ctx.model_detail_map.contains_key(&model) {
        return None;
    }
    let planner = Planner::new(snap, derived, uri, &model, artifact_cap, catalog_stats);
    derived.query_plan(uri, doc.generation, planner.key, || {
        planner.compute(&doc.text)
    })
}

/// The cache key a plan of `uri` is stored under. The push path probes
/// with this, so every route must agree on it.
pub fn plan_key(
    ctx: &Arc<DbtContext>,
    derived: &DerivedCache,
    artifacts: &ArtifactSet,
    uri: &str,
    catalog_stats: bool,
) -> u64 {
    let revision = project_of(ctx, derived).map_or(0, |p| p.revision);
    CompiledProbe::new(ctx, uri).cache_key(base_key(ctx, artifacts, revision, catalog_stats))
}

/// Everything one plan reads, gathered once per request.
struct Planner<'a> {
    snap: &'a Snapshot,
    artifacts: ArtifactSet,
    project: Option<Arc<SchemaSnapshot>>,
    compiled: CompiledProbe,
    model: &'a str,
    revision: u64,
    key: u64,
    catalog_stats: bool,
}

impl<'a> Planner<'a> {
    fn new(
        snap: &'a Snapshot,
        derived: &DerivedCache,
        uri: &str,
        model: &'a str,
        artifact_cap: u64,
        catalog_stats: bool,
    ) -> Self {
        let artifacts = match snap.ctx.project_root.as_deref() {
            Some(root) => derived.artifacts(root, artifact_cap),
            None => ArtifactSet::default(),
        };
        let project = project_of(&snap.ctx, derived);
        let revision = project.as_ref().map_or(0, |p| p.revision);
        let compiled = CompiledProbe::new(&snap.ctx, uri);
        // A fresh `dbt compile` changes nothing else in the key.
        let key = compiled.cache_key(base_key(&snap.ctx, &artifacts, revision, catalog_stats));
        Planner {
            snap,
            artifacts,
            project,
            compiled,
            model,
            revision,
            key,
            catalog_stats,
        }
    }

    fn compute(&self, text: &str) -> Option<CachedPlan> {
        let ctx = &self.snap.ctx;
        let file = self.compiled.read(self.snap.compiled_cap_bytes);
        let mut input =
            PlanInput::from_context(ctx, text, Some(self.model), &self.artifacts, self.revision)
                .with_compiled(file.input())
                .with_project(self.project.as_deref());
        input.catalog_stats = self.catalog_stats;
        let mut plan = drive_plan(&input);
        let suppressions = suppress::scan(text, &ctx.dialect);
        suppress::mark(&mut plan, &suppressions, text);
        let json = serde_json::to_string(&plan).ok()?;
        Some(CachedPlan {
            plan,
            suppressions,
            json: Arc::from(json),
        })
    }
}

fn project_of(ctx: &DbtContext, derived: &DerivedCache) -> Option<Arc<SchemaSnapshot>> {
    ctx.project_root
        .as_deref()
        .and_then(|root| derived.project_schemas(root))
}

/// Changes whenever a plan input does: the context snapshot, a re-read
/// manifest or catalog, a new schema revision, or `perf.catalog_stats`.
/// Pointer identity, like the results slot; never reported.
fn base_key(
    ctx: &Arc<DbtContext>,
    artifacts: &ArtifactSet,
    revision: u64,
    catalog_stats: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    revision.hash(&mut hasher);
    (Arc::as_ptr(ctx) as usize).hash(&mut hasher);
    artifacts
        .manifest
        .as_ref()
        .map(|m| Arc::as_ptr(m) as usize)
        .hash(&mut hasher);
    artifacts
        .catalog
        .as_ref()
        .map(|c| Arc::as_ptr(c) as usize)
        .hash(&mut hasher);
    catalog_stats.hash(&mut hasher);
    hasher.finish()
}

fn invalid(guidance: Guidance) -> Reply {
    Reply::error(error_codes::INVALID_PARAMS, guidance.to_message())
}
