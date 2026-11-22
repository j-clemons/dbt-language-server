//! How one conjunct uses a pruning key, in the terms a finding states.
//!
//! The backend's verdict says whether a key is pruned; this says which
//! form of predicate earned it, so the message can name it.

use cogitator_ir::common::BinaryOp;
use cogitator_ir::mir::{ColId, Mir, ScalarId, ScalarKind};

use super::support::{any_scalar, reads_col, touches_opaque};

/// How the side holding the column treats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Use<'a> {
    Bare,
    Call(&'a str),
    Cast,
    Arithmetic,
    /// A bare column compared with a different column.
    ComparedToColumn,
}

pub(super) fn is_comparison(op: BinaryOp) -> bool {
    use BinaryOp as B;
    matches!(op, B::Eq | B::NotEq | B::Lt | B::LtEq | B::Gt | B::GtEq)
}

/// `None` when the conjunct supports no statement about `col`: it holds a
/// hole or a subquery, is not a comparison, or reads `col` on both sides.
pub(super) fn use_of<'a>(mir: &'a Mir, conjunct: ScalarId, col: ColId) -> Option<Use<'a>> {
    if touches_opaque(mir, conjunct) || has_subquery(mir, conjunct) {
        return None;
    }
    match &mir.scalars.get(conjunct)?.kind {
        ScalarKind::Binary { op, l, r } if is_comparison(*op) => {
            match (reads_col(mir, *l, col), reads_col(mir, *r, col)) {
                (true, false) => side_use(mir, *l, *r, col),
                (false, true) => side_use(mir, *r, *l, col),
                _ => None,
            }
        }
        ScalarKind::InList { e, list, .. } => {
            let list_reads = list.iter().any(|item| reads_col(mir, *item, col));
            if list_reads || !reads_col(mir, *e, col) {
                return None;
            }
            match list.first() {
                Some(item) => side_use(mir, *e, *item, col),
                None => None,
            }
        }
        _ => None,
    }
}

fn side_use<'a>(mir: &'a Mir, side: ScalarId, other: ScalarId, col: ColId) -> Option<Use<'a>> {
    match &mir.scalars.get(side)?.kind {
        ScalarKind::Col(c) if *c == col => {
            let other_col = any_scalar(mir, other, |k| {
                matches!(k, ScalarKind::Col(_) | ScalarKind::Outer(_))
            });
            Some(if other_col {
                Use::ComparedToColumn
            } else {
                Use::Bare
            })
        }
        ScalarKind::Call { name, .. } => mir.syms.resolve(*name).map(Use::Call),
        ScalarKind::Cast { .. } => Some(Use::Cast),
        ScalarKind::Binary { op, .. } if is_arithmetic(*op) => Some(Use::Arithmetic),
        _ => None,
    }
}

fn is_arithmetic(op: BinaryOp) -> bool {
    use BinaryOp as B;
    matches!(op, B::Plus | B::Minus | B::Mul | B::Div | B::Mod)
}

pub(super) fn has_subquery(mir: &Mir, root: ScalarId) -> bool {
    any_scalar(mir, root, |k| matches!(k, ScalarKind::Subquery { .. }))
}
