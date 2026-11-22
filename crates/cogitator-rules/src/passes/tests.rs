//! The pass manager: fixpoint, budgets and rollback.

use cogitator_ir::mir::{plan_hash, RelOp};
use cogitator_ir::{Invariant, VerifyMode};

use super::budget::Budgets;
use super::manager::run_passes;
use super::test_rules::*;
use super::trace::{Outcome, Phase, SkipReason};
use crate::cx::Cx;
use crate::meta::{Needs, RuleMeta};
use crate::registry::Registered;
use crate::rule::Canonicalize;

#[test]
fn the_real_registry_reaches_a_fixpoint() {
    let out = run_passes(
        input(
            where_one_eq_one(),
            VerifyMode::EveryRewrite,
            Budgets::STANDARD,
        ),
        &Cx::new(Needs::NONE),
    );
    assert!(!out.trace.incomplete, "{:#?}", out.trace);
    assert!(matches!(out.mir.rels[out.mir.root].op, RelOp::Scan { .. }));
    let canon = outcomes(&out, Phase::Canonicalize);
    assert_eq!(
        canon.first().map(|e| e.outcome.clone()),
        Some(Outcome::Applied)
    );
    assert!(
        canon
            .iter()
            .rev()
            .take(3)
            .all(|e| e.outcome == Outcome::NoChange),
        "the last pass must change nothing: {canon:#?}"
    );
}

#[test]
fn a_rule_that_never_settles_stops_at_the_pass_budget() {
    static RULES: &[(&RuleMeta, &dyn Canonicalize)] = &[(&GROW_META, &Grow)];
    let out = run(
        &canon_only(RULES),
        scan_plan(),
        VerifyMode::EveryRewrite,
        Budgets::STANDARD,
    );
    assert!(out.trace.incomplete);
    let canon = outcomes(&out, Phase::Canonicalize);
    assert_eq!(canon.len(), 9);
    assert_eq!(
        canon.last().map(|e| e.outcome.clone()),
        Some(Outcome::Skipped {
            reason: SkipReason::Budget
        })
    );
}

#[test]
fn a_repeating_plan_ends_the_fixpoint() {
    static RULES: &[(&RuleMeta, &dyn Canonicalize)] = &[(&FLIP_META, &Flip)];
    let out = run(
        &canon_only(RULES),
        scan_plan(),
        VerifyMode::EveryRewrite,
        Budgets::STANDARD,
    );
    assert!(!out.trace.incomplete, "{:#?}", out.trace);
    assert_eq!(outcomes(&out, Phase::Canonicalize).len(), 1);
}

#[test]
fn node_budget_stops_rewriting_but_not_detection() {
    static RULES: &[(&RuleMeta, &dyn Canonicalize)] = &[(&GROW_META, &Grow)];
    let mut registry = canon_only(RULES);
    registry.push(Registered {
        meta: &FIND_META,
        mir: Some(&Find),
        ..EMPTY
    });
    let budgets = Budgets {
        max_nodes: 3,
        ..Budgets::STANDARD
    };
    let out = run(&registry, scan_plan(), VerifyMode::EveryRewrite, budgets);
    assert!(out.trace.incomplete);
    assert_eq!(out.findings.len(), 2, "detection still runs");
}

#[test]
fn application_budget_is_enforced() {
    static RULES: &[(&RuleMeta, &dyn Canonicalize)] = &[(&GROW_META, &Grow)];
    let budgets = Budgets {
        max_applications: 2,
        ..Budgets::STANDARD
    };
    let out = run(
        &canon_only(RULES),
        scan_plan(),
        VerifyMode::EveryRewrite,
        budgets,
    );
    assert!(out.trace.incomplete);
    let applied = outcomes(&out, Phase::Canonicalize)
        .iter()
        .filter(|e| e.outcome == Outcome::Applied)
        .count();
    assert_eq!(applied, 2);
}

#[test]
fn a_bad_rewrite_is_rolled_back_and_later_rules_still_run() {
    static RULES: &[(&RuleMeta, &dyn Canonicalize)] = &[(&BAD_META, &Bad), (&GROW_META, &Grow)];
    let original = scan_plan();
    let hash = plan_hash(&original);
    let budgets = Budgets {
        max_passes: 1,
        ..Budgets::STANDARD
    };
    let out = run(
        &canon_only(RULES),
        original,
        VerifyMode::EveryRewrite,
        budgets,
    );
    let canon = outcomes(&out, Phase::Canonicalize);
    assert_eq!(
        canon[0].outcome,
        Outcome::RolledBack {
            invariants: vec![Invariant::MirRootColumns]
        }
    );
    assert_eq!(canon[1].outcome, Outcome::Applied);
    // Grow's Distinct is built over the restored scan, not the bad one
    let RelOp::Distinct(input) = out.mir.rels[out.mir.root].op else {
        panic!("expected the grown plan");
    };
    let mut restored = out.mir.clone();
    restored.root = input;
    assert_eq!(plan_hash(&restored), hash);
}

#[test]
fn boundary_mode_rolls_back_the_whole_phase() {
    // nothing is verified between rewrites, so Grow builds on Bad's scan
    static RULES: &[(&RuleMeta, &dyn Canonicalize)] = &[(&BAD_META, &Bad), (&GROW_META, &Grow)];
    let original = scan_plan();
    let root = original.root;
    let out = run(
        &canon_only(RULES),
        original,
        VerifyMode::Boundaries,
        Budgets {
            max_passes: 1,
            ..Budgets::STANDARD
        },
    );
    assert_eq!(out.mir.root, root);
    let canon = outcomes(&out, Phase::Canonicalize);
    assert!(matches!(canon[0].outcome, Outcome::Applied), "{canon:#?}");
    assert_eq!(
        canon.last().map(|e| e.outcome.clone()),
        Some(Outcome::RolledBack {
            invariants: vec![Invariant::MirRootColumns]
        })
    );
}
