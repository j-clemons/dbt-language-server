//! `canon.required_columns`: narrows each scan to the columns the plan
//! reads (DuckDB `remove_unused_columns.cpp`, DataFusion
//! `optimize_projections/`, both pinned in specs/ir-pipeline/05).
//!
//! ColIds are stable from definition to output, so "required" is one set
//! over the whole plan, grown from the root and every expression that
//! still runs. An `open` scan stays as it was: its column list is not
//! the table's, so narrowing it would say nothing true.

use std::collections::BTreeSet;

use cogitator_ir::mir::{ColId, Mir, RelId, RelOp, ScalarId, ScalarKind};
use cogitator_ir::RuleId;

use super::rebuild::{self, no_fold, Rebuilder};
use crate::cx::Cx;
use crate::meta::{FixSafety, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Canonicalize, Rewrite, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("canon.required_columns"),
    level: RuleLevel::Mir,
    class: RuleClass::Canon,
    needs: Needs::NONE,
    severity: Severity::Off,
    fix: FixSafety::None,
};

/// A CTE body can follow its reference in id order, which takes another
/// round to settle. Past this the plan is left alone, not half-pruned.
const MAX_ROUNDS: u32 = 16;

pub struct RequiredColumns;

impl Rule for RequiredColumns {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Canonicalize for RequiredColumns {
    fn apply(&self, ir: Mir, _cx: &Cx<'_>) -> Rewrite<Mir> {
        let Some(required) = required_columns(&ir) else {
            return Rewrite::Unchanged(ir);
        };
        rebuild::run(ir, META.id, no_fold, |rb, mir, old, op| {
            narrow(rb, mir, old, op, &required)
        })
    }
}

fn narrow(rb: &mut Rebuilder, mir: &mut Mir, old: RelId, op: RelOp, required: &Required) -> RelId {
    let op = match op {
        RelOp::Scan {
            table,
            mut cols,
            open: false,
        } => {
            cols.retain(|c| required.has(*c));
            RelOp::Scan {
                table,
                cols,
                open: false,
            }
        }
        RelOp::Project { input, mut items } => {
            items.retain(|(c, s)| required.has(*c) || has_opaque(mir, *s));
            RelOp::Project { input, items }
        }
        other => other,
    };
    rb.emit(mir, old, op)
}

struct Required {
    cols: Vec<bool>,
    changed: bool,
}

impl Required {
    fn has(&self, col: ColId) -> bool {
        self.cols.get(col.index()).copied().unwrap_or(true)
    }

    fn add(&mut self, col: ColId) {
        if let Some(slot) = self.cols.get_mut(col.index()) {
            self.changed |= !std::mem::replace(slot, true);
        }
    }

    fn add_all(&mut self, cols: &[ColId]) {
        cols.iter().for_each(|c| self.add(*c));
    }
}

fn required_columns(mir: &Mir) -> Option<Required> {
    let reachable = mir.reachable();
    let mut required = Required {
        cols: vec![false; mir.cols.len()],
        changed: false,
    };
    required.add_all(mir.root_columns());
    for _ in 0..MAX_ROUNDS {
        required.changed = false;
        // users before operands, so a Project knows what is asked of it
        for &id in reachable.iter().rev() {
            visit(mir, id, &mut required)?;
        }
        if !required.changed {
            return Some(required);
        }
    }
    None
}

fn visit(mir: &Mir, id: RelId, required: &mut Required) -> Option<()> {
    let rel = mir.rels.get(id)?;
    let input_cols = |required: &mut Required| {
        for input in rel.op.inputs() {
            if let Some(input) = mir.rels.get(input) {
                required.add_all(&input.out);
            }
        }
    };
    match &rel.op {
        RelOp::Project { items, .. } => {
            for (col, scalar) in items {
                if required.has(*col) || has_opaque(mir, *scalar) {
                    read_scalar(mir, *scalar, required)?;
                }
            }
        }
        RelOp::Distinct(_) | RelOp::SetOp { .. } | RelOp::Opaque { .. } => {
            // duplicates compare every column; a hole may read any
            input_cols(required);
            required.add_all(&rel.out);
        }
        RelOp::CteRef { cte, out } => {
            let def = mir.ctes.get(*cte)?;
            for (reference, inner) in out.iter().zip(&def.out) {
                if required.has(*reference) {
                    required.add(*inner);
                }
            }
        }
        RelOp::Scan { .. } => {}
        op => {
            for scalar in op.scalars() {
                read_scalar(mir, scalar, required)?;
            }
        }
    }
    // an opaque expression may read any column of its input
    if rel.op.scalars().iter().any(|s| has_opaque(mir, *s)) {
        input_cols(required);
    }
    Some(())
}

/// Marks the columns a scalar reads, including correlated columns and
/// the full output of any subquery under it.
fn read_scalar(mir: &Mir, root: ScalarId, required: &mut Required) -> Option<()> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let node = &mir.scalars.get(id)?.kind;
        match node {
            ScalarKind::Col(c) | ScalarKind::Outer(c) => required.add(*c),
            ScalarKind::Subquery {
                rel, correlated, ..
            } => {
                required.add_all(correlated);
                required.add_all(&mir.rels.get(*rel)?.out);
            }
            _ => {}
        }
        stack.extend(node.children());
    }
    Some(())
}

fn has_opaque(mir: &Mir, root: ScalarId) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match mir.scalars.get(id).map(|s| &s.kind) {
            None | Some(ScalarKind::Opaque { .. }) => return true,
            Some(node) => stack.extend(node.children()),
        }
    }
    false
}
