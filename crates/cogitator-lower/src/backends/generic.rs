//! Fallback for unknown dialects: no physical claims, logical rules only.

use cogitator_ir::lir::{
    Backend, Capabilities, CtePolicy, Lir, PhysColumn, PhysicalFacts, Pred, Pruning,
};
use cogitator_ir::mir::Mir;

use crate::{lower_plan, DialectBackend};

static CAPABILITIES: Capabilities = Capabilities {
    backend: Backend::Generic,
    join_hints: &[],
    cte_policies: &[CtePolicy::Auto],
    pruning_keys: &[],
    doc: "",
};

pub struct Generic;

impl DialectBackend for Generic {
    fn capabilities(&self) -> &'static Capabilities {
        &CAPABILITIES
    }

    fn lower(&self, mir: &Mir, phys: &PhysicalFacts) -> Lir {
        lower_plan(mir, phys, self)
    }

    fn pruning(&self, _pred: &Pred, _col: &PhysColumn) -> Pruning {
        Pruning::Unknown
    }
}
