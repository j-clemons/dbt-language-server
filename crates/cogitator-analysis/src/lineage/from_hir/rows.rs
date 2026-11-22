//! Rows: what each query outputs, one [`Row`] per select item.

use cogitator_ir::hir::{Body, ExprId, ExprKind, FromId, Ident, QueryId, SelectItem};

use super::lookup::Wanted;
use super::{unique, Ctx, Row, Star, Walker};
use crate::lineage::SourceRef;

impl Walker<'_> {
    pub(super) fn build_rows(&mut self, id: QueryId) -> Vec<Row> {
        let Some(query) = self.hir.queries.get(id) else {
            return Vec::new();
        };
        match &query.body {
            Body::Select(select) => {
                let ctx = Ctx {
                    scope: query.scope,
                    select: Some(select),
                };
                let mut rows = Vec::with_capacity(select.items.len());
                for item in &select.items {
                    match self.star_columns(item, ctx) {
                        Some(columns) => rows.extend(columns),
                        None => rows.extend(self.item_row(item, ctx, id)),
                    }
                }
                rows
            }
            Body::SetOp { by_name, l, r, .. } => {
                let (left, right) = (self.rows_of(*l), self.rows_of(*r));
                if *by_name {
                    merge_by_name(&left, &right)
                } else {
                    merge_by_position(&left, &right)
                }
            }
            Body::Values(rows) => self.values_rows(rows, id),
            Body::Hole(_) => Vec::new(),
        }
    }

    /// One row per column of a star over `Closed` inputs. `None` when the
    /// star's column list is not known in full, or the item is no star:
    /// the caller then keeps the `*` row, so nothing here guesses.
    fn star_columns(&mut self, item: &SelectItem, ctx: Ctx<'_>) -> Option<Vec<Row>> {
        let SelectItem::Star {
            from,
            exclude,
            replace,
            ..
        } = item
        else {
            return None;
        };
        if !self.expand_stars {
            return None;
        }
        let targets = match from {
            Some(from) => vec![*from],
            None => self.scope_leaves(ctx),
        };
        let hir = self.hir;
        let known = |t: &FromId| {
            hir.from
                .get(*t)
                .is_some_and(|f| !f.open && !f.output.is_empty())
        };
        if targets.is_empty() || !targets.iter().all(known) {
            return None;
        }
        let mut rows = Vec::new();
        for target in targets {
            let item = hir.from.get(target)?;
            for (position, col) in item.output.iter().enumerate() {
                if exclude.iter().any(|e| e.folded == col.name) {
                    continue;
                }
                let replaced = replace.iter().find(|(_, name)| name.folded == col.name);
                let label = hir.syms.text(col.name).to_string(); // boundary: reports own names
                let wanted = Wanted {
                    key: Some(col.name),
                    label: &label,
                    bound: u32::try_from(position).ok(),
                };
                let sources = match replaced {
                    Some((expr, _)) => self.expr_sources(*expr, ctx),
                    None => self.item_sources(target, wanted, ctx),
                };
                rows.push(Row {
                    key: Some(col.name),
                    label: Some(label),
                    star: None,
                    sources: unique(sources),
                });
            }
        }
        Some(rows)
    }

    /// `None` only for a `t.*` whose qualifier names nothing in scope.
    fn item_row(&mut self, item: &SelectItem, ctx: Ctx<'_>, query: QueryId) -> Option<Row> {
        match item {
            SelectItem::Expr { expr, alias } => Some(self.expr_row(*expr, *alias, ctx, query)),
            SelectItem::Star {
                from,
                qualifier,
                exclude,
                replace,
                ..
            } => {
                let targets = match (from, qualifier) {
                    (Some(from), _) => vec![*from],
                    (None, Some(_)) => return None,
                    (None, None) => self.scope_leaves(ctx),
                };
                let label = match qualifier {
                    Some(q) => format!("{}.*", self.surface(*q)),
                    None => "*".to_string(),
                };
                let star = Star {
                    exclude: exclude.iter().map(|i| i.folded).collect(),
                    replace: replace
                        .iter()
                        .map(|(e, name)| (name.folded, self.expr_sources(*e, ctx)))
                        .collect(),
                };
                Some(Row {
                    key: None,
                    label: Some(label),
                    star: Some(star),
                    sources: self.star_sources(&targets, ctx),
                })
            }
            // A placeholder item keeps positions aligned for column lists
            SelectItem::Jinja(_) | SelectItem::Hole(_) => Some(Row::default()),
        }
    }

    fn star_sources(&mut self, targets: &[FromId], ctx: Ctx<'_>) -> Vec<SourceRef> {
        let mut out = Vec::new();
        for target in targets {
            out.extend(self.star_of_item(*target, ctx));
        }
        unique(out)
    }

    fn expr_row(
        &mut self,
        expr: ExprId,
        alias: Option<Ident>,
        ctx: Ctx<'_>,
        query: QueryId,
    ) -> Row {
        let sources = self.expr_sources(expr, ctx);
        let named = match alias {
            Some(alias) => Some((alias.folded, self.surface(alias))),
            None => self.implicit_name(expr, query),
        };
        match named {
            Some((key, label)) => Row {
                key: Some(key),
                label: Some(label),
                star: None,
                sources,
            },
            None => Row {
                sources,
                ..Row::default()
            },
        }
    }

    /// `col`, `t.col` and the struct path `s.f` name their output.
    fn implicit_name(
        &self,
        expr: ExprId,
        query: QueryId,
    ) -> Option<(cogitator_ir::Symbol, String)> {
        let node = self.hir.exprs.get(expr)?;
        if let ExprKind::Col(col) = &node.kind {
            let col = self.hir.cols.get(*col)?;
            return Some((col.name.folded, self.surface(col.name)));
        }
        let output = self
            .hir
            .queries
            .get(query)?
            .outputs
            .iter()
            .find(|o| o.expr == Some(expr))?;
        let key = output.name?;
        let written = self
            .text
            .get(output.span.start as usize..output.span.end as usize)?;
        let last = written.rsplit('.').next()?.trim_matches('"');
        Some((key, last.to_string())) // boundary: reports own their names
    }

    /// VALUES has no names; each position still has its expressions' sources.
    fn values_rows(&mut self, rows: &[Vec<ExprId>], query: QueryId) -> Vec<Row> {
        let scope = self.hir.queries.get(query).map(|q| q.scope);
        let Some(scope) = scope else {
            return Vec::new();
        };
        let ctx = Ctx {
            scope,
            select: None,
        };
        let width = rows.first().map_or(0, Vec::len);
        (0..width)
            .map(|i| {
                let mut sources = Vec::new();
                for row in rows {
                    if let Some(expr) = row.get(i) {
                        sources.extend(self.expr_sources(*expr, ctx));
                    }
                }
                Row {
                    sources: unique(sources),
                    ..Row::default()
                }
            })
            .collect()
    }
}

/// A set operation outputs by position; the left arm names the columns.
fn merge_by_position(left: &[Row], right: &[Row]) -> Vec<Row> {
    let width = left.len().max(right.len());
    (0..width)
        .map(|i| match (left.get(i), right.get(i)) {
            (Some(l), Some(r)) => merged(l, r),
            (Some(row), None) | (None, Some(row)) => row.clone(), // boundary: per-query rows
            (None, None) => Row::default(),
        })
        .collect()
}

/// `UNION BY NAME`: matching names combine, the rest are appended.
fn merge_by_name(left: &[Row], right: &[Row]) -> Vec<Row> {
    let mut out: Vec<Row> = left.to_vec(); // boundary: per-query rows
    for row in right {
        let same = out.iter().position(|o| o.key.is_some() && o.key == row.key);
        match same {
            Some(i) => out[i] = merged(&out[i], row),
            None => out.push(row.clone()), // boundary: per-query rows
        }
    }
    out
}

fn merged(l: &Row, r: &Row) -> Row {
    let named = if l.label.is_some() { l } else { r };
    let mut sources = l.sources.clone(); // boundary: per-query rows
    sources.extend(r.sources.iter().cloned());
    let star = match (&l.star, &r.star) {
        (Some(a), Some(b)) => Some(merged_stars(a, b)),
        (Some(s), None) | (None, Some(s)) => Some(s.clone()),
        (None, None) => None,
    };
    Row {
        key: named.key,
        label: named.label.clone(),
        star,
        sources: unique(sources),
    }
}

/// A column an arm still provides is not excluded from the union; a
/// replacement in either arm feeds it.
fn merged_stars(a: &Star, b: &Star) -> Star {
    let mut replace = a.replace.clone(); // boundary: per-query rows
    replace.extend(b.replace.iter().cloned());
    Star {
        exclude: a
            .exclude
            .iter()
            .filter(|name| b.exclude.contains(name))
            .copied()
            .collect(),
        replace,
    }
}
