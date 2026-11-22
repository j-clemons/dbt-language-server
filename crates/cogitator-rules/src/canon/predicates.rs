//! `canon.predicates`: splits conjuncts and moves filters towards the
//! scans. The safety rules live in `pushdown`; this module decides what
//! is offered to it and rebuilds the node that gave the predicates up.

use std::collections::BTreeSet;

use cogitator_ir::common::JoinKind;
use cogitator_ir::mir::{Mir, RelId, RelOp, ScalarId};
use cogitator_ir::RuleId;

use super::join_sides::on_sides;
use super::pred::{self, Pred};
use super::pushdown::Pushdown;
use super::rebuild::{self, no_fold, Rebuilder};
use crate::cx::Cx;
use crate::meta::{FixSafety, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Canonicalize, Rewrite, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("canon.predicates"),
    level: RuleLevel::Mir,
    class: RuleClass::Canon,
    needs: Needs::NONE,
    severity: Severity::Off,
    fix: FixSafety::None,
};

pub struct Predicates;

impl Rule for Predicates {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Canonicalize for Predicates {
    fn apply(&self, ir: Mir, _cx: &Cx<'_>) -> Rewrite<Mir> {
        rebuild::run(ir, META.id, no_fold, step)
    }
}

fn step(rb: &mut Rebuilder, mir: &mut Mir, old: RelId, op: RelOp) -> RelId {
    match op {
        RelOp::Filter { input, conjuncts } => filter(rb, mir, old, input, &conjuncts),
        RelOp::Join { kind, l, r, on } => join(rb, mir, old, kind, l, r, &on),
        op => rb.emit(mir, old, op),
    }
}

fn filter(
    rb: &mut Rebuilder,
    mir: &mut Mir,
    old: RelId,
    input: RelId,
    conjuncts: &[ScalarId],
) -> RelId {
    let split = pred::split_conjuncts(mir, conjuncts);
    let preds = movable(mir, &split);
    let offered: BTreeSet<ScalarId> = preds.iter().map(|p| p.id).collect();
    let (below, stayed) = Pushdown { rb }.push(mir, input, preds, 0);
    let remaining = remaining(&split, &offered, &stayed);
    if remaining.is_empty() {
        return below;
    }
    let op = RelOp::Filter {
        input: below,
        conjuncts: remaining,
    };
    rb.emit(mir, old, op)
}

fn join(
    rb: &mut Rebuilder,
    mir: &mut Mir,
    old: RelId,
    kind: JoinKind,
    l: RelId,
    r: RelId,
    on: &[ScalarId],
) -> RelId {
    let split = pred::split_conjuncts(mir, on);
    let preds = movable(mir, &split);
    let offered: BTreeSet<ScalarId> = preds.iter().map(|p| p.id).collect();
    let (left, right, stayed) = Pushdown { rb }.split_join(mir, l, r, preds, on_sides(kind), 0);
    let op = RelOp::Join {
        kind,
        l: left.node,
        r: right.node,
        on: remaining(&split, &offered, &stayed),
    };
    rb.emit(mir, old, op)
}

fn movable(mir: &Mir, conjuncts: &[ScalarId]) -> Vec<Pred> {
    conjuncts
        .iter()
        .filter_map(|c| pred::inspect(mir, *c))
        .collect()
}

/// `split` minus what moved, in the authored order.
fn remaining(split: &[ScalarId], offered: &BTreeSet<ScalarId>, stayed: &[Pred]) -> Vec<ScalarId> {
    let stayed: BTreeSet<ScalarId> = stayed.iter().map(|p| p.id).collect();
    split
        .iter()
        .copied()
        .filter(|c| !offered.contains(c) || stayed.contains(c))
        .collect()
}
