//! Derived tables, table functions and UNNEST.

use cogitator_ir::hir::{ExprId, FromCol, FromId, FromKind, Ident, ScopeKind};
use cogitator_ir::{ColType, Span};
use sqlparser::ast::{self, TableAlias};

use super::binder::AliasMode;
use super::from::{alias_columns, name_idents};
use super::Lowerer;

impl Lowerer<'_> {
    pub(super) fn derived(
        &mut self,
        lateral: bool,
        subquery: &ast::Query,
        alias: Option<&TableAlias>,
    ) -> FromId {
        let saved = std::mem::replace(&mut self.mode, AliasMode::Off);
        let q = self.query(subquery, ScopeKind::Derived, !lateral);
        self.mode = saved;
        let (mut output, open) = self.query_output(q);
        if let Some(cols) = alias_columns(alias) {
            output = self.apply_columns(cols, output);
        }
        let alias_ident = alias.and_then(|a| self.alias_ident(&a.name));
        let names = alias_ident.map(|a| vec![a.folded]).unwrap_or_default();
        let span = self.hir.queries.get(q).map_or(Span::default(), |q| q.span);
        self.leaf(
            span,
            FromKind::Derived {
                q,
                lateral,
                alias: alias_ident,
            },
            output,
            open,
            names,
        )
    }

    pub(super) fn table_function(
        &mut self,
        expr: &ast::Expr,
        alias: Option<&TableAlias>,
    ) -> FromId {
        if let ast::Expr::Function(f) = expr {
            let args = match &f.args {
                ast::FunctionArguments::List(list) => list.args.as_slice(),
                _ => &[],
            };
            return self.table_fn(&f.name, args, alias);
        }
        let saved = std::mem::replace(&mut self.mode, AliasMode::Off);
        let child = self.expr(expr);
        self.mode = saved;
        let span = self
            .hir
            .exprs
            .get(child)
            .map_or(Span::default(), |e| e.span);
        self.open_table_fn(span, Vec::new(), vec![child], alias)
    }

    pub(super) fn table_fn(
        &mut self,
        name: &ast::ObjectName,
        args: &[ast::FunctionArg],
        alias: Option<&TableAlias>,
    ) -> FromId {
        let path = name_idents(name);
        let name_idents: Vec<Ident> = path.iter().map(|i| self.ident(i)).collect();
        let span = self.path_span(&path);
        let saved = std::mem::replace(&mut self.mode, AliasMode::Off);
        let args = self.function_arg_list(args);
        self.mode = saved;
        self.open_table_fn(span, name_idents, args, alias)
    }

    pub(super) fn open_table_fn(
        &mut self,
        span: Span,
        name: Vec<Ident>,
        args: Vec<ExprId>,
        alias: Option<&TableAlias>,
    ) -> FromId {
        let alias_ident = alias.and_then(|a| self.alias_ident(&a.name));
        let names = alias_ident.map(|a| vec![a.folded]).unwrap_or_default();
        let output = alias_columns(alias).map_or_else(Vec::new, |cols| self.column_set(cols));
        self.leaf(
            span,
            FromKind::TableFn {
                name,
                args,
                alias: alias_ident,
            },
            output,
            true,
            names,
        )
    }

    pub(super) fn unnest(
        &mut self,
        exprs: &[ast::Expr],
        with_offset: bool,
        offset_alias: Option<&ast::Ident>,
        alias: Option<&TableAlias>,
    ) -> FromId {
        let saved = std::mem::replace(&mut self.mode, AliasMode::Off);
        let lowered: Vec<_> = exprs.iter().map(|e| self.expr(e)).collect();
        self.mode = saved;
        let span = self.union_spans(&lowered);
        let alias_ident = alias.and_then(|a| self.alias_ident(&a.name));
        let names = alias_ident.map(|a| vec![a.folded]).unwrap_or_default();
        // `unnest(x) as v` names the element column; `as t(v)` the columns.
        let mut output = match alias_columns(alias) {
            Some(cols) => self.column_set(cols),
            None => alias_ident
                .map(|a| {
                    vec![FromCol {
                        name: a.folded,
                        ty: ColType::UNKNOWN,
                    }]
                })
                .unwrap_or_default(),
        };
        if let Some(offset) = offset_alias {
            output.push(FromCol {
                name: self.sym_for(offset),
                ty: ColType::UNKNOWN,
            });
        }
        self.leaf(
            span,
            FromKind::Unnest {
                exprs: lowered,
                with_offset,
                alias: alias_ident,
            },
            output,
            true,
            names,
        )
    }

    pub(super) fn column_set(&mut self, cols: &[ast::TableAliasColumnDef]) -> Vec<FromCol> {
        cols.iter()
            .map(|c| FromCol {
                name: self.sym_for(&c.name),
                ty: ColType::UNKNOWN,
            })
            .collect()
    }

    /// Positional rename by a table alias column list.
    pub(super) fn apply_columns(
        &mut self,
        cols: &[ast::TableAliasColumnDef],
        mut output: Vec<FromCol>,
    ) -> Vec<FromCol> {
        for (i, def) in cols.iter().enumerate() {
            let name = self.sym_for(&def.name);
            match output.get_mut(i) {
                Some(col) => col.name = name,
                None => output.push(FromCol {
                    name,
                    ty: ColType::UNKNOWN,
                }),
            }
        }
        output
    }
}
