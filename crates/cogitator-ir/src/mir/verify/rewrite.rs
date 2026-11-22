//! The rewrite-preservation invariants, applied each time `root` moves.

use crate::mir::{ColId, Mir, RelId};
use crate::{Invariant, Violation};

/// Compares the plan at `mir.root` against the plan at `before`. Root
/// columns keep their order and names; ColIds may differ (a rewrite can
/// introduce a fresh one) but types may only get more specific.
pub fn verify_rewrite(mir: &Mir, before: RelId) -> Vec<Violation> {
    let mut out = Vec::new();
    let (Some(old), Some(new)) = (mir.rels.get(before), mir.rels.get(mir.root)) else {
        out.push(Violation::new(
            Invariant::MirRootColumns,
            "rewrite endpoints are out of range",
        ));
        return out;
    };
    if old.out.len() != new.out.len() {
        out.push(Violation::new(
            Invariant::MirRootColumns,
            format!(
                "root width changed from {} to {}",
                old.out.len(),
                new.out.len()
            ),
        ));
        return out;
    }
    for (pos, (&a, &b)) in old.out.iter().zip(new.out.iter()).enumerate() {
        compare_column(mir, pos, a, b, &mut out);
    }
    out
}

fn compare_column(mir: &Mir, pos: usize, old: ColId, new: ColId, out: &mut Vec<Violation>) {
    let (Some(old_def), Some(new_def)) = (mir.cols.get(old), mir.cols.get(new)) else {
        out.push(Violation::new(
            Invariant::MirRootColumns,
            format!("root column {pos} refers to a missing definition"),
        ));
        return;
    };
    if old_def.name != new_def.name {
        out.push(Violation::new(
            Invariant::MirRootColumns,
            format!("root column {pos} was renamed"),
        ));
    }
    if !new_def.ty.refines(&old_def.ty) {
        out.push(Violation::new(
            Invariant::MirTypeSpecificity,
            format!(
                "root column {pos} widened from {:?} to {:?}",
                old_def.ty, new_def.ty
            ),
        ));
    }
}
