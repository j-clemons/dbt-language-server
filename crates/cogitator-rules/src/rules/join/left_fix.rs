//! The two fixes for `join.left_nullified`. Each edit lands on an exact
//! span next to a keyword the document text confirms; if anything is
//! not as expected the fix is withheld, not guessed.

use cogitator_ir::common::{BinaryOp, JoinKind};
use cogitator_ir::hir::{Body, ExprId, ExprKind, FromId, FromKind, Hir, JoinCond, Resolution};
use cogitator_ir::mir::{HirNode, Mir, ScalarId};
use cogitator_ir::origin::SourceOrigin;
use cogitator_ir::Span;

use super::support::{and_chain, expr, from_leaves, hir_ref};
use super::text::{is_word_ci, match_case, slice, word_before};
use crate::findings::{Fix, FixEdit};
use crate::meta::FixSafety;

/// Only the live template is editable; a compiled span is not.
fn editable(hir: &Hir) -> bool {
    matches!(hir.origin, SourceOrigin::Template { .. })
}

/// `LEFT [OUTER] JOIN` to `INNER JOIN`, bag-preserving because a
/// null-rejecting filter already drops the rows the outer join adds.
pub(super) fn inner_join(hir: &Hir, text: &str, join: FromId, kind: JoinKind) -> Option<Fix> {
    if !editable(hir) {
        return None;
    }
    let keyword = match kind {
        JoinKind::Left => "left",
        JoinKind::Right => "right",
        _ => return None,
    };
    let FromKind::Join { r, .. } = &hir.from.get(join)?.kind else {
        return None;
    };
    let right = hir.from.get(*r)?;
    if !matches!(right.kind, FromKind::Base { .. }) || right.span.is_empty() {
        return None;
    }
    let join_word = word_before(text, right.span.start)?;
    if !is_word_ci(text, join_word, "join") {
        return None;
    }
    let mut first = word_before(text, join_word.start)?;
    if is_word_ci(text, first, "outer") {
        first = word_before(text, first.start)?;
    }
    if !is_word_ci(text, first, keyword) {
        return None;
    }
    let span = Span::new(first.start, join_word.end);
    let new_text = match_case(slice(text, first)?, "INNER JOIN");
    Some(Fix {
        title: format!("Change to {new_text}"),
        safety: FixSafety::Preserving,
        edits: vec![FixEdit { span, new_text }],
    })
}

/// Moves one comparison from WHERE to ON. Never preferred: it keeps the
/// join outer, so rows the WHERE dropped come back null-extended.
pub(super) fn move_to_on(
    hir: &Hir,
    mir: &Mir,
    text: &str,
    join: FromId,
    conjunct: ScalarId,
) -> Option<Fix> {
    if !editable(hir) {
        return None;
    }
    let HirNode::Expr(moved) = hir_ref(mir, mir.scalars.get(conjunct)?.prov)?.node else {
        return None;
    };
    let FromKind::Join { on, .. } = &hir.from.get(join)?.kind else {
        return None;
    };
    let JoinCond::On(on) = on else {
        return None;
    };
    let conjuncts = where_conjuncts(hir, moved, join)?;
    if !cols_in_scope(hir, moved, join) {
        return None;
    }
    let at = conjuncts.iter().position(|c| *c == moved)?;
    let span = comparison_span(hir, text, moved)?;
    let removal = removal_span(hir, text, &conjuncts, at, span)?;
    let insert_at = conjunction_end(hir, *on)?;
    let moved_text = slice(text, span)?;
    Some(Fix {
        title: format!("Move `{moved_text}` to the ON clause"),
        safety: FixSafety::ChangesIntent,
        edits: vec![
            FixEdit {
                span: Span::new(insert_at, insert_at),
                new_text: format!(" and {moved_text}"),
            },
            FixEdit {
                span: removal,
                new_text: String::new(),
            },
        ],
    })
}

/// The conjuncts of the WHERE that holds `moved`, in the SELECT whose
/// FROM holds `join`. Nothing else may be moved textually: a filter
/// pushed in from an outer query names that query's columns.
fn where_conjuncts(hir: &Hir, moved: ExprId, join: FromId) -> Option<Vec<ExprId>> {
    hir.queries.iter().find_map(|(_, query)| {
        let Body::Select(select) = &query.body else {
            return None;
        };
        let filter = select.filter?;
        let conjuncts = and_chain(hir, filter);
        (conjuncts.contains(&moved) && select.from.iter().any(|f| from_contains(hir, *f, join)))
            .then_some(conjuncts)
    })
}

fn from_contains(hir: &Hir, root: FromId, target: FromId) -> bool {
    let mut stack = vec![root];
    let mut budget = hir.from.len() + 1;
    while let Some(id) = stack.pop() {
        if budget == 0 {
            return false;
        }
        budget -= 1;
        if id == target {
            return true;
        }
        if let Some(FromKind::Join { l, r, .. }) = hir.from.get(id).map(|f| &f.kind) {
            stack.push(*l);
            stack.push(*r);
        }
    }
    false
}

/// True when every column `moved` reads is visible at `join`'s ON: bound
/// and belonging to the join's own items, so nothing joined later.
fn cols_in_scope(hir: &Hir, moved: ExprId, join: FromId) -> bool {
    let scope = from_leaves(hir, join);
    let mut stack = vec![moved];
    while let Some(id) = stack.pop() {
        let Some(node) = expr(hir, id) else {
            return false;
        };
        if node.kind.subquery().is_some() {
            return false;
        }
        if let ExprKind::Col(c) = &node.kind {
            let Some(Some(Resolution::Bound { from, .. })) = hir.cols.get(*c).map(|c| &c.res)
            else {
                return false;
            };
            if !scope.contains(from) {
                return false;
            }
        }
        stack.extend(node.kind.children());
    }
    true
}

/// A `leaf op leaf` comparison's full span, from its two exact leaves
/// with only the operator between them.
fn comparison_span(hir: &Hir, text: &str, id: ExprId) -> Option<Span> {
    let ExprKind::Binary { op, l, r } = &expr(hir, id)?.kind else {
        return None;
    };
    if !is_comparison(*op) {
        return None;
    }
    let (left, right) = (expr(hir, *l)?, expr(hir, *r)?);
    if !left.kind.span_is_exact() || !right.kind.span_is_exact() {
        return None;
    }
    let between = text.get(left.span.end as usize..right.span.start as usize)?;
    if !is_operator_text(between.trim()) {
        return None;
    }
    // A parenthesised comparison would leave its parens behind.
    let bytes = text.as_bytes();
    let before = left.span.start.checked_sub(1).map(|i| bytes[i as usize]);
    let after = bytes.get(right.span.end as usize).copied();
    if before == Some(b'(') || after == Some(b')') {
        return None;
    }
    Some(Span::new(left.span.start, right.span.end))
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq
            | BinaryOp::Like
            | BinaryOp::ILike
    )
}

fn is_operator_text(op: &str) -> bool {
    matches!(op, "=" | "<>" | "!=" | "<" | "<=" | ">" | ">=")
        || op.eq_ignore_ascii_case("like")
        || op.eq_ignore_ascii_case("ilike")
}

/// What to delete so WHERE stays valid: the keyword with a lone
/// conjunct, else one conjunct and exactly one adjacent `and`.
fn removal_span(
    hir: &Hir,
    text: &str,
    conjuncts: &[ExprId],
    at: usize,
    span: Span,
) -> Option<Span> {
    if conjuncts.len() == 1 {
        let keyword = word_before(text, span.start)?;
        return is_word_ci(text, keyword, "where").then_some(Span::new(keyword.start, span.end));
    }
    let only_and =
        |between: Span| slice(text, between).is_some_and(|s| s.trim().eq_ignore_ascii_case("and"));
    if at == 0 {
        let next = comparison_span(hir, text, *conjuncts.get(1)?)?;
        return only_and(Span::new(span.end, next.start))
            .then_some(Span::new(span.start, next.start));
    }
    let prev = comparison_span(hir, text, *conjuncts.get(at - 1)?)?;
    only_and(Span::new(prev.end, span.start)).then_some(Span::new(prev.end, span.end))
}

/// Where a new conjunct can be appended to an ON clause: the end of its
/// last conjunct, provided the top level is only ANDs. Anything else,
/// such as an OR, would be regrouped by the appended text.
fn conjunction_end(hir: &Hir, on: ExprId) -> Option<u32> {
    let last = *and_chain(hir, on).last()?;
    match &expr(hir, last)?.kind {
        ExprKind::Binary { op, r, .. } if is_comparison(*op) => {
            let right = expr(hir, *r)?;
            right.kind.span_is_exact().then_some(right.span.end)
        }
        ExprKind::Col(_) | ExprKind::Lit(_) => expr(hir, last).map(|e| e.span.end),
        _ => None,
    }
}
