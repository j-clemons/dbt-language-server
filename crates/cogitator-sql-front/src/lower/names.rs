//! Identifiers and the placeholder-to-region lookup.

use cogitator_ir::hir::{Ident, JinjaId, JinjaRole};
use cogitator_ir::{Span, Symbol};
use sqlparser::ast;

use super::Lowerer;
use crate::mask::Region;

impl Lowerer<'_> {
    /// Exact byte span of a token-derived sqlparser span.
    pub(super) fn sql_span(&self, span: sqlparser::tokenizer::Span) -> Option<Span> {
        self.pos.span(span)
    }

    pub(super) fn ident_span(&self, ident: &ast::Ident) -> Span {
        self.sql_span(ident.span).unwrap_or_default()
    }

    pub(super) fn path_span(&self, path: &[ast::Ident]) -> Span {
        let span_of = |i: &ast::Ident| self.sql_span(i.span);
        match (
            path.first().and_then(span_of),
            path.last().and_then(span_of),
        ) {
            (Some(first), Some(last)) => Span::new(first.start, last.end),
            (one, _) => one.unwrap_or_default(),
        }
    }

    pub(super) fn ident(&mut self, ident: &ast::Ident) -> Ident {
        let folded = self.rules.fold_ident(ident);
        let folded = self.hir.syms.intern(&folded);
        Ident {
            span: self.ident_span(ident),
            folded,
            quoted: ident.quote_style.is_some(),
        }
    }

    pub(super) fn sym_for(&mut self, ident: &ast::Ident) -> Symbol {
        let folded = self.rules.fold_ident(ident);
        self.hir.syms.intern(&folded)
    }

    pub(super) fn intern(&mut self, text: &str) -> Symbol {
        self.hir.syms.intern(text)
    }

    /// `alias` unless it is a placeholder, whose text is not a name.
    pub(super) fn alias_ident(&mut self, ident: &ast::Ident) -> Option<Ident> {
        let span = self.ident_span(ident);
        if self.region_at(span).is_some() {
            return None;
        }
        Some(self.ident(ident))
    }

    /// The region whose placeholder overlaps `span`.
    pub(super) fn region_at(&self, span: Span) -> Option<usize> {
        let i = self.words.partition_point(|(w, _)| w.end <= span.start);
        self.words
            .get(i)
            .filter(|(w, _)| w.start < span.end)
            .map(|(_, r)| *r)
    }

    /// The region overlapping any ident in `path`.
    pub(super) fn region_in_path(&self, path: &[ast::Ident]) -> Option<usize> {
        path.iter()
            .find_map(|ident| self.region_at(self.ident_span(ident)))
    }

    /// Claims region `index` for one consumer, setting its role. `None`
    /// when it was already taken, which the caller treats as a hole.
    pub(super) fn take_region(&mut self, index: usize, role: JinjaRole) -> Option<JinjaId> {
        let slot = self.consumed.get_mut(index)?;
        if std::mem::replace(slot, true) {
            return None;
        }
        let id = JinjaId::from_raw(index as u32);
        if let Some(node) = self.hir.jinja.get_mut(id) {
            node.role = role;
        }
        Some(id)
    }

    pub(super) fn region(&self, index: usize) -> Option<&Region> {
        self.regions.get(index)
    }

    /// The region's span, for a node standing in for it.
    pub(super) fn region_span(&self, index: usize) -> Span {
        self.region(index).map_or(Span::default(), |r| r.span)
    }
}
