//! The per-rule-visit record shown in the plan view.

use cogitator_ir::common::RuleId;
use cogitator_ir::mir::RelId;
use cogitator_ir::{Invariant, Level};

use crate::meta::Need;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Canonicalize,
    DetectHir,
    DetectMir,
    DetectLir,
}

/// One rule visit. A rule skipped for unmet needs still gets an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    pub rule: RuleId,
    pub phase: Phase,
    /// 0-based fixpoint iteration; 0 for detect phases.
    pub pass: u32,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    NoChange,
    Findings {
        count: u32,
    },
    Skipped {
        reason: SkipReason,
    },
    /// A rewrite failed verification; `root` was reset.
    RolledBack {
        invariants: Vec<Invariant>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Unmet(Need),
    Budget,
    /// The level was not supplied, or is stale after canonicalisation.
    Unavailable(Level),
    /// The MIR handed to the pass manager failed verification.
    InvalidPlan,
}

/// The plan's printed form after one rewrite, kept only when
/// `PassInput::snapshots` asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The rule that produced this plan; [`Snapshot::INPUT`] for the
    /// plan as it entered canonicalisation.
    pub rule: RuleId,
    pub pass: u32,
    pub root: RelId,
    pub text: String,
}

impl Snapshot {
    pub const INPUT: RuleId = RuleId("input");
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trace {
    pub entries: Vec<TraceEntry>,
    /// A budget ran out before the phases completed, or the plan could
    /// not be verified.
    pub incomplete: bool,
    pub snapshots: Vec<Snapshot>,
}

impl Trace {
    pub fn record(&mut self, rule: RuleId, phase: Phase, pass: u32, outcome: Outcome) {
        self.entries.push(TraceEntry {
            rule,
            phase,
            pass,
            outcome,
        });
    }
}
