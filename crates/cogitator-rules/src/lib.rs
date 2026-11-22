//! The rule engine contract (specs/ir-pipeline/04, 05): rule traits,
//! metadata, findings, the static registry, and pass-manager types.
//!
//! Each rule keeps its own file so rules can be worked on independently.

pub mod canon;
pub mod cx;
pub mod findings;
pub mod meta;
pub mod passes;
pub mod registry;
pub mod rule;
pub mod rules;

pub use cx::{ColumnFilter, ConsumerFacts, Cx, DagFacts, SharedSubplan};
pub use findings::{Evidence, Finding, Findings, Fix, FixEdit};
pub use meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
pub use passes::{Budgets, Outcome, Phase, SkipReason, Trace, TraceEntry};
pub use registry::{Registered, REGISTRY};
pub use rule::{Canonicalize, Detect, Rewrite, Rule};
