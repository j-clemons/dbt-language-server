//! FROM items: each leaf is pushed into the current frame so later
//! items and ON conditions can see it.

use cogitator_ir::common::JoinKind;
use cogitator_ir::hir::{FromCol, FromId, FromKind, Ident, JoinCond};
use cogitator_ir::{DegradationKind, Span, Symbol};
use sqlparser::ast::{self, JoinConstraint, JoinOperator, TableAlias, TableFactor};

use super::{union, Lowerer};

impl Lowerer<'_> {
    pub(super) fn lower_from(&mut self, tables: &[ast::TableWithJoins]) -> Vec<FromId> {
        tables.iter().map(|t| self.table_with_joins(t)).collect()
    }

    fn table_with_joins(&mut self, table: &ast::TableWithJoins) -> FromId {
        let mut left = self.table_factor(&table.relation);
        for join in &table.joins {
            let right = self.table_factor(&join.relation);
            let (kind, constraint) = join_kind(&join.join_operator);
            let on = self.join_cond(constraint);
            let span = self.join_span(left, right);
            left = self.alloc_from(
                span,
                FromKind::Join {
                    kind,
                    l: left,
                    r: right,
                    on,
                },
                Vec::new(),
                false,
                Vec::new(),
            );
        }
        left
    }

    fn join_span(&self, l: FromId, r: FromId) -> Span {
        match (self.hir.from.get(l), self.hir.from.get(r)) {
            (Some(a), Some(b)) => union(a.span, b.span),
            _ => Span::default(),
        }
    }

    fn join_cond(&mut self, constraint: Option<&JoinConstraint>) -> JoinCond {
        match constraint {
            None | Some(JoinConstraint::None) => JoinCond::None,
            Some(JoinConstraint::Natural) => JoinCond::Natural,
            Some(JoinConstraint::On(e)) => JoinCond::On(self.expr(e)),
            Some(JoinConstraint::Using(names)) => {
                let idents: Vec<Ident> = names
                    .iter()
                    .filter_map(|n| match n.0.last()? {
                        ast::ObjectNamePart::Identifier(i) => Some(self.ident(i)),
                        ast::ObjectNamePart::Function(_) => None,
                    })
                    .collect();
                let using: Vec<Symbol> = idents.iter().map(|i| i.folded).collect();
                if let Some(frame) = self.frames.last_mut() {
                    frame.using.extend(using);
                }
                JoinCond::Using(idents)
            }
        }
    }

    fn table_factor(&mut self, factor: &TableFactor) -> FromId {
        match factor {
            TableFactor::Table {
                name, alias, args, ..
            } => match args {
                Some(args) => self.table_fn(name, &args.args, alias.as_ref()),
                None => self.base_table(name, alias.as_ref()),
            },
            TableFactor::Derived {
                lateral,
                subquery,
                alias,
                ..
            } => self.derived(*lateral, subquery, alias.as_ref()),
            TableFactor::TableFunction { expr, alias } => self.table_function(expr, alias.as_ref()),
            TableFactor::Function {
                name, args, alias, ..
            } => self.table_fn(name, args, alias.as_ref()),
            TableFactor::UNNEST {
                alias,
                array_exprs,
                with_offset,
                with_offset_alias,
                ..
            } => self.unnest(
                array_exprs,
                *with_offset,
                with_offset_alias.as_ref(),
                alias.as_ref(),
            ),
            TableFactor::NestedJoin {
                table_with_joins,
                alias,
            } => {
                if alias.is_some() {
                    self.degrade(
                        DegradationKind::UnsupportedSyntax,
                        Span::default(),
                        "an alias on a parenthesised join is not modelled",
                    );
                }
                self.table_with_joins(table_with_joins)
            }
            other => self.unsupported_factor(other),
        }
    }

    fn unsupported_factor(&mut self, factor: &TableFactor) -> FromId {
        self.degrade(
            DegradationKind::UnsupportedSyntax,
            Span::default(),
            "this table factor (PIVOT, JSON_TABLE, …) is not modelled",
        );
        // The source relation stays analysable as a sibling of the hole.
        if let TableFactor::Pivot { table, .. }
        | TableFactor::Unpivot { table, .. }
        | TableFactor::MatchRecognize { table, .. } = factor
        {
            self.table_factor(table);
        }
        let names = factor_alias(factor)
            .map(|a| vec![self.sym_for(&a.name)])
            .unwrap_or_default();
        self.leaf(Span::default(), FromKind::Hole, Vec::new(), true, names)
    }

    /// Allocates a leaf and makes it visible in the current frame.
    pub(super) fn leaf(
        &mut self,
        span: Span,
        kind: FromKind,
        output: Vec<FromCol>,
        open: bool,
        names: Vec<Symbol>,
    ) -> FromId {
        let id = self.alloc_from(span, kind, output, open, names);
        if let Some(frame) = self.frames.last_mut() {
            frame.items.push(id);
        }
        id
    }
}

pub(super) fn alias_columns(alias: Option<&TableAlias>) -> Option<&[ast::TableAliasColumnDef]> {
    alias
        .map(|a| a.columns.as_slice())
        .filter(|cols| !cols.is_empty())
}

pub(super) fn name_idents(name: &ast::ObjectName) -> Vec<ast::Ident> {
    name.0
        .iter()
        .filter_map(|part| match part {
            // boundary: a short path, compared and interned, never stored
            ast::ObjectNamePart::Identifier(i) => Some(i.clone()),
            ast::ObjectNamePart::Function(_) => None,
        })
        .collect()
}

fn factor_alias(factor: &TableFactor) -> Option<&TableAlias> {
    match factor {
        TableFactor::Pivot { alias, .. }
        | TableFactor::Unpivot { alias, .. }
        | TableFactor::MatchRecognize { alias, .. }
        | TableFactor::JsonTable { alias, .. }
        | TableFactor::OpenJsonTable { alias, .. }
        | TableFactor::XmlTable { alias, .. }
        | TableFactor::SemanticView { alias, .. } => alias.as_ref(),
        _ => None,
    }
}

/// Maps a join operator to the HIR kind and its constraint.
fn join_kind(op: &JoinOperator) -> (JoinKind, Option<&JoinConstraint>) {
    use JoinOperator as J;
    match op {
        J::Join(c) | J::Inner(c) | J::StraightJoin(c) => (JoinKind::Inner, Some(c)),
        J::Left(c) | J::LeftOuter(c) => (JoinKind::Left, Some(c)),
        J::Right(c) | J::RightOuter(c) => (JoinKind::Right, Some(c)),
        J::FullOuter(c) => (JoinKind::Full, Some(c)),
        J::CrossJoin(c) => (JoinKind::Cross, Some(c)),
        J::Semi(c) | J::LeftSemi(c) | J::RightSemi(c) => (JoinKind::Semi, Some(c)),
        J::Anti(c) | J::LeftAnti(c) | J::RightAnti(c) => (JoinKind::Anti, Some(c)),
        J::CrossApply => (JoinKind::Cross, None),
        J::OuterApply => (JoinKind::Left, None),
        J::AsOf { constraint, .. } => (JoinKind::Left, Some(constraint)),
        J::ArrayJoin | J::LeftArrayJoin | J::InnerArrayJoin => (JoinKind::Cross, None),
    }
}
