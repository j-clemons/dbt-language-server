//! Bare column identifiers in a multi-relation FROM/JOIN: ambiguous the
//! day both relations grow a column of that name. The fix qualifies the
//! column with a relation's alias: when exactly one relation in scope is
//! known to have the column, that qualification changes no result and is
//! offered alone, as the preferred fix; otherwise every relation is
//! offered and none is preferred, as the server cannot know which is meant.

use cogitator_error::guidance::Guidance;
use cogitator_lsp::{
    diagnostic_severity, Diagnostic, DiagnosticData, FixKind, MultiEditFix, Range, TextEdit,
};
use cogitator_parser::{Keyword, TokenKind};

use super::super::completion::scope::{cte_columns, RelationTarget, ScopeIndex};
use super::super::{token_range, DbtContext, Document};

/// At most this many "Qualify with …" fixes per column: a menu, not a wall.
const MAX_QUALIFIERS: usize = 5;

pub fn check(ctx: &DbtContext, doc: &Document) -> Vec<Diagnostic> {
    let scope = ScopeIndex::build(doc);
    if scope.relations().len() < 2 {
        return Vec::new();
    }
    let qualifiers = qualifiers(ctx, doc, &scope);
    let tokens = &doc.parsed.tokens;
    let text = &doc.parsed.text;
    let mut out = Vec::new();
    let mut in_select = false;
    let mut in_jinja = false;
    for (i, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Word(Keyword::SELECT) => in_select = true,
            TokenKind::Word(Keyword::FROM) => in_select = false,
            TokenKind::JinjaOpen(_) => in_jinja = true,
            TokenKind::JinjaClose(_) => in_jinja = false,
            TokenKind::Word(Keyword::NoKeyword) if in_select && !in_jinja => {
                // qualified (a.b), function calls f(…) and aliases `as x`
                // are fine; a bare identifier is the finding
                let before_dot = text[..token.start as usize].trim_end().ends_with('.');
                let after = text[token.end as usize..].trim_start();
                let qualifies = after.starts_with('.') || after.starts_with('(');
                let aliased = i > 0 && tokens[i - 1].kind == TokenKind::Word(Keyword::AS);
                if before_dot || qualifies || aliased {
                    continue;
                }
                let name = doc.parsed.literal(token);
                let range = token_range(token);
                let mut diagnostic = Diagnostic::plain(
                    range,
                    Guidance::new(format!("bare column '{name}' in a multi-relation query"))
                        .fix("qualify it with its relation's alias.")
                        .to_diagnostic(),
                    diagnostic_severity::WARNING,
                    "lint-unaliased-column",
                    "cogitator",
                );
                let fixes = qualify_fixes(&qualifiers, name, range);
                if !fixes.is_empty() {
                    diagnostic.data = Some(DiagnosticData {
                        fix: FixKind::MultiEdit { fixes },
                        token_range: range,
                        suggestions: Vec::new(),
                    });
                }
                out.push(diagnostic);
            }
            _ => {}
        }
    }
    out
}

/// One relation in scope: the name it is qualified by (its alias when it
/// has one) and the columns the project knows it to have.
struct Qualifier {
    name: String,
    columns: Vec<String>,
}

/// Each distinct relation once, by its alias where it has one: the scope
/// lists an aliased relation under its alias first, then under its name.
fn qualifiers(ctx: &DbtContext, doc: &Document, scope: &ScopeIndex) -> Vec<Qualifier> {
    let mut seen: Vec<&RelationTarget> = Vec::new();
    let mut out = Vec::new();
    for (key, target) in scope.relations() {
        if seen.contains(&target) {
            continue;
        }
        seen.push(target);
        out.push(Qualifier {
            name: key.clone(),
            columns: known_columns(ctx, doc, target),
        });
    }
    out
}

fn known_columns(ctx: &DbtContext, doc: &Document, target: &RelationTarget) -> Vec<String> {
    match target {
        RelationTarget::Model(name) => ctx
            .model_detail_map
            .get(name)
            .map(|m| m.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default(),
        RelationTarget::SourceTable { source, table } => ctx
            .source_detail_map
            .get(source)
            .and_then(|s| s.tables.get(table))
            .map(|t| t.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default(),
        RelationTarget::Cte(name) => cte_columns(doc, name),
        RelationTarget::Unknown(_) => Vec::new(),
    }
}

/// The relations known to have the column decide: exactly one makes a
/// single preserving fix; none or several offer every relation, unranked.
fn qualify_fixes(qualifiers: &[Qualifier], column: &str, range: Range) -> Vec<MultiEditFix> {
    let fix = |q: &Qualifier, preserving| MultiEditFix {
        title: format!("Qualify with '{}'", q.name),
        edits: vec![TextEdit {
            range: Range::zero_width(range.start),
            new_text: format!("{}.", q.name),
        }],
        preserving,
    };
    let having: Vec<&Qualifier> = qualifiers
        .iter()
        .filter(|q| q.columns.iter().any(|c| c.eq_ignore_ascii_case(column)))
        .collect();
    if let [only] = having.as_slice() {
        return vec![fix(only, true)];
    }
    qualifiers
        .iter()
        .take(MAX_QUALIFIERS)
        .map(|q| fix(q, false))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cogitator_lsp::Position;

    fn q(name: &str, columns: &[&str]) -> Qualifier {
        Qualifier {
            name: name.into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn at() -> Range {
        Range {
            start: Position {
                line: 2,
                character: 4,
            },
            end: Position {
                line: 2,
                character: 12,
            },
        }
    }

    #[test]
    fn one_relation_with_the_column_is_the_one_preserving_fix() {
        let qs = [q("o", &["order_id"]), q("c", &["customer_id"])];
        let fixes = qualify_fixes(&qs, "ORDER_ID", at());
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].title, "Qualify with 'o'");
        assert!(fixes[0].preserving);
        assert_eq!(fixes[0].edits[0].new_text, "o.");
        assert_eq!(fixes[0].edits[0].range, Range::zero_width(at().start));
    }

    #[test]
    fn no_or_several_relations_with_the_column_offer_every_relation() {
        let qs = [q("o", &["id"]), q("c", &["id"])];
        let fixes = qualify_fixes(&qs, "id", at());
        assert_eq!(fixes.len(), 2);
        assert!(fixes.iter().all(|f| !f.preserving));
        let unknown = [q("o", &[]), q("c", &[])];
        let fixes = qualify_fixes(&unknown, "id", at());
        assert_eq!(fixes.len(), 2);
        assert!(fixes.iter().all(|f| !f.preserving));
    }
}
