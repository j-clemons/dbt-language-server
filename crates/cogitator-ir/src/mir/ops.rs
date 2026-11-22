//! Scalar expressions and the aggregate/window call shapes.

use serde::Serialize;

use super::{ColId, ProvId, RelId, ScalarId};
use crate::common::{BinaryOp, HoleRef, Lit, UnaryOp};
use crate::types::{ColType, SqlType};
use crate::Symbol;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scalar {
    pub kind: ScalarKind,
    pub ty: ColType,
    pub prov: ProvId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ScalarKind {
    Col(ColId),
    /// A correlated reference to a column of an enclosing query.
    Outer(ColId),
    Lit(Lit),
    Unary {
        op: UnaryOp,
        e: ScalarId,
    },
    Binary {
        op: BinaryOp,
        l: ScalarId,
        r: ScalarId,
    },
    Call {
        name: Symbol,
        args: Vec<ScalarId>,
        distinct: bool,
    },
    Case {
        operand: Option<ScalarId>,
        arms: Vec<(ScalarId, ScalarId)>,
        else_: Option<ScalarId>,
    },
    Cast {
        e: ScalarId,
        ty: SqlType,
    },
    InList {
        e: ScalarId,
        list: Vec<ScalarId>,
        negated: bool,
    },
    Subquery {
        kind: SubqueryKind,
        rel: RelId,
        /// The enclosing columns `rel` reads through `Outer`.
        correlated: Vec<ColId>,
    },
    Opaque {
        children: Vec<ScalarId>,
        hole: HoleRef,
    },
}

impl ScalarKind {
    pub fn children(&self) -> Vec<ScalarId> {
        match self {
            ScalarKind::Col(_) | ScalarKind::Outer(_) | ScalarKind::Lit(_) => Vec::new(),
            ScalarKind::Unary { e, .. } | ScalarKind::Cast { e, .. } => vec![*e],
            ScalarKind::Binary { l, r, .. } => vec![*l, *r],
            ScalarKind::Call { args, .. } => args.clone(),
            ScalarKind::Case {
                operand,
                arms,
                else_,
            } => {
                let mut out: Vec<ScalarId> = operand.iter().copied().collect();
                out.extend(arms.iter().flat_map(|(w, t)| [*w, *t]));
                out.extend(else_.iter().copied());
                out
            }
            ScalarKind::InList { e, list, .. } => {
                let mut out = vec![*e];
                out.extend(list.iter().copied());
                out
            }
            ScalarKind::Subquery { kind, .. } => match kind {
                SubqueryKind::In { e, .. } => vec![*e],
                SubqueryKind::Exists | SubqueryKind::Scalar => Vec::new(),
            },
            ScalarKind::Opaque { children, .. } => children.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SubqueryKind {
    Exists,
    Scalar,
    In { e: ScalarId, negated: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SortKey {
    pub expr: ScalarId,
    pub desc: Option<bool>,
    pub nulls_first: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AggCall {
    pub func: Symbol,
    pub args: Vec<ScalarId>,
    pub distinct: bool,
    pub filter: Option<ScalarId>,
}

impl AggCall {
    pub fn scalars(&self) -> Vec<ScalarId> {
        self.args.iter().chain(&self.filter).copied().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WinCall {
    pub func: Symbol,
    pub args: Vec<ScalarId>,
    pub partition_by: Vec<ScalarId>,
    pub order_by: Vec<SortKey>,
    pub frame: Option<Symbol>,
}

impl WinCall {
    pub fn scalars(&self) -> Vec<ScalarId> {
        let mut out: Vec<ScalarId> = self
            .args
            .iter()
            .chain(&self.partition_by)
            .copied()
            .collect();
        out.extend(self.order_by.iter().map(|k| k.expr));
        out
    }
}
