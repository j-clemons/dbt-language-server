//! Pruning-key usage: classifies each predicate over a key and asks the
//! backend. Only filters directly above the scan decide `Used`/`Blocked`;
//! a key read by a filter higher up is `Unknown`, not `NotFiltered`.

use std::collections::{BTreeMap, BTreeSet};

use cogitator_ir::lir::{KeyKind, PhysColumn, Pred, PredShape, PruneKey, PruneUse, Pruning};
use cogitator_ir::mir::{ColId, Mir, RelId, RelOp, ScalarId, ScalarKind, TableId};
use cogitator_ir::Symbol;

use super::classify::{classify, walk};
use super::Lowering;
use crate::DialectBackend;

/// What the plan adds to a `Pred` that its shape cannot say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredDetail {
    None,
    /// `col + 1 > x`: the column is not isolated.
    ArithmeticOnColumn,
    /// `col = other_col`.
    ComparedToColumn,
}

pub struct PredContext<'a> {
    pub pred: Pred,
    pub col: &'a PhysColumn,
    /// The function name, when the shape is `Function`.
    pub function: Option<&'a str>,
    pub detail: PredDetail,
}

/// `pruning` plus `refine`: the full verdict for a predicate on `col`.
/// `None` when the predicate does not read `col`.
pub fn decide_pruning(
    backend: &dyn DialectBackend,
    mir: &Mir,
    scalar: ScalarId,
    col_id: ColId,
    col: &PhysColumn,
) -> Option<(Pred, Pruning)> {
    let classified = classify(mir, scalar, col_id)?;
    let pred = Pred {
        scalar,
        shape: classified.shape,
    };
    let function = match classified.shape {
        PredShape::Function { name } => mir.syms.resolve(name),
        _ => None,
    };
    let ctx = PredContext {
        pred,
        col,
        function,
        detail: classified.detail,
    };
    Some((pred, backend.refine(backend.pruning(&pred, col), &ctx)))
}

/// `Filter` conjuncts directly above each `Scan`, through stacked Filters.
pub(super) fn adjacent_filters(mir: &Mir, reachable: &[RelId]) -> BTreeMap<RelId, Vec<ScalarId>> {
    let mut map: BTreeMap<RelId, Vec<ScalarId>> = BTreeMap::new();
    for id in reachable {
        let Some(RelOp::Filter { input, conjuncts }) = mir.rels.get(*id).map(|r| &r.op) else {
            continue;
        };
        let mut below = *input;
        while let Some(RelOp::Filter { input, .. }) = mir.rels.get(below).map(|r| &r.op) {
            below = *input;
        }
        if matches!(mir.rels.get(below).map(|r| &r.op), Some(RelOp::Scan { .. })) {
            map.entry(below)
                .or_default()
                .extend(conjuncts.iter().copied());
        }
    }
    map
}

/// Every column any reachable `Filter` reads.
pub(super) fn columns_filtered(mir: &Mir, reachable: &[RelId]) -> BTreeSet<ColId> {
    let mut out = BTreeSet::new();
    for id in reachable {
        if let Some(RelOp::Filter { conjuncts, .. }) = mir.rels.get(*id).map(|r| &r.op) {
            for root in conjuncts {
                walk(mir, *root, &mut |kind| {
                    if let ScalarKind::Col(col) | ScalarKind::Outer(col) = kind {
                        out.insert(*col);
                    }
                });
            }
        }
    }
    out
}

impl Lowering<'_> {
    /// Keys the backend can use, in `TableFacts` order, restricted to
    /// columns this scan reads.
    pub(super) fn pruning_keys(
        &self,
        scan: RelId,
        table: TableId,
        cols: &[ColId],
    ) -> Vec<PruneKey> {
        let Some(facts) = self.phys.table(table) else {
            return Vec::new();
        };
        let Some(&placeholder) = cols.first() else {
            return Vec::new();
        };
        let allowed = self.backend.capabilities().pruning_keys;
        let mut keys = Vec::new();
        let mut unread = Vec::new();
        for column in &facts.columns {
            let Some(kind) = column.key.filter(|k| allowed.contains(k)) else {
                continue;
            };
            let col = self.scan_col(cols, column.name);
            unread.push(col.is_none());
            keys.push(match col {
                Some(col) => self.key(scan, col, kind, column),
                // A key the query never reads still breaks the key order,
                // so it holds a place until the backend has finished.
                None => PruneKey {
                    col: placeholder,
                    kind,
                    usage: PruneUse::NotFiltered,
                },
            });
        }
        self.backend.finish_keys(&mut keys);
        let mut unread = unread.into_iter();
        keys.retain(|_| !unread.next().unwrap_or(false));
        keys
    }

    fn scan_col(&self, cols: &[ColId], name: Symbol) -> Option<ColId> {
        cols.iter()
            .copied()
            .find(|c| self.mir.cols.get(*c).and_then(|def| def.name) == Some(name))
    }

    fn key(&self, scan: RelId, col: ColId, kind: KeyKind, column: &PhysColumn) -> PruneKey {
        let mut blocked = None;
        let mut unknown = false;
        let conjuncts = self.scan_filters.get(&scan).map_or(&[][..], Vec::as_slice);
        for scalar in conjuncts {
            let Some((_, verdict)) = decide_pruning(self.backend, self.mir, *scalar, col, column)
            else {
                continue;
            };
            match verdict {
                Pruning::Prunes => {
                    return PruneKey {
                        col,
                        kind,
                        usage: PruneUse::Used(*scalar),
                    }
                }
                Pruning::Blocks { why, .. } => {
                    blocked = blocked.or(Some(PruneUse::Blocked {
                        by: *scalar,
                        reason: why,
                    }));
                }
                Pruning::Unknown => unknown = true,
            }
        }
        let usage = match blocked {
            Some(usage) => usage,
            None if unknown => PruneUse::Unknown,
            None if self.filtered_elsewhere.contains(&col) => PruneUse::Unknown,
            None => PruneUse::NotFiltered,
        };
        PruneKey { col, kind, usage }
    }
}
