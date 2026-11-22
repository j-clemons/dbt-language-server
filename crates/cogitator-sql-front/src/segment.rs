//! SQL over the mask: sqlparser tokens with byte positions, and the
//! pre-parse position pass that takes clause-position Jinja out of the
//! token stream before it can parse as an alias.

use cogitator_dialects::Dialect;
use cogitator_ir::Span;
use cogitator_parser::backend::sql_dialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer};

use crate::mask::Region;

/// Maps sqlparser locations to byte offsets of the masked text. Lookups
/// are by exact location, so no line scan ever runs per identifier.
#[derive(Debug, Default)]
pub(crate) struct Positions {
    marks: Vec<(u64, u64, u32)>,
}

impl Positions {
    fn byte(&self, at: Location) -> Option<u32> {
        let key = (at.line, at.column);
        let i = self
            .marks
            .binary_search_by_key(&key, |&(line, column, _)| (line, column))
            .ok()?;
        self.marks.get(i).map(|m| m.2)
    }

    /// The byte span of a sqlparser span; `None` for `Span::empty()`.
    pub(crate) fn span(&self, span: sqlparser::tokenizer::Span) -> Option<Span> {
        if span.start.line == 0 {
            return None;
        }
        Some(Span::new(self.byte(span.start)?, self.byte(span.end)?))
    }
}

#[derive(Debug)]
pub(crate) struct Lexed {
    pub tokens: Vec<TokenWithSpan>,
    pub pos: Positions,
    /// Significant tokens only: the number the token cap counts.
    pub significant: usize,
}

pub(crate) fn lex(mask: &str, dialect: &Dialect) -> Result<Lexed, String> {
    let dialect = sql_dialect(dialect);
    let tokens = Tokenizer::new(dialect.as_ref(), mask)
        .tokenize_with_location()
        .map_err(|e| e.to_string())?;
    let pos = positions(mask, &tokens);
    let significant = tokens.iter().filter(|t| is_significant(&t.token)).count();
    Ok(Lexed {
        tokens,
        pos,
        significant,
    })
}

pub(crate) fn is_significant(token: &Token) -> bool {
    !matches!(token, Token::Whitespace(_) | Token::EOF)
}

/// One forward walk over the mask: columns count chars, not bytes.
fn positions(mask: &str, tokens: &[TokenWithSpan]) -> Positions {
    let mut marks = Vec::with_capacity(tokens.len() * 2);
    let mut chars = mask.chars();
    let (mut byte, mut line, mut column) = (0u32, 1u64, 1u64);
    let mut seek = |to: Location| {
        while (line, column) < (to.line, to.column) {
            let Some(c) = chars.next() else { break };
            byte += c.len_utf8() as u32;
            if c == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        (to.line, to.column, byte)
    };
    for tw in tokens {
        if tw.span.start.line == 0 {
            continue;
        }
        marks.push(seek(tw.span.start));
        marks.push(seek(tw.span.end));
    }
    Positions { marks }
}

/// Decides which placeholders sit at a clause position and blanks them.
/// `from t {{ m() }}` would otherwise read as an alias, and `where a = 1
/// {{ m() }}` as a syntax error. Everything else stays a placeholder.
pub(crate) fn resolve_positions(lexed: &mut Lexed, regions: &mut [Region]) {
    let mut remove = Vec::new();
    let mut prev: Option<usize> = None;
    let mut r = 0;
    for (i, tw) in lexed.tokens.iter().enumerate() {
        if !is_significant(&tw.token) {
            continue;
        }
        let Some(span) = lexed.pos.span(tw.span) else {
            prev = Some(i);
            continue;
        };
        // Regions wholly before this token: in a string, comment or hole.
        while regions
            .get(r)
            .is_some_and(|reg| reg.word.is_none_or(|w| w.end <= span.start))
        {
            if let Some(reg) = regions.get_mut(r) {
                reg.word = None;
            }
            r += 1;
        }
        let mut removed = false;
        if matches!(tw.token, Token::Word(_)) {
            while let Some(reg) = regions.get_mut(r) {
                let Some(word) = reg.word.filter(|w| w.start < span.end) else {
                    break;
                };
                if word == span {
                    let expects = prev
                        .and_then(|p| lexed.tokens.get(p))
                        .is_some_and(|p| expects_operand(&p.token));
                    if !expects {
                        reg.clause = true;
                        reg.word = None;
                        remove.push(i);
                        removed = true;
                    }
                } else {
                    // Pasted to surrounding SQL (`a_{{ x }}`): the whole
                    // word stands for the region; lowering finds it by overlap.
                    reg.word = Some(span);
                }
                r += 1;
            }
        }
        if !removed {
            prev = Some(i);
        }
    }
    for reg in regions.iter_mut().skip(r) {
        reg.word = None;
    }
    if !remove.is_empty() {
        let mut index = 0usize;
        lexed.tokens.retain(|_| {
            let keep = remove.binary_search(&index).is_err();
            index += 1;
            keep
        });
    }
}

/// True when the previous token calls for an operand or a relation next.
fn expects_operand(token: &Token) -> bool {
    match token {
        Token::Word(w) => matches!(
            w.keyword,
            Keyword::SELECT
                | Keyword::DISTINCT
                | Keyword::ALL
                | Keyword::FROM
                | Keyword::JOIN
                | Keyword::ON
                | Keyword::WHERE
                | Keyword::AND
                | Keyword::OR
                | Keyword::NOT
                | Keyword::BY
                | Keyword::HAVING
                | Keyword::QUALIFY
                | Keyword::WHEN
                | Keyword::THEN
                | Keyword::ELSE
                | Keyword::CASE
                | Keyword::IN
                | Keyword::BETWEEN
                | Keyword::LIKE
                | Keyword::ILIKE
                | Keyword::AS
                | Keyword::USING
                | Keyword::LIMIT
                | Keyword::OFFSET
                | Keyword::WITH
                | Keyword::UNION
                | Keyword::EXCEPT
                | Keyword::INTERSECT
                | Keyword::MINUS
                | Keyword::VALUES
                | Keyword::OVER
                | Keyword::PARTITION
                | Keyword::ANY
                | Keyword::SOME
                | Keyword::EXISTS
                | Keyword::LATERAL
                | Keyword::INTERVAL
                | Keyword::EXCLUDE
                | Keyword::REPLACE
                | Keyword::TOP
        ),
        Token::RParen
        | Token::RBracket
        | Token::RBrace
        | Token::Number(..)
        | Token::SingleQuotedString(_)
        | Token::DoubleQuotedString(_)
        | Token::Placeholder(_) => false,
        // Operators, commas, `(`, `.` and `::` all expect what follows.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::scan;
    use cogitator_ir::origin::TemplateVariant;

    fn clause_of(text: &str) -> Vec<bool> {
        let dialect = Dialect::new("duckdb");
        let mut s = scan(text, TemplateVariant::Plain).expect("scan");
        let mut lexed = lex(&s.mask, &dialect).expect("lex");
        resolve_positions(&mut lexed, &mut s.regions);
        s.regions.iter().map(|r| r.clause).collect()
    }

    #[test]
    fn clause_position_after_an_operand() {
        assert_eq!(clause_of("select a from t {{ m() }}"), vec![true]);
        assert_eq!(clause_of("select a {{ m() }} from t"), vec![true]);
        assert_eq!(
            clause_of("select a from t where x = 1 {{ m() }}"),
            vec![true]
        );
        assert_eq!(clause_of("select a from ({{ m() }})"), vec![false]);
        assert_eq!(clause_of("select {{ m() }} from t"), vec![false]);
    }

    #[test]
    fn value_positions_stay_placeholders() {
        assert_eq!(clause_of("select a as {{ m() }} from t"), vec![false]);
        assert_eq!(clause_of("select a, {{ m() }} from t"), vec![false]);
        assert_eq!(clause_of("select a from t where {{ m() }}"), vec![false]);
        assert_eq!(clause_of("select x::{{ m() }} from t"), vec![false]);
    }

    #[test]
    fn a_lone_macro_is_at_clause_position() {
        assert_eq!(clause_of("{{ m() }}"), vec![true]);
    }

    #[test]
    fn a_keyword_named_relation_still_ends_the_clause() {
        // `final` is a sqlparser keyword but a table name here.
        assert_eq!(clause_of("select 1 from final {{ m() }}"), vec![true]);
    }

    #[test]
    fn positions_resolve_tokens_to_bytes() {
        let dialect = Dialect::new("duckdb");
        let lexed = lex("select é,\n a", &dialect).expect("lex");
        let word: Vec<_> = lexed
            .tokens
            .iter()
            .filter(|t| matches!(t.token, Token::Word(_)))
            .map(|t| lexed.pos.span(t.span).expect("span"))
            .collect();
        assert_eq!(word[0], Span::new(0, 6));
        assert_eq!(word[1], Span::new(7, 9));
        assert_eq!(word[2], Span::new(12, 13));
    }
}
