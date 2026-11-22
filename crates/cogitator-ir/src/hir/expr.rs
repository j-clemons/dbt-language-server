//! Expressions, column references and resolution results.

use serde::Serialize;

use super::{ColRefId, ExprId, FromId, JinjaId, QueryId};
use crate::common::{BinaryOp, Lit, UnaryOp};
use crate::types::SqlType;
use crate::{Span, Symbol};

/// An identifier: `folded` is the dialect-folded name used for
/// resolution, `span` the exact text in the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Ident {
    pub span: Span,
    pub folded: Symbol,
    pub quoted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Expr {
    pub span: Span,
    pub kind: ExprKind,
}

#[derive(Debug, Clone, Serialize)]
pub enum ExprKind {
    Col(ColRefId),
    Lit(Lit),
    Unary {
        op: UnaryOp,
        e: ExprId,
    },
    Binary {
        op: BinaryOp,
        l: ExprId,
        r: ExprId,
    },
    Call {
        name: Symbol,
        args: Vec<ExprId>,
        distinct: bool,
        filter: Option<ExprId>,
        over: Option<WindowSpec>,
    },
    Case {
        operand: Option<ExprId>,
        arms: Vec<(ExprId, ExprId)>,
        else_: Option<ExprId>,
    },
    Cast {
        e: ExprId,
        ty: SqlType,
    },
    InList {
        e: ExprId,
        list: Vec<ExprId>,
        negated: bool,
    },
    InQuery {
        e: ExprId,
        q: QueryId,
        negated: bool,
    },
    Between {
        e: ExprId,
        lo: ExprId,
        hi: ExprId,
        negated: bool,
    },
    Exists(QueryId),
    Scalar(QueryId),
    Jinja(JinjaId),
    Opaque {
        children: Vec<ExprId>,
    },
    Hole,
}

impl ExprKind {
    /// Direct expression children (subqueries are separate roots).
    pub fn children(&self) -> Vec<ExprId> {
        match self {
            ExprKind::Col(_)
            | ExprKind::Lit(_)
            | ExprKind::Exists(_)
            | ExprKind::Scalar(_)
            | ExprKind::Jinja(_)
            | ExprKind::Hole => Vec::new(),
            ExprKind::Unary { e, .. } | ExprKind::Cast { e, .. } | ExprKind::InQuery { e, .. } => {
                vec![*e]
            }
            ExprKind::Binary { l, r, .. } => vec![*l, *r],
            ExprKind::Call {
                args, filter, over, ..
            } => {
                let mut out = args.clone();
                out.extend(filter.iter().copied());
                if let Some(spec) = over {
                    out.extend(spec.exprs());
                }
                out
            }
            ExprKind::Case {
                operand,
                arms,
                else_,
            } => {
                let mut out: Vec<ExprId> = operand.iter().copied().collect();
                out.extend(arms.iter().flat_map(|(w, t)| [*w, *t]));
                out.extend(else_.iter().copied());
                out
            }
            ExprKind::InList { e, list, .. } => {
                let mut out = vec![*e];
                out.extend(list.iter().copied());
                out
            }
            ExprKind::Between { e, lo, hi, .. } => vec![*e, *lo, *hi],
            ExprKind::Opaque { children } => children.clone(),
        }
    }

    /// True when the node's span comes straight from one token or Jinja
    /// region and may drive a fix. Every other `Expr` span is derived
    /// from its children's spans (sqlparser leaves most AST spans empty)
    /// and is approximate: fine for highlighting, never for an edit.
    pub fn span_is_exact(&self) -> bool {
        matches!(
            self,
            ExprKind::Col(_) | ExprKind::Lit(_) | ExprKind::Jinja(_)
        )
    }

    /// The subquery this expression opens, if any.
    pub fn subquery(&self) -> Option<QueryId> {
        match self {
            ExprKind::InQuery { q, .. } | ExprKind::Exists(q) | ExprKind::Scalar(q) => Some(*q),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WindowSpec {
    pub name: Option<Ident>,
    pub partition_by: Vec<ExprId>,
    pub order_by: Vec<super::OrderItem>,
    /// Frame text as written; frames are not interpreted yet.
    pub frame: Option<Symbol>,
}

impl WindowSpec {
    pub fn exprs(&self) -> Vec<ExprId> {
        let mut out = self.partition_by.clone();
        out.extend(self.order_by.iter().map(|o| o.expr));
        out
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ColRef {
    pub span: Span,
    pub qualifier: Vec<Ident>,
    pub name: Ident,
    /// `None` only when the binder never visited this reference, which
    /// the verifier reports; "no such column" is `Some(Unresolved)`.
    pub res: Option<Resolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Resolution {
    Bound { from: FromId, col: u32 },
    Outer { depth: u8, from: FromId, col: u32 },
    OpenSchema(FromId),
    Candidates(Vec<FromId>),
    LateralAlias(u32),
    Unresolved,
}
