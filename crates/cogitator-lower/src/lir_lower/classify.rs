//! Classifies one predicate by how it uses a pruning key.

use cogitator_ir::common::BinaryOp;
use cogitator_ir::lir::PredShape;
use cogitator_ir::mir::{ColId, Mir, ScalarId, ScalarKind};

use super::prune::PredDetail;

pub(super) struct Classified {
    pub(super) shape: PredShape,
    pub(super) detail: PredDetail,
}

fn shape(shape: PredShape) -> Option<Classified> {
    Some(Classified {
        shape,
        detail: PredDetail::None,
    })
}

pub(super) fn walk(mir: &Mir, root: ScalarId, f: &mut impl FnMut(&ScalarKind)) {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if let Some(scalar) = mir.scalars.get(id) {
            f(&scalar.kind);
            stack.extend(scalar.kind.children());
        }
    }
}

fn reads(mir: &Mir, root: ScalarId, col: ColId) -> bool {
    let mut hit = false;
    walk(mir, root, &mut |kind| match kind {
        ScalarKind::Col(c) | ScalarKind::Outer(c) => hit |= *c == col,
        ScalarKind::Subquery { correlated, .. } => hit |= correlated.contains(&col),
        _ => {}
    });
    hit
}

fn reads_other_column(mir: &Mir, root: ScalarId, col: ColId) -> bool {
    let mut hit = false;
    walk(mir, root, &mut |kind| {
        if let ScalarKind::Col(c) | ScalarKind::Outer(c) = kind {
            hit |= *c != col;
        }
    });
    hit
}

fn has_subquery(mir: &Mir, root: ScalarId) -> bool {
    let mut hit = false;
    walk(mir, root, &mut |kind| {
        hit |= matches!(kind, ScalarKind::Subquery { .. });
    });
    hit
}

pub(super) fn classify(mir: &Mir, id: ScalarId, col: ColId) -> Option<Classified> {
    if !reads(mir, id, col) {
        return None;
    }
    match &mir.scalars.get(id)?.kind {
        ScalarKind::Binary {
            op: BinaryOp::Or,
            l,
            r,
        } => classify_or(mir, *l, *r, col),
        ScalarKind::Binary { op, l, r } if is_comparison(*op) => {
            classify_compare(mir, *op, *l, *r, col)
        }
        ScalarKind::InList { e, list, .. } => {
            let (mut set_subquery, mut set_col) = (false, false);
            for item in list {
                set_subquery |= has_subquery(mir, *item);
                set_col |= reads(mir, *item, col) || reads_other_column(mir, *item, col);
            }
            if set_subquery {
                shape(PredShape::Subquery)
            } else if set_col {
                Some(Classified {
                    shape: PredShape::Other,
                    detail: PredDetail::ComparedToColumn,
                })
            } else {
                classify_side(mir, *e, col, PredShape::BareCompare)
            }
        }
        ScalarKind::Subquery { .. } => shape(PredShape::Subquery),
        _ => shape(PredShape::Other),
    }
}

fn is_comparison(op: BinaryOp) -> bool {
    use BinaryOp as B;
    matches!(op, B::Eq | B::NotEq | B::Lt | B::LtEq | B::Gt | B::GtEq)
}

fn classify_or(mir: &Mir, l: ScalarId, r: ScalarId, col: ColId) -> Option<Classified> {
    if reads_other_column(mir, l, col) || reads_other_column(mir, r, col) {
        return shape(PredShape::OrAcrossColumns);
    }
    let prunable = |side: ScalarId| {
        classify(mir, side, col)
            .is_some_and(|c| matches!(c.shape, PredShape::BareCompare | PredShape::Range))
    };
    if prunable(l) && prunable(r) {
        shape(PredShape::Range)
    } else {
        shape(PredShape::Other)
    }
}

fn classify_compare(
    mir: &Mir,
    op: BinaryOp,
    l: ScalarId,
    r: ScalarId,
    col: ColId,
) -> Option<Classified> {
    let (side, other) = match (reads(mir, l, col), reads(mir, r, col)) {
        (true, false) => (l, r),
        (false, true) => (r, l),
        _ => return shape(PredShape::Other),
    };
    if has_subquery(mir, other) {
        return shape(PredShape::Subquery);
    }
    if reads_other_column(mir, other, col) {
        return Some(Classified {
            shape: PredShape::Other,
            detail: PredDetail::ComparedToColumn,
        });
    }
    let bare = match op {
        BinaryOp::Eq => PredShape::BareCompare,
        BinaryOp::NotEq => PredShape::Other,
        _ => PredShape::Range,
    };
    classify_side(mir, side, col, bare)
}

/// How the side holding `col` wraps it.
fn classify_side(mir: &Mir, side: ScalarId, col: ColId, bare: PredShape) -> Option<Classified> {
    match &mir.scalars.get(side)?.kind {
        ScalarKind::Col(c) if *c == col => shape(bare),
        ScalarKind::Cast { e, .. } if is_col(mir, *e, col) => shape(PredShape::Cast),
        ScalarKind::Call { name, .. } => shape(PredShape::Function { name: *name }),
        ScalarKind::Binary { op, .. } if !is_comparison(*op) => Some(Classified {
            shape: PredShape::Other,
            detail: PredDetail::ArithmeticOnColumn,
        }),
        _ => shape(PredShape::Other),
    }
}

fn is_col(mir: &Mir, id: ScalarId, col: ColId) -> bool {
    matches!(mir.scalars.get(id).map(|s| &s.kind), Some(ScalarKind::Col(c)) if *c == col)
}
