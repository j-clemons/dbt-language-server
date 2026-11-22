//! The diagnostics-adjacent request handlers: codeAction (quickfix
//! assembly from context diagnostics), pull diagnostics with lint +
//! artifact merge, and the workspace-wide report with revision ids.

use std::sync::atomic::Ordering;

use cogitator_analysis::features;

use cogitator_lsp::{Envelope, FullDocumentDiagnosticReport};

use super::Dispatcher;

impl Dispatcher {
    pub(super) fn code_action(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<Envelope<cogitator_lsp::CodeActionParams>>(content)
        else {
            self.invalid_params(content, "textDocument/codeAction");
            return;
        };
        let Some(params) = req.params else {
            self.invalid_params(content, "textDocument/codeAction");
            return;
        };
        let uri = params.text_document.uri.into_owned();
        let context = params.context.unwrap_or_default();
        let only: Option<Vec<String>> = context
            .only
            .map(|kinds| kinds.into_iter().map(|k| k.into_owned()).collect());
        let diagnostics = context.diagnostics;
        let range = params.range;
        let snap = self.snapshot_for(&uri);
        let derived = self.derived.clone();
        // offloaded since the editing-actions batch: target choices,
        // the relation catalog and lineage all touch the filesystem
        self.offload_blocking(req.id, "textDocument/codeAction", move || {
            let targets = crate::server::handlers::profiles::target_choices(&snap, &derived, &uri);
            let mut actions = features::code_actions(
                &snap.ctx,
                snap.doc.as_deref(),
                &uri,
                range,
                only.as_deref(),
                targets.as_deref(),
                &diagnostics,
            );
            let mut extra = Vec::new();
            if let Some(doc) = snap.doc.as_deref() {
                if uri.ends_with(".yml") || uri.ends_with(".yaml") {
                    if let Some((model, details)) = features::model_entry_at(&snap.ctx, &uri, range)
                    {
                        let model_uri = cogitator_lsp::fs_path_to_uri(&details.uri);
                        let tests = features::model_tests(&snap.ctx, &model_uri)
                            .map(|(_, tests)| tests)
                            .unwrap_or_default();
                        extra.extend(features::scaffold_tests_action(
                            &doc.text, &uri, model, details, &tests,
                        ));
                        let lineage = crate::server::handlers::commands::lineage_for(
                            &derived, &snap.ctx, None, &model_uri,
                        );
                        if let Some(lineage) = (*lineage).as_ref() {
                            extra.extend(features::inherit_description_actions(
                                &snap.ctx, &doc.text, &uri, details, lineage,
                            ));
                        }
                    }
                } else {
                    let target =
                        crate::server::handlers::profiles::active_target_relation(&snap, &derived);
                    let catalog = features::RelationCatalog::build(&snap.ctx, target.as_ref());
                    extra.extend(features::convert_literal_relations(doc, &uri, &catalog));
                }
            }
            extra.retain(|action| action.allowed_by(only.as_deref()));
            actions.extend(extra);
            crate::server::reply::Reply::result(&actions)
        });
    }

    /// textDocument/diagnostic (pull): same static set the push pipeline uses.
    pub(super) fn pull_diagnostics(&mut self, content: &[u8]) {
        let Some((id, uri, _)) = Self::position_request(content) else {
            self.invalid_params(content, "textDocument/diagnostic");
            return;
        };
        let snap = self.snapshot_for(&uri);
        let lints =
            features::lint::LintOptions::from_flags(self.toggles.lints.load(Ordering::Relaxed));
        let derived = self.derived.clone();
        let artifact_cap = self.limits.artifact_cap_bytes.load(Ordering::Relaxed);
        let perf = self.perf_settings();
        self.offload_blocking(id, "textDocument/diagnostic", move || {
            // the pull path affords lineage (cached per generation), so
            // the undocumented-columns and contract checks run here, not
            // on push; typed, so an expanded star counts as its columns
            let lineage = (lints.undocumented_columns && !uri.ends_with(".yml")).then(|| {
                crate::server::handlers::lineage::typed_lineage_for(
                    &snap,
                    &derived,
                    &uri,
                    artifact_cap,
                )
            });
            let mut items = crate::server::diagnostics::static_diagnostics(
                &snap.ctx,
                snap.doc.as_deref(),
                &uri,
                lints,
                lineage.as_ref().and_then(|l| (**l).as_ref()),
            );
            if let Some(root) = snap.ctx.project_root.as_deref() {
                let set = derived.artifacts(root, artifact_cap);
                items.extend(features::artifact_diagnostics(&snap.ctx, &uri, &set));
            }
            if perf.active() {
                if let (Some(doc), Some(plan)) = (
                    snap.doc.as_deref(),
                    crate::server::handlers::plan::document_plan(
                        &snap,
                        &derived,
                        &uri,
                        artifact_cap,
                        perf.catalog_stats,
                    ),
                ) {
                    items.extend(crate::server::diagnostics::perf_diagnostics(
                        doc, &plan, &perf,
                    ));
                }
            }
            crate::server::reply::Reply::result(&FullDocumentDiagnosticReport {
                kind: "full",
                items,
            })
        });
    }

    /// workspace/diagnostic (01 §6.4): project-wide dangling findings
    /// under the global context-revision result-id scheme.
    pub(super) fn workspace_diagnostics(&mut self, content: &[u8]) {
        let Ok(req) =
            serde_json::from_slice::<Envelope<cogitator_lsp::WorkspaceDiagnosticParams>>(content)
        else {
            self.invalid_params(content, "workspace/diagnostic");
            return;
        };
        let previous: std::collections::HashMap<String, String> = req
            .params
            .unwrap_or_default()
            .previous_result_ids
            .into_iter()
            .map(|p| (p.uri, p.value))
            .collect();
        let ctx = self.snapshot();
        let revision = self.context_revision.load(Ordering::Relaxed).to_string();
        self.offload_blocking(req.id, "workspace/diagnostic", move || {
            let findings =
                features::project_diagnostics(&ctx, &|path| std::fs::read_to_string(path).ok());
            let mut items = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for (uri, diagnostics) in findings {
                seen.insert(uri.clone());
                if previous.get(&uri) == Some(&revision) {
                    items.push(cogitator_lsp::WorkspaceDocumentReport::Unchanged {
                        uri,
                        result_id: revision.clone(),
                        version: None,
                    });
                } else {
                    items.push(cogitator_lsp::WorkspaceDocumentReport::Full {
                        uri,
                        result_id: revision.clone(),
                        version: None,
                        items: diagnostics,
                    });
                }
            }
            // files the client still tracks that are clean now answer an
            // empty Full so stale findings clear
            for (uri, _) in previous {
                if !seen.contains(&uri) {
                    items.push(cogitator_lsp::WorkspaceDocumentReport::Full {
                        uri,
                        result_id: revision.clone(),
                        version: None,
                        items: Vec::new(),
                    });
                }
            }
            crate::server::reply::Reply::result(&cogitator_lsp::WorkspaceDiagnosticReport { items })
        });
    }
}
