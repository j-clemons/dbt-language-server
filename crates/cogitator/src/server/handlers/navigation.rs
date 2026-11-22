//! Effect-heavy navigation handlers: openSchema/openModel, the lineage
//! message, test jumps and extract-to-YAML — pure over the snapshot
//! (plus a precomputed lineage), returning ordered effects.

use cogitator_analysis::features;
use cogitator_analysis::lineage::{ModelLineage, SourceRef};
use cogitator_error::guidance::Guidance;
use cogitator_lsp::{message_type, Position, ShowDocumentParams};

use super::super::reply::{Effect, Reply};
use super::super::snapshot::Snapshot;

fn show_document(uri: String, selection: Option<cogitator_lsp::Range>) -> Effect {
    Effect::ShowDocument(ShowDocumentParams {
        uri,
        take_focus: true,
        selection,
    })
}

fn warning(text: impl Into<String>) -> Effect {
    Effect::ShowMessage {
        level: message_type::WARNING,
        text: text.into(),
    }
}

fn info(text: impl Into<String>) -> Effect {
    Effect::ShowMessage {
        level: message_type::INFO,
        text: text.into(),
    }
}

/// `dbt.openSchema`: navigate to the schema entry, or explain.
#[tracing::instrument(skip_all, fields(uri = %short_uri(uri)))]
pub fn open_schema(snap: &Snapshot, uri: &str) -> Reply {
    let doc = snap.doc.as_deref();
    match features::go_to_schema(&snap.ctx, doc, uri, Position::default()) {
        Some(location) => Reply::null().with(show_document(location.uri, Some(location.range))),
        None => Reply::null().with(warning(
            Guidance::new("No schema entry found for this model.")
                .because("the model has no entry in a properties YAML file.")
                .fix("add it under `models:` in a schema.yml inside your model-paths.")
                .to_message(),
        )),
    }
}

/// `dbt.openModel`: navigate to a model's file by uri or name.
#[tracing::instrument(skip_all, fields(uri = %short_uri(uri)))]
pub fn open_model(snap: &Snapshot, uri: &str, model: Option<&str>) -> Reply {
    let target = if !uri.is_empty() {
        Some(uri.to_string())
    } else {
        model
            .and_then(|m| snap.ctx.model_detail_map.get(m))
            .filter(|d| !d.uri.as_os_str().is_empty())
            .map(|d| cogitator_lsp::fs_path_to_uri(&d.uri))
    };
    match target {
        Some(target) => Reply::null().with(show_document(target, None)),
        None => {
            let requested = model.unwrap_or_default();
            let guidance = cogitator_error::guidance::Guidance::new(format!(
                "Cannot open model '{requested}'."
            ))
            .because("no uri was given and the name is not in the project.")
            .suggest(cogitator_error::guidance::closest_matches(
                requested,
                snap.ctx.model_detail_map.keys().map(String::as_str),
            ))
            .fix("pass {\"uri\": …} or {\"model\": <a known model>}.");
            Reply::null().with(warning(guidance.to_message()))
        }
    }
}

/// `dbt.showLineage`: the precomputed lineage formatted into a message.
#[tracing::instrument(skip_all)]
pub fn show_lineage(lineage: Option<&ModelLineage>) -> Reply {
    match lineage {
        Some(lineage) if !lineage.columns.is_empty() => {
            let mut lines: Vec<String> = lineage
                .columns
                .iter()
                .take(20)
                .map(|c| {
                    let sources: Vec<String> = c
                        .sources
                        .iter()
                        .map(|s| match s {
                            SourceRef::Model { name, column } => format!("{name}.{column}"),
                            SourceRef::Source {
                                source,
                                table,
                                column,
                            } => format!("{source}.{table}.{column}"),
                            SourceRef::Unknown { name, column } => format!("{name}.{column}"),
                        })
                        .collect();
                    format!("{} ← {}", c.name, sources.join(", "))
                })
                .collect();
            if lineage.columns.len() > 20 {
                lines.push(format!("… and {} more columns", lineage.columns.len() - 20));
            }
            Reply::null().with(info(format!(
                "Column lineage{}:\n{}",
                lineage.origin_note(),
                lines.join("\n")
            )))
        }
        _ => Reply::null().with(warning(
            Guidance::new("Could not derive column lineage for this model.")
                .because(
                    "the template contains Jinja control blocks ({% if %}/{% for %}) \
                     or the SQL did not parse as a single SELECT.",
                )
                .fix(
                    "lineage works on models whose rendered SQL is one SELECT \
                     statement; check for unclosed expressions or control flow.",
                )
                .to_message(),
        )),
    }
}

/// `dbt.goToTests`: jump to the first test and list the rest.
#[tracing::instrument(skip_all, fields(uri = %short_uri(uri)))]
pub fn go_to_tests(snap: &Snapshot, uri: &str) -> Reply {
    match features::model_tests(&snap.ctx, uri) {
        Some((model, tests)) if !tests.is_empty() => {
            let first = &tests[0];
            let mut reply = Reply::null().with(show_document(first.uri.clone(), Some(first.range)));
            if tests.len() > 1 {
                let lines: Vec<String> = tests
                    .iter()
                    .map(|t| {
                        let mut line = format!("• {}", t.name);
                        if let Some(column) = &t.column {
                            line.push_str(&format!(" on {column}"));
                        }
                        if let Some(v) = t.version {
                            line.push_str(&format!(" (v{v})"));
                        }
                        line
                    })
                    .collect();
                reply = reply.with(info(format!("Tests for {model}:\n{}", lines.join("\n"))));
            }
            reply
        }
        _ => Reply::null().with(warning(
            Guidance::new("No tests defined for this model.")
                .because("its schema YAML has no data_tests/tests entries.")
                .fix(
                    "add them under the model or its columns — dbt.extractModelYaml \
                     scaffolds the entry if it is missing.",
                )
                .to_message(),
        )),
    }
}

/// `dbt.extractModelYaml`: scaffold `<model>.yml` or add a version.
#[tracing::instrument(skip_all, fields(uri = %short_uri(uri)))]
pub fn extract_model_yaml(snap: &Snapshot, uri: &str, lineage: Option<&ModelLineage>) -> Reply {
    match features::extract_model_yaml(&snap.ctx, uri, lineage) {
        None => Reply::null().with(warning(
            Guidance::new("Cannot extract a schema YAML here.")
                .because(
                    "this is not a known model, a <model>.yml already exists \
                     without an entry, or the schema file did not parse.",
                )
                .fix("open the model's .sql file and check its YAML.")
                .to_message(),
        )),
        Some(features::ExtractEdit::CreateFile { path, content }) => {
            let new_uri = cogitator_lsp::fs_path_to_uri(&path);
            Reply::null()
                .with(Effect::ApplyEdit(cogitator_lsp::ApplyWorkspaceEditParams {
                    label: "Extract model YAML".to_string(),
                    edit: cogitator_lsp::WorkspaceEdit {
                        document_changes: vec![
                            cogitator_lsp::DocumentChange::Create(cogitator_lsp::CreateFile::new(
                                new_uri.clone(),
                            )),
                            cogitator_lsp::DocumentChange::Edit(
                                cogitator_lsp::TextDocumentEdit::new(
                                    new_uri.clone(),
                                    vec![cogitator_lsp::TextEdit {
                                        range: cogitator_lsp::Range::default(),
                                        new_text: content,
                                    }],
                                ),
                            ),
                        ],
                    },
                }))
                .with(show_document(new_uri, None))
                .with(info(format!(
                    "Created {} with the model's columns — fill in the \
                     descriptions and add data_tests.",
                    path.display()
                )))
        }
        Some(features::ExtractEdit::AddVersion {
            schema_path,
            edits,
            next,
        }) => {
            let schema_uri = cogitator_lsp::fs_path_to_uri(&schema_path);
            let selection = edits.first().map(|(range, _)| *range);
            let text_edits: Vec<cogitator_lsp::TextEdit> = edits
                .iter()
                .map(|(range, text)| cogitator_lsp::TextEdit {
                    range: *range,
                    new_text: text.clone(),
                })
                .collect();
            Reply::null()
                .with(Effect::ApplyEdit(cogitator_lsp::ApplyWorkspaceEditParams {
                    label: format!("Add model version {next}"),
                    edit: cogitator_lsp::WorkspaceEdit {
                        document_changes: vec![cogitator_lsp::DocumentChange::Edit(
                            cogitator_lsp::TextDocumentEdit::new(schema_uri.clone(), text_edits),
                        )],
                    },
                }))
                .with(show_document(schema_uri, selection))
                .with(info(format!(
                    "Added version {next} to {}.\n\
                     Next: create the versioned model file \
                     (<model>_v{next}.sql) or set `defined_in` on the \
                     version entry.",
                    schema_path.display()
                )))
        }
    }
}

/// The file name only — spans must not leak whole paths needlessly.
pub(crate) fn short_uri(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or(uri)
}
