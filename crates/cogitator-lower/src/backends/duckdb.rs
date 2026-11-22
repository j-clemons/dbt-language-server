//! DuckDB row-group zonemaps and ART indexes.
//!
//! | Predicate | Verdict | Source |
//! |---|---|---|
//! | bare comparison or range | prunes (zonemap) | [`INDEXING_DOC`]: "can skip any row group whose min-max range does not contain that filter value" |
//! | function, cast, OR, subquery | unknown | the optimizer rewrites some of these itself, and the docs do not enumerate which |
//!
//! Zonemaps exist on every general-purpose column; `Sort` marks a column
//! the data is ordered by ("the more ordered the data within a column,
//! the more valuable the zonemap indexes will be"). `Unique` is an ART
//! key: per the same page, "indexes are only eligible for index scans if
//! they index a single column without expressions", which `PhysColumn`
//! cannot distinguish, so the verdict rests on the zonemap alone.
//!
//! CTEs are "materialized by default" with heuristic inlining
//! ([`CTE_DOC`]), which only the engine resolves: the policy is `Auto`.

use cogitator_ir::lir::{
    Backend, Capabilities, CtePolicy, KeyKind, Lir, PhysColumn, PhysicalFacts, Pred, PredShape,
    Pruning,
};
use cogitator_ir::mir::Mir;

use crate::{lower_plan, DialectBackend};

pub const INDEXING_DOC: &str = "https://duckdb.org/docs/2.0/guides/performance/indexing.html";
pub const CTE_DOC: &str = "https://duckdb.org/docs/2.0/sql/query_syntax/with.html";

static CAPABILITIES: Capabilities = Capabilities {
    backend: Backend::DuckDb,
    join_hints: &[],
    cte_policies: &[CtePolicy::Auto],
    pruning_keys: &[KeyKind::Sort, KeyKind::Unique],
    doc: INDEXING_DOC,
};

pub struct DuckDb;

impl DialectBackend for DuckDb {
    fn capabilities(&self) -> &'static Capabilities {
        &CAPABILITIES
    }

    fn lower(&self, mir: &Mir, phys: &PhysicalFacts) -> Lir {
        lower_plan(mir, phys, self)
    }

    fn pruning(&self, pred: &Pred, _col: &PhysColumn) -> Pruning {
        match pred.shape {
            PredShape::BareCompare | PredShape::Range => Pruning::Prunes,
            PredShape::Function { .. }
            | PredShape::Cast
            | PredShape::Subquery
            | PredShape::OrAcrossColumns
            | PredShape::Other => Pruning::Unknown,
        }
    }
}
