//! Statement parsing with recovery. sqlparser has none: parse the whole
//! statement first, and on failure split at paren depth 0 into CTEs and
//! the final query, parsing each on its own. A failing part is a hole.

use cogitator_dialects::Dialect;
use cogitator_ir::common::HoleKind;
use cogitator_ir::{Degradation, DegradationKind, Span};
use cogitator_parser::backend::sql_dialect;
use sqlparser::ast::{Ident, Query};
use sqlparser::dialect::Dialect as SqlDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, TokenWithSpan};

use crate::caps::{depth_upper_bound, MAX_EXPR_DEPTH};
use crate::segment::{is_significant, Positions};

mod split;

use split::split_parts;

#[derive(Debug)]
pub(crate) enum Part {
    Parsed(Box<Query>),
    Hole(HoleKind, Span),
}

#[derive(Debug)]
pub(crate) struct CtePart {
    pub name: Ident,
    pub columns: Vec<Ident>,
    pub body: Part,
}

#[derive(Debug)]
pub(crate) struct Parts {
    pub recursive: bool,
    pub ctes: Vec<CtePart>,
    pub body: Part,
}

#[derive(Debug)]
pub(crate) enum Outcome {
    Whole(Box<Query>),
    Parts(Parts),
    Hole(HoleKind, Span),
}

#[derive(Debug)]
pub(crate) struct Parsed {
    pub outcome: Outcome,
    pub degraded: Vec<Degradation>,
}

pub(crate) fn parse(
    tokens: Vec<TokenWithSpan>,
    pos: &Positions,
    dialect: &Dialect,
    has_jinja: bool,
) -> Parsed {
    let dialect = sql_dialect(dialect);
    let mut degraded = Vec::new();
    let tokens = first_statement(tokens, &mut degraded);
    if !tokens.iter().any(|t| is_significant(&t.token)) {
        let kind = if has_jinja {
            HoleKind::Jinja
        } else {
            HoleKind::ParseError
        };
        degraded.push(Degradation::new(
            DegradationKind::ParseHole,
            "the model has no SQL to parse",
        ));
        return Parsed {
            outcome: Outcome::Hole(kind, Span::default()),
            degraded,
        };
    }
    let depth = depth_upper_bound(&tokens);
    let (mut tokens, reason) = if depth > MAX_EXPR_DEPTH {
        (
            tokens,
            format!("expression depth {depth} exceeds {MAX_EXPR_DEPTH}"),
        )
    } else {
        match parse_query(dialect.as_ref(), tokens) {
            Ok(query) => {
                return Parsed {
                    outcome: Outcome::Whole(query),
                    degraded,
                }
            }
            Err((tokens, err)) => (tokens, err),
        }
    };
    tokens.retain(|t| is_significant(&t.token));
    let outcome = match split_parts(tokens, pos, dialect.as_ref(), &mut degraded) {
        Some(parts) => Outcome::Parts(parts),
        None => {
            degraded.push(Degradation::new(DegradationKind::ParseHole, reason));
            Outcome::Hole(HoleKind::ParseError, Span::default())
        }
    };
    Parsed { outcome, degraded }
}

/// The first statement; a dbt model has exactly one.
fn first_statement(
    mut tokens: Vec<TokenWithSpan>,
    degraded: &mut Vec<Degradation>,
) -> Vec<TokenWithSpan> {
    let mut depth = 0i32;
    let mut cut = None;
    for (i, tw) in tokens.iter().enumerate() {
        match tw.token {
            Token::LParen => depth += 1,
            Token::RParen => depth -= 1,
            Token::SemiColon if depth <= 0 => {
                cut = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(cut) = cut else { return tokens };
    if tokens[cut + 1..].iter().any(|t| is_significant(&t.token)) {
        degraded.push(Degradation::new(
            DegradationKind::UnsupportedSyntax,
            "more than one statement; only the first is analysed",
        ));
    }
    tokens.truncate(cut);
    tokens
}

/// Runs `parse_query` and insists on consuming everything. Hands the
/// tokens back on failure: the split path needs them and they are not
/// cloneable cheaply.
fn parse_query(
    dialect: &dyn SqlDialect,
    tokens: Vec<TokenWithSpan>,
) -> Result<Box<Query>, (Vec<TokenWithSpan>, String)> {
    let mut parser = Parser::new(dialect).with_tokens_with_locations(tokens);
    let result = parser.parse_query();
    match result {
        Ok(query) if parser.peek_token_ref().token == Token::EOF => Ok(query),
        Ok(_) => Err((
            parser.into_tokens(),
            "trailing tokens after the query".to_string(),
        )),
        Err(err) => Err((parser.into_tokens(), err.to_string())),
    }
}

#[cfg(test)]
mod tests;
