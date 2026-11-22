//! HIR: surface-faithful, name-resolved SQL with Jinja as first-class
//! nodes (specs/ir-pipeline/01). Spans point at the user's document.
//!
//! Numeric ids reference arena slots in allocation order; a CTE arena
//! was added beside the design's arenas so `CteId` has an owner.

pub mod expr;
pub mod from;
pub mod jinja;
pub mod print;
pub mod query;
pub mod verify;

use serde::Serialize;

use crate::common::ModelConfig;
use crate::origin::SourceOrigin;
use crate::{Arena, Idx, Interner};

pub use expr::{ColRef, Expr, ExprKind, Ident, Resolution, WindowSpec};
pub use from::{From, FromCol, FromKind, JoinCond, RelTarget};
pub use jinja::{JinjaKind, JinjaNode, JinjaRole};
pub use print::print_hir;
pub use query::{
    Body, Cte, Distinct, GroupBy, OrderItem, OutputCol, Query, Scope, ScopeKind, Select, SelectItem,
};
pub use verify::verify_hir;

pub type QueryId = Idx<Query>;
pub type ScopeId = Idx<Scope>;
pub type CteId = Idx<Cte>;
pub type FromId = Idx<From>;
pub type ExprId = Idx<Expr>;
pub type ColRefId = Idx<ColRef>;
pub type JinjaId = Idx<JinjaNode>;

#[derive(Debug, Clone, Serialize)]
pub struct Hir {
    pub origin: SourceOrigin,
    pub syms: Interner,
    pub queries: Arena<Query>,
    pub scopes: Arena<Scope>,
    pub ctes: Arena<Cte>,
    pub from: Arena<From>,
    pub exprs: Arena<Expr>,
    pub cols: Arena<ColRef>,
    pub jinja: Arena<JinjaNode>,
    pub config: ModelConfig,
    pub root: Option<QueryId>,
}

impl Hir {
    pub fn new(origin: SourceOrigin) -> Self {
        Self {
            origin,
            syms: Interner::new(),
            queries: Arena::new(),
            scopes: Arena::new(),
            ctes: Arena::new(),
            from: Arena::new(),
            exprs: Arena::new(),
            cols: Arena::new(),
            jinja: Arena::new(),
            config: ModelConfig::default(),
            root: None,
        }
    }
}

/// Total nodes across the arenas, for the front-end node cap.
pub fn node_count(hir: &Hir) -> usize {
    hir.queries.len() + hir.from.len() + hir.exprs.len() + hir.cols.len() + hir.jinja.len()
}
