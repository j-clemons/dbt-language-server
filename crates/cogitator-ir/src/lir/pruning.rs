//! Pruning-key facts, per-backend capability tables, and the inputs to
//! `DialectBackend::pruning` (in `cogitator-lower`).

use std::collections::BTreeMap;

use serde::Serialize;

use super::Backend;
use crate::mir::{ColId, ScalarId, TableId};
use crate::types::SqlType;
use crate::Symbol;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum KeyKind {
    Partition,
    Cluster,
    Sort,
    Unique,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BlockReason {
    FunctionWrapped,
    CastWrapped,
    SubqueryValue,
    OrAcrossColumns,
    NotAPruningKey,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PruneUse {
    Used(ScalarId),
    Blocked { by: ScalarId, reason: BlockReason },
    NotFiltered,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PruneKey {
    pub col: ColId,
    pub kind: KeyKind,
    pub usage: PruneUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum JoinHint {
    None,
    Hash,
    Merge,
    NestedLoop,
    Broadcast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum CtePolicy {
    Inline,
    Materialize,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Side {
    Left,
    Right,
}

/// A backend's capability table. `doc` links its pruning documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub backend: Backend,
    pub join_hints: &'static [JoinHint],
    pub cte_policies: &'static [CtePolicy],
    pub pruning_keys: &'static [KeyKind],
    pub doc: &'static str,
}

/// The shape of a predicate over one column, enough to decide pruning
/// without a full expression walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PredShape {
    BareCompare,
    Range,
    Function { name: Symbol },
    Cast,
    Subquery,
    OrAcrossColumns,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Pred {
    pub scalar: ScalarId,
    pub shape: PredShape,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhysColumn {
    pub name: Symbol,
    pub ty: SqlType,
    pub key: Option<KeyKind>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TableFacts {
    pub columns: Vec<PhysColumn>,
    pub row_count: Option<u64>,
}

/// Physical metadata per table, from the catalog or manifest, keyed by
/// `TableId` in a `BTreeMap` so iteration order is defined.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PhysicalFacts {
    pub tables: BTreeMap<u32, TableFacts>,
}

impl PhysicalFacts {
    pub fn table(&self, id: TableId) -> Option<&TableFacts> {
        self.tables.get(&id.raw())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pruning {
    Prunes,
    Blocks { why: BlockReason, doc: &'static str },
    Unknown,
}
