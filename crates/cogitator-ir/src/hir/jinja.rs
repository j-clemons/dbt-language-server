//! Jinja as first-class HIR nodes.

use serde::Serialize;

use crate::{Span, Symbol};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct JinjaNode {
    pub span: Span,
    pub kind: JinjaKind,
    pub role: JinjaRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum JinjaKind {
    Ref,
    Source,
    This,
    Var,
    Macro(Symbol),
    IsIncremental,
    Config,
    Expression,
    Statement,
    Comment,
}

/// How a region participates in the SQL around it. Relation, Scalar and
/// Composite regions must be consumed by exactly one node; Blank and
/// ClausePosition regions need no consumer (see specs/ir-pipeline/01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum JinjaRole {
    Relation,
    Scalar,
    Blank,
    Composite,
    ClausePosition,
}
