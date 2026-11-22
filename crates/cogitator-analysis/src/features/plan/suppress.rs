//! Suppression comments (specs/ir-pipeline/08): `{# cogitator: ignore id
//! -- reason #}` / `-- cogitator: ignore id -- reason`, covering the same
//! line or the line below, and `ignore-file`.
//!
//! The comments come from the existing machinery: Jinja comment spans
//! (`jinja_spans`) and sqlparser's own comment tokens. Only the directive
//! text inside a comment is read here.

use std::collections::BTreeSet;

use cogitator_dialects::Dialect;
use cogitator_error::guidance::{closest_matches, Guidance};
use cogitator_ir::Span;
use cogitator_parser::backend::sql_dialect;
use cogitator_parser::jinja_spans::{blanked, jinja_spans, JinjaSpanKind};
use cogitator_parser::LineTable;
use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};

use super::diagnostics_config::advice_rule_ids;
use super::report::QueryPlan;

/// One `ignore` directive: `ids` on lines `first..=last`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LineSuppression {
    first: u32,
    last: u32,
    ids: Vec<String>,
}

/// A rule id no rule answers to, reported through the diagnostics path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownRule {
    /// The offending id's bytes in the document text.
    pub span: Span,
    pub message: String,
}

/// What one document's comments suppress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Suppressions {
    lines: Vec<LineSuppression>,
    file_ids: BTreeSet<String>,
    file_all: bool,
    pub unknown: Vec<UnknownRule>,
}

impl Suppressions {
    /// Whether a finding of rule `id` starting at byte `offset` is
    /// suppressed.
    pub fn suppresses(&self, table: &LineTable, id: &str, offset: u32) -> bool {
        if self.file_all || self.file_ids.contains(id) {
            return true;
        }
        let (line, _) = table.line_col(offset);
        self.lines
            .iter()
            .any(|s| s.first <= line && line <= s.last && s.ids.iter().any(|known| known == id))
    }
}

/// Every suppression directive in `text`.
pub fn scan(text: &str, dialect: &Dialect) -> Suppressions {
    let table = LineTable::new(text);
    let mut out = Suppressions::default();
    for span in jinja_spans(text) {
        if span.kind != JinjaSpanKind::Comment {
            continue;
        }
        let Some(comment) = text.get(span.range.clone()) else {
            continue;
        };
        let (inner, skipped) = jinja_comment_body(comment);
        let offset = span.range.start as u32 + skipped;
        add(&mut out, &table, inner, offset);
    }
    for (body, offset) in sql_comments(text, &table, dialect) {
        add(&mut out, &table, body, offset);
    }
    out
}

/// The text inside `{#[-+] … [-+]#}` and the bytes before it.
fn jinja_comment_body(comment: &str) -> (&str, u32) {
    let opened = comment.strip_prefix("{#").unwrap_or(comment);
    let body = opened.trim_start_matches(['-', '+']);
    let skipped = comment.len() - body.len();
    let body = body.strip_suffix("#}").unwrap_or(body);
    (body.trim_end_matches(['-', '+']), skipped as u32)
}

/// The `--` comments of `text`, with each body's byte offset. Jinja is
/// blanked first (lengths kept, so offsets hold). A text sqlparser cannot
/// tokenize mid-edit yields none; the Jinja-comment directives still apply.
fn sql_comments<'a>(text: &'a str, table: &LineTable, dialect: &Dialect) -> Vec<(&'a str, u32)> {
    let masked = blanked(
        text,
        &[
            JinjaSpanKind::Comment,
            JinjaSpanKind::RawTag,
            JinjaSpanKind::Code,
        ],
    );
    let sql_dialect = sql_dialect(dialect);
    let Ok(tokens) = Tokenizer::new(sql_dialect.as_ref(), &masked).tokenize_with_location() else {
        tracing::debug!("suppressions: tokenizer declined the text; `--` comments ignored");
        return Vec::new();
    };
    tokens
        .into_iter()
        .filter_map(|token| {
            let Token::Whitespace(Whitespace::SingleLineComment { comment, prefix }) = token.token
            else {
                return None;
            };
            if prefix != "--" {
                return None;
            }
            let start = byte_offset(text, table, &token.span.start)? + prefix.len() as u32;
            let end = start as usize + comment.trim_end_matches(['\r', '\n']).len();
            Some((text.get(start as usize..end)?, start))
        })
        .collect()
}

/// sqlparser locations are 1-based (line, char column); bytes here.
fn byte_offset(text: &str, table: &LineTable, at: &sqlparser::tokenizer::Location) -> Option<u32> {
    let line_start = table.line_start(at.line.checked_sub(1)? as usize)?;
    let line = text.get(line_start as usize..)?;
    let chars = at.column.checked_sub(1)? as usize;
    let within = line
        .char_indices()
        .nth(chars)
        .map_or(line.len(), |(byte, _)| byte);
    Some(line_start + within as u32)
}

/// Records one comment body found at byte `offset`.
fn add(out: &mut Suppressions, table: &LineTable, body: &str, offset: u32) {
    let Some(directive) = parse(body) else { return };
    let end = offset + body.len().max(1) as u32 - 1;
    let (first, _) = table.line_col(offset);
    let (last, _) = table.line_col(end);
    let known: Vec<&str> = advice_rule_ids().collect();
    let mut ids = Vec::new();
    for (id, at) in directive.ids {
        if known.contains(&id) {
            ids.push(id.to_string());
            continue;
        }
        let span = Span::new(offset + at, offset + at + id.len() as u32);
        let message = Guidance::new(format!("'{id}' is not a cogitator rule."))
            .suggest(closest_matches(id, known.iter().copied()))
            .to_diagnostic();
        out.unknown.push(UnknownRule { span, message });
    }
    match directive.kind {
        Kind::File if directive.bare => out.file_all = true,
        Kind::File => out.file_ids.extend(ids),
        Kind::Line if ids.is_empty() => {}
        // the comment's own lines and the one below it
        Kind::Line => out.lines.push(LineSuppression {
            first,
            last: last + 1,
            ids,
        }),
    }
}

enum Kind {
    Line,
    File,
}

struct Directive<'a> {
    kind: Kind,
    /// Ids with their byte offset inside the comment body.
    ids: Vec<(&'a str, u32)>,
    bare: bool,
}

/// `cogitator: ignore[-file] [ids] [-- reason]`, ids separated by commas
/// or spaces; a bare `ignore-file` covers every rule. `None` when the
/// comment is not a directive.
fn parse(body: &str) -> Option<Directive<'_>> {
    let rest = body.trim_start().strip_prefix("cogitator:")?.trim_start();
    let (kind, tail) = if let Some(tail) = rest.strip_prefix("ignore-file") {
        (Kind::File, tail)
    } else {
        (Kind::Line, rest.strip_prefix("ignore")?)
    };
    if !tail.is_empty() && !tail.starts_with(char::is_whitespace) && !tail.starts_with(',') {
        return None;
    }
    let ids_text = tail.split("--").next().unwrap_or("");
    let ids: Vec<(&str, u32)> = ids_text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(|id| {
            // both slices borrow `body`, so their distance is the offset
            let at = id.as_ptr() as usize - body.as_ptr() as usize;
            (id, at as u32)
        })
        .collect();
    Some(Directive {
        kind,
        bare: ids.is_empty(),
        ids,
    })
}

/// Marks each suppressed finding of `plan`. They stay in the plan; only
/// the diagnostics path drops them.
pub fn mark(plan: &mut QueryPlan, suppressions: &Suppressions, text: &str) {
    let table = LineTable::new(text);
    for finding in &mut plan.findings {
        finding.suppressed = suppressions.suppresses(&table, finding.id, finding.span.start);
    }
}

#[cfg(test)]
#[path = "suppress_tests.rs"]
mod tests;
