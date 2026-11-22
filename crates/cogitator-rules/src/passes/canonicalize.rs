//! The canonicalisation phase: rules in registry order, repeated until a
//! pass changes nothing or the plan repeats a stable hash.
//!
//! Rewrites only append and move `root`, so undoing one is resetting
//! `root` to the last verified node.

use std::collections::BTreeSet;

use cogitator_ir::common::RuleId;
use cogitator_ir::hir::Hir;
use cogitator_ir::mir::{plan_hash, print_mir, verify_mir, verify_rewrite, Mir, RelId};
use cogitator_ir::{Invariant, VerifyMode, Violation};

use super::budget::Budgets;
use super::expansion::measure;
use super::trace::{Outcome, Phase, SkipReason, Snapshot, Trace};
use crate::cx::Cx;
use crate::rule::Canonicalize;

pub(super) struct Config<'a> {
    pub hir: Option<&'a Hir>,
    pub verify: VerifyMode,
    pub budgets: Budgets,
    pub snapshots: bool,
}

/// Runs `rules` to fixpoint. `mir` must have passed entry verification.
pub(super) fn run(
    mut mir: Mir,
    rules: &[&'static dyn Canonicalize],
    config: &Config<'_>,
    cx: &Cx<'_>,
    trace: &mut Trace,
) -> Mir {
    if rules.is_empty() {
        return mir;
    }
    if config.snapshots {
        snapshot(&mir, Snapshot::INPUT, 0, trace);
    }
    let mut state = State {
        last_verified: mir.root,
        applications: 0,
        last_applied: None,
    };
    let mut seen = BTreeSet::from([plan_hash(&mir)]);
    let mut pass = 0u32;
    loop {
        let mut changed = false;
        let mut stop = false;
        for rule in rules {
            let (next, visit) = visit(mir, *rule, pass, &mut state, config, cx, trace);
            mir = next;
            changed |= matches!(visit, Visit::Applied);
            stop |= matches!(visit, Visit::Stop);
        }
        if stop || !changed || !seen.insert(plan_hash(&mir)) {
            break;
        }
        pass = pass.saturating_add(1);
    }
    verify_boundary(&mut mir, &state, config, trace);
    mir
}

struct State {
    last_verified: RelId,
    applications: u32,
    /// The rule to report a phase-boundary rollback against.
    last_applied: Option<(RuleId, u32)>,
}

enum Visit {
    Applied,
    Unchanged,
    /// A budget ran out; rewriting ends after this pass.
    Stop,
}

fn visit(
    mut mir: Mir,
    rule: &'static dyn Canonicalize,
    pass: u32,
    state: &mut State,
    config: &Config<'_>,
    cx: &Cx<'_>,
    trace: &mut Trace,
) -> (Mir, Visit) {
    let meta = rule.meta();
    let id = meta.id;
    let phase = Phase::Canonicalize;
    if let Some(reason) = cx.skip_reason(meta) {
        trace.record(id, phase, pass, Outcome::Skipped { reason });
        return (mir, Visit::Unchanged);
    }
    let charged = measure(&mir).charged();
    if config
        .budgets
        .exceeded(pass, charged, state.applications)
        .is_some()
    {
        return (mir, exhausted(id, pass, trace));
    }
    let before = mir.root;
    mir = rule.apply(mir, cx).into_inner();
    if mir.root == before {
        trace.record(id, phase, pass, Outcome::NoChange);
        return (mir, Visit::Unchanged);
    }
    state.applications = state.applications.saturating_add(1);
    if !measure(&mir).walkable() {
        mir.root = state.last_verified;
        return (mir, exhausted(id, pass, trace));
    }
    if config.verify == VerifyMode::EveryRewrite {
        let violations = violations_since(&mir, before, config.hir);
        if !violations.is_empty() {
            mir.root = state.last_verified;
            let outcome = Outcome::RolledBack {
                invariants: invariants_of(&violations),
            };
            trace.record(id, phase, pass, outcome);
            return (mir, Visit::Unchanged);
        }
        state.last_verified = mir.root;
    }
    state.last_applied = Some((id, pass));
    trace.record(id, phase, pass, Outcome::Applied);
    if config.snapshots {
        snapshot(&mir, id, pass, trace);
    }
    (mir, Visit::Applied)
}

fn exhausted(rule: RuleId, pass: u32, trace: &mut Trace) -> Visit {
    trace.incomplete = true;
    let reason = SkipReason::Budget;
    trace.record(rule, Phase::Canonicalize, pass, Outcome::Skipped { reason });
    Visit::Stop
}

/// `Boundaries` mode has verified nothing since entry: a failure here
/// discards the whole phase and is reported against the last rewrite.
fn verify_boundary(mir: &mut Mir, state: &State, config: &Config<'_>, trace: &mut Trace) {
    if config.verify != VerifyMode::Boundaries || mir.root == state.last_verified {
        return;
    }
    let violations = violations_since(mir, state.last_verified, config.hir);
    if violations.is_empty() {
        return;
    }
    mir.root = state.last_verified;
    if let Some((rule, pass)) = state.last_applied {
        let outcome = Outcome::RolledBack {
            invariants: invariants_of(&violations),
        };
        trace.record(rule, Phase::Canonicalize, pass, outcome);
    }
}

fn violations_since(mir: &Mir, before: RelId, hir: Option<&Hir>) -> Vec<Violation> {
    let mut violations = verify_mir(mir, hir);
    if violations.is_empty() {
        violations = verify_rewrite(mir, before);
    }
    violations
}

fn invariants_of(violations: &[Violation]) -> Vec<Invariant> {
    let mut invariants: Vec<Invariant> = violations.iter().map(|v| v.invariant).collect();
    invariants.sort_unstable();
    invariants.dedup();
    invariants
}

fn snapshot(mir: &Mir, rule: RuleId, pass: u32, trace: &mut Trace) {
    trace.snapshots.push(Snapshot {
        rule,
        pass,
        root: mir.root,
        text: print_mir(mir),
    });
}
