//! The pass manager: skipped needs, determinism and degraded input.

use cogitator_ir::common::{BinaryOp, RuleId};
use cogitator_ir::hir::Hir;
use cogitator_ir::lir::Lir;
use cogitator_ir::mir::{print_mir, Mir, RelId, RelOp, ScalarKind};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::testkit::{int_col, LirBuilder, MirBuilder};
use cogitator_ir::{Level, VerifyMode};

use super::budget::Budgets;
use super::manager::{run_passes, LirLowering};
use super::test_rules::*;
use super::trace::{Outcome, Phase, SkipReason, Snapshot};
use crate::cx::Cx;
use crate::meta::{Need, Needs, RuleMeta};
use crate::registry::Registered;
use crate::rule::Canonicalize;

#[test]
fn unmet_needs_are_recorded_never_silent() {
    static RULES: &[(&RuleMeta, &dyn Canonicalize)] = &[(&NEEDY_META, &Needy)];
    let mut registry = canon_only(RULES);
    registry.push(Registered {
        meta: &FIND_NEEDY_META,
        mir: Some(&FindNeedy),
        ..EMPTY
    });
    let out = run(
        &registry,
        scan_plan(),
        VerifyMode::EveryRewrite,
        Budgets::STANDARD,
    );
    let needy = out
        .trace
        .entries
        .iter()
        .find(|e| e.rule == RuleId("test.needy"))
        .expect("trace entry");
    assert_eq!(
        needy.outcome,
        Outcome::Skipped {
            reason: SkipReason::Unmet(Need::Schema)
        }
    );
    let find = out
        .trace
        .entries
        .iter()
        .find(|e| e.rule == RuleId("test.find_needy"))
        .expect("trace entry");
    assert_eq!(
        find.outcome,
        Outcome::Skipped {
            reason: SkipReason::Unmet(Need::Dag)
        }
    );
    assert!(out.findings.is_empty());
}

#[test]
fn every_real_rule_is_traced_with_its_needs() {
    let out = run_passes(
        input(scan_plan(), VerifyMode::EveryRewrite, Budgets::STANDARD),
        &Cx::new(Needs::NONE),
    );
    let star = out
        .trace
        .entries
        .iter()
        .find(|e| e.rule == RuleId("cross.select_star_chain"))
        .expect("traced");
    assert!(matches!(
        star.outcome,
        Outcome::Skipped {
            reason: SkipReason::Unmet(_)
        }
    ));
    // hir rules with no HIR supplied are skipped, not dropped
    assert!(out.trace.entries.iter().any(|e| e.phase == Phase::DetectHir
        && e.outcome
            == Outcome::Skipped {
                reason: SkipReason::Unavailable(Level::Hir)
            }));
}

#[test]
fn findings_come_out_sorted_and_runs_are_deterministic() {
    let registry = vec![Registered {
        meta: &FIND_META,
        mir: Some(&Find),
        ..EMPTY
    }];
    let plan = || where_one_eq_one();
    let run_once = || {
        run(
            &registry,
            plan(),
            VerifyMode::EveryRewrite,
            Budgets::STANDARD,
        )
    };
    let (a, b) = (run_once(), run_once());
    assert_eq!(a.findings, b.findings);
    assert_eq!(a.trace, b.trace);
    assert_eq!(print_mir(&a.mir), print_mir(&b.mir));
    assert!(a.findings[0].span.start < a.findings[1].span.start);

    let once = || {
        run_passes(
            input(plan(), VerifyMode::EveryRewrite, Budgets::STANDARD),
            &Cx::new(Needs::NONE),
        )
    };
    let (x, y) = (once(), once());
    assert_eq!(x.trace, y.trace);
    assert_eq!(print_mir(&x.mir), print_mir(&y.mir));
}

#[test]
fn a_shared_scalar_plan_cannot_blow_the_budget() {
    // 2^64 expanded nodes from 66 arena nodes: walking it would not end
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let a = b.out(scan)[0];
    let mut s = b.col_ref(a);
    for _ in 0..64 {
        s = b.binary(BinaryOp::Plus, s, s);
    }
    let filter = b.filter(scan, vec![s]);
    let out = run_passes(
        input(
            b.finish(filter),
            VerifyMode::EveryRewrite,
            Budgets::STANDARD,
        ),
        &Cx::new(Needs::NONE),
    );
    assert!(out.trace.incomplete);
    assert!(out
        .trace
        .entries
        .iter()
        .filter(|e| e.phase == Phase::Canonicalize)
        .all(|e| e.outcome
            == Outcome::Skipped {
                reason: SkipReason::Budget
            }));
    assert!(out.findings.is_empty());
}

#[test]
fn an_invalid_plan_skips_rewriting_and_detection_without_panicking() {
    let mut mir = scan_plan();
    mir.root = RelId::from_raw(500);
    let out = run_passes(
        input(mir, VerifyMode::EveryRewrite, Budgets::STANDARD),
        &Cx::new(Needs::NONE),
    );
    assert!(out.trace.incomplete);
    assert!(out
        .trace
        .entries
        .iter()
        .filter(|e| e.phase != Phase::DetectHir)
        .all(|e| matches!(e.outcome, Outcome::Skipped { .. })));
}

#[test]
fn snapshots_only_when_requested() {
    let run_with_snapshots = |snapshots| {
        let mut pass_input = input(
            where_one_eq_one(),
            VerifyMode::EveryRewrite,
            Budgets::STANDARD,
        );
        pass_input.snapshots = snapshots;
        run_passes(pass_input, &Cx::new(Needs::NONE))
    };
    assert!(run_with_snapshots(false).trace.snapshots.is_empty());
    let snapshots = run_with_snapshots(true).trace.snapshots;
    assert_eq!(snapshots[0].rule, Snapshot::INPUT);
    assert!(snapshots[0].text.contains("Filter"));
    assert!(!snapshots.last().expect("rewritten").text.contains("Filter"));
}

#[test]
fn a_stale_lir_is_not_detected_on() {
    let mir = where_one_eq_one();
    let scan = RelId::from_raw(0);
    let mut lb = LirBuilder::new();
    let phys = lb.scan(&mir, scan);
    let lir = lb.finish(phys);
    let mut pass_input = input(mir, VerifyMode::EveryRewrite, Budgets::STANDARD);
    pass_input.lir = Some(lir);
    let out = run_passes(pass_input, &Cx::new(Needs::NONE));
    assert!(out.lir.is_none());
    assert!(out.trace.entries.iter().any(|e| e.phase == Phase::DetectLir
        && e.outcome
            == Outcome::Skipped {
                reason: SkipReason::Unavailable(Level::Lir)
            }));
}

#[test]
fn the_lowering_hook_sees_the_canonical_plan() {
    let lower = |mir: &Mir| -> Option<Lir> {
        assert!(matches!(mir.rels[mir.root].op, RelOp::Scan { .. }));
        let mut lb = LirBuilder::new();
        let phys = lb.scan(mir, mir.root);
        Some(lb.finish(phys))
    };
    let mut pass_input = input(
        where_one_eq_one(),
        VerifyMode::EveryRewrite,
        Budgets::STANDARD,
    );
    pass_input.lower_lir = Some(LirLowering(&lower));
    let out = run_passes(pass_input, &Cx::new(Needs::NONE));
    assert!(out.lir.is_some());
}

#[test]
fn hir_detect_runs_even_if_the_mir_is_bad() {
    let hir = Hir::new(SourceOrigin::Template {
        variant: TemplateVariant::Plain,
    });
    let mut mir = scan_plan();
    mir.root = RelId::from_raw(500);
    let mut pass_input = input(mir, VerifyMode::EveryRewrite, Budgets::STANDARD);
    pass_input.hir = Some(&hir);
    let out = run_passes(pass_input, &Cx::new(Needs::NONE));
    assert!(out.trace.entries.iter().any(|e| e.phase == Phase::DetectHir
        && !matches!(
            e.outcome,
            Outcome::Skipped {
                reason: SkipReason::Unavailable(_)
            }
        )));
}

#[test]
fn an_invisible_column_fails_entry_verification() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let other = b.col("x", int_col());
    let bad = b.scalar(ScalarKind::Col(other), int_col());
    let filter = b.filter(scan, vec![bad]);
    let out = run_passes(
        input(
            b.finish(filter),
            VerifyMode::EveryRewrite,
            Budgets::STANDARD,
        ),
        &Cx::new(Needs::NONE),
    );
    assert!(out.trace.incomplete);
}
