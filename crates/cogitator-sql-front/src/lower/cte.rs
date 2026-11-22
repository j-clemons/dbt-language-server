//! WITH clauses, parsed whole or recovered part by part.

use cogitator_ir::hir::{Cte, CteId, FromKind, QueryId, RelTarget, ScopeKind};
use sqlparser::ast;

use super::Lowerer;
use crate::parse::{Part, Parts};

impl Lowerer<'_> {
    pub(super) fn with_clause(&mut self, with: Option<&ast::With>) -> Vec<CteId> {
        let Some(with) = with else { return Vec::new() };
        let mut ids = Vec::with_capacity(with.cte_tables.len());
        for cte in &with.cte_tables {
            let columns: Vec<&ast::Ident> = cte.alias.columns.iter().map(|c| &c.name).collect();
            ids.push(self.cte(&cte.alias.name, &columns, with.recursive, |this| {
                this.query(&cte.query, ScopeKind::Cte, true)
            }));
        }
        ids
    }

    pub(super) fn cte_parts(&mut self, parts: &Parts) -> Vec<CteId> {
        let mut ids = Vec::with_capacity(parts.ctes.len());
        for cte in &parts.ctes {
            let columns: Vec<&ast::Ident> = cte.columns.iter().collect();
            ids.push(self.cte(
                &cte.name,
                &columns,
                parts.recursive,
                |this| match &cte.body {
                    Part::Parsed(query) => this.query(query, ScopeKind::Cte, true),
                    Part::Hole(kind, span) => this.hole_query(*kind, *span, Vec::new()),
                },
            ));
        }
        ids
    }

    /// Declares one CTE around the lowering of its body. A `WITH RECURSIVE`
    /// entry is visible to itself and marked recursive only if the body
    /// really refers to it.
    pub(super) fn cte(
        &mut self,
        name: &ast::Ident,
        columns: &[&ast::Ident],
        recursive: bool,
        body: impl FnOnce(&mut Self) -> QueryId,
    ) -> CteId {
        let name = self.ident(name);
        let columns: Vec<_> = columns.iter().map(|c| self.ident(c)).collect();
        // A placeholder query id; patched below once the body exists.
        let id = self.hir.ctes.alloc(Cte {
            span: name.span,
            name,
            columns,
            query: QueryId::from_raw(u32::MAX),
            recursive: false,
        });
        let first_from = self.hir.from.len();
        if recursive {
            self.visible.push((name.folded, id));
        }
        let query = body(self);
        let referenced = recursive && self.references_cte(first_from, id);
        if let Some(cte) = self.hir.ctes.get_mut(id) {
            cte.query = query;
            cte.recursive = referenced;
        }
        if !recursive {
            self.visible.push((name.folded, id));
        }
        id
    }

    /// True if any FROM item allocated since `first_from` targets `cte`.
    pub(super) fn references_cte(&self, first_from: usize, cte: CteId) -> bool {
        self.hir.from.iter().skip(first_from).any(|(_, item)| {
            matches!(&item.kind, FromKind::Base { target: RelTarget::Cte(c), .. } if *c == cte)
        })
    }
}
