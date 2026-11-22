//! Recovery: splits a failed statement at paren depth 0 and parses each
//! CTE and the final query on its own.

use cogitator_ir::common::HoleKind;
use cogitator_ir::{Degradation, DegradationKind, Span};
use sqlparser::ast::Ident;
use sqlparser::dialect::Dialect as SqlDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, TokenWithSpan};

use super::{parse_query, CtePart, Part, Parts};
use crate::caps::{depth_upper_bound, MAX_EXPR_DEPTH};
use crate::segment::Positions;

/// `tokens` are the statement's significant tokens.
pub(super) fn split_parts(
    tokens: Vec<TokenWithSpan>,
    pos: &Positions,
    dialect: &dyn SqlDialect,
    degraded: &mut Vec<Degradation>,
) -> Option<Parts> {
    if !is_keyword(tokens.first()?, Keyword::WITH) {
        let body = parse_part(tokens, pos, dialect, degraded, "the query");
        return Some(Parts {
            recursive: false,
            ctes: Vec::new(),
            body,
        });
    }
    let mut at = 1;
    let recursive = tokens
        .get(at)
        .is_some_and(|t| is_keyword(t, Keyword::RECURSIVE));
    if recursive {
        at += 1;
    }
    let mut ctes = Vec::new();
    loop {
        let header = parse_cte_header(&tokens, &mut at)?;
        let close = matching_paren(&tokens, at)?;
        let body_tokens = tokens.get(at + 1..close)?.to_vec();
        let what = format!("CTE `{}`", header.name.value);
        let body = parse_part(body_tokens, pos, dialect, degraded, &what);
        ctes.push(CtePart {
            name: header.name,
            columns: header.columns,
            body,
        });
        at = close + 1;
        if tokens.get(at).is_some_and(|t| t.token == Token::Comma) {
            at += 1;
            continue;
        }
        break;
    }
    let final_tokens = tokens.get(at..)?.to_vec();
    let body = parse_part(final_tokens, pos, dialect, degraded, "the final query");
    Some(Parts {
        recursive,
        ctes,
        body,
    })
}

struct CteHeader {
    name: Ident,
    columns: Vec<Ident>,
}

/// `name [(cols)] AS [NOT] [MATERIALIZED]`, leaving `at` on the `(`.
fn parse_cte_header(tokens: &[TokenWithSpan], at: &mut usize) -> Option<CteHeader> {
    let name = word_ident(tokens.get(*at)?)?;
    *at += 1;
    let mut columns = Vec::new();
    if tokens.get(*at).is_some_and(|t| t.token == Token::LParen) {
        let close = matching_paren(tokens, *at)?;
        columns.extend(
            tokens
                .get(*at + 1..close)?
                .iter()
                .filter(|t| t.token != Token::Comma)
                .map(word_ident)
                .collect::<Option<Vec<_>>>()?,
        );
        *at = close + 1;
    }
    if !is_keyword(tokens.get(*at)?, Keyword::AS) {
        return None;
    }
    *at += 1;
    if tokens.get(*at).is_some_and(|t| is_keyword(t, Keyword::NOT)) {
        *at += 1;
    }
    if tokens
        .get(*at)
        .is_some_and(|t| is_keyword(t, Keyword::MATERIALIZED))
    {
        *at += 1;
    }
    tokens
        .get(*at)
        .is_some_and(|t| t.token == Token::LParen)
        .then_some(CteHeader { name, columns })
}

fn matching_paren(tokens: &[TokenWithSpan], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, tw) in tokens.iter().enumerate().skip(open) {
        match tw.token {
            Token::LParen => depth += 1,
            Token::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_keyword(tw: &TokenWithSpan, keyword: Keyword) -> bool {
    matches!(&tw.token, Token::Word(w) if w.keyword == keyword)
}

fn word_ident(tw: &TokenWithSpan) -> Option<Ident> {
    match &tw.token {
        Token::Word(w) => Some(Ident {
            value: w.value.clone(), // boundary: owned by the AST Ident
            quote_style: w.quote_style,
            span: tw.span,
        }),
        _ => None,
    }
}

/// Parses one part; a failure or depth overflow becomes a Hole, reported.
fn parse_part(
    tokens: Vec<TokenWithSpan>,
    pos: &Positions,
    dialect: &dyn SqlDialect,
    degraded: &mut Vec<Degradation>,
    what: &str,
) -> Part {
    let span = token_range(&tokens, pos);
    if tokens.is_empty() {
        degraded.push(Degradation::new(
            DegradationKind::ParseHole,
            format!("{what} is empty"),
        ));
        return Part::Hole(HoleKind::ParseError, span);
    }
    let depth = depth_upper_bound(&tokens);
    if depth > MAX_EXPR_DEPTH {
        degraded.push(
            Degradation::new(
                DegradationKind::CapExceeded,
                format!("expression depth {depth} in {what} exceeds {MAX_EXPR_DEPTH}"),
            )
            .at(span),
        );
        return Part::Hole(HoleKind::Truncated, span);
    }
    match parse_query(dialect, tokens) {
        Ok(query) => Part::Parsed(query),
        Err((_, err)) => {
            degraded.push(
                Degradation::new(
                    DegradationKind::ParseHole,
                    format!("{what} does not parse: {err}"),
                )
                .at(span),
            );
            Part::Hole(HoleKind::ParseError, span)
        }
    }
}

fn token_range(tokens: &[TokenWithSpan], pos: &Positions) -> Span {
    let span_of = |t: &TokenWithSpan| pos.span(t.span);
    match (
        tokens.first().and_then(span_of),
        tokens.last().and_then(span_of),
    ) {
        (Some(first), Some(last)) => Span::new(first.start, last.end),
        _ => Span::default(),
    }
}
