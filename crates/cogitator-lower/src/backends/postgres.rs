//! Postgres indexes, known only from a dbt `indexes` config: without one
//! `PhysicalFacts` carries no key and nothing is claimed.
//!
//! | Predicate on an indexed column | Verdict | Source |
//! |---|---|---|
//! | bare comparison or range | prunes | [`BTREE_DOC`]: the planner considers a B-tree "whenever an indexed column is involved in a comparison" |
//! | function around the column | blocks | [`EXPRESSION_DOC`]: such a query needs an index "defined on the result of" that expression; a dbt column index is not one |
//! | cast, OR, subquery, other | unknown | the planner decides; not stated |
//!
//! CTEs ([`CTE_DOC`]): "by default, this happens if the parent query
//! references the `WITH` query just once, but not if it references the
//! `WITH` query more than once". Folding also needs a side-effect-free
//! body, which is not checked here.

use cogitator_ir::lir::{
    Backend, BlockReason, Capabilities, CtePolicy, KeyKind, Lir, PhysColumn, PhysicalFacts, Pred,
    PredShape, Pruning,
};
use cogitator_ir::mir::Mir;

use crate::{lower_plan, DialectBackend};

pub const BTREE_DOC: &str = "https://www.postgresql.org/docs/current/indexes-types.html";
pub const EXPRESSION_DOC: &str =
    "https://www.postgresql.org/docs/current/indexes-expressional.html";
pub const CTE_DOC: &str = "https://www.postgresql.org/docs/current/queries-with.html";

static CAPABILITIES: Capabilities = Capabilities {
    backend: Backend::Postgres,
    join_hints: &[],
    cte_policies: &[CtePolicy::Inline, CtePolicy::Materialize],
    pruning_keys: &[KeyKind::Sort, KeyKind::Unique],
    doc: BTREE_DOC,
};

pub struct Postgres;

impl DialectBackend for Postgres {
    fn capabilities(&self) -> &'static Capabilities {
        &CAPABILITIES
    }

    fn lower(&self, mir: &Mir, phys: &PhysicalFacts) -> Lir {
        lower_plan(mir, phys, self)
    }

    fn pruning(&self, pred: &Pred, _col: &PhysColumn) -> Pruning {
        match pred.shape {
            PredShape::BareCompare | PredShape::Range => Pruning::Prunes,
            PredShape::Function { .. } => Pruning::Blocks {
                why: BlockReason::FunctionWrapped,
                doc: EXPRESSION_DOC,
            },
            PredShape::Cast
            | PredShape::Subquery
            | PredShape::OrAcrossColumns
            | PredShape::Other => Pruning::Unknown,
        }
    }

    fn cte_policy(&self, uses: u32) -> CtePolicy {
        if uses > 1 {
            CtePolicy::Materialize
        } else {
            CtePolicy::Inline
        }
    }
}
