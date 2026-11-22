//! The pass driver: canonicalise to fixpoint, then detect at each level
//! (specs/ir-pipeline/04). Phase order is fixed and no output depends on
//! hash iteration, so a plan gives the same result on every run.

use std::fmt;

use cogitator_ir::hir::Hir;
use cogitator_ir::lir::Lir;
use cogitator_ir::mir::{verify_mir, Mir};
use cogitator_ir::{Level, VerifyMode};

use super::budget::Budgets;
use super::canonicalize::{self, Config};
use super::detect;
use super::expansion::measure;
use super::trace::{Outcome, Phase, SkipReason, Trace};
use crate::cx::Cx;
use crate::findings::{Finding, Findings};
use crate::registry::{Registered, REGISTRY};

/// Lowers the canonicalised MIR to LIR. Supplied by the caller because
/// the backends live in `cogitator-lower`, and it verifies the result
/// because only the backend has its capability table.
#[derive(Clone, Copy)]
pub struct LirLowering<'a>(pub &'a dyn Fn(&Mir) -> Option<Lir>);

impl fmt::Debug for LirLowering<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LirLowering")
    }
}

#[derive(Debug)]
pub struct PassInput<'a> {
    pub hir: Option<&'a Hir>,
    pub mir: Mir,
    /// LIR lowered from `mir` as supplied. Canonicalisation moving
    /// `root` makes it stale; prefer `lower_lir`.
    pub lir: Option<Lir>,
    /// When set, replaces `lir`: called on the plan canonicalisation
    /// settled on, so LIR detect sees pushed filters.
    pub lower_lir: Option<LirLowering<'a>>,
    pub verify: VerifyMode,
    pub budgets: Budgets,
    /// Keep a printed plan per rewrite in `Trace::snapshots`.
    pub snapshots: bool,
}

#[derive(Debug)]
pub struct PassOutput {
    pub mir: Mir,
    pub lir: Option<Lir>,
    pub findings: Vec<Finding>,
    pub trace: Trace,
}

pub fn run_passes(input: PassInput<'_>, cx: &Cx<'_>) -> PassOutput {
    run_with(REGISTRY, input, cx)
}

pub(super) fn run_with(registry: &[Registered], input: PassInput<'_>, cx: &Cx<'_>) -> PassOutput {
    let PassInput {
        hir,
        mir,
        lir,
        lower_lir,
        verify,
        budgets,
        snapshots,
    } = input;
    let mut trace = Trace::default();
    let mut findings = Findings::new();
    let entry = check_entry(&mir, hir);
    let entry_root = mir.root;

    let canon: Vec<_> = registry.iter().filter_map(|r| r.canon).collect();
    let mir = match entry {
        Ok(()) => {
            let config = Config {
                hir,
                verify,
                budgets,
                snapshots,
            };
            canonicalize::run(mir, &canon, &config, cx, &mut trace)
        }
        Err(reason) => {
            trace.incomplete = true;
            for rule in &canon {
                let id = rule.meta().id;
                trace.record(id, Phase::Canonicalize, 0, Outcome::Skipped { reason });
            }
            mir
        }
    };

    let hir_rules: Vec<_> = registry
        .iter()
        .filter_map(|r| r.hir.map(|d| (r.meta, d)))
        .collect();
    let hir_level = hir.ok_or(SkipReason::Unavailable(Level::Hir));
    detect::run(
        Phase::DetectHir,
        hir_level,
        &hir_rules,
        cx,
        &mut findings,
        &mut trace,
    );

    let mir_rules: Vec<_> = registry
        .iter()
        .filter_map(|r| r.mir.map(|d| (r.meta, d)))
        .collect();
    let mir_level = entry.map(|()| &mir);
    detect::run(
        Phase::DetectMir,
        mir_level,
        &mir_rules,
        cx,
        &mut findings,
        &mut trace,
    );

    let lir = match (entry, lower_lir) {
        (Err(_), _) => None,
        (Ok(()), Some(lower)) => (lower.0)(&mir),
        (Ok(()), None) if mir.root == entry_root => lir,
        (Ok(()), None) => None,
    };
    let lir_rules: Vec<_> = registry
        .iter()
        .filter_map(|r| r.lir.map(|d| (r.meta, d)))
        .collect();
    let lir_level = match (entry, lir.as_ref()) {
        (Err(reason), _) => Err(reason),
        (Ok(()), None) => Err(SkipReason::Unavailable(Level::Lir)),
        (Ok(()), Some(level)) => Ok(level),
    };
    let lir_cx = Cx {
        mir: Some(&mir),
        ..*cx
    };
    detect::run(
        Phase::DetectLir,
        lir_level,
        &lir_rules,
        &lir_cx,
        &mut findings,
        &mut trace,
    );

    PassOutput {
        mir,
        lir,
        findings: findings.into_sorted(),
        trace,
    }
}

/// The plan's size is checked first: verifying walks scalar trees and a
/// plan with heavy sharing would never finish.
fn check_entry(mir: &Mir, hir: Option<&Hir>) -> Result<(), SkipReason> {
    if !measure(mir).walkable() {
        return Err(SkipReason::Budget);
    }
    if verify_mir(mir, hir).is_empty() {
        Ok(())
    } else {
        Err(SkipReason::InvalidPlan)
    }
}
