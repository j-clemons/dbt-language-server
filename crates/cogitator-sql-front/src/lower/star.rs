//! `SELECT *` and `t.*` with EXCLUDE/REPLACE, expanded against the FROM
//! items as far as their outputs are known.

use cogitator_ir::hir::{ExprId, FromId, Ident, OutputCol, SelectItem};
use cogitator_ir::{ColType, DegradationKind, Span, Symbol};
use sqlparser::ast;

use super::from::name_idents;
use super::select::ItemOut;
use super::Lowerer;

impl Lowerer<'_> {
    pub(super) fn qualified_star(
        &mut self,
        kind: &ast::SelectItemQualifiedWildcardKind,
        options: &ast::WildcardAdditionalOptions,
    ) -> ItemOut {
        match kind {
            ast::SelectItemQualifiedWildcardKind::ObjectName(name) => {
                let idents = name_idents(name);
                self.star(Some(&idents), options)
            }
            ast::SelectItemQualifiedWildcardKind::Expr(_) => {
                self.degrade(
                    DegradationKind::UnsupportedSyntax,
                    Span::default(),
                    "a star over an expression is not modelled",
                );
                ItemOut {
                    item: SelectItem::Hole(Span::default()),
                    outputs: Vec::new(),
                    open: true,
                    alias: None,
                }
            }
        }
    }

    /// Lowers the star, expanding its columns where known.
    pub(super) fn star(
        &mut self,
        qualifier: Option<&[ast::Ident]>,
        options: &ast::WildcardAdditionalOptions,
    ) -> ItemOut {
        let span = self
            .sql_span(options.wildcard_token.0.span)
            .or_else(|| qualifier.map(|q| self.path_span(q)))
            .unwrap_or_default();
        let mut exclude: Vec<Ident> = Vec::new();
        if let Some(ex) = &options.opt_exclude {
            exclude.extend(exclude_idents(ex).iter().map(|i| self.ident(i)));
        }
        if let Some(ex) = &options.opt_except {
            let all = std::iter::once(&ex.first_element).chain(ex.additional_elements.iter());
            exclude.extend(all.map(|i| self.ident(i)));
        }
        if options.opt_rename.is_some() || options.opt_ilike.is_some() {
            self.degrade(
                DegradationKind::UnsupportedSyntax,
                span,
                "RENAME/ILIKE star options are not modelled",
            );
        }
        let replace = self.star_replace(options);
        let qualifier_ident = qualifier.and_then(|q| q.last()).map(|q| self.ident(q));
        let (source, targets) = self.star_targets(qualifier);
        let (outputs, open) = self.expand_star(span, &targets, &exclude, &replace);
        ItemOut {
            item: SelectItem::Star {
                span,
                from: source,
                qualifier: qualifier_ident,
                exclude,
                replace,
            },
            outputs,
            open,
            alias: None,
        }
    }

    fn star_replace(&mut self, options: &ast::WildcardAdditionalOptions) -> Vec<(ExprId, Ident)> {
        let Some(replace) = &options.opt_replace else {
            return Vec::new();
        };
        replace
            .items
            .iter()
            .map(|item| (self.expr(&item.expr), self.ident(&item.column_name)))
            .collect()
    }

    /// The FROM item a qualifier names, and the items the star covers.
    fn star_targets(&mut self, qualifier: Option<&[ast::Ident]>) -> (Option<FromId>, Vec<FromId>) {
        let Some(qualifier) = qualifier else {
            let items = self.frames.last().map(|f| f.items.clone());
            return (None, items.unwrap_or_default());
        };
        let wanted: Vec<Symbol> = qualifier.iter().map(|i| self.sym_for(i)).collect();
        let found = self.frames.last().and_then(|f| {
            f.items
                .iter()
                .copied()
                .find(|item| self.names_from(*item, &wanted))
        });
        (found, found.into_iter().collect())
    }

    fn expand_star(
        &mut self,
        span: Span,
        targets: &[FromId],
        exclude: &[Ident],
        replace: &[(ExprId, Ident)],
    ) -> (Vec<OutputCol>, bool) {
        let width: usize = targets
            .iter()
            .filter_map(|t| self.hir.from.get(*t))
            .map(|f| f.output.len())
            .sum();
        if !self.reserve(width) {
            return (Vec::new(), true);
        }
        let mut excluded: Vec<Symbol> = exclude.iter().map(|i| i.folded).collect();
        excluded.sort_unstable();
        let mut replacements: Vec<(Symbol, ExprId)> =
            replace.iter().map(|(e, name)| (name.folded, *e)).collect();
        replacements.sort_by_key(|(name, _)| *name);
        let (mut out, mut open) = (Vec::with_capacity(width), targets.is_empty());
        for &target in targets {
            let Some(item) = self.hir.from.get(target) else {
                continue;
            };
            open |= item.open;
            for col in &item.output {
                if excluded.binary_search(&col.name).is_ok() {
                    continue;
                }
                let replaced = replacements
                    .binary_search_by_key(&col.name, |(name, _)| *name)
                    .ok()
                    .and_then(|i| replacements.get(i));
                out.push(OutputCol {
                    span,
                    name: Some(col.name),
                    expr: replaced.map(|(_, e)| *e),
                    ty: if replaced.is_some() {
                        ColType::UNKNOWN
                    } else {
                        col.ty.clone() // boundary: output types are per-plan values
                    },
                });
            }
        }
        (out, open)
    }
}

fn exclude_idents(item: &ast::ExcludeSelectItem) -> Vec<&ast::Ident> {
    let names: Vec<&ast::ObjectName> = match item {
        ast::ExcludeSelectItem::Single(n) => vec![n],
        ast::ExcludeSelectItem::Multiple(ns) => ns.iter().collect(),
    };
    names
        .into_iter()
        .filter_map(|n| match n.0.last()? {
            ast::ObjectNamePart::Identifier(i) => Some(i),
            ast::ObjectNamePart::Function(_) => None,
        })
        .collect()
}
