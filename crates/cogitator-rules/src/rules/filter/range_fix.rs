//! The quick fix behind `range_rewrite`: `DATE(ts) = DATE 'd'` to a
//! half-open range on `ts`, offered only on a template whose text shows
//! exactly that predicate. `Call` and `Cast` spans are approximate
//! (`ExprKind::span_is_exact`), so the edit is read back from the exact
//! column and literal spans; anything else in between withholds the fix.
//!
//! `DATE` is a word and `(`, `)` and `=` are punctuation, so comments or
//! parentheses inside the predicate fail the check rather than shift it.

use cogitator_ir::common::{BinaryOp, Lit};
use cogitator_ir::hir::{Body, ExprId, ExprKind, Hir};
use cogitator_ir::mir::{ColId, HirNode, Mir, ScalarId};
use cogitator_ir::origin::SourceOrigin;
use cogitator_ir::types::SqlType;
use cogitator_ir::Span;

use super::range::range_rewrite;
use crate::cx::Cx;
use crate::findings::{Fix, FixEdit};
use crate::meta::FixSafety;
use crate::rules::join::support::{and_chain, expr, hir_ref};
use crate::rules::join::text::{is_word_ci, slice, word_before};

/// The fix for `conjunct`, or `None` unless the document holds exactly
/// the predicate the plan does.
pub(super) fn range_fix(
    mir: &Mir,
    cx: &Cx<'_>,
    conjunct: ScalarId,
    col: ColId,
    ty: &SqlType,
    name: &str,
) -> Option<Fix> {
    let (hir, text) = (cx.hir?, cx.text?);
    if !matches!(hir.origin, SourceOrigin::Template { .. }) {
        return None;
    }
    let HirNode::Expr(id) = hir_ref(mir, mir.scalars.get(conjunct)?.prov)?.node else {
        return None;
    };
    if !is_where_conjunct(hir, id) {
        return None;
    }
    let ExprKind::Binary {
        op: BinaryOp::Eq,
        l,
        r,
    } = expr(hir, id)?.kind
    else {
        return None;
    };
    let (trunc, literal, span) = if let Some(trunc) = truncation(hir, text, l, name) {
        let literal = date_literal(hir, text, r)?;
        only_equals(text, trunc.span.end, literal.span.start)?;
        let span = Span::new(trunc.span.start, literal.span.end);
        (trunc, literal, span)
    } else {
        let trunc = truncation(hir, text, r, name)?;
        let literal = date_literal(hir, text, l)?;
        only_equals(text, literal.span.end, trunc.span.start)?;
        let span = Span::new(literal.span.start, trunc.span.end);
        (trunc, literal, span)
    };
    let new_text = range_rewrite(mir, conjunct, col, ty, trunc.column)?;
    if !new_text.contains(&format!("DATE '{}'", literal.value)) {
        return None;
    }
    Some(Fix {
        title: format!("Compare `{}` with a date range", trunc.column),
        safety: FixSafety::Preserving,
        edits: vec![FixEdit { span, new_text }],
    })
}

/// True when `id` is one AND-ed conjunct of some SELECT's WHERE, so an
/// `AND` in its place cannot regroup anything around it.
fn is_where_conjunct(hir: &Hir, id: ExprId) -> bool {
    hir.queries.values().any(|query| {
        let Body::Select(select) = &query.body else {
            return false;
        };
        select
            .filter
            .is_some_and(|filter| and_chain(hir, filter).contains(&id))
    })
}

struct Truncation<'a> {
    /// The `DATE(…)` or `CAST(… AS DATE)` as written.
    span: Span,
    /// The column as written, qualifier included.
    column: &'a str,
}

/// `date(col)` or `cast(col as date)`, read back from the column's exact
/// span. `name` is the plan's column: the written one must end with it.
fn truncation<'a>(hir: &Hir, text: &'a str, id: ExprId, name: &str) -> Option<Truncation<'a>> {
    let (arg, is_cast) = match &expr(hir, id)?.kind {
        ExprKind::Call {
            args,
            distinct: false,
            filter: None,
            ..
        } => match args.as_slice() {
            [arg] => (*arg, false),
            _ => return None,
        },
        ExprKind::Cast {
            e,
            ty: SqlType::Date,
        } => (*e, true),
        _ => return None,
    };
    let column_expr = expr(hir, arg)?;
    if !matches!(column_expr.kind, ExprKind::Col(_)) {
        return None;
    }
    let column_span = column_expr.span;
    let column = slice(text, column_span)?;
    if !column.rsplit('.').next()?.eq_ignore_ascii_case(name) {
        return None;
    }
    let before = text.get(..column_span.start as usize)?.trim_end();
    let paren = before.strip_suffix('(')?.len();
    let word = word_before(text, paren as u32)?;
    if !is_word_ci(text, word, if is_cast { "cast" } else { "date" }) {
        return None;
    }
    let end = if is_cast {
        cast_end(text, column_span.end)?
    } else {
        close_paren(text, column_span.end)?
    };
    Some(Truncation {
        span: Span::new(word.start, end),
        column,
    })
}

/// After `cast(col`: `as date)`, returning the end of the `)`.
fn cast_end(text: &str, from: u32) -> Option<u32> {
    let as_word = next_word(text, from)?;
    if !is_word_ci(text, as_word, "as") {
        return None;
    }
    let date_word = next_word(text, as_word.end)?;
    if !is_word_ci(text, date_word, "date") {
        return None;
    }
    close_paren(text, date_word.end)
}

fn next_word(text: &str, from: u32) -> Option<Span> {
    let rest = text.get(from as usize..)?;
    let start = from as usize + (rest.len() - rest.trim_start().len());
    let len = text
        .get(start..)?
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    (len > 0).then(|| Span::new(start as u32, (start + len) as u32))
}

/// The end of a `)` that follows `from` across whitespace only.
fn close_paren(text: &str, from: u32) -> Option<u32> {
    let rest = text.get(from as usize..)?;
    let trimmed = rest.trim_start();
    trimmed
        .starts_with(')')
        .then(|| from + (rest.len() - trimmed.len()) as u32 + 1)
}

/// `Some` only when the text between the two sides is a single `=`.
fn only_equals(text: &str, from: u32, to: u32) -> Option<()> {
    let between = text.get(from as usize..to as usize)?;
    (between.trim() == "=").then_some(())
}

struct DateLiteral<'a> {
    /// `DATE '…'` as written.
    span: Span,
    value: &'a str,
}

/// A `DATE '…'` literal: the keyword before the value, and the value
/// quoted as the plan read it. The HIR span covers only the quoted part.
fn date_literal<'a>(hir: &'a Hir, text: &str, id: ExprId) -> Option<DateLiteral<'a>> {
    let node = expr(hir, id)?;
    let ExprKind::Lit(Lit::Typed { ty, value }) = node.kind else {
        return None;
    };
    if !hir.syms.resolve(ty)?.eq_ignore_ascii_case("date") {
        return None;
    }
    let value = hir.syms.resolve(value)?;
    if slice(text, node.span)? != format!("'{value}'") {
        return None;
    }
    let keyword = word_before(text, node.span.start)?;
    is_word_ci(text, keyword, "date").then_some(DateLiteral {
        span: Span::new(keyword.start, node.span.end),
        value,
    })
}
