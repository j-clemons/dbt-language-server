//! The textDocument/* and workspace/* REQUEST handlers: one shared
//! decode→snapshot→pure-feature→serialize composition point for the
//! position-shaped ones, offload for anything that touches the fs.

use std::sync::atomic::Ordering;

use cogitator_analysis::{features, yaml_features, DbtContext};

use cogitator_lsp::{
    Envelope, HoverResult, MaybeId, Position, PositionParams, ResultResponse, SelectionRangeParams,
    WorkspaceSymbolParams,
};

use super::Dispatcher;

impl Dispatcher {
    /// Composition point for every position-shaped request: parse → snapshot
    /// → pure feature function → serialize (functional core / imperative
    /// shell, 05 §3). Bad params on a request answer InvalidParams.
    pub(super) fn respond_position<R: serde::Serialize>(
        &mut self,
        content: &[u8],
        method: &str,
        feature: impl FnOnce(&DbtContext, Option<&cogitator_analysis::Document>, &str, Position) -> R,
    ) {
        let Some((id, uri, pos)) = Self::position_request(content) else {
            self.invalid_params(content, method);
            return;
        };
        // multi-root: the context must be the one whose root owns the
        // file, not always the primary (its siblings use snapshot_for)
        let ctx = self.ctx_for(&uri);
        let result = feature(&ctx, self.documents.get(&uri), &uri, pos);
        self.send(&ResultResponse::new(id, result));
    }

    pub(super) fn position_request<'a>(content: &'a [u8]) -> Option<(i64, String, Position)> {
        let req = serde_json::from_slice::<Envelope<PositionParams<'a>>>(content).ok()?;
        let params = req.params?;
        Some((
            req.id,
            params.text_document.uri.into_owned(),
            params.position,
        ))
    }

    pub(super) fn hover(&mut self, content: &[u8]) {
        let derived = self.derived.clone();
        self.respond_position(content, "textDocument/hover", |ctx, doc, uri, pos| {
            HoverResult {
                // Q8: unknown doc → empty hover
                contents: doc
                    .map(|d| {
                        if cogitator_analysis::yaml_cogitator::is_cogitator_config(uri) {
                            cogitator_analysis::yaml_cogitator::cogitator_hover(&d.text, pos)
                                .unwrap_or_default()
                        } else {
                            // dispatcher path: a cache probe, never a compute
                            let lineage = derived.lineage_cached(uri, d.generation);
                            features::hover_with_lineage(
                                ctx,
                                d,
                                pos,
                                lineage.as_deref().and_then(Option::as_ref),
                            )
                        }
                    })
                    .unwrap_or_default(),
            }
        });
        self.warm_lineage(content);
    }

    /// Fills the lineage cache after a hover missed it, off the
    /// dispatcher, so the next hover of this text has the lineage line.
    fn warm_lineage(&self, content: &[u8]) {
        let Some((_, uri, _)) = Self::position_request(content) else {
            return;
        };
        if !uri.ends_with(".sql") {
            return;
        }
        let snap = self.snapshot_for(&uri);
        let Some(generation) = snap.doc.as_deref().map(|doc| doc.generation) else {
            return;
        };
        if self.derived.lineage_cached(&uri, generation).is_some() {
            return;
        }
        let derived = self.derived.clone();
        let cap = self
            .limits
            .artifact_cap_bytes
            .load(std::sync::atomic::Ordering::Relaxed);
        tokio::task::spawn_blocking(move || {
            crate::server::handlers::lineage::typed_lineage_for(&snap, &derived, &uri, cap);
        });
    }

    pub(super) fn definition(&mut self, content: &[u8]) {
        // a documented SQL function materializes its bundled docs as a
        // cached Markdown file and jumps there (03 §9) — offloaded,
        // since that touches the filesystem; everything else keeps the
        // inline pure path
        let Some((id, uri, pos)) = Self::position_request(content) else {
            self.invalid_params(content, "textDocument/definition");
            return;
        };
        let snap = self.snapshot_for(&uri);
        let function = snap
            .doc
            .as_deref()
            .and_then(|doc| features::function_doc_target(&snap.ctx, doc, pos));
        if let (Some(function), Some(root)) = (function, snap.ctx.project_root.clone()) {
            let fallback = cogitator_lsp::Location {
                uri: uri.clone(),
                range: cogitator_lsp::Range::zero_width(pos),
            };
            self.offload_blocking(id, "textDocument/definition", move || {
                match super::super::function_docs::materialize(&root, &function) {
                    Some(path) => super::super::reply::Reply::result(&cogitator_lsp::Location {
                        uri: cogitator_lsp::fs_path_to_uri(&path),
                        range: cogitator_lsp::Range::default(),
                    }),
                    None => super::super::reply::Reply::result(&fallback),
                }
            });
            return;
        }
        let location = match snap.doc.as_deref() {
            Some(doc) => features::definition(&snap.ctx, doc, &uri, pos),
            None => cogitator_lsp::Location {
                uri: uri.clone(),
                range: cogitator_lsp::Range::zero_width(pos),
            },
        };
        self.send(&ResultResponse::new(id, location));
    }

    pub(super) fn completion(&mut self, content: &[u8]) {
        self.respond_position(content, "textDocument/completion", |ctx, doc, uri, pos| {
            match doc {
                // the server's own config file: schema-table completion
                Some(d) if cogitator_analysis::yaml_cogitator::is_cogitator_config(uri) => {
                    cogitator_analysis::yaml_cogitator::cogitator_completion(&d.text, pos)
                }
                // dbt YAML files get schema-key completion (08 §2.6 v1)
                Some(d) if crate::server::diagnostics::is_yaml(uri) => {
                    yaml_features::yaml_completion(uri, &d.text, pos)
                }
                Some(d) => features::completion(ctx, d, pos),
                None => Vec::new(),
            }
        });
    }

    pub(super) fn resolve_completion(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<MaybeId>(content) else {
            self.invalid_params(content, "completionItem/resolve");
            return;
        };
        let Some(id) = req.id else { return };
        let Ok(item) = serde_json::from_slice::<serde_json::Value>(content) else {
            self.invalid_params(content, "completionItem/resolve");
            return;
        };
        let mut item = item.get("params").cloned().unwrap_or(item);
        let ctx = self.snapshot();
        if let Some(docs) = item
            .get("data")
            .and_then(|data| features::resolve_completion_documentation(&ctx, data))
        {
            if let Some(map) = item.as_object_mut() {
                map.insert("documentation".into(), serde_json::Value::String(docs));
            }
        }
        self.send(&ResultResponse::new(id, item));
    }

    pub(super) fn references(&mut self, content: &[u8]) {
        self.respond_position(content, "textDocument/references", features::references);
    }

    pub(super) fn document_symbol(&mut self, content: &[u8]) {
        self.respond_position(content, "textDocument/documentSymbol", |_, doc, uri, _| {
            doc.map(|d| features::document_symbols(d, uri))
                .unwrap_or_default()
        });
    }

    pub(super) fn document_highlight(&mut self, content: &[u8]) {
        self.respond_position(
            content,
            "textDocument/documentHighlight",
            |_, doc, _, pos| {
                doc.map(|d| features::document_highlights(d, pos))
                    .unwrap_or_default()
            },
        );
    }

    pub(super) fn folding_range(&mut self, content: &[u8]) {
        self.respond_position(content, "textDocument/foldingRange", |_, doc, _, _| {
            doc.map(features::folding_ranges).unwrap_or_default()
        });
    }

    pub(super) fn selection_range(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<Envelope<SelectionRangeParams>>(content) else {
            self.invalid_params(content, "textDocument/selectionRange");
            return;
        };
        let Some(params) = req.params else {
            self.invalid_params(content, "textDocument/selectionRange");
            return;
        };
        let uri = params.text_document.uri.into_owned();
        let ranges = self
            .documents
            .get(&uri)
            .map(|doc| features::selection_ranges(doc, &params.positions))
            .unwrap_or_default();
        self.send(&ResultResponse::new(req.id, ranges));
    }

    pub(super) fn semantic_tokens(&mut self, content: &[u8]) {
        self.respond_position(
            content,
            "textDocument/semanticTokens/full",
            |_, doc, _, _| {
                doc.map(features::semantic_tokens)
                    .unwrap_or(cogitator_lsp::SemanticTokens { data: Vec::new() })
            },
        );
    }

    pub(super) fn rename(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<Envelope<cogitator_lsp::RenameParams>>(content)
        else {
            self.invalid_params(content, "textDocument/rename");
            return;
        };
        let Some(params) = req.params else {
            self.invalid_params(content, "textDocument/rename");
            return;
        };
        let uri = params.text_document.uri.into_owned();
        let new_name = params.new_name.into_owned();
        let position = params.position;
        let snap = self.snapshot_for(&uri);
        self.offload_blocking(req.id, "textDocument/rename", move || {
            let edit =
                features::rename_model(&snap.ctx, snap.doc.as_deref(), &uri, position, &new_name);
            crate::server::reply::Reply::result(&edit)
        });
    }

    pub(super) fn prepare_rename(&mut self, content: &[u8]) {
        self.respond_position(content, "textDocument/prepareRename", |_, doc, _, pos| {
            doc.and_then(|d| features::prepare_rename(d, pos))
        });
    }

    /// prepare answers `[item]` for a macro call under the cursor, null
    /// anywhere else (the LSP contract for "nothing here").
    pub(super) fn prepare_call_hierarchy(&mut self, content: &[u8]) {
        self.respond_position(
            content,
            "textDocument/prepareCallHierarchy",
            |ctx, doc, _, pos| {
                doc.and_then(|d| features::prepare_macro_hierarchy(ctx, d, pos))
                    .map(|item| vec![item])
            },
        );
    }

    pub(super) fn call_hierarchy_incoming(&mut self, content: &[u8]) {
        let Ok(req) =
            serde_json::from_slice::<Envelope<cogitator_lsp::CallHierarchyCallsParams>>(content)
        else {
            self.invalid_params(content, "callHierarchy/incomingCalls");
            return;
        };
        let Some(params) = req.params else {
            self.invalid_params(content, "callHierarchy/incomingCalls");
            return;
        };
        let ctx = self.ctx_for(&params.item.uri);
        // offloaded: every caller file is re-read from disk
        self.offload_blocking(req.id, "callHierarchy/incomingCalls", move || {
            let calls = features::macro_incoming_calls(&ctx, &params.item);
            crate::server::reply::Reply::result(&calls)
        });
    }

    pub(super) fn call_hierarchy_outgoing(&mut self, content: &[u8]) {
        let Ok(req) =
            serde_json::from_slice::<Envelope<cogitator_lsp::CallHierarchyCallsParams>>(content)
        else {
            self.invalid_params(content, "callHierarchy/outgoingCalls");
            return;
        };
        let empty: Vec<cogitator_lsp::CallHierarchyIncomingCall> = Vec::new();
        self.send(&ResultResponse::new(req.id, empty));
    }

    pub(super) fn code_lens(&mut self, content: &[u8]) {
        // offloaded since the artifact batch: the run-status lens stats
        // target/ files (05 §6.1 keeps fs off this task)
        let Some((id, uri, _)) = Self::position_request(content) else {
            self.invalid_params(content, "textDocument/codeLens");
            return;
        };
        let snap = self.snapshot_for(&uri);
        let derived = self.derived.clone();
        let cap = self.limits.artifact_cap_bytes.load(Ordering::Relaxed);
        self.offload_blocking(id, "textDocument/codeLens", move || {
            let mut lenses = features::code_lenses(&snap.ctx, &uri);
            if let Some(root) = snap.ctx.project_root.as_deref() {
                let set = derived.artifacts(root, cap);
                lenses.extend(features::run_lens(&snap.ctx, &uri, &set));
                lenses.extend(features::freshness_lenses(&snap.ctx, &uri, &set));
            }
            crate::server::reply::Reply::result(&lenses)
        });
    }

    pub(super) fn inlay_hint(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<Envelope<cogitator_lsp::InlayHintParams>>(content)
        else {
            self.invalid_params(content, "textDocument/inlayHint");
            return;
        };
        let Some(params) = req.params else {
            self.invalid_params(content, "textDocument/inlayHint");
            return;
        };
        let uri = params.text_document.uri.into_owned();
        let range = params.range;
        let snap = self.snapshot_for(&uri);
        let derived = self.derived.clone();
        // offloaded: the relation labels read the profiles inventory
        self.offload_blocking(req.id, "textDocument/inlayHint", move || {
            let target = crate::server::handlers::profiles::active_target_relation(&snap, &derived);
            let hints = match snap.doc.as_deref() {
                Some(doc) => features::inlay_hints(&snap.ctx, doc, &uri, range, target.as_ref()),
                None => Vec::new(),
            };
            crate::server::reply::Reply::result(&hints)
        });
    }

    pub(super) fn document_link(&mut self, content: &[u8]) {
        self.respond_position(content, "textDocument/documentLink", |ctx, doc, _, _| {
            doc.map(|d| features::document_links(ctx, d))
                .unwrap_or_default()
        });
    }

    pub(super) fn linked_editing(&mut self, content: &[u8]) {
        self.respond_position(
            content,
            "textDocument/linkedEditingRange",
            |_, doc, _, pos| doc.and_then(|d| features::linked_editing(d, pos)),
        );
    }

    pub(super) fn formatting(&mut self, content: &[u8]) {
        let Ok(req) =
            serde_json::from_slice::<Envelope<cogitator_lsp::DocumentFormattingParams>>(content)
        else {
            self.invalid_params(content, "textDocument/formatting");
            return;
        };
        let Some(params) = req.params else {
            self.invalid_params(content, "textDocument/formatting");
            return;
        };
        let uri = params.text_document.uri.into_owned();
        let options = features::formatting::FormatOptions {
            tab_size: params.options.tab_size,
            insert_spaces: params.options.insert_spaces,
        };
        let snap = self.snapshot_for(&uri);
        self.offload_blocking(req.id, "textDocument/formatting", move || {
            // decline-on-doubt answers an empty edit list, never an error
            let edits = snap
                .doc
                .as_deref()
                .filter(|_| !uri.ends_with(".yml") && !uri.ends_with(".yaml"))
                .and_then(|doc| features::formatting::format_document(&doc.text, &options))
                .unwrap_or_default();
            crate::server::reply::Reply::result(&edits)
        });
    }

    pub(super) fn signature_help(&mut self, content: &[u8]) {
        self.respond_position(content, "textDocument/signatureHelp", |ctx, doc, _, pos| {
            doc.and_then(|d| features::signature_help(ctx, d, pos))
        });
    }

    pub(super) fn workspace_symbol(&mut self, content: &[u8]) {
        let Ok(req) = serde_json::from_slice::<Envelope<WorkspaceSymbolParams>>(content) else {
            self.invalid_params(content, "workspace/symbol");
            return;
        };
        let query = req
            .params
            .and_then(|p| p.query.map(|q| q.into_owned()))
            .unwrap_or_default();
        let ctx = self.snapshot();
        self.send(&ResultResponse::new(
            req.id,
            features::workspace_symbols(&ctx, &query),
        ));
    }
}
