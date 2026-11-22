//! The bag-preserving rewrite of `DATE(ts) = d` into a range on `ts`.
//!
//! BigQuery's `DATE(ts)`, `CAST(ts AS DATE)` and `TIMESTAMP(d)` all use
//! UTC unless told otherwise, so the range selects exactly the rows of
//! the original, NULL `ts` included (neither form keeps them).
//!
//! This builds the text; `range_fix` turns it into an edit once the
//! document confirms the whole predicate, because a `Call` span stops
//! before its `)` and parentheses leave no trace in the HIR
//! (`ExprKind::span_is_exact`).
//! `testdata/oracle/filter_non_sargable_date_range` checks the rewrite
//! itself.

use cogitator_ir::common::{BinaryOp, Lit};
use cogitator_ir::mir::{ColId, Mir, ScalarId, ScalarKind};
use cogitator_ir::types::{SqlType, Tz};

/// The equivalent range for `conjunct`, or `None` unless it is exactly
/// `DATE(col) = DATE '…'` (either side) on a column known to be a
/// timestamp.
pub(super) fn range_rewrite(
    mir: &Mir,
    conjunct: ScalarId,
    col: ColId,
    ty: &SqlType,
    name: &str,
) -> Option<String> {
    let constructor = match ty {
        SqlType::Timestamp { tz: Tz::Without } => "DATETIME",
        SqlType::Timestamp { .. } => "TIMESTAMP",
        _ => return None,
    };
    let is_name = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'.';
    if name.is_empty() || !name.bytes().all(is_name) {
        return None;
    }
    let ScalarKind::Binary {
        op: BinaryOp::Eq,
        l,
        r,
    } = mir.scalars.get(conjunct)?.kind
    else {
        return None;
    };
    let day = if truncates_to_date(mir, l, col) {
        date_literal(mir, r)?
    } else if truncates_to_date(mir, r, col) {
        date_literal(mir, l)?
    } else {
        return None;
    };
    Some(format!(
        "{name} >= {constructor}(DATE '{day}') AND \
         {name} < {constructor}(DATE_ADD(DATE '{day}', INTERVAL 1 DAY))"
    ))
}

/// `DATE(col)` with one argument, or `CAST(col AS DATE)`.
fn truncates_to_date(mir: &Mir, id: ScalarId, col: ColId) -> bool {
    let is_col = |e: ScalarId| matches!(mir.scalars.get(e).map(|s| &s.kind), Some(ScalarKind::Col(c)) if *c == col);
    match mir.scalars.get(id).map(|s| &s.kind) {
        Some(ScalarKind::Call {
            name,
            args,
            distinct: false,
        }) => {
            mir.syms
                .resolve(*name)
                .is_some_and(|n| n.eq_ignore_ascii_case("date"))
                && matches!(args.as_slice(), [arg] if is_col(*arg))
        }
        Some(ScalarKind::Cast {
            e,
            ty: SqlType::Date,
        }) => is_col(*e),
        _ => false,
    }
}

/// The value of a `DATE '…'` literal that is safe to quote back.
fn date_literal(mir: &Mir, id: ScalarId) -> Option<&str> {
    let ScalarKind::Lit(Lit::Typed { ty, value }) = mir.scalars.get(id)?.kind else {
        return None;
    };
    if !mir.syms.resolve(ty)?.eq_ignore_ascii_case("date") {
        return None;
    }
    let text = mir.syms.resolve(value)?;
    let safe = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit() || b == b'-');
    safe.then_some(text)
}
