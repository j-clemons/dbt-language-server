//! Shared helpers for the canonicaliser tests.

use cogitator_ir::mir::{print_mir, verify_mir, verify_rewrite, Mir};
use cogitator_ir::{assert_clean, ColType, Nullability, SqlType};

use crate::cx::Cx;
use crate::meta::Needs;
use crate::rule::{Canonicalize, Rewrite};

pub(super) fn bool_col() -> ColType {
    ColType::new(SqlType::Bool, Nullability::Unknown)
}

/// Applies `rule`, verifies the rewrite, and renders before and after.
pub(super) fn apply(rule: &dyn Canonicalize, mir: Mir) -> (Mir, String, bool) {
    assert_clean(&verify_mir(&mir, None));
    let before_root = mir.root;
    let before = print_mir(&mir);
    let result = rule.apply(mir, &Cx::new(Needs::NONE));
    let changed = result.changed();
    let mir = result.into_inner();
    assert_clean(&verify_mir(&mir, None));
    assert_clean(&verify_rewrite(&mir, before_root));
    let text = format!("before:\n{before}after:\n{}", print_mir(&mir));
    (mir, text, changed)
}

/// Applying `rule` again must be a no-op.
pub(super) fn assert_stable(rule: &dyn Canonicalize, mir: Mir) {
    let root = mir.root;
    let out = rule.apply(mir, &Cx::new(Needs::NONE));
    assert!(matches!(out, Rewrite::Unchanged(_)), "not at fixpoint");
    assert_eq!(out.into_inner().root, root);
}
