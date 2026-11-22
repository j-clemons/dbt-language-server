//! One detect phase: each rule that implements it, in registry order.

use crate::cx::Cx;
use crate::findings::Findings;
use crate::meta::RuleMeta;
use crate::rule::Detect;

use super::trace::{Outcome, Phase, SkipReason, Trace};

/// `ir` is `Err(reason)` when the level cannot be read, which every rule
/// of the phase then reports.
pub(super) fn run<L>(
    phase: Phase,
    ir: Result<&L, SkipReason>,
    rules: &[(&'static RuleMeta, &'static dyn Detect<L>)],
    cx: &Cx<'_>,
    out: &mut Findings,
    trace: &mut Trace,
) {
    for (meta, rule) in rules {
        let unmet = cx.skip_reason(meta).map_or(Ok(()), Err);
        let outcome = match (ir, unmet) {
            (Err(reason), _) | (_, Err(reason)) => Outcome::Skipped { reason },
            (Ok(level), Ok(())) => {
                let before = out.len();
                rule.check(level, cx, out);
                match u32::try_from(out.len().saturating_sub(before)).unwrap_or(u32::MAX) {
                    0 => Outcome::NoChange,
                    count => Outcome::Findings { count },
                }
            }
        };
        trace.record(meta.id, phase, 0, outcome);
    }
}
