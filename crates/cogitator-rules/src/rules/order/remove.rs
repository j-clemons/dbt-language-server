//! The text range an `order.in_cte` fix deletes. Every word it relies on
//! is read back from the document; anything unexpected withholds the fix.

use cogitator_ir::hir::{ExprKind, Hir, Query};
use cogitator_ir::origin::SourceOrigin;
use cogitator_ir::Span;

use crate::rules::join::support::expr;
use crate::rules::join::text::{is_word_ci, slice, word_before};
use crate::rules::subq::scan::{holds_comment_or_jinja, next_nonspace, word_after};

/// The span of the whole `ORDER BY …` clause of `query`, plus the blank
/// space leading into it, when it can be deleted exactly.
pub(super) fn order_by_span(hir: &Hir, text: &str, query: &Query) -> Option<Span> {
    if !matches!(hir.origin, SourceOrigin::Template { .. }) {
        return None;
    }
    let first = expr(hir, query.order_by.first()?.expr)?;
    let last = expr(hir, query.order_by.last()?.expr)?;
    if !first.kind.span_is_exact()
        || !last.kind.span_is_exact()
        || matches!(first.kind, ExprKind::Jinja(_))
        || matches!(last.kind, ExprKind::Jinja(_))
    {
        return None;
    }
    let by = word_before(text, first.span.start)?;
    if !is_word_ci(text, by, "by") {
        return None;
    }
    let order = word_before(text, by.start)?;
    if !is_word_ci(text, order, "order") {
        return None;
    }
    let end = clause_end(text, last.span.end)?;
    // only a closing paren may follow: anything else is a clause the
    // HIR does not show (OFFSET, FETCH) or a comment
    if text.as_bytes().get(next_nonspace(text, end)? as usize) != Some(&b')') {
        return None;
    }
    let span = Span::new(order.start, end);
    if holds_comment_or_jinja(slice(text, span)?) {
        return None;
    }
    Some(Span::new(leading_blank_start(text, order.start), end))
}

/// The end of the last key's options: `ASC|DESC`, `NULLS FIRST|LAST`.
fn clause_end(text: &str, last_key_end: u32) -> Option<u32> {
    let mut end = last_key_end;
    let is = |span: Span, word: &str| is_word_ci(text, span, word);
    if let Some(word) = word_after(text, end) {
        if is(word, "asc") || is(word, "desc") {
            end = word.end;
        }
    }
    if let Some(nulls) = word_after(text, end) {
        if is(nulls, "nulls") {
            let word = word_after(text, nulls.end)?;
            if !is(word, "first") && !is(word, "last") {
                return None;
            }
            end = word.end;
        }
    }
    Some(end)
}

/// Extends the deletion over the whitespace before the clause, unless
/// that would pull text onto a `--` comment's line.
fn leading_blank_start(text: &str, order_start: u32) -> u32 {
    let Some(before) = text.get(..order_start as usize) else {
        return order_start;
    };
    let kept = before.trim_end();
    let comment_on_line = kept.rsplit('\n').next().unwrap_or(kept).contains("--");
    if comment_on_line {
        order_start
    } else {
        kept.len() as u32
    }
}
