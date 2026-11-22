//! FROM items and what a base relation refers to.

use serde::Serialize;

use super::expr::Ident;
use super::{CteId, ExprId, FromId, JinjaId, QueryId};
use crate::common::JoinKind;
use crate::types::ColType;
use crate::{Span, Symbol};

#[derive(Debug, Clone, Serialize)]
pub struct From {
    pub span: Span,
    pub kind: FromKind,
    /// Known output columns, for `Resolution::Bound`'s `col` index.
    pub output: Vec<FromCol>,
    /// True when `output` may be missing columns (unknown schema).
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FromCol {
    pub name: Symbol,
    pub ty: ColType,
}

#[derive(Debug, Clone, Serialize)]
pub enum FromKind {
    Base {
        target: RelTarget,
        alias: Option<Ident>,
    },
    Derived {
        q: QueryId,
        lateral: bool,
        alias: Option<Ident>,
    },
    Join {
        kind: JoinKind,
        l: FromId,
        r: FromId,
        on: JoinCond,
    },
    Unnest {
        exprs: Vec<ExprId>,
        with_offset: bool,
        alias: Option<Ident>,
    },
    TableFn {
        name: Vec<Ident>,
        args: Vec<ExprId>,
        alias: Option<Ident>,
    },
    Hole,
}

#[derive(Debug, Clone, Serialize)]
pub enum RelTarget {
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
    Cte(CteId),
    Physical(Vec<Ident>),
    Jinja(JinjaId),
}

#[derive(Debug, Clone, Serialize)]
pub enum JoinCond {
    None,
    On(ExprId),
    Using(Vec<Ident>),
    Natural,
}
