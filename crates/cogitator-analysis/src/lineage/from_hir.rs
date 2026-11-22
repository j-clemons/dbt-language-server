//! Column lineage over the bound HIR: one [`Row`] per select item of every
//! query, resolved through CTEs, derived tables and set operations.
//!
//! Only the projection carries lineage, so a CTE's or derived table's WHERE
//! and JOIN do not feed the columns above it. A subquery expression is the
//! exception: its whole body decides the value (see [`subquery`]).

use std::collections::HashSet;
use std::rc::Rc;

use cogitator_dialects::Dialect;
use cogitator_ir::common::ModelConfig;
use cogitator_ir::hir::{Body, Hir, Ident, QueryId, ScopeId, Select};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::{DegradationKind, Lowered, Symbol};
use cogitator_parser::{JinjaKind, ParsedDocument, TokenKind};
use cogitator_sql_front::{build_hir, HirInput, SchemaEnv};

use super::{ColumnLineage, ModelLineage, SourceRef};

mod exprs;
mod lookup;
mod rows;
mod subquery;

/// Lineage for one model document. `None` when the template has Jinja
/// control blocks, the SQL does not parse, or a front-end cap cut it short:
/// a partial answer here would read as a complete one.
pub fn column_lineage(doc: &ParsedDocument, dialect: &Dialect) -> Option<ModelLineage> {
    if has_control_block(doc) {
        return None;
    }
    let lowered = build_hir(&template_input(doc, dialect), &SchemaEnv::default());
    let columns = columns_of(&lowered, &doc.text, Some(doc), false)?;
    Some(ModelLineage {
        columns,
        origin: None,
    })
}

pub(super) fn template_input<'a>(doc: &'a ParsedDocument, dialect: &'a Dialect) -> HirInput<'a> {
    HirInput {
        text: &doc.text,
        dialect,
        origin: SourceOrigin::Template {
            variant: TemplateVariant::Plain,
        },
        config: ModelConfig::default(),
    }
}

/// The columns of a lowered HIR, `None` when the front end gave up or cut
/// the query short. `text` is what the HIR's spans index; `doc` is `None`
/// on the compiled route, which has no template to read Jinja from.
pub(super) fn columns_of(
    lowered: &Lowered<Hir>,
    text: &str,
    doc: Option<&ParsedDocument>,
    expand_stars: bool,
) -> Option<Vec<ColumnLineage>> {
    let incomplete = lowered.degraded.iter().any(|d| {
        matches!(
            d.kind,
            DegradationKind::ParseHole | DegradationKind::CapExceeded
        )
    });
    if incomplete {
        return None;
    }
    let hir = lowered.value.as_ref()?;
    let root = hir.root?;
    if !matches!(
        hir.queries.get(root)?.body,
        Body::Select(_) | Body::SetOp { .. }
    ) {
        return None;
    }
    let mut walker = Walker::new(hir, text, doc, expand_stars);
    let rows = walker.rows_of(root);
    Some(rows.iter().filter_map(Row::to_column).collect())
}

/// The 07 §3.2 rule: any statement tag but config and set declines. That
/// includes a lone `{% if is_incremental() %}`, which HIR could analyse as
/// its `Incremental` variant but lineage has never answered for.
pub(super) fn has_control_block(doc: &ParsedDocument) -> bool {
    let mut tokens = doc.tokens.iter();
    while let Some(open) = tokens.next() {
        if open.kind != TokenKind::JinjaOpen(JinjaKind::Stmt) {
            continue;
        }
        let tag = tokens
            .by_ref()
            .take_while(|t| t.kind != TokenKind::JinjaClose(JinjaKind::Stmt))
            .find(|t| t.kind == TokenKind::Ident || t.kind == TokenKind::Config)
            .map_or("", |t| doc.literal(t));
        if !matches!(tag, "config" | "set" | "endset") {
            return true;
        }
    }
    false
}

/// One select item's contribution to a query's output.
#[derive(Debug, Clone, Default)]
struct Row {
    /// The folded name downstream references match on.
    key: Option<Symbol>,
    /// The name as written; `None` drops the row from the report.
    label: Option<String>,
    /// `Some` for `*` and `t.*`, whose `key` is `None`.
    star: Option<Star>,
    sources: Vec<SourceRef>,
}

/// EXCLUDE and REPLACE of one star item.
#[derive(Debug, Clone, Default)]
struct Star {
    exclude: Vec<Symbol>,
    replace: Vec<(Symbol, Vec<SourceRef>)>,
}

impl Row {
    fn to_column(&self) -> Option<ColumnLineage> {
        let mut sources = self.sources.clone(); // boundary: the report owns its data
        if let Some(star) = &self.star {
            sources.extend(star.replace.iter().flat_map(|(_, s)| s.iter().cloned()));
        }
        Some(ColumnLineage {
            name: self.label.clone()?, // boundary: as above
            sources: unique(sources),
        })
    }
}

/// Where a reference is being resolved: its query's scope and SELECT.
#[derive(Clone, Copy)]
struct Ctx<'h> {
    scope: ScopeId,
    select: Option<&'h Select>,
}

/// Per-query results memoised for one walk, since CTEs are read repeatedly.
struct Walker<'h> {
    hir: &'h Hir,
    /// The text the spans index: the template, or the compiled SQL.
    text: &'h str,
    /// The template's tokens, for `{{ adapter.get_relation(ref('a')) }}`.
    doc: Option<&'h ParsedDocument>,
    /// Expand a star over known columns; off on the plain route, whose
    /// answers the corpus snapshots pin.
    expand_stars: bool,
    memo: Vec<Option<Rc<[Row]>>>,
    /// Guards `WITH RECURSIVE`: a query being built has no rows yet.
    visiting: Vec<bool>,
}

impl<'h> Walker<'h> {
    fn new(
        hir: &'h Hir,
        text: &'h str,
        doc: Option<&'h ParsedDocument>,
        expand_stars: bool,
    ) -> Self {
        Self {
            hir,
            text,
            doc,
            expand_stars,
            memo: vec![None; hir.queries.len()],
            visiting: vec![false; hir.queries.len()],
        }
    }

    /// The rows of `id`, computed once.
    fn rows_of(&mut self, id: QueryId) -> Rc<[Row]> {
        let index = id.index();
        if let Some(Some(rows)) = self.memo.get(index) {
            return Rc::clone(rows);
        }
        if self.visiting.get(index).copied().unwrap_or(true) {
            return Rc::from(Vec::new());
        }
        self.visiting[index] = true;
        let rows: Rc<[Row]> = Rc::from(self.build_rows(id));
        self.visiting[index] = false;
        self.memo[index] = Some(Rc::clone(&rows));
        rows
    }

    /// The identifier as written, unquoted; the folded form is for matching.
    fn surface(&self, ident: Ident) -> String {
        let written = self
            .text
            .get(ident.span.start as usize..ident.span.end as usize)
            .filter(|s| !s.is_empty());
        match written {
            Some(text) if ident.quoted => unquote(text),
            Some(text) => text.to_string(), // boundary: reports own their names
            None => self.hir.syms.text(ident.folded).to_string(), // boundary: as above
        }
    }
}

/// `"a""b"` → `a"b`, for any of the dialects' quote pairs.
fn unquote(text: &str) -> String {
    let mut chars = text.chars();
    let Some(open) = chars.next() else {
        return String::new();
    };
    let close = if open == '[' { ']' } else { open };
    let inner = chars.as_str().strip_suffix(close).unwrap_or(chars.as_str());
    let doubled = format!("{close}{close}");
    inner.replace(&doubled, close.encode_utf8(&mut [0; 4]))
}

/// First occurrence wins, so the order stays that of the references.
fn unique(sources: Vec<SourceRef>) -> Vec<SourceRef> {
    let mut seen = HashSet::with_capacity(sources.len());
    sources
        .into_iter()
        .filter(|s| seen.insert(s.clone())) // boundary: a lookup-only set
        .collect()
}
