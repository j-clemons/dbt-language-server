//! HIR invariants (specs/ir-pipeline/00). `text` is the document the
//! spans index. Order is fixed so reports are deterministic.
//!
//! Id range runs first and stops the run, since the other checks follow
//! ids. Every other check is total and reports through `Violation`.

mod ids;
mod structure;
#[cfg(test)]
mod tests;

use super::Hir;
use crate::Violation;

/// Returns every violation found; empty means the HIR is clean.
pub fn verify_hir(hir: &Hir, text: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    ids::check(hir, &mut out);
    if !out.is_empty() {
        return out;
    }
    structure::scope_tree(hir, &mut out);
    structure::col_refs_resolved(hir, &mut out);
    structure::cte_backward(hir, &mut out);
    structure::jinja_consumed(hir, &mut out);
    structure::spans(hir, text, &mut out);
    out
}
