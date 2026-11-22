//! Queries, SELECT bodies, scopes and CTE declarations.

use serde::Serialize;

use super::expr::{Ident, WindowSpec};
use super::{CteId, ExprId, FromId, JinjaId, QueryId, ScopeId};
use crate::common::{HoleKind, SetOpKind};
use crate::types::ColType;
use crate::{Span, Symbol};

#[derive(Debug, Clone, Serialize)]
pub struct Query {
    pub span: Span,
    pub scope: ScopeId,
    pub ctes: Vec<CteId>,
    pub body: Body,
    pub order_by: Vec<OrderItem>,
    pub limit: Option<ExprId>,
    pub outputs: Vec<OutputCol>,
}

#[derive(Debug, Clone, Serialize)]
pub enum Body {
    Select(Box<Select>),
    SetOp {
        op: SetOpKind,
        all: bool,
        by_name: bool,
        l: QueryId,
        r: QueryId,
    },
    Values(Vec<Vec<ExprId>>),
    Hole(HoleKind),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Select {
    pub span: Span,
    pub distinct: Distinct,
    pub items: Vec<SelectItem>,
    /// Comma-separated FROM items, in source order.
    pub from: Vec<FromId>,
    pub filter: Option<ExprId>,
    pub group_by: GroupBy,
    pub having: Option<ExprId>,
    pub qualify: Option<ExprId>,
    pub windows: Vec<(Ident, WindowSpec)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub enum Distinct {
    #[default]
    None,
    All,
    On(Vec<ExprId>),
}

#[derive(Debug, Clone, Serialize)]
pub enum SelectItem {
    Expr {
        expr: ExprId,
        alias: Option<Ident>,
    },
    Star {
        span: Span,
        /// `t.*`'s resolved FROM item; `None` for a bare `*`.
        from: Option<FromId>,
        qualifier: Option<Ident>,
        exclude: Vec<Ident>,
        replace: Vec<(ExprId, Ident)>,
    },
    Jinja(JinjaId),
    Hole(Span),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub enum GroupBy {
    #[default]
    None,
    Exprs(Vec<ExprId>),
    All,
    Sets(Vec<Vec<ExprId>>),
    Rollup(Vec<ExprId>),
    Cube(Vec<ExprId>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OrderItem {
    pub expr: ExprId,
    pub desc: Option<bool>,
    pub nulls_first: Option<bool>,
}

/// One output column of a query. A star over an open schema leaves the
/// remaining outputs unknown; the query's scope records that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputCol {
    pub span: Span,
    pub name: Option<Symbol>,
    pub expr: Option<ExprId>,
    pub ty: ColType,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cte {
    pub span: Span,
    pub name: Ident,
    pub columns: Vec<Ident>,
    pub query: QueryId,
    pub recursive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ScopeKind {
    Root,
    Query,
    Cte,
    Derived,
    Subquery,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scope {
    pub span: Span,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    /// FROM items visible in this scope (name resolution order).
    pub from: Vec<FromId>,
}

impl Select {
    /// Every expression the SELECT itself holds, in clause order.
    pub fn exprs(&self) -> Vec<ExprId> {
        let mut out = Vec::new();
        if let Distinct::On(exprs) = &self.distinct {
            out.extend(exprs.iter().copied());
        }
        for item in &self.items {
            match item {
                SelectItem::Expr { expr, .. } => out.push(*expr),
                SelectItem::Star { replace, .. } => {
                    out.extend(replace.iter().map(|(e, _)| *e));
                }
                SelectItem::Jinja(_) | SelectItem::Hole(_) => {}
            }
        }
        out.extend(self.filter);
        match &self.group_by {
            GroupBy::Exprs(e) | GroupBy::Rollup(e) | GroupBy::Cube(e) => {
                out.extend(e.iter().copied());
            }
            GroupBy::Sets(sets) => out.extend(sets.iter().flatten().copied()),
            GroupBy::None | GroupBy::All => {}
        }
        out.extend(self.having);
        out.extend(self.qualify);
        for (_, spec) in &self.windows {
            out.extend(spec.exprs());
        }
        out
    }
}

impl Query {
    /// Body expressions plus ORDER BY and LIMIT.
    pub fn exprs(&self) -> Vec<ExprId> {
        let mut out = match &self.body {
            Body::Select(select) => select.exprs(),
            Body::Values(rows) => rows.iter().flatten().copied().collect(),
            Body::SetOp { .. } | Body::Hole(_) => Vec::new(),
        };
        out.extend(self.order_by.iter().map(|o| o.expr));
        out.extend(self.limit);
        out
    }
}
