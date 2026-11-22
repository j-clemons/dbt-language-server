//! textDocument/codeAction wire types (01 §6.2): actions carry a kind,
//! context diagnostics and optionally a WorkspaceEdit, so quickfixes
//! ride the same response as the command-only dbt actions.

use std::borrow::Cow;

use serde::Deserialize;
use serde::Serialize;

use super::core::Range;
use super::params::TextDocumentIdentifier;
use super::results::{Command, Diagnostic};
use super::workspace::WorkspaceEdit;

#[derive(Debug, Deserialize)]
pub struct CodeActionParams<'a> {
    #[serde(borrow, rename = "textDocument")]
    pub text_document: TextDocumentIdentifier<'a>,
    #[serde(default)]
    pub range: Range,
    #[serde(default)]
    pub context: Option<CodeActionContext<'a>>,
}

/// `only` filters by kind prefix; `diagnostics` carry the typed `data`
/// payloads back for quickfix assembly (01 §6.1-6.2).
#[derive(Debug, Default, Deserialize)]
pub struct CodeActionContext<'a> {
    #[serde(borrow, default)]
    pub only: Option<Vec<Cow<'a, str>>>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

/// The kind of the `source.fixAll` bundle: a sub-kind of `source.fixAll`,
/// so a client requesting either one receives it.
pub const FIX_ALL_KIND: &str = "source.fixAll.cogitator";

/// A CodeAction is either command-only (the dbt workflow set — clients
/// run `command` through workspace/executeCommand) or an edit-bearing
/// quickfix; unset members stay off the wire.
#[derive(Debug, Serialize)]
pub struct CodeAction {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<Diagnostic>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<WorkspaceEdit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Command>,
    /// Editors apply a preferred action on one key press. Only a fix
    /// that preserves results is ever set (specs/ir-pipeline/08).
    #[serde(rename = "isPreferred", skip_serializing_if = "Option::is_none")]
    pub is_preferred: Option<bool>,
}

impl CodeAction {
    /// The classic kind-less dbt action: a command, nothing else.
    pub fn command_only(title: String, command: Command) -> Self {
        CodeAction {
            title,
            kind: None,
            diagnostics: None,
            edit: None,
            command: Some(command),
            is_preferred: None,
        }
    }

    /// An edit-bearing quickfix tied to the diagnostic it fixes.
    pub fn quickfix(title: String, diagnostic: Diagnostic, edit: WorkspaceEdit) -> Self {
        CodeAction {
            title,
            kind: Some("quickfix"),
            diagnostics: Some(vec![diagnostic]),
            edit: Some(edit),
            command: None,
            is_preferred: None,
        }
    }

    /// Marks this quickfix as the one to apply by default.
    pub fn preferred(mut self) -> Self {
        self.is_preferred = Some(true);
        self
    }

    /// The bundled `source.fixAll` action (edits only, no diagnostics).
    pub fn fix_all(title: String, edit: WorkspaceEdit) -> Self {
        CodeAction {
            title,
            kind: Some(FIX_ALL_KIND),
            diagnostics: None,
            edit: Some(edit),
            command: None,
            is_preferred: None,
        }
    }

    /// Does this action survive the client's `only` filter? Kind-less
    /// actions honestly drop out of any kind-filtered request.
    pub fn allowed_by(&self, only: Option<&[String]>) -> bool {
        let Some(kinds) = only.filter(|k| !k.is_empty()) else {
            return true;
        };
        let Some(kind) = self.kind else { return false };
        kinds
            .iter()
            .any(|k| k.is_empty() || kind.starts_with(k.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quickfix() -> CodeAction {
        CodeAction::quickfix(
            "Change".into(),
            Diagnostic::plain(
                Range::default(),
                "x".into(),
                super::super::results::diagnostic_severity::WARNING,
                "c",
                "s",
            ),
            WorkspaceEdit {
                document_changes: Vec::new(),
            },
        )
    }

    #[test]
    fn kind_filtering_matrix() {
        let command = CodeAction::command_only(
            "dbt run".into(),
            Command {
                title: "dbt run".into(),
                command: "dbt.run".into(),
                arguments: Vec::new(),
            },
        );
        let fix = quickfix();
        // no filter: everything passes
        assert!(command.allowed_by(None));
        assert!(fix.allowed_by(None));
        // empty filter list = no filter
        assert!(command.allowed_by(Some(&[])));
        // kind filter: kind-less commands drop, quickfixes prefix-match
        let only = vec!["quickfix".to_string()];
        assert!(!command.allowed_by(Some(&only)));
        assert!(fix.allowed_by(Some(&only)));
        let refactor = vec!["refactor".to_string()];
        assert!(!fix.allowed_by(Some(&refactor)));
        let everything = vec![String::new()];
        assert!(fix.allowed_by(Some(&everything)));
    }

    #[test]
    fn wire_shape_omits_unset_members() {
        let json = serde_json::to_value(quickfix()).unwrap();
        assert_eq!(json["kind"], "quickfix");
        assert!(json.get("command").is_none(), "{json}");
        assert!(json["diagnostics"].is_array(), "{json}");
    }
}
