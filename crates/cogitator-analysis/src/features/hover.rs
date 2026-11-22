//! textDocument/hover (03 §9.1).

use cogitator_lsp::Position;
use cogitator_parser::TokenKind;

use super::{display_value, DbtContext, Document};
use crate::lineage::{self, ModelLineage};

/// Hover (03 §9.1). Default: empty contents. Computes lineage; a server
/// handler on the dispatcher path uses [`hover_with_lineage`].
pub fn hover(ctx: &DbtContext, doc: &Document, pos: Position) -> String {
    hover_inner(ctx, doc, pos, Lineage::Compute)
}

/// Hover with the lineage the caller already holds. `None` omits the
/// lineage line; this never computes one.
pub fn hover_with_lineage(
    ctx: &DbtContext,
    doc: &Document,
    pos: Position,
    lineage: Option<&ModelLineage>,
) -> String {
    hover_inner(ctx, doc, pos, Lineage::Given(lineage))
}

enum Lineage<'a> {
    Compute,
    Given(Option<&'a ModelLineage>),
}

fn hover_inner(ctx: &DbtContext, doc: &Document, pos: Position, source: Lineage<'_>) -> String {
    let Some(idx) = doc.parsed.find_token_at_cursor(pos.line, pos.character) else {
        return String::new();
    };
    let parsed = &doc.parsed;
    let token = &parsed.tokens[idx];
    let literal = parsed.literal(token);

    match token.kind {
        TokenKind::Ref => ctx
            .model_detail_map
            .get(literal)
            .map(|m| m.description.clone())
            .unwrap_or_default(),
        TokenKind::Doc => ctx
            .doc_detail_map
            .get(literal)
            .map(|d| d.content.clone())
            .unwrap_or_default(),
        TokenKind::SourceName => ctx
            .source_detail_map
            .get(literal)
            .map(|s| s.description.clone())
            .unwrap_or_default(),
        TokenKind::SourceTable => {
            let Some(source_token) = parsed.lookback_find(idx, TokenKind::SourceName, 4) else {
                return String::new();
            };
            let source_name = parsed.literal(source_token);
            let Some(source) = ctx.source_detail_map.get(source_name) else {
                return String::new();
            };
            let table = source.tables.get(literal);
            format!(
                "Source: {}\n{}\n\nTable: {}\n{}",
                source.name,
                source.description,
                table.map(|t| t.name.as_str()).unwrap_or_default(),
                table.map(|t| t.description.as_str()).unwrap_or_default(),
            )
        }
        TokenKind::Var => {
            let value = ctx
                .variable_detail_map
                .get(literal)
                .map(|v| display_value(&v.value))
                .unwrap_or_default();
            format!("{literal}: {value}")
        }
        TokenKind::Macro => {
            let package = parsed
                .lookback_find(idx, TokenKind::Package, 2)
                .map(|t| parsed.literal(t).to_string())
                .unwrap_or_else(|| ctx.project.name.clone());
            ctx.macro_detail_map
                .get(&package)
                .and_then(|m| m.get(literal))
                .map(|m| m.description.clone())
                .unwrap_or_default()
        }
        _ => {
            // documented column of a relation in this document? (editor-
            // experience feature: column hover, with lineage when derivable)
            if let Some(hit) = super::completion::scope::resolve_column(ctx, doc, literal) {
                let mut text = format!("Column of {}", hit.relation);
                if !hit.description.is_empty() {
                    text.push_str("\n\n");
                    text.push_str(&hit.description);
                }
                let computed;
                let lineage = match source {
                    Lineage::Compute => {
                        computed = lineage::column_lineage(&doc.parsed, &ctx.dialect);
                        computed.as_ref()
                    }
                    Lineage::Given(given) => given,
                };
                if let Some(lineage) = lineage {
                    let sources: Vec<String> = lineage
                        .columns
                        .iter()
                        .filter(|c| c.name == literal || c.name == "*")
                        .flat_map(|c| c.sources.iter())
                        .map(|s| match s {
                            crate::lineage::SourceRef::Model { name, column } => {
                                format!("{name}.{column}")
                            }
                            crate::lineage::SourceRef::Source {
                                source,
                                table,
                                column,
                            } => {
                                format!("{source}.{table}.{column}")
                            }
                            crate::lineage::SourceRef::Unknown { name, column } => {
                                format!("{name}.{column}")
                            }
                        })
                        .filter(|s| s.ends_with(&format!(".{literal}")) || !s.ends_with(".*"))
                        .collect();
                    if !sources.is_empty() {
                        text.push_str("\n\nLineage: ");
                        text.push_str(&sources.join(", "));
                        text.push_str(lineage.origin_note());
                    }
                }
                return text;
            }
            ctx.dialect
                .function_doc(literal)
                .map(str::to_string)
                .unwrap_or_default()
        }
    }
}
