//! dbt's injected ephemeral CTEs (`__dbt__cte__x`), found in the tokens.

use cogitator_ir::Span;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, TokenWithSpan};

/// dbt's name for an ephemeral model injected as a CTE.
pub(super) const EPHEMERAL_PREFIX: &str = "__dbt__cte__";

/// The span of dbt's injected ephemeral CTEs. dbt puts them directly
/// after a leading `WITH`: with user CTEs behind them, the `WITH` stays;
/// without any, it is dbt's own and goes too.
pub(super) fn injected_ctes(tokens: &[(&TokenWithSpan, Span)]) -> Vec<Span> {
    let Some((first, with_span)) = tokens.first() else {
        return Vec::new();
    };
    if !matches!(&first.token, Token::Word(w) if w.keyword == Keyword::WITH) {
        return Vec::new();
    }
    let mut i = 1;
    let mut start = None;
    let mut end = 0;
    let mut more_ctes = false;
    while let Some(after) = one_ephemeral(tokens, i) {
        start.get_or_insert(tokens.get(i).map_or(0, |(_, s)| s.start));
        end = after.end;
        i = after.next;
        match tokens.get(i) {
            Some((token, comma)) if matches!(token.token, Token::Comma) => {
                end = comma.end;
                i += 1;
                more_ctes = true;
            }
            _ => {
                more_ctes = false;
                break;
            }
        }
    }
    match start {
        Some(start) if more_ctes => vec![Span::new(start, end)],
        Some(_) => vec![Span::new(with_span.start, end)],
        None => Vec::new(),
    }
}

struct Ephemeral {
    end: u32,
    next: usize,
}

/// `__dbt__cte__x as ( … )` at `at`.
fn one_ephemeral(tokens: &[(&TokenWithSpan, Span)], at: usize) -> Option<Ephemeral> {
    let (name, _) = tokens.get(at)?;
    let Token::Word(word) = &name.token else {
        return None;
    };
    if !word.value.starts_with(EPHEMERAL_PREFIX) {
        return None;
    }
    let (keyword, _) = tokens.get(at + 1)?;
    if !matches!(&keyword.token, Token::Word(w) if w.keyword == Keyword::AS) {
        return None;
    }
    let (paren, _) = tokens.get(at + 2)?;
    if !matches!(paren.token, Token::LParen) {
        return None;
    }
    let mut depth = 0u32;
    for (offset, (token, span)) in tokens.iter().enumerate().skip(at + 2) {
        match token.token {
            Token::LParen => depth += 1,
            Token::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(Ephemeral {
                        end: span.end,
                        next: offset + 1,
                    });
                }
            }
            _ => {}
        }
    }
    None
}
