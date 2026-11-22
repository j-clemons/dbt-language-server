//! Resolving one column against one FROM item.

use cogitator_ir::hir::{FromCol, FromId, FromKind, Ident, RelTarget};
use cogitator_ir::{Span, Symbol};
use cogitator_parser::TokenKind;

use super::{unique, Ctx, Row, Walker};
use crate::lineage::SourceRef;

/// The column being looked up: `key` is `None` for `*`.
#[derive(Clone, Copy)]
pub(super) struct Wanted<'a> {
    pub key: Option<Symbol>,
    pub label: &'a str,
    /// The binder's `Bound` index, for positional renames.
    pub bound: Option<u32>,
}

impl Walker<'_> {
    /// Every source of `*` on `from`.
    pub(super) fn star_of_item(&mut self, from: FromId, ctx: Ctx<'_>) -> Vec<SourceRef> {
        let wanted = Wanted {
            key: None,
            label: "*",
            bound: None,
        };
        self.item_sources(from, wanted, ctx)
    }

    pub(super) fn item_sources(
        &mut self,
        from: FromId,
        wanted: Wanted<'_>,
        ctx: Ctx<'_>,
    ) -> Vec<SourceRef> {
        let hir = self.hir;
        let Some(item) = hir.from.get(from) else {
            return Vec::new();
        };
        match &item.kind {
            FromKind::Base { target, .. } => self.target_sources(target, wanted),
            FromKind::Derived { q, alias, .. } => {
                let rows = self.rows_of(*q);
                let hit = rows_lookup(&rows, wanted, false);
                match (hit, alias) {
                    (Some(sources), _) => sources,
                    (None, Some(alias)) => vec![self.unknown(self.surface(*alias), wanted)],
                    (None, None) => Vec::new(),
                }
            }
            FromKind::Unnest { exprs, .. } if may_output(&item.output, wanted) => {
                self.exprs_sources(exprs, ctx)
            }
            FromKind::TableFn { name, args, .. } => {
                let mut out: Vec<SourceRef> = name
                    .last()
                    .map(|n| self.unknown(self.surface(*n), wanted))
                    .into_iter()
                    .collect();
                if may_output(&item.output, wanted) {
                    out.extend(self.exprs_sources(args, ctx));
                }
                out
            }
            FromKind::Unnest { .. } | FromKind::Join { .. } | FromKind::Hole => Vec::new(),
        }
    }

    fn target_sources(&mut self, target: &RelTarget, wanted: Wanted<'_>) -> Vec<SourceRef> {
        let hir = self.hir;
        match target {
            RelTarget::Model { name, .. } => vec![model(hir.syms.text(*name), wanted.label)],
            RelTarget::Source { source, table } => vec![SourceRef::Source {
                source: hir.syms.text(*source).to_string(), // boundary: reports own names
                table: hir.syms.text(*table).to_string(),   // boundary: as above
                column: wanted.label.to_string(),           // boundary: as above
            }],
            // `{{ this }}` has no name in an empty schema environment
            RelTarget::This => vec![self.unknown("this".to_string(), wanted)],
            RelTarget::Physical(path) => match path.last() {
                Some(name) => vec![self.unknown(self.surface(*name), wanted)],
                None => Vec::new(),
            },
            RelTarget::Cte(id) => {
                let Some(cte) = hir.ctes.get(*id) else {
                    return Vec::new();
                };
                let rows = self.rows_of(cte.query);
                let renamed = !cte.columns.is_empty();
                rows_lookup(&rows, wanted, renamed)
                    .unwrap_or_else(|| vec![self.unknown(self.surface(cte.name), wanted)])
            }
            RelTarget::Jinja(id) => self.jinja_relation(hir.jinja.get(*id).map(|n| n.span), wanted),
        }
    }

    /// A relation only known as Jinja: `{{ adapter.get_relation(ref('a')) }}`
    /// still names `a`; with no static name anywhere there is no source.
    fn jinja_relation(&self, span: Option<Span>, wanted: Wanted<'_>) -> Vec<SourceRef> {
        let (Some(span), Some(doc)) = (span, self.doc) else {
            return Vec::new();
        };
        let start = doc.tokens.partition_point(|t| t.start < span.start);
        let end = doc.tokens.partition_point(|t| t.start < span.end);
        let Some(region) = doc.tokens.get(start..end) else {
            return Vec::new();
        };
        if let Some(reference) = region.iter().find(|t| t.kind == TokenKind::Ref) {
            return vec![model(doc.literal(reference), wanted.label)];
        }
        let source = region.iter().find(|t| t.kind == TokenKind::SourceName);
        let table = region.iter().find(|t| t.kind == TokenKind::SourceTable);
        match (source, table) {
            (Some(source), Some(table)) => vec![SourceRef::Source {
                source: doc.literal(source).to_string(), // boundary: reports own names
                table: doc.literal(table).to_string(),   // boundary: as above
                column: wanted.label.to_string(),        // boundary: as above
            }],
            _ => Vec::new(),
        }
    }

    pub(super) fn unknown(&self, name: String, wanted: Wanted<'_>) -> SourceRef {
        SourceRef::Unknown {
            name,
            column: wanted.label.to_string(), // boundary: reports own names
        }
    }

    /// The FROM leaves of the query being resolved, in source order.
    pub(super) fn scope_leaves(&self, ctx: Ctx<'_>) -> Vec<FromId> {
        self.hir
            .scopes
            .get(ctx.scope)
            .map(|s| s.from.clone()) // boundary: a short per-select list
            .unwrap_or_default()
    }

    /// The scope leaf a `q.col` qualifier names, among those it can be
    /// when the column is missing: aliased items and CTEs.
    pub(super) fn leaf_named(&self, qualifier: Ident, ctx: Ctx<'_>) -> Option<FromId> {
        let hir = self.hir;
        hir.scopes.get(ctx.scope)?.from.iter().copied().find(|id| {
            let Some(item) = hir.from.get(*id) else {
                return false;
            };
            match &item.kind {
                FromKind::Base {
                    alias: Some(alias), ..
                }
                | FromKind::Derived {
                    alias: Some(alias), ..
                } => alias.folded == qualifier.folded,
                FromKind::Base {
                    target: RelTarget::Cte(cte),
                    alias: None,
                } => hir
                    .ctes
                    .get(*cte)
                    .is_some_and(|c| c.name.folded == qualifier.folded),
                _ => false,
            }
        })
    }
}

/// Whether a function table can output `wanted`: an alias list closes its
/// columns, so a name it does not list came from a sibling FROM item.
fn may_output(output: &[FromCol], wanted: Wanted<'_>) -> bool {
    match wanted.key {
        Some(key) if !output.is_empty() => output.iter().any(|c| c.name == key),
        _ => true,
    }
}

fn model(name: &str, column: &str) -> SourceRef {
    SourceRef::Model {
        name: name.to_string(),     // boundary: reports own names
        column: column.to_string(), // boundary: as above
    }
}

/// The sources `wanted` gets from `rows`, `None` when no row answers.
///
/// A star row also answers a name, with that name substituted, so
/// `select * from x` keeps column granularity. A `renamed` query (its
/// column list renames by position) tries the position first.
fn rows_lookup(rows: &[Row], wanted: Wanted<'_>, renamed: bool) -> Option<Vec<SourceRef>> {
    if renamed {
        if let Some(sources) = positional(rows, wanted) {
            return Some(sources);
        }
    }
    let mut out = Vec::new();
    let mut answered = false;
    for row in rows {
        match (wanted.key, &row.star) {
            (None, star) => {
                answered = true;
                out.extend(row.sources.iter().cloned());
                if let Some(star) = star {
                    out.extend(star.replace.iter().flat_map(|(_, s)| s.iter().cloned()));
                }
            }
            (Some(key), Some(star)) => {
                if star.exclude.contains(&key) {
                    continue;
                }
                answered = true;
                match star.replace.iter().find(|(name, _)| *name == key) {
                    Some((_, replaced)) => out.extend(replaced.iter().cloned()),
                    None => out.extend(substituted(&row.sources, wanted.label)),
                }
            }
            (Some(key), None) if row.key == Some(key) => {
                answered = true;
                out.extend(substituted(&row.sources, wanted.label));
            }
            (Some(_), None) => {}
        }
    }
    if answered {
        return Some(unique(out));
    }
    if renamed {
        None
    } else {
        positional(rows, wanted)
    }
}

/// The `Bound` position, meaningful only when no star shifts positions.
fn positional(rows: &[Row], wanted: Wanted<'_>) -> Option<Vec<SourceRef>> {
    if rows.iter().any(|r| r.star.is_some()) {
        return None;
    }
    let row = rows.get(wanted.bound? as usize)?;
    Some(substituted(&row.sources, wanted.label).collect())
}

/// A `*` source becomes the requested name; other sources pass through.
fn substituted<'a>(
    sources: &'a [SourceRef],
    requested: &'a str,
) -> impl Iterator<Item = SourceRef> + 'a {
    sources.iter().map(move |source| match source {
        SourceRef::Model { name, column } if column == "*" => SourceRef::Model {
            name: name.clone(),
            column: requested.to_string(),
        },
        SourceRef::Source {
            source,
            table,
            column,
        } if column == "*" => SourceRef::Source {
            source: source.clone(),
            table: table.clone(),
            column: requested.to_string(),
        },
        SourceRef::Unknown { name, column } if column == "*" => SourceRef::Unknown {
            name: name.clone(),
            column: requested.to_string(),
        },
        other => other.clone(), // boundary: the report owns its data
    })
}
