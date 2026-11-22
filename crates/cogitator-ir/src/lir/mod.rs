//! LIR: states only what is derivable without running the query
//! (specs/ir-pipeline/03). Operators reference MIR column and scalar
//! ids; a `Lir` is always read with the `Mir` it was lowered from.
//!
//! A MIR `Distinct` lowers to a key-only `Aggregate`; INTERSECT, EXCEPT
//! and UNNEST lower to `Opaque`, which still carries pruning-relevant
//! inputs and outputs.

pub mod print;
pub mod pruning;
pub mod verify;

use serde::Serialize;

use crate::common::{JoinKind, Materialization};
use crate::mir::{ColId, CteId, RelId, ScalarId, SortKey, TableId};
use crate::{Arena, Idx};

pub use print::print_lir;
pub use pruning::{
    BlockReason, Capabilities, CtePolicy, JoinHint, KeyKind, PhysColumn, PhysicalFacts, Pred,
    PredShape, PruneKey, PruneUse, Pruning, Side, TableFacts,
};
pub use verify::verify_lir;

pub type PhysId = Idx<Phys>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Backend {
    Snowflake,
    BigQuery,
    DuckDb,
    Postgres,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Bound {
    AtMost(u64),
    AtLeast(u64),
}

/// Never a selectivity or a dollar figure (specs/ir-pipeline/06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Estimate {
    Unknown,
    Stats { rows: u64 },
    Heuristic(Bound),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sink {
    pub mat: Materialization,
    pub partition_by: Vec<ColId>,
    pub cluster_by: Vec<ColId>,
    pub contract: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Lir {
    pub nodes: Arena<Phys>,
    pub root: PhysId,
    pub backend: Backend,
    pub sink: Sink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phys {
    pub op: PhysOp,
    /// The MIR node this operator was lowered from.
    pub mir: RelId,
    pub out: Vec<ColId>,
    pub est: Estimate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PhysOp {
    TableScan {
        table: TableId,
        cols: Vec<ColId>,
        pruning: Vec<PruneKey>,
    },
    Filter {
        input: PhysId,
        conjuncts: Vec<ScalarId>,
    },
    Project {
        input: PhysId,
        items: Vec<(ColId, ScalarId)>,
    },
    Join {
        kind: JoinKind,
        l: PhysId,
        r: PhysId,
        on: Vec<ScalarId>,
        hint: JoinHint,
        build: Option<Side>,
    },
    Aggregate {
        input: PhysId,
        keys: Vec<(ColId, ScalarId)>,
        aggs: Vec<ColId>,
    },
    Window {
        input: PhysId,
        funcs: Vec<ColId>,
    },
    Sort {
        input: PhysId,
        keys: Vec<SortKey>,
    },
    Limit {
        input: PhysId,
        limit: Option<ScalarId>,
        offset: Option<ScalarId>,
    },
    Union {
        inputs: Vec<PhysId>,
        all: bool,
        out: Vec<ColId>,
    },
    Cte {
        policy: CtePolicy,
        uses: u32,
        cte: CteId,
        body: PhysId,
        out: Vec<ColId>,
    },
    Opaque {
        inputs: Vec<PhysId>,
        out: Vec<ColId>,
    },
}

impl PhysOp {
    pub fn inputs(&self) -> Vec<PhysId> {
        match self {
            PhysOp::TableScan { .. } => Vec::new(),
            PhysOp::Filter { input, .. }
            | PhysOp::Project { input, .. }
            | PhysOp::Aggregate { input, .. }
            | PhysOp::Window { input, .. }
            | PhysOp::Sort { input, .. }
            | PhysOp::Limit { input, .. }
            | PhysOp::Cte { body: input, .. } => vec![*input],
            PhysOp::Join { l, r, .. } => vec![*l, *r],
            PhysOp::Union { inputs, .. } | PhysOp::Opaque { inputs, .. } => inputs.clone(),
        }
    }

    pub fn scalars(&self) -> Vec<ScalarId> {
        match self {
            PhysOp::TableScan { .. }
            | PhysOp::Window { .. }
            | PhysOp::Union { .. }
            | PhysOp::Cte { .. }
            | PhysOp::Opaque { .. } => Vec::new(),
            PhysOp::Filter { conjuncts: s, .. } | PhysOp::Join { on: s, .. } => s.clone(),
            PhysOp::Project { items, .. } | PhysOp::Aggregate { keys: items, .. } => {
                items.iter().map(|(_, s)| *s).collect()
            }
            PhysOp::Sort { keys, .. } => keys.iter().map(|k| k.expr).collect(),
            PhysOp::Limit { limit, offset, .. } => limit.iter().chain(offset).copied().collect(),
        }
    }
}
