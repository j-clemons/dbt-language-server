//! Hand-built LIR over an existing MIR.

use crate::common::{JoinKind, Materialization};
use crate::lir::{
    Backend, Capabilities, CtePolicy, Estimate, JoinHint, KeyKind, Lir, Phys, PhysId, PhysOp,
    PruneKey, Side, Sink,
};
use crate::mir::{ColId, CteId, Mir, RelId, RelOp, ScalarId};
use crate::Arena;

/// Capabilities of a backend with no hints and no special keys.
pub fn generic_caps() -> Capabilities {
    Capabilities {
        backend: Backend::Generic,
        join_hints: &[],
        cte_policies: &[CtePolicy::Auto],
        pruning_keys: &[],
        doc: "",
    }
}

pub struct LirBuilder {
    nodes: Arena<Phys>,
    backend: Backend,
    sink: Sink,
}

impl Default for LirBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl LirBuilder {
    pub fn new() -> Self {
        Self {
            nodes: Arena::new(),
            backend: Backend::Generic,
            sink: Sink {
                mat: Materialization::Table,
                partition_by: Vec::new(),
                cluster_by: Vec::new(),
                contract: false,
            },
        }
    }

    pub fn backend(mut self, backend: Backend) -> Self {
        self.backend = backend;
        self
    }

    pub fn sink(mut self, sink: Sink) -> Self {
        self.sink = sink;
        self
    }

    pub fn node(&mut self, op: PhysOp, mir: RelId, out: Vec<ColId>) -> PhysId {
        self.nodes.alloc(Phys {
            op,
            mir,
            out,
            est: Estimate::Unknown,
        })
    }

    /// A `TableScan` of the MIR `Scan` at `rel`; other ops yield an
    /// empty scan with no columns so a test can still build around it.
    pub fn scan(&mut self, mir: &Mir, rel: RelId) -> PhysId {
        let (table, cols) = match mir.rels.get(rel).map(|r| &r.op) {
            Some(RelOp::Scan { table, cols, .. }) => (*table, cols.clone()),
            _ => (crate::mir::TableId::from_raw(0), Vec::new()),
        };
        self.node(
            PhysOp::TableScan {
                table,
                cols: cols.clone(),
                pruning: Vec::new(),
            },
            rel,
            cols,
        )
    }

    pub fn with_pruning(&mut self, id: PhysId, key: PruneKey) {
        if let Some(Phys {
            op: PhysOp::TableScan { pruning, .. },
            ..
        }) = self.nodes.get_mut(id)
        {
            pruning.push(key);
        }
    }

    pub fn filter(&mut self, input: PhysId, conjuncts: Vec<ScalarId>, mir: RelId) -> PhysId {
        let out = self.out_of(input);
        self.node(PhysOp::Filter { input, conjuncts }, mir, out)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn join(
        &mut self,
        kind: JoinKind,
        l: PhysId,
        r: PhysId,
        on: Vec<ScalarId>,
        hint: JoinHint,
        build: Option<Side>,
        mir: RelId,
    ) -> PhysId {
        let mut out = self.out_of(l);
        out.extend(self.out_of(r));
        self.node(
            PhysOp::Join {
                kind,
                l,
                r,
                on,
                hint,
                build,
            },
            mir,
            out,
        )
    }

    pub fn cte(
        &mut self,
        policy: CtePolicy,
        uses: u32,
        cte: CteId,
        body: PhysId,
        out: Vec<ColId>,
        mir: RelId,
    ) -> PhysId {
        self.node(
            PhysOp::Cte {
                policy,
                uses,
                cte,
                body,
                out: out.clone(),
            },
            mir,
            out,
        )
    }

    pub fn finish(self, root: PhysId) -> Lir {
        Lir {
            nodes: self.nodes,
            root,
            backend: self.backend,
            sink: self.sink,
        }
    }

    fn out_of(&self, id: PhysId) -> Vec<ColId> {
        self.nodes.get(id).map_or(Vec::new(), |n| n.out.clone())
    }
}

/// `KeyKind` values a capability table may list, for table-driven tests.
pub const ALL_KEY_KINDS: [KeyKind; 4] = [
    KeyKind::Partition,
    KeyKind::Cluster,
    KeyKind::Sort,
    KeyKind::Unique,
];
