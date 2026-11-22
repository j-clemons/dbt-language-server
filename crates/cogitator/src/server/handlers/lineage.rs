//! The shell of typed column lineage: gathers the project snapshot,
//! artifacts and (for control flow only) the compiled file, then hands
//! the pure function values. Offloaded callers only.

use std::sync::Arc;

use cogitator_analysis::artifacts::ArtifactSet;
use cogitator_analysis::features::model_name_from_uri;
use cogitator_analysis::features::plan::schemas::SchemaSnapshot;
use cogitator_analysis::features::plan::PlanInput;
use cogitator_analysis::lineage::{self, ModelLineage};
use cogitator_analysis::Document;
use cogitator_parser::ParsedDocument;

use super::super::derived_cache::DerivedCache;
use super::super::snapshot::Snapshot;
use super::plan::plan_key;
use super::plan_compiled::CompiledProbe;

/// Lineage inputs gathered once per request (per fan-out, for dbt.impact).
pub struct TypedLineage<'a> {
    snap: &'a Snapshot,
    derived: &'a DerivedCache,
    artifacts: ArtifactSet,
    project: Option<Arc<SchemaSnapshot>>,
}

impl<'a> TypedLineage<'a> {
    pub fn new(snap: &'a Snapshot, derived: &'a DerivedCache, artifact_cap: u64) -> Self {
        let root = snap.ctx.project_root.as_deref();
        TypedLineage {
            snap,
            derived,
            artifacts: root
                .map_or_else(ArtifactSet::default, |r| derived.artifacts(r, artifact_cap)),
            project: root.and_then(|r| derived.project_schemas(r)),
        }
    }

    /// An open document, cached per (generation, plan key). The key moves
    /// with the schema revision, the context, the artifacts and the
    /// compiled file's stamp, like a plan's.
    pub fn for_open_document(&self, doc: &Document, uri: &str) -> Arc<Option<ModelLineage>> {
        let key = plan_key(&self.snap.ctx, self.derived, &self.artifacts, uri, false);
        self.derived.typed_lineage(uri, doc.generation, key, || {
            self.compute(&doc.parsed, uri, |text| saved_as(uri, text))
        })
    }

    /// A file not open in the editor, read from disk and computed per call.
    pub fn for_saved_file(&self, uri: &str) -> Option<ModelLineage> {
        let path = cogitator_lsp::uri_to_fs_path(uri).unwrap_or_else(|| uri.to_string());
        let text = std::fs::read_to_string(path).ok()?;
        let parsed =
            cogitator_parser::pipeline::parse_document(&text, &self.snap.ctx.dialect).ok()?;
        self.compute(&parsed, uri, |_| true)
    }

    /// `saved` rules out unsaved edits before a compiled file is consulted:
    /// the compiled SQL describes the file on disk.
    fn compute(
        &self,
        parsed: &ParsedDocument,
        uri: &str,
        saved: impl FnOnce(&str) -> bool,
    ) -> Option<ModelLineage> {
        let ctx = &self.snap.ctx;
        let model = model_name_from_uri(uri);
        let revision = self.project.as_ref().map_or(0, |p| p.revision);
        let input = PlanInput::from_context(
            ctx,
            &parsed.text,
            model.as_deref(),
            &self.artifacts,
            revision,
        )
        .with_project(self.project.as_deref());
        if !lineage::needs_compiled(parsed) {
            return lineage::typed_column_lineage(parsed, &input);
        }
        if !saved(&parsed.text) {
            return None;
        }
        let file = CompiledProbe::new(ctx, uri).read(self.snap.compiled_cap_bytes);
        lineage::typed_column_lineage(parsed, &input.with_compiled(file.input()))
    }
}

/// True when `text` is exactly what is on disk for `uri`.
fn saved_as(uri: &str, text: &str) -> bool {
    let Some(path) = cogitator_lsp::uri_to_fs_path(uri) else {
        return false;
    };
    std::fs::read_to_string(path).is_ok_and(|disk| disk == text)
}

/// Typed lineage for `uri`, open or not.
pub fn typed_lineage_for(
    snap: &Snapshot,
    derived: &DerivedCache,
    uri: &str,
    artifact_cap: u64,
) -> Arc<Option<ModelLineage>> {
    let typed = TypedLineage::new(snap, derived, artifact_cap);
    match snap.doc.as_deref() {
        Some(doc) => typed.for_open_document(doc, uri),
        None => Arc::new(typed.for_saved_file(uri)),
    }
}
