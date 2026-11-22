//! Fixture rules and plans for the pass-manager tests.

use cogitator_ir::common::{BinaryOp, RuleId};
use cogitator_ir::mir::{Mir, RelOp};
use cogitator_ir::testkit::{int_col, MirBuilder};
use cogitator_ir::{Level, Span, VerifyMode};

use super::budget::Budgets;
use super::manager::{run_with, PassInput, PassOutput};
use super::trace::{Phase, TraceEntry};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::registry::Registered;
use crate::rule::{Canonicalize, Detect, Rewrite, Rule};

const fn meta(id: &'static str, class: RuleClass, needs: Needs) -> RuleMeta {
    RuleMeta {
        id: RuleId(id),
        level: RuleLevel::Mir,
        class,
        needs,
        severity: Severity::Hint,
        fix: FixSafety::None,
    }
}

pub(super) static GROW_META: RuleMeta = meta("test.grow", RuleClass::Canon, Needs::NONE);
pub(super) static FLIP_META: RuleMeta = meta("test.flip", RuleClass::Canon, Needs::NONE);
pub(super) static BAD_META: RuleMeta = meta("test.bad", RuleClass::Canon, Needs::NONE);
pub(super) static NEEDY_META: RuleMeta = meta(
    "test.needy",
    RuleClass::Canon,
    Needs::NONE.with(Need::Schema),
);
pub(super) static FIND_META: RuleMeta = meta("test.find", RuleClass::Advice, Needs::NONE);
pub(super) static FIND_NEEDY_META: RuleMeta = meta(
    "test.find_needy",
    RuleClass::Advice,
    Needs::NONE.with(Need::Dag),
);

/// Wraps the root in a new Distinct each visit: a plan that never settles.
pub(super) struct Grow;
/// Alternates between two equal-shaped plans: the hash repeats.
pub(super) struct Flip;
/// Drops a root column, so verification rejects it.
pub(super) struct Bad;
pub(super) struct Needy;
pub(super) struct Find;

impl Rule for Grow {
    fn meta(&self) -> &'static RuleMeta {
        &GROW_META
    }
}
impl Rule for Flip {
    fn meta(&self) -> &'static RuleMeta {
        &FLIP_META
    }
}
impl Rule for Bad {
    fn meta(&self) -> &'static RuleMeta {
        &BAD_META
    }
}
impl Rule for Needy {
    fn meta(&self) -> &'static RuleMeta {
        &NEEDY_META
    }
}
impl Rule for Find {
    fn meta(&self) -> &'static RuleMeta {
        &FIND_META
    }
}

impl Canonicalize for Grow {
    fn apply(&self, mut ir: Mir, _cx: &Cx<'_>) -> Rewrite<Mir> {
        let prov = ir.rels[ir.root].prov;
        let op = RelOp::Distinct(ir.root);
        ir.root = ir.add_rel(op, prov);
        Rewrite::Changed(ir)
    }
}

impl Canonicalize for Flip {
    fn apply(&self, mut ir: Mir, _cx: &Cx<'_>) -> Rewrite<Mir> {
        // the plan's root flips between its two rels: same text
        let op = ir.rels[ir.root].op.clone();
        let prov = ir.rels[ir.root].prov;
        ir.root = ir.add_rel(op, prov);
        Rewrite::Changed(ir)
    }
}

impl Canonicalize for Bad {
    fn apply(&self, mut ir: Mir, _cx: &Cx<'_>) -> Rewrite<Mir> {
        let prov = ir.rels[ir.root].prov;
        let RelOp::Scan { table, .. } = ir.rels[ir.root].op else {
            return Rewrite::Unchanged(ir);
        };
        ir.root = ir.add_rel(
            RelOp::Scan {
                table,
                cols: Vec::new(),
                open: false,
            },
            prov,
        );
        Rewrite::Changed(ir)
    }
}

impl Canonicalize for Needy {
    fn apply(&self, ir: Mir, _cx: &Cx<'_>) -> Rewrite<Mir> {
        Rewrite::Unchanged(ir)
    }
}

impl Detect<Mir> for Find {
    fn check(&self, ir: &Mir, _cx: &Cx<'_>, out: &mut Findings) {
        for start in [9u32, 1] {
            out.push(Finding {
                id: RuleId("test.find"),
                span: Span::new(start, start + 1),
                level: Level::Mir,
                severity: Severity::Hint,
                message: format!("{} rels", ir.rels.len()),
                evidence: Evidence::Exact,
                fixes: vec![],
            });
        }
    }
}

pub(super) struct FindNeedy;
impl Rule for FindNeedy {
    fn meta(&self) -> &'static RuleMeta {
        &FIND_NEEDY_META
    }
}
impl Detect<Mir> for FindNeedy {
    fn check(&self, _ir: &Mir, _cx: &Cx<'_>, out: &mut Findings) {
        out.push(Finding {
            id: RuleId("test.find_needy"),
            span: Span::new(0, 1),
            level: Level::Mir,
            severity: Severity::Hint,
            message: "never".to_string(),
            evidence: Evidence::Exact,
            fixes: vec![],
        });
    }
}

pub(super) const EMPTY: Registered = Registered {
    meta: &GROW_META,
    canon: None,
    hir: None,
    mir: None,
    lir: None,
};

pub(super) fn canon_only(
    rules: &'static [(&'static RuleMeta, &'static dyn Canonicalize)],
) -> Vec<Registered> {
    rules
        .iter()
        .map(|(meta, rule)| Registered {
            meta,
            canon: Some(*rule),
            ..EMPTY
        })
        .collect()
}

pub(super) fn scan_plan() -> Mir {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    b.finish(scan)
}

pub(super) fn input(mir: Mir, verify: VerifyMode, budgets: Budgets) -> PassInput<'static> {
    PassInput {
        hir: None,
        mir,
        lir: None,
        lower_lir: None,
        verify,
        budgets,
        snapshots: false,
    }
}

pub(super) fn run(
    registry: &[Registered],
    mir: Mir,
    verify: VerifyMode,
    budgets: Budgets,
) -> PassOutput {
    run_with(registry, input(mir, verify, budgets), &Cx::new(Needs::NONE))
}

pub(super) fn outcomes(out: &PassOutput, phase: Phase) -> Vec<&TraceEntry> {
    out.trace
        .entries
        .iter()
        .filter(|e| e.phase == phase)
        .collect()
}

pub(super) fn where_one_eq_one() -> Mir {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let (l, r) = (b.number("1"), b.number("1"));
    let pred = b.binary(BinaryOp::Eq, l, r);
    let filter = b.filter(scan, vec![pred]);
    b.finish(filter)
}
