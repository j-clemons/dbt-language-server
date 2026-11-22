//! Compiled text made ready for lowering and alignment: dbt-injected
//! ephemeral CTEs are blanked (the template never wrote them), and each
//! relation dbt rendered becomes a `ref`-like region over a placeholder.
//! All positions come from sqlparser's tokens.

use std::collections::BTreeSet;

use cogitator_dialects::Dialect;
use cogitator_ir::Span;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, TokenWithSpan};

use super::ephemeral::{injected_ctes, EPHEMERAL_PREFIX};
use super::relations::{word_parts, RelationTable, Target};
use crate::caps::MAX_TEXT_BYTES;
use crate::jinja::{Class, RelInfo};
use crate::mask::{Region, RegionKind};
use crate::segment;

/// Compiled SQL with its relations and injected CTEs located.
#[derive(Debug, Clone)]
pub struct Prepared {
    mask: String,
    regions: Vec<Region>,
    /// Injected CTEs, sorted: compiled bytes the template never wrote.
    blanks: Vec<Span>,
}

impl Prepared {
    pub(super) fn mask(&self) -> &str {
        &self.mask
    }

    pub(super) fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub(super) fn blanks(&self) -> &[Span] {
        &self.blanks
    }
}

/// `Err` carries why the compiled text is unusable. `model` is the model
/// being planned: a reference to its own relation is `{{ this }}`.
pub fn prepare(
    compiled: &str,
    dialect: &Dialect,
    relations: &RelationTable,
    model: Option<&str>,
) -> Result<Prepared, String> {
    if compiled.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "the compiled SQL is {} bytes; the limit is {MAX_TEXT_BYTES}",
            compiled.len()
        ));
    }
    let lexed = segment::lex(compiled, dialect)?;
    let tokens: Vec<(&TokenWithSpan, Span)> = lexed
        .tokens
        .iter()
        .filter(|t| segment::is_significant(&t.token))
        .filter_map(|t| Some((t, lexed.pos.span(t.span)?)))
        .collect();
    let blanks = injected_ctes(&tokens);
    let cte_names = cte_names(&tokens);
    let mut regions = Vec::new();
    let mut i = 0;
    while let Some(&(token, span)) = tokens.get(i) {
        i += 1;
        if !is_relation_keyword(&token.token) || in_blank(&blanks, span) {
            continue;
        }
        while let Some(path) = path_at(&tokens, i) {
            i = path.next;
            if let Some(region) = region_for(&path, compiled, relations, model, &cte_names) {
                regions.push(region);
            }
            let after_alias = skip_alias(&tokens, i);
            match tokens.get(after_alias) {
                Some((token, _)) if matches!(token.token, Token::Comma) => i = after_alias + 1,
                _ => break,
            }
        }
    }
    let mask = render_mask(compiled, &blanks, &regions)?;
    Ok(Prepared {
        mask,
        regions,
        blanks,
    })
}

fn is_relation_keyword(token: &Token) -> bool {
    matches!(token, Token::Word(w) if matches!(w.keyword, Keyword::FROM | Keyword::JOIN))
}

/// Past an optional `[AS] alias`, so a comma join's next item is found.
fn skip_alias(tokens: &[(&TokenWithSpan, Span)], at: usize) -> usize {
    match tokens.get(at).map(|(t, _)| &t.token) {
        Some(Token::Word(w)) if w.keyword == Keyword::AS => at + 2,
        Some(Token::Word(w)) if w.keyword == Keyword::NoKeyword => at + 1,
        _ => at,
    }
}

fn in_blank(blanks: &[Span], span: Span) -> bool {
    blanks.iter().any(|b| b.contains(span))
}

struct Path {
    parts: Vec<String>,
    span: Span,
    /// The token after the path.
    next: usize,
}

/// A dotted name at `at`, unless a call or another dot follows it.
fn path_at(tokens: &[(&TokenWithSpan, Span)], at: usize) -> Option<Path> {
    let &(first, first_span) = tokens.get(at)?;
    let Token::Word(word) = &first.token else {
        return None;
    };
    let mut parts = word_parts(word);
    let mut end = first_span.end;
    let mut i = at + 1;
    while let (Some((period, _)), Some(&(next, next_span))) = (tokens.get(i), tokens.get(i + 1)) {
        let (Token::Period, Token::Word(word)) = (&period.token, &next.token) else {
            break;
        };
        parts.extend(word_parts(word));
        end = next_span.end;
        i += 2;
    }
    if tokens
        .get(i)
        .is_some_and(|(t, _)| matches!(t.token, Token::LParen | Token::Period))
    {
        return None;
    }
    Some(Path {
        parts,
        span: Span::new(first_span.start, end),
        next: i,
    })
}

fn region_for(
    path: &Path,
    compiled: &str,
    relations: &RelationTable,
    model: Option<&str>,
    cte_names: &BTreeSet<String>,
) -> Option<Region> {
    let target = match path.parts.as_slice() {
        [single] => match single.strip_prefix(EPHEMERAL_PREFIX) {
            Some(name) if !name.is_empty() => Target::Model(name.to_string()),
            // a bare name the query defines is its own CTE, never a model
            _ if cte_names.contains(&single.to_lowercase()) => return None,
            _ => relations.lookup(&path.parts)?.clone(),
        },
        _ => relations.lookup(&path.parts)?.clone(),
    };
    let (class, kind) = match target {
        Target::Model(name) if model == Some(name.as_str()) => {
            (Class::Relation(RelInfo::This), RegionKind::This)
        }
        Target::Model(name) => (
            Class::Relation(RelInfo::Model {
                package: None,
                name,
                version: None,
            }),
            RegionKind::Ref,
        ),
        Target::Source(source, table) => (
            Class::Relation(RelInfo::Source { source, table }),
            RegionKind::Source,
        ),
    };
    Some(Region {
        span: path.span,
        word: Some(first_line(compiled, path.span)),
        class,
        kind,
        clause: false,
    })
}

/// A placeholder cannot span a newline; the path's later lines stay blank.
fn first_line(text: &str, span: Span) -> Span {
    let len = text
        .get(span.start as usize..span.end as usize)
        .and_then(|path| path.find(['\n', '\r']))
        .unwrap_or(span.len() as usize);
    Span::new(span.start, span.start + len.max(1) as u32)
}

/// Lowercased names of everything written `name as (`.
fn cte_names(tokens: &[(&TokenWithSpan, Span)]) -> BTreeSet<String> {
    tokens
        .windows(3)
        .filter_map(|window| match window {
            [(name, _), (keyword, _), (paren, _)]
                if matches!(paren.token, Token::LParen)
                    && matches!(&keyword.token, Token::Word(w) if w.keyword == Keyword::AS) =>
            {
                match &name.token {
                    Token::Word(word) => Some(word.value.to_lowercase()),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

fn render_mask(compiled: &str, blanks: &[Span], regions: &[Region]) -> Result<String, String> {
    let mut out = compiled.as_bytes().to_vec();
    for span in blanks.iter().copied().chain(regions.iter().map(|r| r.span)) {
        if let Some(bytes) = out.get_mut(span.start as usize..span.end as usize) {
            for b in bytes.iter_mut().filter(|b| **b != b'\n') {
                *b = b' ';
            }
        }
    }
    for word in regions.iter().filter_map(|r| r.word) {
        if let Some(bytes) = out.get_mut(word.start as usize..word.end as usize) {
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = if i == 0 { b'_' } else { b'j' };
            }
        }
    }
    String::from_utf8(out).map_err(|e| format!("mask is not valid UTF-8: {e}"))
}
