//! Vocabulary shared by more than one IR level.

use serde::Serialize;

use crate::{Span, Symbol};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    Semi,
    Anti,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum SetOpKind {
    Union,
    Intersect,
    Except,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum UnaryOp {
    Not,
    Neg,
    IsNull,
    IsNotNull,
    IsTrue,
    IsFalse,
    Other(Symbol),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum BinaryOp {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    Concat,
    Like,
    NotLike,
    ILike,
    NotILike,
    Other(Symbol),
}

/// Literal text stays raw: the IR never does arithmetic on it, and
/// dialect number syntax does not fit a fixed-width integer anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Lit {
    Null,
    Bool(bool),
    Number(Symbol),
    Str(Symbol),
    Typed { ty: Symbol, value: Symbol },
    Interval(Symbol),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum HoleKind {
    ParseError,
    Unsupported,
    Jinja,
    /// A front-end cap (text, tokens, depth, nodes) cut the input short.
    Truncated,
}

/// Where a MIR/LIR `Opaque` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct HoleRef {
    pub kind: HoleKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Materialization {
    Table,
    View,
    Incremental,
    Ephemeral,
    MaterializedView,
    Snapshot,
    Other(Symbol),
}

/// The subset of dbt model config the pipeline reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModelConfig {
    pub materialized: Option<Materialization>,
    pub unique_key: Vec<Symbol>,
    pub partition_by: Vec<Symbol>,
    pub cluster_by: Vec<Symbol>,
    pub incremental_strategy: Option<Symbol>,
    pub contract_enforced: bool,
    /// One entry per declared index, its columns in order.
    pub indexes: Vec<Vec<Symbol>>,
}

/// A rule id (`join.cartesian`); the registry in `cogitator-rules` owns
/// the valid set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RuleId(pub &'static str);

impl RuleId {
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// The three IR levels, in pipeline order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Level {
    Hir,
    Mir,
    Lir,
}
