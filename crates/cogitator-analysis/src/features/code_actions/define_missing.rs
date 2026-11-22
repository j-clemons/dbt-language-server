//! Define what a reference names but the project lacks, at the cursor:
//! a `ref('x')` to no model offers "Create model 'x'" (a new `x.sql`
//! beside the referencing model), a `var('x')` the project does not
//! define offers "Define var 'x' in dbt_project.yml". These complement
//! the did-you-mean quickfixes, for when the name is new rather than
//! misspelt. Pure: the shell hands in dbt_project.yml's text.

use std::path::Path;

use cogitator_lsp::{
    CodeAction, CreateFile, DocumentChange, Position, Range, TextDocumentEdit, TextEdit,
    WorkspaceEdit,
};
use cogitator_parser::TokenKind;

use super::super::{file_uri, DbtContext, Document};
use super::yaml_text::{block_end, indent, insert_after_line, is_content, plain_name};

/// The two actions, for whichever kind of reference is under the cursor.
/// `read` reads a project file (dbt_project.yml), as the shell sees it.
pub fn define_missing_actions(
    ctx: &DbtContext,
    doc: &Document,
    uri: &str,
    range: Range,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Vec<CodeAction> {
    let parsed = &doc.parsed;
    let Some(idx) = parsed.find_token_at_cursor(range.start.line, range.start.character) else {
        return Vec::new();
    };
    let token = &parsed.tokens[idx];
    let name = parsed.literal(token);
    let action = match token.kind {
        TokenKind::Ref if !ctx.model_detail_map.contains_key(name) => {
            create_model_action(uri, name)
        }
        TokenKind::Var if !ctx.variable_detail_map.contains_key(name) => {
            ctx.project_root.as_deref().and_then(|root| {
                let project = root.join("dbt_project.yml");
                let text = read(&project)?;
                define_var_action(&project, &text, name)
            })
        }
        _ => None,
    };
    action.into_iter().collect()
}

/// A new `<name>.sql` in the referencing model's directory, holding a
/// placeholder select so the project parses until it is written.
fn create_model_action(uri: &str, name: &str) -> Option<CodeAction> {
    if !uri.ends_with(".sql") {
        return None;
    }
    let name = plain_name(name)?;
    let path = cogitator_lsp::uri_to_fs_path(uri)?;
    let new = Path::new(&path).parent()?.join(format!("{name}.sql"));
    let new_uri = file_uri(&new);
    let stub = format!(
        "-- {name}: a new model, referenced before it was written\nselect 1 as placeholder\n"
    );
    Some(edit_action(
        format!("Create model '{name}'"),
        vec![
            DocumentChange::Create(CreateFile::new(new_uri.clone())),
            DocumentChange::Edit(TextDocumentEdit::new(
                new_uri,
                vec![TextEdit {
                    range: Range::zero_width(Position {
                        line: 0,
                        character: 0,
                    }),
                    new_text: stub,
                }],
            )),
        ],
    ))
}

/// `<name>: null` as the last top-level entry of `vars:`, or a new
/// `vars:` section at the end of the file when there is none.
fn define_var_action(project: &Path, text: &str, name: &str) -> Option<CodeAction> {
    let name = plain_name(name)?;
    let edit = var_edit(text, name)?;
    Some(edit_action(
        format!("Define var '{name}' in dbt_project.yml"),
        vec![DocumentChange::Edit(TextDocumentEdit::new(
            file_uri(project),
            vec![edit],
        ))],
    ))
}

fn var_edit(text: &str, name: &str) -> Option<TextEdit> {
    let lines: Vec<&str> = text.lines().collect();
    let vars = lines
        .iter()
        .position(|l| indent(l) == 0 && l.starts_with("vars:"));
    let Some(vars) = vars else {
        let last = lines.iter().rposition(|l| is_content(l)).unwrap_or(0);
        let body = format!("\nvars:\n  {name}: null\n");
        return Some(insert_after_line(text, last, body));
    };
    // `vars: {}` or any inline value: leave it to the reader
    let value = lines[vars]["vars:".len()..].trim();
    if !value.is_empty() && !value.starts_with('#') {
        return None;
    }
    let end = block_end(&lines, vars, 0);
    let child = (vars + 1..=end)
        .find(|&i| is_content(lines[i]))
        .map_or(2, |i| indent(lines[i]));
    let body = format!("{}{name}: null\n", " ".repeat(child));
    Some(insert_after_line(text, end, body))
}

fn edit_action(title: String, document_changes: Vec<DocumentChange>) -> CodeAction {
    CodeAction {
        title,
        kind: Some("quickfix"),
        diagnostics: None,
        edit: Some(WorkspaceEdit { document_changes }),
        command: None,
        is_preferred: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_var_joins_the_top_level_of_an_existing_vars_section() {
        let text = "name: p\nvars:\n  a: 1\n  pkg:\n    b: 2\n\nmodels:\n  x: 1\n";
        let edit = var_edit(text, "c").unwrap();
        assert_eq!(
            edit.range.start,
            Position {
                line: 5,
                character: 0
            }
        );
        assert_eq!(edit.new_text, "  c: null\n");
    }

    #[test]
    fn a_project_without_vars_gets_a_section() {
        let edit = var_edit("name: p\n", "c").unwrap();
        assert_eq!(
            edit.range.start,
            Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(edit.new_text, "\nvars:\n  c: null\n");
    }

    #[test]
    fn an_inline_vars_value_is_left_alone() {
        assert!(var_edit("vars: {}\n", "c").is_none());
    }

    #[test]
    fn a_new_model_is_created_beside_the_referencing_one() {
        let uri = cogitator_lsp::fs_path_to_uri(Path::new("/p/models/marts/orders.sql"));
        let action = create_model_action(&uri, "order_items").unwrap();
        assert_eq!(action.title, "Create model 'order_items'");
        let wire = serde_json::to_value(action.edit.unwrap()).unwrap();
        let created = wire["documentChanges"][0]["uri"].as_str().unwrap();
        assert!(
            created.ends_with("/models/marts/order_items.sql"),
            "{created}"
        );
        assert_eq!(wire["documentChanges"][0]["kind"], "create");
        assert_eq!(wire["documentChanges"][1]["textDocument"]["uri"], created);
        assert!(create_model_action(&uri, "not a name").is_none());
    }
}
