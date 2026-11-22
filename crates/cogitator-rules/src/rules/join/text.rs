//! Checks a fix makes against the document before it edits. `hir` spans
//! for keywords do not exist, so an edit anchored on an exact span next
//! to a keyword confirms the keyword is really there. This reads words
//! back from a known position; it does not tokenize SQL.

use cogitator_ir::Span;

/// `text[span]`, or `None` when the span is not valid in `text`.
pub(crate) fn slice(text: &str, span: Span) -> Option<&str> {
    if !span.is_valid_in(text) || span.is_empty() {
        return None;
    }
    text.get(span.start as usize..span.end as usize)
}

/// The word ending at (or just before, across whitespace) `pos`, if it
/// is outside any comment. A comment or punctuation in the way is no
/// word, which is what makes the caller give up.
pub(crate) fn word_before(text: &str, pos: u32) -> Option<Span> {
    let before = text.get(..pos as usize)?;
    let trimmed = before.trim_end();
    let bytes = trimmed.as_bytes();
    let end = trimmed.len();
    let start = bytes
        .iter()
        .rposition(|b| !is_word(*b))
        .map_or(0, |i| i + 1);
    if start == end {
        return None;
    }
    let span = Span::new(start as u32, end as u32);
    (!in_comment(text, span)).then_some(span)
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whether `span` sits inside a `--` line comment or an unclosed `/*`.
fn in_comment(text: &str, span: Span) -> bool {
    let Some(before) = text.get(..span.start as usize) else {
        return true;
    };
    let line = before.rsplit('\n').next().unwrap_or(before);
    if line.contains("--") {
        return true;
    }
    match (before.rfind("/*"), before.rfind("*/")) {
        (Some(open), Some(close)) => open > close,
        (Some(_), None) => true,
        _ => false,
    }
}

/// True when `text[span]` is `word`, ignoring ASCII case.
pub(crate) fn is_word_ci(text: &str, span: Span, word: &str) -> bool {
    slice(text, span).is_some_and(|s| s.eq_ignore_ascii_case(word))
}

/// Case-matches a replacement to the keyword it replaces.
pub(crate) fn match_case(original: &str, replacement: &str) -> String {
    if original.chars().any(|c| c.is_ascii_lowercase()) {
        replacement.to_ascii_lowercase()
    } else {
        replacement.to_ascii_uppercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_word_before_a_position() {
        let text = "a left  join\n b";
        let span = word_before(text, text.len() as u32 - 1).expect("word");
        assert_eq!(slice(text, span), Some("join"));
    }

    #[test]
    fn punctuation_in_the_way_is_not_a_word() {
        assert!(word_before("a, b", 2).is_none());
    }

    #[test]
    fn a_keyword_inside_a_comment_does_not_count() {
        let text = "a left join\n-- left join\n b";
        let at = text.len() as u32 - 1;
        let prev = word_before(text, at);
        assert!(prev.is_none(), "{prev:?}");
    }

    #[test]
    fn the_replacement_follows_the_written_case() {
        assert_eq!(match_case("LEFT", "inner join"), "INNER JOIN");
        assert_eq!(match_case("left", "INNER JOIN"), "inner join");
    }
}
