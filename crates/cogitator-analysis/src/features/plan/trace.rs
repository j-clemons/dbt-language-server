//! The rule trace and the needs summary of a plan, in wire vocabulary.

use cogitator_ir::{Invariant, Level};
use cogitator_rules::meta::{Need, Needs};
use cogitator_rules::passes::{Outcome, Phase, SkipReason, Trace};
use serde::Serialize;

/// One rule visit. `skipped` rows always carry a `reason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRow {
    pub rule: &'static str,
    pub phase: &'static str,
    pub pass: u32,
    /// `applied`, `noChange`, `findings`, `skipped` or `rolledBack`.
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<Invariant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraceView {
    pub entries: Vec<TraceRow>,
    /// A budget ran out, or the plan could not be verified.
    pub incomplete: bool,
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Canonicalize => "canonicalize",
        Phase::DetectHir => "detectHir",
        Phase::DetectMir => "detectMir",
        Phase::DetectLir => "detectLir",
    }
}

fn need_name(need: Need) -> &'static str {
    match need {
        Need::Schema => "schema",
        Need::Types => "types",
        Need::Stats => "stats",
        Need::Dialect => "dialect",
        Need::Mat => "mat",
        Need::Dag => "dag",
        Need::Tests => "tests",
        Need::Phys => "phys",
        Need::AllBranches => "allBranches",
    }
}

fn skip_reason(reason: SkipReason) -> String {
    match reason {
        SkipReason::Unmet(need) => format!("unmet:{}", need_name(need)),
        SkipReason::Budget => "budget".to_string(),
        SkipReason::Unavailable(level) => format!("unavailable:{level:?}").to_lowercase(),
        SkipReason::InvalidPlan => "invalidPlan".to_string(),
    }
}

fn row(rule: &'static str, phase: Phase, pass: u32, outcome: &Outcome) -> TraceRow {
    let mut row = TraceRow {
        rule,
        phase: phase_name(phase),
        pass,
        outcome: "applied",
        count: None,
        reason: None,
        invariants: Vec::new(),
    };
    match outcome {
        Outcome::Applied => {}
        Outcome::NoChange => row.outcome = "noChange",
        Outcome::Findings { count } => {
            row.outcome = "findings";
            row.count = Some(*count);
        }
        Outcome::Skipped { reason } => {
            row.outcome = "skipped";
            row.reason = Some(skip_reason(*reason));
        }
        Outcome::RolledBack { invariants } => {
            row.outcome = "rolledBack";
            row.invariants = invariants.clone();
        }
    }
    row
}

impl TraceView {
    pub(super) fn of(trace: &Trace) -> Self {
        TraceView {
            entries: trace
                .entries
                .iter()
                .map(|e| row(e.rule.as_str(), e.phase, e.pass, &e.outcome))
                .collect(),
            incomplete: trace.incomplete,
        }
    }

    /// Every rule skipped because `level` is unavailable, in run order.
    pub(super) fn unavailable(level: Level) -> Self {
        let reason = Outcome::Skipped {
            reason: SkipReason::Unavailable(level),
        };
        let mut entries = Vec::new();
        for rule in cogitator_rules::registry::REGISTRY {
            let id = rule.meta.id.as_str();
            let phases = [
                (rule.canon.is_some(), Phase::Canonicalize),
                (rule.hir.is_some(), Phase::DetectHir),
                (rule.mir.is_some(), Phase::DetectMir),
                (rule.lir.is_some(), Phase::DetectLir),
            ];
            for (_, phase) in phases.iter().filter(|(present, _)| *present) {
                entries.push(row(id, *phase, 0, &reason));
            }
        }
        TraceView {
            entries,
            incomplete: true,
        }
    }
}

/// What the plan could establish, named for the plan view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NeedsView {
    pub available: Vec<&'static str>,
    pub missing: Vec<&'static str>,
}

impl NeedsView {
    pub(super) fn of(available: Needs) -> Self {
        let mut view = NeedsView {
            available: Vec::new(),
            missing: Vec::new(),
        };
        for need in [
            Need::Schema,
            Need::Types,
            Need::Stats,
            Need::Dialect,
            Need::Mat,
            Need::Dag,
            Need::Tests,
            Need::Phys,
            Need::AllBranches,
        ] {
            let list = if available.contains(need) {
                &mut view.available
            } else {
                &mut view.missing
            };
            list.push(need_name(need));
        }
        view
    }
}
