//! Hand-built HIR. Every constructor takes the node's span so rule tests
//! can report on real positions; use `span_of` to find one in the text.

use crate::common::{BinaryOp, JoinKind, Lit};
use crate::hir::{
    Body, ColRef, Cte, CteId, Expr, ExprId, ExprKind, From, FromCol, FromId, FromKind, Hir, Ident,
    JinjaId, JinjaKind, JinjaNode, JinjaRole, JoinCond, OutputCol, Query, QueryId, RelTarget,
    Resolution, Scope, ScopeId, ScopeKind, Select, SelectItem,
};
use crate::origin::{SourceOrigin, TemplateVariant};
use crate::{ColType, Span, Symbol};

pub struct HirBuilder {
    pub hir: Hir,
    text: String,
    root_scope: ScopeId,
}

impl HirBuilder {
    /// `text` is what the spans index; `build` passes it to `verify_hir`.
    pub fn new(text: &str) -> Self {
        let mut hir = Hir::new(SourceOrigin::Template {
            variant: TemplateVariant::Plain,
        });
        let root_scope = hir.scopes.alloc(Scope {
            span: Span::new(0, text.len() as u32),
            parent: None,
            kind: ScopeKind::Root,
            from: Vec::new(),
        });
        Self {
            hir,
            text: text.to_string(),
            root_scope,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The span of the first occurrence of `needle` in the text.
    pub fn span_of(&self, needle: &str) -> Span {
        let start = self.text.find(needle).unwrap_or(0) as u32;
        Span::new(start, start + needle.len() as u32)
    }

    pub fn sym(&mut self, text: &str) -> Symbol {
        self.hir.syms.intern(text)
    }

    pub fn ident(&mut self, span: Span, name: &str) -> Ident {
        Ident {
            span,
            folded: self.sym(name),
            quoted: false,
        }
    }

    fn output(&mut self, cols: &[&str]) -> Vec<FromCol> {
        cols.iter()
            .map(|name| FromCol {
                name: self.sym(name),
                ty: ColType::UNKNOWN,
            })
            .collect()
    }

    fn add_from(&mut self, span: Span, kind: FromKind, cols: &[&str]) -> FromId {
        let output = self.output(cols);
        self.hir.from.alloc(From {
            span,
            kind,
            output,
            open: false,
        })
    }

    pub fn base_model(
        &mut self,
        span: Span,
        name: &str,
        alias: Option<&str>,
        cols: &[&str],
    ) -> FromId {
        let name = self.sym(name);
        let alias = alias.map(|a| self.ident(span, a));
        let target = RelTarget::Model {
            package: None,
            name,
            version: None,
        };
        self.add_from(span, FromKind::Base { target, alias }, cols)
    }

    pub fn base_cte(
        &mut self,
        span: Span,
        cte: CteId,
        alias: Option<&str>,
        cols: &[&str],
    ) -> FromId {
        let alias = alias.map(|a| self.ident(span, a));
        let target = RelTarget::Cte(cte);
        self.add_from(span, FromKind::Base { target, alias }, cols)
    }

    pub fn base_jinja(&mut self, span: Span, jinja: JinjaId, cols: &[&str]) -> FromId {
        let target = RelTarget::Jinja(jinja);
        let kind = FromKind::Base {
            target,
            alias: None,
        };
        self.add_from(span, kind, cols)
    }

    pub fn join(
        &mut self,
        span: Span,
        kind: JoinKind,
        l: FromId,
        r: FromId,
        on: JoinCond,
    ) -> FromId {
        self.add_from(span, FromKind::Join { kind, l, r, on }, &[])
    }

    pub fn expr(&mut self, span: Span, kind: ExprKind) -> ExprId {
        self.hir.exprs.alloc(Expr { span, kind })
    }

    pub fn number(&mut self, span: Span, text: &str) -> ExprId {
        let sym = self.sym(text);
        self.expr(span, ExprKind::Lit(Lit::Number(sym)))
    }

    pub fn binary(&mut self, span: Span, op: BinaryOp, l: ExprId, r: ExprId) -> ExprId {
        self.expr(span, ExprKind::Binary { op, l, r })
    }

    /// A column reference resolved to `col` of `from`.
    pub fn column(&mut self, span: Span, name: &str, from: FromId, col: u32) -> ExprId {
        self.col_expr(span, name, Some(Resolution::Bound { from, col }))
    }

    /// A column reference with an explicit (or absent) resolution.
    pub fn col_expr(&mut self, span: Span, name: &str, res: Option<Resolution>) -> ExprId {
        let name = self.ident(span, name);
        let id = self.hir.cols.alloc(ColRef {
            span,
            qualifier: Vec::new(),
            name,
            res,
        });
        self.expr(span, ExprKind::Col(id))
    }

    pub fn jinja(&mut self, span: Span, kind: JinjaKind, role: JinjaRole) -> JinjaId {
        self.hir.jinja.alloc(JinjaNode { span, kind, role })
    }

    /// A query over `from`, selecting `items`, with the given CTEs.
    pub fn select(
        &mut self,
        span: Span,
        ctes: Vec<CteId>,
        items: Vec<SelectItem>,
        from: Vec<FromId>,
        filter: Option<ExprId>,
    ) -> QueryId {
        let select = Select {
            span,
            items,
            from,
            filter,
            ..Select::default()
        };
        self.query(span, ctes, Body::Select(Box::new(select)))
    }

    pub fn query(&mut self, span: Span, ctes: Vec<CteId>, body: Body) -> QueryId {
        let scope = self.hir.scopes.alloc(Scope {
            span,
            parent: Some(self.root_scope),
            kind: ScopeKind::Query,
            from: Vec::new(),
        });
        let outputs = self.outputs_of(&body);
        self.hir.queries.alloc(Query {
            span,
            scope,
            ctes,
            body,
            order_by: Vec::new(),
            limit: None,
            outputs,
        })
    }

    fn outputs_of(&self, body: &Body) -> Vec<OutputCol> {
        let Body::Select(select) = body else {
            return Vec::new();
        };
        select
            .items
            .iter()
            .filter_map(|item| match item {
                SelectItem::Expr { expr, alias } => Some(OutputCol {
                    span: self
                        .hir
                        .exprs
                        .get(*expr)
                        .map_or(Span::default(), |e| e.span),
                    name: alias.map(|a| a.folded),
                    expr: Some(*expr),
                    ty: ColType::UNKNOWN,
                }),
                _ => None,
            })
            .collect()
    }

    pub fn cte(&mut self, span: Span, name: &str, query: QueryId) -> CteId {
        let name = self.ident(span, name);
        self.hir.ctes.alloc(Cte {
            span,
            name,
            columns: Vec::new(),
            query,
            recursive: false,
        })
    }

    pub fn finish(mut self, root: QueryId) -> Hir {
        self.hir.root = Some(root);
        self.hir
    }
}
