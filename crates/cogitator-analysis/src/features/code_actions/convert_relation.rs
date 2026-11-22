//! "Convert literal table → ref()/source()" (03 §10): a dotted
//! identifier in FROM/JOIN position that matches a known model's or
//! source table's warehouse coordinates becomes the dbt call.

use std::collections::HashMap;

use cogitator_lsp::{
    CodeAction, DocumentChange, Position, Range, TextDocumentEdit, TextEdit, WorkspaceEdit,
};
use cogitator_parser::{Keyword, TokenKind};

use super::super::relation::TargetRelation;
use super::super::{DbtContext, Document};

#[derive(Debug, Clone)]
pub enum ConvertTarget {
    Model(String),
    Source(String, String),
}

/// Known relations, keyed by their lowercased dotted coordinates —
/// both `db.schema.identifier` and `schema.identifier` spellings.
pub struct RelationCatalog {
    map: HashMap<String, ConvertTarget>,
}

impl RelationCatalog {
    pub fn build(ctx: &DbtContext, target: Option<&TargetRelation>) -> Self {
        let mut map = HashMap::new();
        let (database, schema) = target
            .map(|t| (t.database.clone(), t.schema.clone()))
            .unwrap_or((None, None));
        let mut insert = |db: Option<&str>, schema: Option<&str>, ident: &str, t: ConvertTarget| {
            if let Some(schema) = schema {
                map.insert(format!("{schema}.{ident}").to_lowercase(), t.clone());
                if let Some(db) = db {
                    map.insert(format!("{db}.{schema}.{ident}").to_lowercase(), t);
                }
            }
        };
        for name in ctx.model_detail_map.keys() {
            insert(
                database.as_deref(),
                schema.as_deref(),
                name,
                ConvertTarget::Model(name.clone()),
            );
        }
        for source in ctx.source_detail_map.values() {
            for table in source.tables.values() {
                // dbt defaults: source schema falls back to the source
                // name, the identifier to the declared table name
                let src_schema = source.schema.as_deref().unwrap_or(&source.name);
                let identifier = table.identifier.as_deref().unwrap_or(&table.name);
                insert(
                    source.database.as_deref().or(database.as_deref()),
                    Some(src_schema),
                    identifier,
                    ConvertTarget::Source(source.name.clone(), table.name.clone()),
                );
            }
        }
        RelationCatalog { map }
    }
}

/// One action per literal relation in FROM/JOIN position whose dotted
/// spelling the catalog knows.
pub fn convert_literal_relations(
    doc: &Document,
    uri: &str,
    catalog: &RelationCatalog,
) -> Vec<CodeAction> {
    let tokens = &doc.parsed.tokens;
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if !matches!(
            tokens[i].kind,
            TokenKind::Word(Keyword::FROM) | TokenKind::Word(Keyword::JOIN)
        ) {
            i += 1;
            continue;
        }
        // collect `a.b` / `a.b.c` starting right after FROM/JOIN
        let mut parts = Vec::new();
        let mut j = i + 1;
        while let Some(token) = tokens.get(j) {
            match token.kind {
                // any word can be a part (MAIN, RAW … are sqlparser
                // keywords); the ≥2-dotted-parts rule gates stray hits
                TokenKind::Word(_) | TokenKind::Ident => {
                    parts.push(*token);
                    match tokens.get(j + 1).map(|t| t.kind) {
                        Some(TokenKind::Symbol('.')) => j += 2,
                        _ => break,
                    }
                }
                _ => break,
            }
        }
        i = j + 1;
        if parts.len() < 2 {
            continue;
        }
        let spelled: Vec<&str> = parts.iter().map(|t| doc.parsed.literal(t)).collect();
        let key = spelled.join(".").to_lowercase();
        let Some(target) = catalog.map.get(&key) else {
            continue;
        };
        let (title, call) = match target {
            ConvertTarget::Model(model) => (
                format!("Convert to ref('{model}')"),
                format!("{{{{ ref('{model}') }}}}"),
            ),
            ConvertTarget::Source(source, table) => (
                format!("Convert to source('{source}', '{table}')"),
                format!("{{{{ source('{source}', '{table}') }}}}"),
            ),
        };
        let first = parts.first().expect("non-empty");
        let last = parts.last().expect("non-empty");
        let range = Range {
            start: Position {
                line: first.line,
                character: first.column,
            },
            end: Position {
                line: last.line,
                character: last.column + last.len(),
            },
        };
        let edit = WorkspaceEdit {
            document_changes: vec![DocumentChange::Edit(TextDocumentEdit::new(
                uri.to_string(),
                vec![TextEdit {
                    range,
                    new_text: call,
                }],
            ))],
        };
        out.push(CodeAction {
            title,
            kind: Some("refactor.rewrite"),
            diagnostics: None,
            edit: Some(edit),
            command: None,
            is_preferred: None,
        });
    }
    out
}
