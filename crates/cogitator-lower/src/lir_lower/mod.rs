//! MIR → LIR (specs/ir-pipeline/03). States only what is derivable
//! without running the query. Relations lower in ascending id order,
//! which the MIR verifier guarantees is operands-first; relations under
//! subqueries lower too, so their scans carry pruning facts.

mod classify;
mod prune;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use cogitator_ir::common::{HoleKind, Materialization, SetOpKind};
use cogitator_ir::lir::{
    verify_lir, Estimate, JoinHint, Lir, Phys, PhysId, PhysOp, PhysicalFacts, Sink,
};
use cogitator_ir::mir::{verify_mir, ColId, CteId, Mir, RelId, RelOp, ScalarId, ScalarKind};
use cogitator_ir::{Arena, Degradation, DegradationKind, Lowered, Symbol};

pub use prune::{decide_pruning, PredContext, PredDetail};

use crate::DialectBackend;

pub(crate) fn lower(mir: &Mir, backend: &dyn DialectBackend, phys: &PhysicalFacts) -> Lowered<Lir> {
    let violations = verify_mir(mir, None);
    if !violations.is_empty() {
        return Lowered::declined(Vec::new()).with_violations(violations);
    }
    let lir = backend.lower(mir, phys);
    let violations = verify_lir(&lir, mir, backend.capabilities());
    let degraded = hole_degradations(mir);
    let lowered = if degraded.is_empty() {
        Lowered::complete(lir)
    } else {
        Lowered::partial(lir, degraded)
    };
    lowered.with_violations(violations)
}

/// One per reachable MIR hole: LIR cannot see through them.
fn hole_degradations(mir: &Mir) -> Vec<Degradation> {
    mir.reachable()
        .into_iter()
        .filter_map(|id| match mir.rels.get(id).map(|r| &r.op) {
            // the single-row source of a FROM-less SELECT is not a gap
            Some(RelOp::Opaque {
                hole,
                inputs,
                out,
                open,
            }) if *open || !inputs.is_empty() || !out.is_empty() => {
                let kind = match hole.kind {
                    HoleKind::ParseError => DegradationKind::ParseHole,
                    HoleKind::Jinja => DegradationKind::JinjaControlFlow,
                    HoleKind::Truncated => DegradationKind::CapExceeded,
                    HoleKind::Unsupported => DegradationKind::OpaqueExpression,
                };
                Some(
                    Degradation::new(kind, "an opaque relation hides its physical plan")
                        .at(hole.span),
                )
            }
            _ => None,
        })
        .collect()
}

/// The shared lowering each backend's `lower` delegates to.
pub fn lower_plan(mir: &Mir, phys: &PhysicalFacts, backend: &dyn DialectBackend) -> Lir {
    let reachable = mir.reachable();
    let scan_filters = prune::adjacent_filters(mir, &reachable);
    let filtered_elsewhere = prune::columns_filtered(mir, &reachable);
    let uses = cte_uses(mir, &reachable);
    let col_scalars = col_scalars(mir);
    let mut cx = Lowering {
        mir,
        phys,
        backend,
        nodes: Arena::new(),
        phys_of: vec![None; mir.rels.len()],
        uses,
        scan_filters,
        filtered_elsewhere,
        col_scalars,
    };
    for id in reachable {
        cx.rel(id);
    }
    let root = cx
        .phys_of
        .get(mir.root.index())
        .copied()
        .flatten()
        .unwrap_or_else(|| cx.empty_root());
    Lir {
        nodes: cx.nodes,
        root,
        backend: backend.capabilities().backend,
        sink: sink(mir),
    }
}

/// How many reachable `CteRef`s read each CTE.
fn cte_uses(mir: &Mir, reachable: &[RelId]) -> BTreeMap<CteId, u32> {
    let mut uses = BTreeMap::new();
    for id in reachable {
        if let Some(RelOp::CteRef { cte, .. }) = mir.rels.get(*id).map(|r| &r.op) {
            *uses.entry(*cte).or_insert(0) += 1;
        }
    }
    uses
}

/// Any `Col` scalar per column, so a key-only Aggregate can name keys.
fn col_scalars(mir: &Mir) -> BTreeMap<ColId, ScalarId> {
    let mut map = BTreeMap::new();
    for (id, scalar) in mir.scalars.iter() {
        if let ScalarKind::Col(col) = scalar.kind {
            map.entry(col).or_insert(id);
        }
    }
    map
}

/// The sink from model config. `partition_by` and `cluster_by` name root
/// columns; a name the plan does not output cannot be stated. dbt's
/// default materialization is `view`.
fn sink(mir: &Mir) -> Sink {
    let resolve = |names: &[Symbol]| -> Vec<ColId> {
        names
            .iter()
            .filter_map(|name| {
                mir.root_columns()
                    .iter()
                    .copied()
                    .find(|c| mir.cols.get(*c).and_then(|def| def.name) == Some(*name))
            })
            .collect()
    };
    Sink {
        mat: mir.config.materialized.unwrap_or(Materialization::View),
        partition_by: resolve(&mir.config.partition_by),
        cluster_by: resolve(&mir.config.cluster_by),
        contract: mir.config.contract_enforced,
    }
}

struct Lowering<'a> {
    mir: &'a Mir,
    phys: &'a PhysicalFacts,
    backend: &'a dyn DialectBackend,
    nodes: Arena<Phys>,
    phys_of: Vec<Option<PhysId>>,
    uses: BTreeMap<CteId, u32>,
    scan_filters: BTreeMap<RelId, Vec<ScalarId>>,
    filtered_elsewhere: std::collections::BTreeSet<ColId>,
    col_scalars: BTreeMap<ColId, ScalarId>,
}

impl Lowering<'_> {
    fn empty_root(&mut self) -> PhysId {
        self.push(
            PhysOp::Opaque {
                inputs: Vec::new(),
                out: Vec::new(),
            },
            self.mir.root,
            Vec::new(),
            Estimate::Unknown,
        )
    }

    fn push(&mut self, op: PhysOp, mir: RelId, out: Vec<ColId>, est: Estimate) -> PhysId {
        self.nodes.alloc(Phys { op, mir, out, est })
    }

    fn input(&self, rel: RelId) -> Option<PhysId> {
        self.phys_of.get(rel.index()).copied().flatten()
    }

    fn rel(&mut self, id: RelId) {
        let Some(rel) = self.mir.rels.get(id) else {
            return;
        };
        let (op, est) = self.op(id, &rel.op);
        let node = self.push(op, id, rel.out.clone(), est);
        if let Some(slot) = self.phys_of.get_mut(id.index()) {
            *slot = Some(node);
        }
    }

    fn opaque(&self, rel: &RelOp) -> PhysOp {
        PhysOp::Opaque {
            inputs: rel.inputs().iter().filter_map(|i| self.input(*i)).collect(),
            out: rel.computed_output(&self.mir.rels),
        }
    }

    fn op(&self, id: RelId, rel: &RelOp) -> (PhysOp, Estimate) {
        let mut est = Estimate::Unknown;
        let mapped = match rel {
            RelOp::Scan { table, cols, .. } => {
                if let Some(rows) = self.phys.table(*table).and_then(|t| t.row_count) {
                    est = Estimate::Stats { rows };
                }
                Some(PhysOp::TableScan {
                    table: *table,
                    cols: cols.clone(),
                    pruning: self.pruning_keys(id, *table, cols),
                })
            }
            RelOp::Filter { input, conjuncts } => self.input(*input).map(|input| PhysOp::Filter {
                input,
                conjuncts: conjuncts.clone(),
            }),
            RelOp::Project { input, items } => self.input(*input).map(|input| PhysOp::Project {
                input,
                items: items.clone(),
            }),
            RelOp::Join { kind, l, r, on } => {
                self.input(*l)
                    .zip(self.input(*r))
                    .map(|(l, r)| PhysOp::Join {
                        kind: *kind,
                        l,
                        r,
                        on: on.clone(),
                        hint: JoinHint::None,
                        build: None,
                    })
            }
            RelOp::Aggregate {
                input, keys, aggs, ..
            } => self.input(*input).map(|input| PhysOp::Aggregate {
                input,
                keys: keys.clone(),
                aggs: aggs.iter().map(|(c, _)| *c).collect(),
            }),
            RelOp::Window { input, funcs } => self.input(*input).map(|input| PhysOp::Window {
                input,
                funcs: funcs.iter().map(|(c, _)| *c).collect(),
            }),
            RelOp::Distinct(input) => self.distinct(*input),
            RelOp::Sort { input, keys } => self.input(*input).map(|input| PhysOp::Sort {
                input,
                keys: keys.clone(),
            }),
            RelOp::Limit {
                input,
                limit,
                offset,
            } => self.input(*input).map(|input| PhysOp::Limit {
                input,
                limit: *limit,
                offset: *offset,
            }),
            RelOp::SetOp {
                op: SetOpKind::Union,
                all,
                inputs,
                out,
            } => {
                let lowered: Option<Vec<PhysId>> = inputs.iter().map(|i| self.input(*i)).collect();
                lowered.map(|inputs| PhysOp::Union {
                    inputs,
                    all: *all,
                    out: out.clone(),
                })
            }
            RelOp::SetOp { .. } | RelOp::Unnest { .. } | RelOp::Opaque { .. } => None,
            RelOp::CteRef { cte, out } => self.cte(*cte, out),
        };
        (mapped.unwrap_or_else(|| self.opaque(rel)), est)
    }

    /// DISTINCT is an Aggregate grouping on every input column. The keys
    /// need `Col` scalars, which HIR lowering allocates; a plan without
    /// them stays `Opaque` rather than inventing any.
    fn distinct(&self, input: RelId) -> Option<PhysOp> {
        let out = &self.mir.rels.get(input)?.out;
        let keys: Option<Vec<(ColId, ScalarId)>> = out
            .iter()
            .map(|c| self.col_scalars.get(c).map(|s| (*c, *s)))
            .collect();
        Some(PhysOp::Aggregate {
            input: self.input(input)?,
            keys: keys?,
            aggs: Vec::new(),
        })
    }

    fn cte(&self, cte: CteId, out: &[ColId]) -> Option<PhysOp> {
        let body = self.input(self.mir.ctes.get(cte)?.body)?;
        let uses = self.uses.get(&cte).copied().unwrap_or(1);
        Some(PhysOp::Cte {
            policy: self.backend.cte_policy(uses),
            uses,
            cte,
            body,
            out: out.to_vec(),
        })
    }
}
