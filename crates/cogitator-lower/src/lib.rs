//! HIR → MIR and MIR → LIR lowering (specs/ir-pipeline/02, 03), SQL type
//! inference and one `DialectBackend` per dialect.
//!
//! Both stages are total: gaps in the input become `Opaque` nodes with a
//! `Degradation`, and a verifier `Violation` drops the level instead of
//! panicking (specs/ir-pipeline/00).

pub mod backends;
mod infer;
mod lir_lower;
mod mir_lower;

use cogitator_ir::hir::Hir;
use cogitator_ir::lir::{
    Capabilities, CtePolicy, Lir, PhysColumn, PhysicalFacts, Pred, PruneKey, Pruning,
};
use cogitator_ir::mir::Mir;
use cogitator_ir::Lowered;

pub use lir_lower::{decide_pruning, lower_plan, PredContext, PredDetail};

/// A dialect's physical knowledge. Implemented by one unit struct per
/// module under `backends/`; `lower` must return a `Lir` that passes
/// `verify_lir` against the input `Mir` and `capabilities()`.
pub trait DialectBackend: Sync {
    fn capabilities(&self) -> &'static Capabilities;
    fn lower(&self, mir: &Mir, phys: &PhysicalFacts) -> Lir;

    /// The verdict from the predicate's shape alone. Callers holding a
    /// `Mir` should use [`decide_pruning`], which also applies `refine`.
    fn pruning(&self, pred: &Pred, col: &PhysColumn) -> Pruning;

    /// Adjusts `decided` with facts that need the plan: a function's
    /// name, or how the column is used inside the comparison.
    fn refine(&self, decided: Pruning, _ctx: &PredContext<'_>) -> Pruning {
        decided
    }

    /// Adjusts one scan's keys together, in `TableFacts` column order.
    fn finish_keys(&self, _keys: &mut [PruneKey]) {}

    /// The policy for a CTE with `uses` references in the plan. Must be
    /// listed in `capabilities().cte_policies`.
    fn cte_policy(&self, _uses: u32) -> CtePolicy {
        CtePolicy::Auto
    }
}

/// Lowers a HIR to MIR. Never panics: gaps become `Opaque` nodes with
/// `Degradation`s, and verifier violations drop the level.
pub fn lower_to_mir(hir: &Hir) -> Lowered<Mir> {
    mir_lower::lower(hir)
}

/// Lowers a MIR through `backend`. An invalid MIR declines the level;
/// the result is verified against the backend's capability table.
pub fn lower_to_lir(mir: &Mir, backend: &dyn DialectBackend, phys: &PhysicalFacts) -> Lowered<Lir> {
    lir_lower::lower(mir, backend, phys)
}
