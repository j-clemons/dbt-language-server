//! Select-list expansion: known stars become one item per column, with
//! USING and NATURAL columns merged. A star over an open schema leaves
//! the list open.

use cogitator_ir::common::JoinKind;
use cogitator_ir::hir::{
    ExprId, ExprKind, FromId, FromKind, JinjaId, JinjaRole, JoinCond, Select, SelectItem,
};
use cogitator_ir::mir::ColId;
use cogitator_ir::{ColType, DegradationKind, Nullability, Span, Symbol};

use super::demand::leaves;
use super::{Ctx, MAX_FROM_DEPTH};

/// One output column of a SELECT.
#[derive(Clone)]
pub(super) enum Item {
    Expr { expr: ExprId, name: Option<Symbol> },
    Field(Field),
    Jinja { jinja: JinjaId, span: Span },
}

#[derive(Clone)]
pub(super) struct Field {
    pub(super) name: Option<Symbol>,
    pub(super) col: ColId,
    pub(super) ty: ColType,
    /// A FULL JOIN's USING column: `coalesce(col, right)`.
    pub(super) merged: Option<(ColId, ColType)>,
}

pub(super) struct Expanded {
    pub(super) items: Vec<Item>,
    /// Columns exist that the list cannot name.
    pub(super) open: bool,
    /// `select.items` index → `items` index.
    pub(super) item_of: Vec<Option<usize>>,
}

impl Ctx<'_> {
    pub(super) fn expand_items(&mut self, select: &Select) -> Expanded {
        let mut out = Expanded {
            items: Vec::new(),
            open: false,
            item_of: Vec::with_capacity(select.items.len()),
        };
        for item in &select.items {
            match item {
                SelectItem::Expr { expr, alias } => {
                    out.item_of.push(Some(out.items.len()));
                    let name = alias.map(|a| a.folded).or_else(|| self.expr_name(*expr));
                    out.items.push(Item::Expr { expr: *expr, name });
                }
                SelectItem::Star {
                    span,
                    from,
                    exclude,
                    replace,
                    ..
                } => {
                    out.item_of.push(None);
                    let targets = from.map_or_else(|| select.from.clone(), |f| vec![f]);
                    let exclude: Vec<Symbol> = exclude.iter().map(|e| e.folded).collect();
                    let replace: Vec<(ExprId, Symbol)> =
                        replace.iter().map(|(e, i)| (*e, i.folded)).collect();
                    self.expand_star(&targets, &exclude, &replace, *span, &mut out);
                }
                SelectItem::Jinja(jinja) => self.jinja_item(*jinja, &mut out),
                SelectItem::Hole(span) => {
                    out.item_of.push(None);
                    self.degrade(
                        DegradationKind::ParseHole,
                        *span,
                        "this select item did not parse",
                    );
                    out.open = true;
                }
            }
        }
        out
    }

    fn expr_name(&self, expr: ExprId) -> Option<Symbol> {
        let hir = self.hir;
        match hir.exprs.get(expr).map(|e| &e.kind) {
            Some(ExprKind::Col(col)) => hir.cols.get(*col).map(|c| c.name.folded),
            _ => None,
        }
    }

    fn jinja_item(&mut self, jinja: JinjaId, out: &mut Expanded) {
        let node = self.hir.jinja.get(jinja).copied();
        let Some(node) = node else {
            self.bug(format!("select item jinja {jinja:?} is out of range"));
            out.item_of.push(None);
            return;
        };
        match node.role {
            JinjaRole::Scalar => {
                out.item_of.push(Some(out.items.len()));
                out.items.push(Item::Jinja {
                    jinja,
                    span: node.span,
                });
            }
            JinjaRole::Blank | JinjaRole::ClausePosition => out.item_of.push(None),
            JinjaRole::Composite | JinjaRole::Relation => {
                out.item_of.push(None);
                out.open = true;
                self.degrade(
                    DegradationKind::OpenSchema,
                    node.span,
                    "a Jinja region expands to unknown columns",
                );
            }
        }
    }

    fn expand_star(
        &mut self,
        targets: &[FromId],
        exclude: &[Symbol],
        replace: &[(ExprId, Symbol)],
        span: Span,
        out: &mut Expanded,
    ) {
        let hir = self.hir;
        let any_open = targets.iter().flat_map(|t| leaves(hir, *t)).any(|leaf| {
            hir.from.get(leaf).is_some_and(|item| item.open) || !self.bindings.contains_key(&leaf)
        });
        if any_open {
            out.open = true;
            self.degrade(
                DegradationKind::OpenSchema,
                span,
                "a star over a FROM item whose columns are not fully known",
            );
            return;
        }
        for target in targets {
            for field in self.fields(*target, 0) {
                if field.name.is_some_and(|n| exclude.contains(&n)) {
                    continue;
                }
                let replaced = field
                    .name
                    .and_then(|n| replace.iter().find(|(_, r)| *r == n));
                out.items.push(match replaced {
                    Some((expr, name)) => Item::Expr {
                        expr: *expr,
                        name: Some(*name),
                    },
                    None => Item::Field(field),
                });
            }
        }
    }

    /// The columns a star sees under `from`: USING and NATURAL columns
    /// once and first, then the rest of each side.
    fn fields(&mut self, from: FromId, depth: usize) -> Vec<Field> {
        let hir = self.hir;
        if depth > MAX_FROM_DEPTH {
            return Vec::new();
        }
        let Some(FromKind::Join { kind, l, r, on }) = hir.from.get(from).map(|f| &f.kind) else {
            return self.leaf_fields(from);
        };
        let (left, right) = (self.fields(*l, depth + 1), self.fields(*r, depth + 1));
        if matches!(kind, JoinKind::Semi | JoinKind::Anti) {
            return left;
        }
        let shared: Vec<Symbol> = match on {
            JoinCond::Using(names) => names.iter().map(|n| n.folded).collect(),
            JoinCond::Natural => self.natural_names(*l, *r).unwrap_or_default(),
            JoinCond::None | JoinCond::On(_) => Vec::new(),
        };
        let mut out = Vec::with_capacity(left.len() + right.len());
        for name in &shared {
            if let Some(field) = merged_field(*name, *kind, &left, &right) {
                out.push(field);
            }
        }
        let unshared = |f: &&Field| f.name.is_none_or(|n| !shared.contains(&n));
        out.extend(left.iter().filter(unshared).cloned());
        out.extend(right.iter().filter(unshared).cloned());
        out
    }

    fn leaf_fields(&self, from: FromId) -> Vec<Field> {
        let Some(binding) = self.bindings.get(&from) else {
            return Vec::new();
        };
        binding
            .cols
            .iter()
            .map(|col| Field {
                name: self.col_name(*col),
                col: *col,
                ty: self.ref_type(*col, binding.nullable),
                merged: None,
            })
            .collect()
    }
}

/// The surviving column of a USING pair: the preserved side's, or a
/// coalesce of both for FULL.
fn merged_field(name: Symbol, kind: JoinKind, left: &[Field], right: &[Field]) -> Option<Field> {
    let find = |side: &[Field]| side.iter().find(|f| f.name == Some(name)).cloned();
    let (l, r) = (find(left), find(right));
    match (kind, l, r) {
        (JoinKind::Full, Some(l), Some(r)) => {
            let ty = ColType::new(l.ty.ty.join(&r.ty.ty), Nullability::Unknown);
            Some(Field {
                name: Some(name),
                col: l.col,
                ty,
                merged: Some((r.col, r.ty)),
            })
        }
        (JoinKind::Right, _, Some(r)) => Some(r),
        (_, Some(l), _) => Some(l),
        (_, None, r) => r,
    }
}
