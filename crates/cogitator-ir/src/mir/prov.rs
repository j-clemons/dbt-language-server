//! Provenance: every node traces back to the HIR through `Origin`.
//! Spans are kept alongside so findings can report on the authored
//! location without re-walking the HIR.

use serde::Serialize;

use super::ProvId;
use crate::common::RuleId;
use crate::hir::{ColRefId, ExprId, FromId, JinjaId, QueryId};
use crate::{Arena, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum HirNode {
    Query(QueryId),
    From(FromId),
    Expr(ExprId),
    Col(ColRefId),
    Jinja(JinjaId),
    /// No HIR node: hand-built plans (testkit) and lowering's scaffolding.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HirRef {
    pub node: HirNode,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Origin {
    Hir(HirRef),
    Derived { rule: RuleId, from: Vec<ProvId> },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProvTable {
    pub entries: Arena<Origin>,
}

impl ProvTable {
    pub fn add(&mut self, origin: Origin) -> ProvId {
        self.entries.alloc(origin)
    }

    /// The authored span behind `id`, following `Derived` to its first
    /// source. `from` ids precede their entry (verified), and the hop
    /// count is bounded anyway.
    pub fn span(&self, id: ProvId) -> Option<Span> {
        let mut at = id;
        for _ in 0..=self.entries.len() {
            match self.entries.get(at)? {
                Origin::Hir(r) => return Some(r.span),
                Origin::Derived { from, .. } => at = *from.first()?,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_follows_derived_chains() {
        let mut t = ProvTable::default();
        let base = t.add(Origin::Hir(HirRef {
            node: HirNode::None,
            span: Span::new(3, 9),
        }));
        let derived = t.add(Origin::Derived {
            rule: RuleId("canon.simplify"),
            from: vec![base],
        });
        assert_eq!(t.span(derived), Some(Span::new(3, 9)));
        assert_eq!(t.span(ProvId::from_raw(99)), None);
    }
}
