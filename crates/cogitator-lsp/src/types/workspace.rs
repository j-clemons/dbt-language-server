//! Workspace-edit and $/progress wire types: the shapes behind
//! textDocument/rename results, the server's workspace/applyEdit requests
//! (dbt.extractModelYaml) and window/workDoneProgress. Every field that is
//! `null` on the wire stays a plain Option — no skip_serializing_if, so
//! the payloads match the former hand-built JSON key for key.

use serde::{Deserialize, Serialize};

use super::core::Range;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub range: Range,
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// `version` is always `null` here (the server never tracks document
/// versions for edits it initiates), but the key must be present.
#[derive(Debug, Clone, Serialize)]
pub struct OptionalVersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TextDocumentEdit {
    #[serde(rename = "textDocument")]
    pub text_document: OptionalVersionedTextDocumentIdentifier,
    pub edits: Vec<TextEdit>,
}

impl TextDocumentEdit {
    pub fn new(uri: String, edits: Vec<TextEdit>) -> Self {
        Self {
            text_document: OptionalVersionedTextDocumentIdentifier { uri, version: None },
            edits,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateFile {
    pub kind: &'static str,
    pub uri: String,
}

impl CreateFile {
    pub fn new(uri: String) -> Self {
        Self {
            kind: "create",
            uri,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RenameFile {
    pub kind: &'static str,
    #[serde(rename = "oldUri")]
    pub old_uri: String,
    #[serde(rename = "newUri")]
    pub new_uri: String,
}

impl RenameFile {
    pub fn new(old_uri: String, new_uri: String) -> Self {
        Self {
            kind: "rename",
            old_uri,
            new_uri,
        }
    }
}

/// Untagged: each variant serializes as its own object; the file
/// operations carry their `kind` discriminator as a real field and a
/// TextDocumentEdit carries none, exactly like the LSP spec.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum DocumentChange {
    Edit(TextDocumentEdit),
    Create(CreateFile),
    Rename(RenameFile),
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceEdit {
    #[serde(rename = "documentChanges")]
    pub document_changes: Vec<DocumentChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApplyWorkspaceEditParams {
    pub label: String,
    pub edit: WorkspaceEdit,
}

// ---------------------------------------------------------------------------
// window/workDoneProgress ($/progress)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct WorkDoneProgressCreateParams {
    pub token: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressParams {
    pub token: String,
    pub value: ProgressValue,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ProgressValue {
    Begin { title: String },
    End { message: String },
}
