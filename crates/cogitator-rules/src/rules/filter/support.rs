//! Reading a verified LIR with its MIR: shared by the pruning-aware rules
//! in `filter` and `incr`.

use std::collections::BTreeMap;

use cogitator_ir::hir::Hir;
use cogitator_ir::lir::{Lir, PhysId, PhysOp, PruneKey};
use cogitator_ir::mir::{ColId, Mir, RelId, RelOp, ScalarId, ScalarKind, TableId};
use cogitator_ir::origin::{SourceOrigin, Staleness};
use cogitator_ir::schema::Completeness;
use cogitator_ir::Span;

use crate::findings::Evidence;

/// Vendor pages the rules cite. The `cogitator-lower` backends hold the
/// same URLs and this crate cannot depend on that one outside tests, so
/// `tests/filter_incr.rs` checks each message against the backend's.
pub(crate) const BIGQUERY_PARTITION_DOC: &str =
    "https://docs.cloud.google.com/bigquery/docs/querying-partitioned-tables";
pub(crate) const BIGQUERY_CLUSTER_DOC: &str =
    "https://docs.cloud.google.com/bigquery/docs/querying-clustered-tables";
pub(crate) const BIGQUERY_COSTS_DOC: &str =
    "https://docs.cloud.google.com/bigquery/docs/best-practices-costs";
pub(crate) const SNOWFLAKE_CLUSTERING_DOC: &str =
    "https://docs.snowflake.com/en/user-guide/tables-clustering-micropartitions";
pub(crate) const POSTGRES_EXPRESSION_DOC: &str =
    "https://www.postgresql.org/docs/current/indexes-expressional.html";

/// One `TableScan` of the plan.
pub(crate) struct ScanView<'a> {
    pub id: PhysId,
    pub mir: RelId,
    pub table: TableId,
    pub keys: &'a [PruneKey],
}

/// Every `TableScan`, in allocation order.
pub(crate) fn scans(lir: &Lir) -> impl Iterator<Item = ScanView<'_>> {
    lir.nodes.iter().filter_map(|(id, node)| match &node.op {
        PhysOp::TableScan { table, pruning, .. } => Some(ScanView {
            id,
            mir: node.mir,
            table: *table,
            keys: pruning.as_slice(),
        }),
        _ => None,
    })
}

/// Conjuncts of the `Filter`s directly above each scan, through stacked
/// Filters: the same predicates the backend judged.
pub(crate) fn scan_conjuncts(lir: &Lir) -> BTreeMap<PhysId, Vec<ScalarId>> {
    let mut map: BTreeMap<PhysId, Vec<ScalarId>> = BTreeMap::new();
    for node in lir.nodes.values() {
        let PhysOp::Filter { input, conjuncts } = &node.op else {
            continue;
        };
        let mut below = *input;
        for _ in 0..lir.nodes.len() {
            match lir.nodes.get(below).map(|n| &n.op) {
                Some(PhysOp::Filter { input, .. }) => below = *input,
                Some(PhysOp::TableScan { .. }) => {
                    map.entry(below)
                        .or_default()
                        .extend(conjuncts.iter().copied());
                    break;
                }
                _ => break,
            }
        }
    }
    map
}

/// True when the scan's columns are all known. An open schema can hide a
/// column of the same name, so nothing may be claimed about it.
pub(crate) fn scan_is_closed(mir: &Mir, rel: RelId, table: TableId) -> bool {
    let open = matches!(
        mir.rels.get(rel).map(|r| &r.op),
        Some(RelOp::Scan { open: true, .. }) | None
    );
    let complete = mir
        .tables
        .get(table)
        .is_some_and(|t| t.completeness == Completeness::Closed);
    !open && complete
}

/// True when `found` holds for some node of the scalar tree at `root`.
pub(crate) fn any_scalar(
    mir: &Mir,
    root: ScalarId,
    mut found: impl FnMut(&ScalarKind) -> bool,
) -> bool {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if let Some(scalar) = mir.scalars.get(id) {
            if found(&scalar.kind) {
                return true;
            }
            stack.extend(scalar.kind.children());
        }
    }
    false
}

pub(crate) fn reads_col(mir: &Mir, root: ScalarId, col: ColId) -> bool {
    any_scalar(
        mir,
        root,
        |k| matches!(k, ScalarKind::Col(c) | ScalarKind::Outer(c) if *c == col),
    )
}

/// True when a hole or unmodelled expression sits in the tree: its
/// meaning is unknown, so it supports no claim.
pub(crate) fn touches_opaque(mir: &Mir, root: ScalarId) -> bool {
    any_scalar(mir, root, |k| matches!(k, ScalarKind::Opaque { .. }))
}

pub(crate) fn scalar_span(mir: &Mir, id: ScalarId) -> Span {
    mir.scalars
        .get(id)
        .and_then(|s| mir.prov.span(s.prov))
        .unwrap_or_default()
}

pub(crate) fn col_name(mir: &Mir, col: ColId) -> &str {
    mir.cols
        .get(col)
        .and_then(|c| c.name)
        .and_then(|name| mir.syms.resolve(name))
        .unwrap_or("this column")
}

/// `Exact` needs a plan known to match the document (06): live template
/// text, or compiled SQL that is `Fresh`.
pub(crate) fn plan_evidence(hir: Option<&Hir>) -> Evidence {
    match hir.map(|h| &h.origin) {
        Some(SourceOrigin::Template { .. })
        | Some(SourceOrigin::Compiled {
            staleness: Staleness::Fresh,
            ..
        }) => Evidence::Exact,
        _ => Evidence::Heuristic,
    }
}
