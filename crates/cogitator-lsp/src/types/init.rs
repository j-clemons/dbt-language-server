//! initialize response: capabilities + server info (01 §3.1; post-parity
//! capabilities are additive — see the note in the spec).

use serde::Serialize;

use super::results::ResultResponse;

// ---------------------------------------------------------------------------
// initialize response (01 §3.1 — exact shape)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct InitializeResult {
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

#[derive(Debug, Serialize)]
pub struct ServerCapabilities {
    #[serde(rename = "textDocumentSync")]
    pub text_document_sync: i32,
    #[serde(rename = "hoverProvider")]
    pub hover_provider: bool,
    #[serde(rename = "definitionProvider")]
    pub definition_provider: bool,
    #[serde(rename = "completionProvider")]
    pub completion_provider: CompletionOptions,
    #[serde(rename = "executeCommandProvider")]
    pub execute_command_provider: ExecuteCommandOptions,
    // Post-parity additive capabilities (FEATURE_REQUESTS.md, 01 §3.1 note).
    #[serde(rename = "referencesProvider")]
    pub references_provider: bool,
    #[serde(rename = "documentSymbolProvider")]
    pub document_symbol_provider: bool,
    #[serde(rename = "workspaceSymbolProvider")]
    pub workspace_symbol_provider: bool,
    #[serde(rename = "documentHighlightProvider")]
    pub document_highlight_provider: bool,
    #[serde(rename = "foldingRangeProvider")]
    pub folding_range_provider: bool,
    #[serde(rename = "selectionRangeProvider")]
    pub selection_range_provider: bool,
    #[serde(rename = "semanticTokensProvider")]
    pub semantic_tokens_provider: SemanticTokensOptions,
    #[serde(rename = "signatureHelpProvider")]
    pub signature_help_provider: SignatureHelpOptions,
    #[serde(rename = "diagnosticProvider")]
    pub diagnostic_provider: DiagnosticOptions,
    #[serde(rename = "renameProvider")]
    pub rename_provider: RenameOptions,
    #[serde(rename = "callHierarchyProvider")]
    pub call_hierarchy_provider: bool,
    #[serde(rename = "codeLensProvider")]
    pub code_lens_provider: CodeLensOptions,
    /// Plain bool: command-only actions, no kinds, no resolve.
    #[serde(rename = "codeActionProvider")]
    pub code_action_provider: bool,
    #[serde(rename = "inlayHintProvider")]
    pub inlay_hint_provider: bool,
    #[serde(rename = "documentLinkProvider")]
    pub document_link_provider: super::links::DocumentLinkOptions,
    #[serde(rename = "linkedEditingRangeProvider")]
    pub linked_editing_range_provider: bool,
    #[serde(rename = "documentFormattingProvider")]
    pub document_formatting_provider: bool,
}

#[derive(Debug, Serialize)]
pub struct CompletionOptions {
    #[serde(rename = "resolveProvider")]
    pub resolve_provider: bool,
    /// `" "` is deliberate: dbt SQL completes after keywords (`from `,
    /// `join `); clients that find it noisy can strip it client-side.
    #[serde(rename = "triggerCharacters")]
    pub trigger_characters: &'static [&'static str],
}

#[derive(Debug, Serialize)]
pub struct SemanticTokensOptions {
    pub legend: SemanticTokensLegend,
    pub full: bool,
}

#[derive(Debug, Serialize)]
pub struct SemanticTokensLegend {
    #[serde(rename = "tokenTypes")]
    pub token_types: &'static [&'static str],
    #[serde(rename = "tokenModifiers")]
    pub token_modifiers: &'static [&'static str],
}

#[derive(Debug, Serialize)]
pub struct SignatureHelpOptions {
    #[serde(rename = "triggerCharacters")]
    pub trigger_characters: &'static [&'static str],
}

#[derive(Debug, Serialize)]
pub struct DiagnosticOptions {
    #[serde(rename = "interFileDependencies")]
    pub inter_file_dependencies: bool,
    #[serde(rename = "workspaceDiagnostics")]
    pub workspace_diagnostics: bool,
}

#[derive(Debug, Serialize)]
pub struct RenameOptions {
    #[serde(rename = "prepareProvider")]
    pub prepare_provider: bool,
}

#[derive(Debug, Serialize)]
pub struct CodeLensOptions {
    #[serde(rename = "resolveProvider")]
    pub resolve_provider: bool,
}

/// Semantic token legend (order defines the type indices in the encoding).
pub const SEMANTIC_TOKEN_TYPES: &[&str] = &[
    "keyword",
    "function",
    "macro",
    "variable",
    "string",
    "number",
    "operator",
    "namespace",
];

#[derive(Debug, Serialize)]
pub struct ExecuteCommandOptions {
    pub commands: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct ServerInfo {
    pub name: &'static str,
    pub version: String,
}

/// The executeCommand roster (append-only: clients may assume index 0
/// is dbt.goToSchema). ONE truth for the capability and did-you-mean.
pub const COMMANDS: &[&str] = &[
    "dbt.goToSchema",
    "dbt.columnLineage",
    "dbt.openSchema",
    "dbt.showLineage",
    "dbt.run",
    "dbt.test",
    "dbt.compile",
    "dbt.modelGraph",
    "dbt.profiles",
    "dbt.setTarget",
    "dbt.extractModelYaml",
    "dbt.modelTests",
    "dbt.goToTests",
    "dbt.status",
    "dbt.projectGraph",
    "dbt.modelInfo",
    "dbt.relation",
    "dbt.compiledSql",
    "dbt.parse",
    "dbt.openModel",
    "dbt.setProfilesDir",
    "dbt.cache",
    "dbt.reloadConfig",
    "dbt.sourceFreshness",
    "dbt.cancel",
    "dbt.runSelection",
    "dbt.deadModels",
    "dbt.impact",
    "dbt.expandJinja",
    "dbt.doctor",
    "dbt.preview",
    "dbt.queryPlan",
];

pub fn initialize_response(id: i64, version: &str) -> ResultResponse<InitializeResult> {
    ResultResponse::new(
        id,
        InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: 2,
                hover_provider: true,
                definition_provider: true,
                completion_provider: CompletionOptions {
                    resolve_provider: true,
                    trigger_characters: &[".", "'", "\"", " ", "("],
                },
                execute_command_provider: ExecuteCommandOptions {
                    commands: COMMANDS.to_vec(),
                },
                references_provider: true,
                document_symbol_provider: true,
                workspace_symbol_provider: true,
                document_highlight_provider: true,
                folding_range_provider: true,
                selection_range_provider: true,
                semantic_tokens_provider: SemanticTokensOptions {
                    legend: SemanticTokensLegend {
                        token_types: SEMANTIC_TOKEN_TYPES,
                        token_modifiers: &[],
                    },
                    full: true,
                },
                signature_help_provider: SignatureHelpOptions {
                    trigger_characters: &["(", ","],
                },
                diagnostic_provider: DiagnosticOptions {
                    // false ON PURPOSE (01 §6.3): cross-file truth travels
                    // through workspace/diagnostic, not per-file re-pulls
                    inter_file_dependencies: false,
                    workspace_diagnostics: true,
                },
                rename_provider: RenameOptions {
                    prepare_provider: true,
                },
                call_hierarchy_provider: true,
                code_lens_provider: CodeLensOptions {
                    resolve_provider: false,
                },
                code_action_provider: true,
                inlay_hint_provider: true,
                document_link_provider: super::links::DocumentLinkOptions {
                    resolve_provider: false,
                },
                linked_editing_range_provider: true,
                document_formatting_provider: true,
            },
            server_info: ServerInfo {
                name: "cogitator",
                version: version.to_string(),
            },
        },
    )
}
