//! The pass manager: phases, budgets, trace and the driver
//! (specs/ir-pipeline/04).

pub mod budget;
mod canonicalize;
mod detect;
mod expansion;
pub mod manager;
#[cfg(test)]
mod phases_tests;
#[cfg(test)]
mod test_rules;
#[cfg(test)]
mod tests;
pub mod trace;

pub use budget::{Budgets, Exceeded};
pub use manager::{run_passes, LirLowering, PassInput, PassOutput};
pub use trace::{Outcome, Phase, SkipReason, Snapshot, Trace, TraceEntry};
