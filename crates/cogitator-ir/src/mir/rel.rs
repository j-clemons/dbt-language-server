//! Relational operators.

use serde::Serialize;

use super::ops::{AggCall, Scalar, ScalarKind, SortKey, WinCall};
use super::{ColId, CteId, ProvId, RelId, ScalarId, TableId};
use crate::common::{HoleRef, JoinKind, SetOpKind};
use crate::schema::Completeness;
use crate::{Arena, Symbol};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rel {
    pub op: RelOp,
    /// Declared output; the verifier compares it with `computed_output`.
    pub out: Vec<ColId>,
    pub prov: ProvId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupingSets(pub Vec<Vec<ColId>>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RelOp {
    Scan {
        table: TableId,
        cols: Vec<ColId>,
        open: bool,
    },
    Filter {
        input: RelId,
        conjuncts: Vec<ScalarId>,
    },
    Project {
        input: RelId,
        items: Vec<(ColId, ScalarId)>,
    },
    Join {
        kind: JoinKind,
        l: RelId,
        r: RelId,
        on: Vec<ScalarId>,
    },
    Aggregate {
        input: RelId,
        keys: Vec<(ColId, ScalarId)>,
        aggs: Vec<(ColId, AggCall)>,
        sets: Option<GroupingSets>,
    },
    Window {
        input: RelId,
        funcs: Vec<(ColId, WinCall)>,
    },
    Distinct(RelId),
    SetOp {
        op: SetOpKind,
        all: bool,
        inputs: Vec<RelId>,
        out: Vec<ColId>,
    },
    Sort {
        input: RelId,
        keys: Vec<SortKey>,
    },
    Limit {
        input: RelId,
        limit: Option<ScalarId>,
        offset: Option<ScalarId>,
    },
    Unnest {
        input: RelId,
        exprs: Vec<ScalarId>,
        out: Vec<ColId>,
    },
    CteRef {
        cte: CteId,
        out: Vec<ColId>,
    },
    Opaque {
        inputs: Vec<RelId>,
        out: Vec<ColId>,
        open: bool,
        hole: HoleRef,
    },
}

impl RelOp {
    pub fn inputs(&self) -> Vec<RelId> {
        match self {
            RelOp::Scan { .. } | RelOp::CteRef { .. } => Vec::new(),
            RelOp::Filter { input, .. }
            | RelOp::Project { input, .. }
            | RelOp::Aggregate { input, .. }
            | RelOp::Window { input, .. }
            | RelOp::Distinct(input)
            | RelOp::Sort { input, .. }
            | RelOp::Limit { input, .. }
            | RelOp::Unnest { input, .. } => vec![*input],
            RelOp::Join { l, r, .. } => vec![*l, *r],
            RelOp::SetOp { inputs, .. } | RelOp::Opaque { inputs, .. } => inputs.clone(),
        }
    }

    /// The scalars this node evaluates directly (roots of their trees).
    pub fn scalars(&self) -> Vec<ScalarId> {
        match self {
            RelOp::Scan { .. }
            | RelOp::Distinct(_)
            | RelOp::SetOp { .. }
            | RelOp::CteRef { .. }
            | RelOp::Opaque { .. } => Vec::new(),
            RelOp::Filter { conjuncts: s, .. } | RelOp::Join { on: s, .. } => s.clone(),
            RelOp::Unnest { exprs, .. } => exprs.clone(),
            RelOp::Project { items, .. } => items.iter().map(|(_, s)| *s).collect(),
            RelOp::Aggregate { keys, aggs, .. } => {
                let mut out: Vec<ScalarId> = keys.iter().map(|(_, s)| *s).collect();
                out.extend(aggs.iter().flat_map(|(_, a)| a.scalars()));
                out
            }
            RelOp::Window { funcs, .. } => funcs.iter().flat_map(|(_, w)| w.scalars()).collect(),
            RelOp::Sort { keys, .. } => keys.iter().map(|k| k.expr).collect(),
            RelOp::Limit { limit, offset, .. } => limit.iter().chain(offset).copied().collect(),
        }
    }

    /// Columns this node introduces. A `(c, Col(c))` item in a Project
    /// or Aggregate passes `c` through and does not define it again.
    pub fn defined(&self, scalars: &Arena<Scalar>) -> Vec<ColId> {
        let passthrough = |c: ColId, s: ScalarId| matches!(scalars.get(s).map(|s| &s.kind), Some(ScalarKind::Col(x)) if *x == c);
        match self {
            RelOp::Filter { .. }
            | RelOp::Join { .. }
            | RelOp::Distinct(_)
            | RelOp::Sort { .. }
            | RelOp::Limit { .. } => Vec::new(),
            RelOp::Scan { cols, .. } => cols.clone(),
            RelOp::SetOp { out, .. }
            | RelOp::Unnest { out, .. }
            | RelOp::CteRef { out, .. }
            | RelOp::Opaque { out, .. } => out.clone(),
            RelOp::Project { items, .. } => items
                .iter()
                .filter(|(c, s)| !passthrough(*c, *s))
                .map(|(c, _)| *c)
                .collect(),
            RelOp::Aggregate { keys, aggs, .. } => keys
                .iter()
                .filter(|(c, s)| !passthrough(*c, *s))
                .map(|(c, _)| *c)
                .chain(aggs.iter().map(|(c, _)| *c))
                .collect(),
            RelOp::Window { funcs, .. } => funcs.iter().map(|(c, _)| *c).collect(),
        }
    }

    /// The output this node must have, from its inputs' declared output.
    pub fn computed_output(&self, rels: &Arena<Rel>) -> Vec<ColId> {
        let of = |id: RelId| -> &[ColId] { rels.get(id).map_or(&[], |r| r.out.as_slice()) };
        match self {
            RelOp::Scan { cols, .. } => cols.clone(),
            RelOp::Filter { input, .. }
            | RelOp::Distinct(input)
            | RelOp::Sort { input, .. }
            | RelOp::Limit { input, .. } => of(*input).to_vec(),
            RelOp::Project { items, .. } => items.iter().map(|(c, _)| *c).collect(),
            RelOp::Join { kind, l, r, .. } => {
                let mut out = of(*l).to_vec();
                if !matches!(kind, JoinKind::Semi | JoinKind::Anti) {
                    out.extend_from_slice(of(*r));
                }
                out
            }
            RelOp::Aggregate { keys, aggs, .. } => keys
                .iter()
                .map(|(c, _)| *c)
                .chain(aggs.iter().map(|(c, _)| *c))
                .collect(),
            RelOp::Window { input, funcs } => {
                let mut out = of(*input).to_vec();
                out.extend(funcs.iter().map(|(c, _)| *c));
                out
            }
            RelOp::SetOp { out, .. } | RelOp::CteRef { out, .. } | RelOp::Opaque { out, .. } => {
                out.clone()
            }
            RelOp::Unnest { input, out, .. } => {
                let mut all = of(*input).to_vec();
                all.extend_from_slice(out);
                all
            }
        }
    }
}

/// What a `Scan` reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TableRef {
    Model {
        package: Option<Symbol>,
        name: Symbol,
        version: Option<Symbol>,
    },
    Source {
        source: Symbol,
        table: Symbol,
    },
    This,
    Physical(Vec<Symbol>),
    Jinja,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableDef {
    pub target: TableRef,
    pub completeness: Completeness,
}
