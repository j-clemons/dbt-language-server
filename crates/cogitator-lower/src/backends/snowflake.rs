//! Snowflake micro-partition pruning.
//!
//! | Predicate on a clustering key | Verdict | Source |
//! |---|---|---|
//! | bare comparison or range | prunes | [`PRUNING_DOC`] |
//! | subquery value | blocks | [`PRUNING_DOC`]: "does not prune micro-partitions based on a predicate with a subquery, even if the subquery results in a constant" |
//! | function or cast around the column | unknown | undocumented: at most "may reduce pruning" |
//! | anything else | unknown | undocumented |
//!
//! The CTE evaluation strategy is undocumented, so the policy is `Auto`.

use cogitator_ir::lir::{
    Backend, BlockReason, Capabilities, CtePolicy, KeyKind, Lir, PhysColumn, PhysicalFacts, Pred,
    PredShape, Pruning,
};
use cogitator_ir::mir::Mir;

use crate::{lower_plan, DialectBackend};

pub const PRUNING_DOC: &str =
    "https://docs.snowflake.com/en/user-guide/tables-clustering-micropartitions";

static CAPABILITIES: Capabilities = Capabilities {
    backend: Backend::Snowflake,
    join_hints: &[],
    cte_policies: &[CtePolicy::Auto],
    pruning_keys: &[KeyKind::Cluster],
    doc: PRUNING_DOC,
};

pub struct Snowflake;

impl DialectBackend for Snowflake {
    fn capabilities(&self) -> &'static Capabilities {
        &CAPABILITIES
    }

    fn lower(&self, mir: &Mir, phys: &PhysicalFacts) -> Lir {
        lower_plan(mir, phys, self)
    }

    fn pruning(&self, pred: &Pred, _col: &PhysColumn) -> Pruning {
        match pred.shape {
            PredShape::BareCompare | PredShape::Range => Pruning::Prunes,
            PredShape::Subquery => Pruning::Blocks {
                why: BlockReason::SubqueryValue,
                doc: PRUNING_DOC,
            },
            PredShape::Function { .. }
            | PredShape::Cast
            | PredShape::OrAcrossColumns
            | PredShape::Other => Pruning::Unknown,
        }
    }
}
