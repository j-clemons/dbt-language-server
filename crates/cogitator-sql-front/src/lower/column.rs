//! Column references: the binder's entry point for expressions.

use cogitator_ir::hir::{ColRef, ExprId, ExprKind, Ident, JinjaRole, Resolution};
use cogitator_ir::Span;
use sqlparser::ast;

use super::Lowerer;

impl Lowerer<'_> {
    pub(super) fn column(&mut self, path: &[ast::Ident]) -> ExprId {
        if let Some(region) = self.region_in_path(path) {
            return self.jinja_expr(region, JinjaRole::Scalar);
        }
        let span = self.path_span(path);
        let Some((name, quals)) = path.split_last() else {
            return self.alloc_expr(span, ExprKind::Hole);
        };
        let qualifier: Vec<Ident> = quals.iter().map(|q| self.ident(q)).collect();
        let name_ident = self.ident(name);
        let lambda_ref = qualifier.is_empty() && self.lambda.contains(&name_ident.folded);
        if lambda_ref {
            return self.alloc_expr(
                span,
                ExprKind::Opaque {
                    children: Vec::new(),
                },
            );
        }
        let syms: Vec<_> = qualifier.iter().map(|q| q.folded).collect();
        let res = self.resolve(&syms, name_ident.folded);
        let res = self.affordable(res);
        if res == Resolution::Unresolved {
            if let Some(field) = self.struct_field(path, span, &qualifier) {
                return field;
            }
        }
        self.col_ref(span, qualifier, name_ident, Some(res))
    }

    /// Candidate lists are stored per reference, so `n` references over
    /// `m` open relations cost `n * m`: they count against the node cap.
    pub(super) fn affordable(&mut self, res: Resolution) -> Resolution {
        match res {
            Resolution::Candidates(list) if !self.reserve(list.len()) => {
                Resolution::Candidates(Vec::new())
            }
            other => other,
        }
    }

    /// `a.b` where `a` is a column, not a relation: a struct field.
    pub(super) fn struct_field(
        &mut self,
        path: &[ast::Ident],
        span: Span,
        qualifier: &[Ident],
    ) -> Option<ExprId> {
        let first = *qualifier.first()?;
        // `t.s.g`: `t` names a FROM item and `s` is the struct column
        if let [_, second, ..] = qualifier {
            let qualified = self.resolve(&[first.folded], second.folded);
            let qualified = self.affordable(qualified);
            if qualified != Resolution::Unresolved {
                let col = self.col_ref(
                    self.ident_span(path.get(1)?),
                    vec![first],
                    *second,
                    Some(qualified),
                );
                return Some(self.alloc_expr(
                    span,
                    ExprKind::Opaque {
                        children: vec![col],
                    },
                ));
            }
        }
        let head = self.resolve(&[], first.folded);
        let head = self.affordable(head);
        if head == Resolution::Unresolved {
            return None;
        }
        let name = self.ident(path.first()?);
        let col = self.col_ref(self.ident_span(path.first()?), Vec::new(), name, Some(head));
        Some(self.alloc_expr(
            span,
            ExprKind::Opaque {
                children: vec![col],
            },
        ))
    }

    pub(super) fn col_ref(
        &mut self,
        span: Span,
        qualifier: Vec<Ident>,
        name: Ident,
        res: Option<Resolution>,
    ) -> ExprId {
        let col = self.hir.cols.alloc(ColRef {
            span,
            qualifier,
            name,
            res,
        });
        self.alloc_expr(span, ExprKind::Col(col))
    }

    pub(super) fn jinja_expr(&mut self, region: usize, role: JinjaRole) -> ExprId {
        let span = self.region_span(region);
        match self.take_region(region, role) {
            Some(id) => self.alloc_expr(span, ExprKind::Jinja(id)),
            None => self.alloc_expr(span, ExprKind::Hole),
        }
    }
}
