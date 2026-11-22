//! "Add missing columns to the schema YAML": from a model's SQL, the
//! columns it produces (per lineage) that its properties entry does not
//! document become `- name:` items of that entry — appended to its
//! `columns:` list, or a new `columns:` list when it has none. The fix of
//! the `lint-undocumented-column` hint; the edit lands in the YAML file,
//! whose text the shell reads.

use std::path::Path;

use cogitator_lsp::{CodeAction, DocumentChange, TextDocumentEdit, TextEdit, WorkspaceEdit};

use super::super::lint::undocumented::missing_columns;
use super::yaml_text::{block_end, indent, insert_after_line, is_content, plain_name};
use crate::context::ModelDetails;
use crate::lineage::ModelLineage;

/// The action for `model`, given the text of its properties file; None
/// when nothing is missing or the entry cannot be found in the text.
pub fn add_missing_columns_action(
    model: &str,
    details: &ModelDetails,
    lineage: &ModelLineage,
    yaml_text: &str,
) -> Option<CodeAction> {
    let schema: &Path = details.schema_uri.as_deref()?;
    let missing: Vec<&str> = missing_columns(details, lineage)
        .into_iter()
        .filter_map(plain_name)
        .collect();
    if missing.is_empty() {
        return None;
    }
    let edit = columns_edit(
        yaml_text,
        details.schema_range.start.line as usize,
        &missing,
    )?;
    let yaml_uri = cogitator_lsp::fs_path_to_uri(schema);
    Some(CodeAction {
        title: format!(
            "Add {} missing column(s) of '{model}' to its schema YAML",
            missing.len()
        ),
        kind: Some("quickfix"),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            document_changes: vec![DocumentChange::Edit(TextDocumentEdit::new(
                yaml_uri,
                vec![edit],
            ))],
        }),
        command: None,
        is_preferred: None,
    })
}

/// The insertion for the entry whose `name:` is on `name_line`: after the
/// last item of its `columns:` list, or a new list at the entry's end.
fn columns_edit(text: &str, name_line: usize, missing: &[&str]) -> Option<TextEdit> {
    let lines: Vec<&str> = text.lines().collect();
    // the entry's `- ` line: the name's own line, or the nearest above
    let dash_line = (0..=name_line).rev().find(|&i| {
        lines
            .get(i)
            .is_some_and(|l| l.trim_start().starts_with('-'))
    })?;
    let dash = indent(lines[dash_line]);
    let key_indent = dash + 2;
    let entry_end = block_end(&lines, dash_line, dash);
    let columns_line = (dash_line + 1..=entry_end)
        .find(|&i| indent(lines[i]) == key_indent && lines[i].trim_start().starts_with("columns:"));
    let (after, item_indent, header) = match columns_line {
        Some(line) => {
            // `columns: []` or any inline value: leave it to the reader
            let value = lines[line].trim_start()["columns:".len()..].trim();
            if !value.is_empty() && !value.starts_with('#') {
                return None;
            }
            let end = block_end(&lines, line, key_indent);
            let item_indent = (line + 1..=end)
                .find(|&i| is_content(lines[i]) && lines[i].trim_start().starts_with('-'))
                .map_or(key_indent + 2, |i| indent(lines[i]));
            (end, item_indent, None)
        }
        None => (entry_end, key_indent + 2, Some(key_indent)),
    };
    let mut body = String::new();
    if let Some(header_indent) = header {
        body.push_str(&format!("{}columns:\n", " ".repeat(header_indent)));
    }
    for name in missing {
        let pad = " ".repeat(item_indent);
        body.push_str(&format!("{pad}- name: {name}\n{pad}  description: \"\"\n"));
    }
    Some(insert_after_line(text, after, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cogitator_lsp::Position;

    const YAML: &str = "version: 2\n\nmodels:\n  - name: a\n    description: x\n\n    columns:\n      - name: id\n        description: key\n\n  - name: b\n    description: y\n";

    #[test]
    fn appends_to_an_existing_columns_list() {
        let edit = columns_edit(YAML, 3, &["total"]).unwrap();
        assert_eq!(
            edit.range.start,
            Position {
                line: 9,
                character: 0
            }
        );
        assert_eq!(
            edit.new_text,
            "      - name: total\n        description: \"\"\n"
        );
    }

    #[test]
    fn starts_a_columns_list_where_the_entry_has_none() {
        let edit = columns_edit(YAML, 10, &["id", "n"]).unwrap();
        assert_eq!(
            edit.range.start,
            Position {
                line: 12,
                character: 0
            }
        );
        assert_eq!(
            edit.new_text,
            "    columns:\n      - name: id\n        description: \"\"\n      - name: n\n        description: \"\"\n"
        );
    }

    #[test]
    fn an_inline_columns_value_is_left_alone() {
        let yaml = "models:\n  - name: a\n    columns: []\n";
        assert!(columns_edit(yaml, 1, &["id"]).is_none());
    }
}
