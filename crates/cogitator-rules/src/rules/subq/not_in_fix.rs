//! The `NOT EXISTS` rewrite for `subq.not_in_nullable`. Never preferred:
//! a NULL on either side behaves differently under NOT EXISTS, which is
//! the point. Offered only for a shape every edit can be checked for:
//! one qualified outer column against one column of a single FROM item.

use cogitator_ir::common::BinaryOp;
use cogitator_ir::hir::{
    Body, Distinct, ExprId, ExprKind, From, FromKind, GroupBy, Hir, Ident, RelTarget, SelectItem,
};
use cogitator_ir::origin::SourceOrigin;
use cogitator_ir::Span;

use super::scan::{holds_comment_or_jinja, next_nonspace, word_after};
use crate::findings::{Fix, FixEdit};
use crate::meta::FixSafety;
use crate::rules::join::support::{and_chain, expr};
use crate::rules::join::text::{is_word_ci, match_case, slice, word_before};

pub(super) fn not_exists(hir: &Hir, text: &str, id: ExprId) -> Option<Fix> {
    if !matches!(hir.origin, SourceOrigin::Template { .. }) {
        return None;
    }
    let ExprKind::InQuery {
        e,
        q,
        negated: true,
    } = &expr(hir, id)?.kind
    else {
        return None;
    };
    let outer = expr(hir, *e)?;
    let ExprKind::Col(col) = outer.kind else {
        return None;
    };
    let qualifier = hir.cols.get(col)?.qualifier.first()?;
    let query = hir.queries.get(*q)?;
    if !query.ctes.is_empty() || !query.order_by.is_empty() || query.limit.is_some() {
        return None;
    }
    let Body::Select(select) = &query.body else {
        return None;
    };
    let plain = select.distinct == Distinct::None
        && select.group_by == GroupBy::None
        && select.having.is_none()
        && select.qualify.is_none()
        && select.windows.is_empty();
    let ([SelectItem::Expr { expr: item, .. }], [from_id], true) =
        (select.items.as_slice(), select.from.as_slice(), plain)
    else {
        return None;
    };
    let inner = expr(hir, *item)?;
    if !matches!(inner.kind, ExprKind::Col(_)) {
        return None;
    }
    let from = hir.from.get(*from_id)?;
    if may_shadow(hir, from, qualifier) {
        return None;
    }
    let lead = word_after(text, outer.span.end)?;
    if !is_word_ci(text, lead, "not") {
        return None;
    }
    let case_of = slice(text, lead)?;
    let predicate = format!(
        "{} = {}",
        slice(text, outer.span)?,
        slice(text, inner.span)?
    );
    let head = head_edit(text, outer.span, lead, case_of)?;
    let tail = match select.filter {
        Some(filter) => after_where(hir, text, filter, &predicate, case_of)?,
        None => before_close(text, from.span, &predicate, case_of)?,
    };
    Some(Fix {
        title: "Rewrite as NOT EXISTS".to_owned(),
        safety: FixSafety::ChangesIntent,
        edits: vec![head, tail],
    })
}

/// True when the subquery's FROM item could capture the outer qualifier:
/// the same name, or a name that cannot be read from here.
fn may_shadow(hir: &Hir, from: &From, qualifier: &Ident) -> bool {
    let FromKind::Base { target, alias } = &from.kind else {
        return true;
    };
    if let Some(alias) = alias {
        return alias.folded == qualifier.folded;
    }
    match target {
        RelTarget::Model { name, .. } => *name == qualifier.folded,
        RelTarget::Source { table, .. } => *table == qualifier.folded,
        RelTarget::Cte(cte) => hir
            .ctes
            .get(*cte)
            .is_none_or(|c| c.name.folded == qualifier.folded),
        RelTarget::Physical(parts) => parts.last().is_none_or(|p| p.folded == qualifier.folded),
        RelTarget::This | RelTarget::Jinja(_) => true,
    }
}

/// Replaces `x not in (` with `not exists (`.
fn head_edit(text: &str, outer: Span, not_word: Span, case_of: &str) -> Option<FixEdit> {
    let in_word = word_after(text, not_word.end)?;
    if !is_word_ci(text, in_word, "in") {
        return None;
    }
    let paren = next_nonspace(text, in_word.end)?;
    if text.as_bytes().get(paren as usize) != Some(&b'(') {
        return None;
    }
    Some(FixEdit {
        span: Span::new(outer.start, paren + 1),
        new_text: format!("{} (", match_case(case_of, "NOT EXISTS")),
    })
}

/// Inserts the correlation right after WHERE, whose conjuncts must all
/// be ANDed at the top level or the new term would regroup them.
fn after_where(
    hir: &Hir,
    text: &str,
    filter: ExprId,
    predicate: &str,
    case_of: &str,
) -> Option<FixEdit> {
    let chain = and_chain(hir, filter);
    let is_or = |id: &ExprId| {
        matches!(
            expr(hir, *id).map(|e| &e.kind),
            Some(ExprKind::Binary {
                op: BinaryOp::Or,
                ..
            })
        )
    };
    if chain.iter().any(is_or) {
        return None;
    }
    let first = expr(hir, *chain.first()?)?;
    let word = word_before(text, first.span.start)?;
    if !is_word_ci(text, word, "where") {
        return None;
    }
    Some(FixEdit {
        span: Span::new(word.end, word.end),
        new_text: format!(" {predicate} {}", match_case(case_of, "AND")),
    })
}

/// Inserts `WHERE …` before the closing paren, when only the FROM item's
/// own tokens (an alias) stand between it and that paren.
fn before_close(text: &str, from: Span, predicate: &str, case_of: &str) -> Option<FixEdit> {
    let rest = text.get(from.end as usize..)?;
    let close = rest.find(')')?;
    let between = rest.get(..close)?;
    let allowed = |c: char| c.is_ascii_alphanumeric() || c.is_whitespace() || "_.\"`".contains(c);
    if holds_comment_or_jinja(between) || !between.chars().all(allowed) {
        return None;
    }
    let at = from.end + between.trim_end().len() as u32;
    Some(FixEdit {
        span: Span::new(at, at),
        new_text: format!(" {} {predicate}", match_case(case_of, "WHERE")),
    })
}
