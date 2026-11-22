//! Named relations: physical tables, CTEs, and Jinja relation targets.

use std::sync::Arc;

use cogitator_ir::hir::{CteId, FromCol, FromId, FromKind, Ident, JinjaRole, QueryId, RelTarget};
use cogitator_ir::schema::{Completeness, ModelSchema};
use cogitator_ir::{ColType, Span, Symbol};
use sqlparser::ast::{self, TableAlias};

use super::from::{alias_columns, name_idents};
use super::Lowerer;
use crate::jinja::{Class, RelInfo};

impl Lowerer<'_> {
    pub(super) fn base_table(
        &mut self,
        name: &ast::ObjectName,
        alias: Option<&TableAlias>,
    ) -> FromId {
        let path = name_idents(name);
        if path.is_empty() {
            return self.leaf(
                Span::default(),
                FromKind::Hole,
                Vec::new(),
                true,
                Vec::new(),
            );
        }
        let alias_ident = alias.and_then(|a| self.alias_ident(&a.name));
        if let Some(region) = self.region_in_path(&path) {
            return self.jinja_base(region, alias_ident, alias, &path);
        }
        let span = self.path_span(&path);
        if let [single] = path.as_slice() {
            let folded = self.sym_for(single);
            if let Some(&(_, cte)) = self.visible.iter().rev().find(|(n, _)| *n == folded) {
                return self.cte_base(span, cte, alias_ident, alias, folded);
            }
        }
        let idents: Vec<Ident> = path.iter().map(|i| self.ident(i)).collect();
        let names = alias_ident
            .map(|a| vec![a.folded])
            .unwrap_or_else(|| idents.iter().map(|i| i.folded).collect());
        let output = alias_columns(alias).map_or_else(Vec::new, |cols| self.column_set(cols));
        self.leaf(
            span,
            FromKind::Base {
                target: RelTarget::Physical(idents),
                alias: alias_ident,
            },
            output,
            true,
            names,
        )
    }

    pub(super) fn cte_base(
        &mut self,
        span: Span,
        cte: CteId,
        alias: Option<Ident>,
        alias_ast: Option<&TableAlias>,
        name: Symbol,
    ) -> FromId {
        let (mut output, open) = self.cte_output(cte);
        if let Some(cols) = alias_columns(alias_ast) {
            output = self.apply_columns(cols, output);
        }
        let names = vec![alias.map_or(name, |a| a.folded)];
        self.leaf(
            span,
            FromKind::Base {
                target: RelTarget::Cte(cte),
                alias,
            },
            output,
            open,
            names,
        )
    }

    pub(super) fn cte_output(&mut self, cte: CteId) -> (Vec<FromCol>, bool) {
        let Some(entry) = self.hir.ctes.get(cte) else {
            return (Vec::new(), true);
        };
        let query = entry.query;
        // A self-reference is seen before its body exists: stay open.
        if self.hir.queries.get(query).is_none() {
            return (Vec::new(), true);
        }
        // boundary: per-reference copy of the CTE's column list
        let columns = entry.columns.iter().map(|c| c.folded).collect::<Vec<_>>();
        let (output, open) = self.query_output(query);
        let output = if columns.is_empty() {
            output
        } else {
            columns
                .into_iter()
                .enumerate()
                .map(|(i, name)| FromCol {
                    name,
                    ty: output.get(i).map_or(ColType::UNKNOWN, |c| c.ty.clone()),
                })
                .collect()
        };
        (output, open)
    }

    /// A copy of the query's named outputs, reserved before it is built.
    /// Past the cap the reference is open: that is what the cap degrades to.
    pub(super) fn query_output(&mut self, query: QueryId) -> (Vec<FromCol>, bool) {
        let open = self.query_is_open(query);
        let width = self.hir.queries.get(query).map_or(0, |q| q.outputs.len());
        if !self.reserve(width) {
            return (Vec::new(), true);
        }
        let outputs = self
            .hir
            .queries
            .get(query)
            .map(|q| {
                q.outputs
                    .iter()
                    .filter_map(|col| {
                        col.name.map(|name| FromCol {
                            name,
                            ty: col.ty.clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        (outputs, open)
    }

    pub(super) fn jinja_base(
        &mut self,
        region: usize,
        alias: Option<Ident>,
        alias_ast: Option<&TableAlias>,
        path: &[ast::Ident],
    ) -> FromId {
        let span = self.region_span(region);
        let class = self.region(region).map(|r| r.class.clone());
        let static_rel = match class {
            Some(Class::Relation(rel)) if rel != RelInfo::Dynamic => Some(rel),
            _ => None,
        };
        match static_rel {
            Some(RelInfo::Model {
                package,
                name,
                version,
            }) => {
                let target = RelTarget::Model {
                    package: package.map(|p| self.intern(&p)),
                    name: self.intern(&name),
                    version: version.map(|v| self.intern(&v)),
                };
                let schema = self.env.schemas.model(&name).cloned();
                let folded = self.rules.fold_name(&name);
                let folded = self.intern(&folded);
                self.static_base(span, target, alias, alias_ast, folded, schema)
            }
            Some(RelInfo::Source { source, table }) => {
                let schema = self.env.schemas.source(&source, &table).cloned();
                let target = RelTarget::Source {
                    source: self.intern(&source),
                    table: self.intern(&table),
                };
                let folded = self.rules.fold_name(&table);
                let folded = self.intern(&folded);
                self.static_base(span, target, alias, alias_ast, folded, schema)
            }
            Some(RelInfo::This) => {
                let model = self.env.model.clone();
                let schema = model
                    .as_deref()
                    .and_then(|m| self.env.schemas.model(m).cloned());
                let folded = model
                    .as_deref()
                    .map(|m| self.rules.fold_name(m).into_owned())
                    .unwrap_or_default();
                let folded = self.intern(&folded);
                self.static_base(span, RelTarget::This, alias, alias_ast, folded, schema)
            }
            _ => self.dynamic_base(region, span, alias, path),
        }
    }

    pub(super) fn static_base(
        &mut self,
        span: Span,
        target: RelTarget,
        alias: Option<Ident>,
        alias_ast: Option<&TableAlias>,
        name: Symbol,
        schema: Option<Arc<ModelSchema>>,
    ) -> FromId {
        let (mut output, open) = self.schema_output(schema.as_deref());
        if let Some(cols) = alias_columns(alias_ast) {
            output = self.apply_columns(cols, output);
        }
        let names = vec![alias.map_or(name, |a| a.folded)];
        self.leaf(span, FromKind::Base { target, alias }, output, open, names)
    }

    pub(super) fn schema_output(&mut self, schema: Option<&ModelSchema>) -> (Vec<FromCol>, bool) {
        let Some(schema) = schema else {
            return (Vec::new(), true);
        };
        if !self.reserve(schema.columns.len()) {
            return (Vec::new(), true);
        }
        let output = schema
            .columns
            .iter()
            .map(|c| FromCol {
                name: {
                    let folded = self.rules.fold_name(&c.name);
                    self.hir.syms.intern(&folded)
                },
                ty: c.ty.clone(), // boundary: the plan owns its column types
            })
            .collect();
        (output, schema.completeness != Completeness::Closed)
    }

    /// `{{ my_macro() }}` or `ref(var('x'))`: known only as a relation.
    pub(super) fn dynamic_base(
        &mut self,
        region: usize,
        span: Span,
        alias: Option<Ident>,
        path: &[ast::Ident],
    ) -> FromId {
        let names = alias.map(|a| vec![a.folded]).unwrap_or_else(|| {
            path.last()
                .filter(|i| self.region_at(self.ident_span(i)).is_none())
                .map(|i| vec![self.sym_for(i)])
                .unwrap_or_default()
        });
        match self.take_region(region, JinjaRole::Relation) {
            Some(jinja) => self.leaf(
                span,
                FromKind::Base {
                    target: RelTarget::Jinja(jinja),
                    alias,
                },
                Vec::new(),
                true,
                names,
            ),
            None => self.leaf(span, FromKind::Hole, Vec::new(), true, names),
        }
    }
}
