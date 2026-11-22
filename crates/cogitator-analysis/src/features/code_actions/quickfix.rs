//! Quickfixes assembled from the typed `data` payload that rides the
//! context diagnostics back from the client (01 §6.1-6.2): one action
//! per did-you-mean suggestion, carrying the WorkspaceEdit that applies
//! it. Pure over the wire values — no context, no filesystem.

use cogitator_lsp::{
    CodeAction, Diagnostic, DocumentChange, FixKind, TextDocumentEdit, TextEdit, WorkspaceEdit,
    FIX_ALL_KIND,
};

/// One quickfix per suggestion on every context diagnostic that carries
/// our data payload; foreign diagnostics (no/alien `data`) contribute
/// nothing.
pub fn quickfixes(uri: &str, diagnostics: &[Diagnostic]) -> Vec<CodeAction> {
    let mut out = Vec::new();
    for diagnostic in diagnostics {
        let Some(data) = &diagnostic.data else {
            continue;
        };
        if let FixKind::MultiEdit { fixes } = &data.fix {
            // fixes are already ordered preserving-first; only the first
            // preserving fix may be preferred
            let mut preferred_used = false;
            for fix in fixes {
                let edit = workspace_edit(uri, fix.edits.clone());
                let action = CodeAction::quickfix(fix.title.clone(), diagnostic.clone(), edit);
                let preferred = fix.preserving && !preferred_used;
                preferred_used |= preferred;
                out.push(if preferred {
                    action.preferred()
                } else {
                    action
                });
            }
            continue;
        }
        for suggestion in &data.suggestions {
            let title = match data.fix {
                FixKind::InsertText => format!("Insert '{suggestion}'"),
                FixKind::ReplaceToken | FixKind::MultiEdit { .. } => {
                    format!("Change to '{suggestion}'")
                }
            };
            let edit = workspace_edit(
                uri,
                vec![TextEdit {
                    range: data.token_range,
                    new_text: suggestion.clone(),
                }],
            );
            out.push(CodeAction::quickfix(title, diagnostic.clone(), edit));
        }
    }
    out
}

/// The `source.fixAll` bundle: one action applying every context
/// diagnostic's first preserving fix, skipping one that would overlap an
/// edit already chosen. Only built when the client asked for a
/// `source.fixAll` kind (LSP clients request these explicitly).
pub fn fix_all(
    uri: &str,
    diagnostics: &[Diagnostic],
    only: Option<&[String]>,
) -> Option<CodeAction> {
    let requested = only?
        .iter()
        .any(|k| k.starts_with("source") && FIX_ALL_KIND.starts_with(k.as_str()));
    if !requested {
        return None;
    }
    let mut chosen: Vec<&TextEdit> = Vec::new();
    for diagnostic in diagnostics {
        let Some(FixKind::MultiEdit { fixes }) = diagnostic.data.as_ref().map(|d| &d.fix) else {
            continue;
        };
        let Some(fix) = fixes.iter().find(|f| f.preserving) else {
            continue;
        };
        let conflict = fix
            .edits
            .iter()
            .any(|edit| chosen.iter().any(|taken| overlaps(edit, taken)));
        if !conflict {
            chosen.extend(fix.edits.iter());
        }
    }
    if chosen.is_empty() {
        return None;
    }
    let edits = chosen.into_iter().cloned().collect();
    Some(CodeAction::fix_all(
        "Fix all auto-fixable cogitator problems".to_string(),
        workspace_edit(uri, edits),
    ))
}

fn overlaps(a: &TextEdit, b: &TextEdit) -> bool {
    let (a, b) = (&a.range, &b.range);
    let before = |x: &cogitator_lsp::Range, y: &cogitator_lsp::Range| {
        (x.end.line, x.end.character) <= (y.start.line, y.start.character)
    };
    !(before(a, b) || before(b, a))
}

fn workspace_edit(uri: &str, edits: Vec<TextEdit>) -> WorkspaceEdit {
    WorkspaceEdit {
        document_changes: vec![DocumentChange::Edit(TextDocumentEdit::new(
            uri.to_string(),
            edits,
        ))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cogitator_lsp::{diagnostic_severity, DiagnosticData, MultiEditFix, Position, Range};

    #[test]
    fn multi_edit_produces_one_action_per_fix() {
        let range = Range::zero_width(Position {
            line: 1,
            character: 0,
        });
        let mut diag = Diagnostic::plain(
            range,
            "join makes filter unreachable".into(),
            diagnostic_severity::WARNING,
            "join.left_nullified",
            "cogitator",
        );
        let edit = |text: &str| TextEdit {
            range,
            new_text: text.into(),
        };
        diag.data = Some(DiagnosticData {
            fix: FixKind::MultiEdit {
                fixes: vec![
                    MultiEditFix {
                        title: "Make the join INNER".into(),
                        edits: vec![edit("inner"), edit("")],
                        preserving: true,
                    },
                    MultiEditFix {
                        title: "Move the predicate to ON".into(),
                        edits: vec![edit("on")],
                        preserving: false,
                    },
                ],
            },
            token_range: range,
            suggestions: vec![],
        });
        let actions = quickfixes("file:///m.sql", &[diag]);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].title, "Make the join INNER");
        let wire = serde_json::to_value(actions[0].edit.as_ref().expect("edit")).unwrap();
        assert_eq!(wire["documentChanges"][0]["edits"][0]["newText"], "inner");
        assert_eq!(wire["documentChanges"][0]["edits"][1]["newText"], "");
        assert_eq!(actions[0].is_preferred, Some(true), "the P fix leads");
        assert_eq!(actions[1].is_preferred, None, "N is never preferred");
    }

    fn fix_diag(line: u32, preserving: bool) -> Diagnostic {
        let range = Range::zero_width(Position { line, character: 0 });
        let mut diag = Diagnostic::plain(
            range,
            "m".into(),
            diagnostic_severity::WARNING,
            "model.unused_cte",
            "cogitator",
        );
        diag.data = Some(DiagnosticData {
            fix: FixKind::MultiEdit {
                fixes: vec![MultiEditFix {
                    title: "Delete".into(),
                    edits: vec![TextEdit {
                        range,
                        new_text: String::new(),
                    }],
                    preserving,
                }],
            },
            token_range: range,
            suggestions: vec![],
        });
        diag
    }

    #[test]
    fn fix_all_bundles_only_preserving_fixes() {
        let diags = [fix_diag(1, true), fix_diag(2, false)];
        let only = ["source.fixAll".to_string()];
        let action = fix_all("file:///m.sql", &diags, Some(&only)).expect("bundle");
        assert_eq!(action.kind, Some(FIX_ALL_KIND));
        let wire = serde_json::to_value(action.edit.as_ref().expect("edit")).unwrap();
        assert_eq!(
            wire["documentChanges"][0]["edits"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "{wire}"
        );
    }

    #[test]
    fn fix_all_needs_an_explicit_request_and_a_preserving_fix() {
        let diags = [fix_diag(1, false)];
        let only = ["source.fixAll".to_string()];
        assert!(
            fix_all("file:///m.sql", &diags, Some(&only)).is_none(),
            "no P fix"
        );
        let p = [fix_diag(1, true)];
        assert!(fix_all("file:///m.sql", &p, None).is_none());
        let quickfix = ["quickfix".to_string()];
        assert!(fix_all("file:///m.sql", &p, Some(&quickfix)).is_none());
    }

    #[test]
    fn fix_all_drops_overlapping_fixes() {
        let mut a = fix_diag(1, true);
        if let Some(data) = a.data.as_mut() {
            if let FixKind::MultiEdit { fixes } = &mut data.fix {
                fixes[0].edits[0].range.start.character = 2;
                fixes[0].edits[0].range.end.character = 2;
            }
        }
        let mut wide = fix_diag(1, true);
        if let Some(data) = wide.data.as_mut() {
            if let FixKind::MultiEdit { fixes } = &mut data.fix {
                fixes[0].edits[0].range.end.character = 4;
            }
        }
        // an insertion inside a replaced range conflicts; the first wins
        let only = ["source".to_string()];
        let action = fix_all("file:///m.sql", &[wide, a], Some(&only)).expect("bundle");
        let wire = serde_json::to_value(action.edit.as_ref().expect("edit")).unwrap();
        assert_eq!(
            wire["documentChanges"][0]["edits"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn one_action_per_suggestion_with_the_edit() {
        let range = Range::zero_width(Position {
            line: 3,
            character: 22,
        });
        let mut diag = Diagnostic::plain(
            range,
            "model 'stg_order' not found — did you mean 'stg_orders'?".into(),
            diagnostic_severity::WARNING,
            "dangling-ref",
            "cogitator",
        );
        diag.data = Some(DiagnosticData {
            fix: FixKind::ReplaceToken,
            token_range: range,
            suggestions: vec!["stg_orders".into(), "stg_payments".into()],
        });
        let plain = Diagnostic::plain(
            range,
            "foreign".into(),
            diagnostic_severity::ERROR,
            "x",
            "elsewhere",
        );
        let actions = quickfixes("file:///m.sql", &[diag, plain]);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].title, "Change to 'stg_orders'");
        assert_eq!(actions[0].kind, Some("quickfix"));
        let edit = actions[0].edit.as_ref().expect("edit");
        let wire = serde_json::to_value(edit).unwrap();
        assert_eq!(
            wire["documentChanges"][0]["edits"][0]["newText"], "stg_orders",
            "{wire}"
        );
    }
}
