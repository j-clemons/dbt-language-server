//! Front-end caps (specs/ir-pipeline/00). Every one degrades explicitly;
//! all of them run before anything recursive sees the input.

use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, TokenWithSpan};

pub(crate) const MAX_TEXT_BYTES: usize = 512 * 1024;
pub(crate) const MAX_TOKENS: usize = 40_000;
pub(crate) const MAX_EXPR_DEPTH: usize = 256;
pub(crate) const MAX_NODES: usize = 200_000;

/// One open `(` or `CASE`.
#[derive(Default)]
struct Level {
    is_case: bool,
    /// Left-deep set-operation arms at this level.
    unions: usize,
    /// Operators since the last comma or clause keyword.
    chain: usize,
    /// Deepest finished chain at this level.
    widest: usize,
}

impl Level {
    fn close(&self) -> usize {
        self.unions + self.widest.max(self.chain) + 1
    }
}

/// An upper bound on the depth of the AST these tokens parse into.
///
/// sqlparser's recursion limit stops nesting, not left-deep chains: `a+b+c`
/// loops in the parser and only grows the tree downward, so a long chain
/// overflows the stack in `Drop` or in any walker. Each paren or CASE adds
/// what finished inside it to the chain of its parent, so `((a+b)+c)+d`
/// counts fully; commas and clause keywords end a chain.
pub(crate) fn depth_upper_bound(tokens: &[TokenWithSpan]) -> usize {
    let mut levels = vec![Level::default()];
    for tw in tokens {
        match &tw.token {
            Token::LParen => levels.push(Level::default()),
            Token::RParen => close_paren(&mut levels),
            Token::Comma => end_chain(&mut levels),
            Token::Word(w) => match w.keyword {
                Keyword::CASE => levels.push(Level {
                    is_case: true,
                    ..Level::default()
                }),
                Keyword::END if levels.last().is_some_and(|l| l.is_case) => {
                    close_top(&mut levels);
                }
                Keyword::UNION | Keyword::EXCEPT | Keyword::INTERSECT | Keyword::MINUS => {
                    if let Some(level) = levels.last_mut() {
                        level.unions += 1;
                    }
                    end_chain(&mut levels);
                }
                k if is_chain_keyword(k) => grow(&mut levels),
                k if is_clause_keyword(k) => end_chain(&mut levels),
                _ => {}
            },
            t if is_operator(t) => grow(&mut levels),
            _ => {}
        }
    }
    while levels.len() > 1 {
        close_top(&mut levels);
    }
    levels.pop().map_or(0, |l| l.unions + l.widest.max(l.chain))
}

fn grow(levels: &mut [Level]) {
    if let Some(level) = levels.last_mut() {
        level.chain += 1;
    }
}

fn end_chain(levels: &mut [Level]) {
    if let Some(level) = levels.last_mut() {
        level.widest = level.widest.max(level.chain);
        level.chain = 0;
    }
}

fn close_top(levels: &mut Vec<Level>) {
    let Some(done) = levels.pop() else { return };
    let inner = done.close();
    match levels.last_mut() {
        Some(parent) => parent.chain += inner,
        None => levels.push(Level {
            chain: inner,
            ..Level::default()
        }),
    }
}

/// `)` closes the nearest paren, dropping any unterminated CASE inside it.
fn close_paren(levels: &mut Vec<Level>) {
    while levels.len() > 1 && levels.last().is_some_and(|l| l.is_case) {
        close_top(levels);
    }
    if levels.len() > 1 {
        close_top(levels);
    }
}

fn is_chain_keyword(k: Keyword) -> bool {
    matches!(
        k,
        Keyword::AND
            | Keyword::OR
            | Keyword::NOT
            | Keyword::IS
            | Keyword::IN
            | Keyword::LIKE
            | Keyword::ILIKE
            | Keyword::RLIKE
            | Keyword::REGEXP
            | Keyword::BETWEEN
            | Keyword::SIMILAR
            | Keyword::AT
            | Keyword::COLLATE
            | Keyword::XOR
            | Keyword::DIV
            | Keyword::OVERLAPS
    )
}

fn is_clause_keyword(k: Keyword) -> bool {
    matches!(
        k,
        Keyword::SELECT
            | Keyword::FROM
            | Keyword::WHERE
            | Keyword::GROUP
            | Keyword::HAVING
            | Keyword::QUALIFY
            | Keyword::ORDER
            | Keyword::LIMIT
            | Keyword::OFFSET
            | Keyword::JOIN
            | Keyword::ON
            | Keyword::USING
            | Keyword::WHEN
            | Keyword::THEN
            | Keyword::ELSE
            | Keyword::WINDOW
    )
}

/// Tokens that extend an expression by one level: everything but names,
/// literals and punctuation. Over-counting only degrades earlier.
fn is_operator(token: &Token) -> bool {
    !matches!(
        token,
        Token::EOF
            | Token::Whitespace(_)
            | Token::Word(_)
            | Token::Number(..)
            | Token::Char(_)
            | Token::SingleQuotedString(_)
            | Token::DoubleQuotedString(_)
            | Token::Comma
            | Token::LParen
            | Token::RParen
            | Token::Period
            | Token::SemiColon
            | Token::LBracket
            | Token::RBracket
            | Token::LBrace
            | Token::RBrace
            | Token::Placeholder(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlparser::dialect::GenericDialect;
    use sqlparser::tokenizer::Tokenizer;

    fn depth(sql: &str) -> usize {
        let dialect = GenericDialect {};
        let tokens = Tokenizer::new(&dialect, sql)
            .tokenize_with_location()
            .unwrap_or_default();
        depth_upper_bound(&tokens)
    }

    #[test]
    fn chains_count_and_commas_reset() {
        assert_eq!(depth("select a + b + c"), 2);
        assert_eq!(depth("select a + b, c + d"), 1);
        assert_eq!(depth("select a and b from t where c or d or e"), 2);
    }

    #[test]
    fn parens_add_their_inner_depth_to_the_parent() {
        assert!(depth("select ((a + b) + c) + d") >= 3);
        assert!(depth("select f(a + b, c) + d") >= 2);
    }

    #[test]
    fn set_operations_are_counted_across_clauses() {
        let chain = "select 1 union all ".repeat(10) + "select 1";
        assert_eq!(depth(&chain), 10);
    }

    #[test]
    fn case_ends_nest_like_parens() {
        assert!(depth("select case when a then b end + c") >= 2);
    }
}
