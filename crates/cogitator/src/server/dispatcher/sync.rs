//! Document sync + the push diagnostics pipeline: didOpen/didChange/
//! didSave/didClose, the fs-free publish path, and per-URI fusion
//! compiles. State mutation stays on the dispatcher task (05 §6.2).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use cogitator_analysis::features;
use cogitator_analysis::features::plan::diagnostics_config::PerfSettings;

use cogitator_lsp::{Diagnostic, DidChangeParams, DidOpenParams, DidSaveParams, Envelope};

use super::fusion;
use super::Dispatcher;

impl Dispatcher {
    pub(super) fn did_open(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<Envelope<DidOpenParams>>(content) else {
            tracing::debug!("textDocument/didOpen: bad params");
            return;
        };
        let Some(params) = req.params else { return };
        let uri = params.text_document.uri.into_owned();
        let dialect = self.ctx_for(&uri).dialect.clone();
        self.documents
            .open(uri.clone(), &params.text_document.text, &dialect);
        tracing::debug!(%uri, "opened");
        let _ = self.refresh_tx.send(self.covering_or_lazy_root(&uri));
        if self.is_config_uri(&uri) {
            let _ = self.config_poke_tx.send(());
        }
        self.publish_static_diagnostics(&uri);
        self.spawn_fusion(uri);
    }

    pub(super) fn did_close(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<Envelope<DidSaveParams>>(content) else {
            return;
        };
        let Some(params) = req.params else { return };
        self.documents.close(&params.text_document.uri);
        tracing::debug!(uri = %params.text_document.uri, "closed");
    }

    pub(super) fn did_save(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<Envelope<DidSaveParams>>(content) else {
            return;
        };
        let Some(params) = req.params else { return };
        let uri = params.text_document.uri.into_owned();
        tracing::debug!(%uri, "saved");
        let _ = self.refresh_tx.send(self.refresh_root_for(&uri));
        if self.is_config_uri(&uri) {
            let _ = self.config_poke_tx.send(());
        }
        self.publish_static_diagnostics(&uri);
        self.spawn_fusion(uri);
    }

    pub(super) fn did_change(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<Envelope<DidChangeParams>>(content) else {
            return;
        };
        let Some(params) = req.params else { return };
        let dialect = self.ctx_for(&params.text_document.uri).dialect.clone();
        self.documents
            .change(&params.text_document.uri, &params.content_changes, &dialect);
    }

    /// Diagnostics for one open doc — see server::diagnostics.
    pub(super) fn static_diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        let lints =
            features::lint::LintOptions::from_flags(self.toggles.lints.load(Ordering::Relaxed));
        // push stays inline and lineage-free; the pull path adds the
        // lineage-backed check
        let ctx = self.ctx_for(uri);
        let mut items = crate::server::diagnostics::static_diagnostics(
            &ctx,
            self.documents.get(uri),
            uri,
            lints,
            None,
        );
        // artifacts come from the LAST loaded set — no fs on this task
        // (the offloaded lens/pull paths keep it warm)
        if let Some(root) = ctx.project_root.as_deref() {
            let set = self.derived.artifacts_cached(root);
            items.extend(features::artifact_diagnostics(&ctx, uri, &set));
            items.extend(self.perf_diagnostics_cached(&ctx, uri, &set));
        }
        items
    }

    /// Plan diagnostics from a plan the pull path or `dbt.queryPlan`
    /// already computed for this exact text and schema. This path never
    /// plans: no cached plan means none here.
    fn perf_diagnostics_cached(
        &self,
        ctx: &Arc<cogitator_analysis::DbtContext>,
        uri: &str,
        artifacts: &cogitator_analysis::artifacts::ArtifactSet,
    ) -> Vec<Diagnostic> {
        let perf = self.perf_settings();
        if !perf.active() {
            return Vec::new();
        }
        let Some(doc) = self.documents.get(uri) else {
            return Vec::new();
        };
        let key = crate::server::handlers::plan::plan_key(
            ctx,
            &self.derived,
            artifacts,
            uri,
            perf.catalog_stats,
        );
        self.derived
            .plan_cached(uri, doc.generation, key)
            .map(|plan| crate::server::diagnostics::perf_diagnostics(doc, &plan, &perf))
            .unwrap_or_default()
    }

    /// The live perf settings: a cheap Arc clone.
    pub(super) fn perf_settings(&self) -> Arc<PerfSettings> {
        self.toggles
            .perf
            .read()
            .expect("perf settings lock")
            .clone()
    }

    /// Push pipeline (didOpen/didSave). Fusion owns publishes when enabled
    /// (07 §3 precedence) — never double-publish.
    pub(super) fn publish_static_diagnostics(&mut self, uri: &str) {
        if self.fusion_enabled.load(Ordering::Relaxed) {
            return;
        }
        if self.documents.get(uri).is_none() {
            return;
        }
        let items = self.static_diagnostics(uri);
        let note = cogitator_lsp::publish_diagnostics(uri.to_string(), items);
        self.send(&note);
    }

    /// The refresh key for a just-opened file: its covering secondary
    /// root, the primary root when it owns the file, or — a file from a
    /// FOREIGN dbt project (multi-root, lazy) — the file's own directory,
    /// which the refresh task resolves upward to dbt_project.yml.
    fn covering_or_lazy_root(&self, uri: &str) -> String {
        if let Some((root, _)) = self.extra.for_uri(uri) {
            return root;
        }
        let primary = self.snapshot();
        let under_primary = primary
            .project_root
            .as_deref()
            .map(|root| root.display().to_string())
            .or_else(|| (!self.root_path.is_empty()).then(|| self.root_path.clone()))
            .is_some_and(|root| crate::server::multi_root::root_contains(&root, uri));
        if under_primary {
            return self.root_path.clone();
        }
        // a real file outside every known root: let the refresh task
        // look for its project; non-file uris stay on the primary
        match cogitator_lsp::uri_to_fs_path(uri) {
            Some(path) => std::path::Path::new(&path)
                .parent()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| self.root_path.clone()),
            None => self.root_path.clone(),
        }
    }

    /// Fusion compile per 04 §2, async + cancellable per URI (fixes Q9).
    pub(super) fn spawn_fusion(&mut self, uri: String) {
        if !self.fusion_enabled.load(Ordering::Relaxed) {
            return;
        }
        // completed jobs never removed themselves: drop finished handles so
        // the map stays bounded by the number of URIs actually in flight
        self.fusion_jobs.retain(|_, handle| !handle.is_finished());
        if let Some(previous) = self.fusion_jobs.remove(&uri) {
            previous.abort();
        }
        let ctx = self.ctx_for(&uri);
        let job = fusion::FusionJob {
            fusion_path: self.fusion_path.clone(),
            project_name: ctx.project.name.clone(),
            uri: uri.clone(),
            write_tx: self.link.writer(),
        };
        let handle = tokio::spawn(job.run());
        self.fusion_jobs.insert(uri, handle.abort_handle());
    }
}
