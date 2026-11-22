//! AST to HIR: name resolution happens during the walk, because a scope
//! is only meaningful while its FROM items are in hand. The AST is walked
//! by hand (sqlparser's visitor is unusable at this version).

use std::collections::HashMap;

use cogitator_ir::common::ModelConfig;
use cogitator_ir::hir::{
    CteId, FromCol, Hir, JinjaKind, JinjaNode, QueryId, Scope, ScopeId, ScopeKind,
};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::{Degradation, Interner, Span, Symbol};

use crate::config::apply_config;
use crate::dialect::DialectRules;
use crate::mask::{Region, RegionKind, Scanned};
use crate::parse::{Outcome, Parsed, Part, Parts};
use crate::segment::Positions;
use crate::{HirInput, SchemaEnv};

mod alloc;
mod binder;
mod call;
mod column;
mod cte;
mod expr;
mod from;
mod from_base;
mod from_fn;
mod literal;
mod names;
mod ops;
mod query;
mod select;
mod setop;
mod star;

use binder::{AliasMode, Frame};

/// Above this width a FROM item gets a name index; below, a scan is faster.
const INDEXED_WIDTH: usize = 64;

/// One lowering of one template. `words` maps placeholder bytes to regions.
pub(crate) struct Lowerer<'a> {
    pub(crate) hir: Hir,
    pos: &'a Positions,
    regions: &'a [Region],
    /// Sorted by start: `(placeholder word, region index)`.
    words: Vec<(Span, usize)>,
    consumed: Vec<bool>,
    env: &'a SchemaEnv,
    rules: DialectRules,
    frames: Vec<Frame>,
    /// CTEs in scope, innermost last.
    visible: Vec<(Symbol, CteId)>,
    /// Per `FromId`: the names a qualifier may use, outermost first.
    from_names: Vec<Vec<Symbol>>,
    /// Per `FromId`: the output name index for wide items.
    from_index: Vec<Option<HashMap<Symbol, u32>>>,
    /// Per `QueryId`: true when the output list may be incomplete.
    query_open: Vec<bool>,
    root_scope: ScopeId,
    /// Lambda parameters in scope; their names are not columns.
    lambda: Vec<Symbol>,
    mode: AliasMode,
    depth: usize,
    /// Output columns charged against the node cap.
    extra_nodes: usize,
    cap_reported: bool,
    depth_reported: bool,
    degraded: Vec<Degradation>,
}

/// Lowers a parsed statement into a bound HIR plus its degradations.
pub(crate) fn lower(
    parsed: Parsed,
    scanned: &Scanned,
    pos: &Positions,
    input: &HirInput<'_>,
    env: &SchemaEnv,
) -> (Hir, Vec<Degradation>) {
    let origin = variant_origin(input, scanned.variant);
    let rules = DialectRules::for_dialect(input.dialect);
    let mut hir = Hir::new(origin);
    let doc_span = Span::new(0, input.text.len() as u32);
    let root_scope = hir.scopes.alloc(Scope {
        span: doc_span,
        parent: None,
        kind: ScopeKind::Root,
        from: Vec::new(),
    });
    hir.config = merged_config(input, scanned, &mut hir.syms, &rules);
    alloc_regions(&mut hir, &scanned.regions);
    let words = placeholder_index(&scanned.regions);
    let mut lowerer = Lowerer {
        consumed: vec![false; scanned.regions.len()],
        words,
        hir,
        pos,
        regions: &scanned.regions,
        env,
        rules,
        frames: Vec::new(),
        visible: Vec::new(),
        from_names: Vec::new(),
        from_index: Vec::new(),
        query_open: Vec::new(),
        root_scope,
        lambda: Vec::new(),
        mode: AliasMode::Off,
        depth: 0,
        extra_nodes: 0,
        cap_reported: false,
        depth_reported: false,
        degraded: parsed.degraded,
    };
    let root = lowerer.outcome(&parsed.outcome, doc_span);
    lowerer.hir.root = Some(root);
    (lowerer.hir, lowerer.degraded)
}

fn variant_origin(input: &HirInput<'_>, variant: TemplateVariant) -> SourceOrigin {
    match &input.origin {
        SourceOrigin::Template { .. } => SourceOrigin::Template { variant },
        // boundary: the origin is owned by the plan (08 §1.5)
        other => other.clone(),
    }
}

fn merged_config(
    input: &HirInput<'_>,
    scanned: &Scanned,
    syms: &mut Interner,
    rules: &DialectRules,
) -> ModelConfig {
    // boundary: the plan owns its config (08 §1.5)
    let mut config = input.config.clone();
    for kwargs in &scanned.configs {
        apply_config(&mut config, kwargs, syms, rules);
    }
    config
}

fn alloc_regions(hir: &mut Hir, regions: &[Region]) {
    for region in regions {
        let kind = region_kind(&region.kind, &mut hir.syms);
        hir.jinja.alloc(JinjaNode {
            span: region.span,
            kind,
            role: region.initial_role(),
        });
    }
}

fn region_kind(kind: &RegionKind, syms: &mut Interner) -> JinjaKind {
    match kind {
        RegionKind::Ref => JinjaKind::Ref,
        RegionKind::Source => JinjaKind::Source,
        RegionKind::This => JinjaKind::This,
        RegionKind::Var => JinjaKind::Var,
        RegionKind::Macro(name) => JinjaKind::Macro(syms.intern(name)),
        RegionKind::IsIncremental => JinjaKind::IsIncremental,
        RegionKind::Config => JinjaKind::Config,
        RegionKind::Expression => JinjaKind::Expression,
        RegionKind::Statement => JinjaKind::Statement,
        RegionKind::Comment => JinjaKind::Comment,
    }
}

fn placeholder_index(regions: &[Region]) -> Vec<(Span, usize)> {
    let mut words: Vec<(Span, usize)> = regions
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.word.map(|w| (w, i)))
        .collect();
    words.sort_by_key(|(w, i)| (w.start, *i));
    words
}

impl Lowerer<'_> {
    fn outcome(&mut self, outcome: &Outcome, doc_span: Span) -> QueryId {
        match outcome {
            Outcome::Whole(query) => self.query(query, ScopeKind::Query, false),
            Outcome::Parts(parts) => self.parts(parts),
            Outcome::Hole(kind, span) => {
                let span = if span.is_empty() { doc_span } else { *span };
                self.hole_query(*kind, span, Vec::new())
            }
        }
    }

    /// The root of a statement that only parsed in pieces.
    fn parts(&mut self, parts: &Parts) -> QueryId {
        let scope = self.alloc_scope(ScopeKind::Query, self.root_scope, Span::default());
        self.frames.push(Frame::new(scope, false));
        let mark = self.visible.len();
        let ctes = self.cte_parts(parts);
        let root = match &parts.body {
            Part::Parsed(query) => {
                let id = self.query(query, ScopeKind::Query, false);
                self.prepend_ctes(id, ctes);
                id
            }
            Part::Hole(kind, span) => self.hole_query(*kind, *span, ctes),
        };
        self.visible.truncate(mark);
        self.frames.pop();
        root
    }
}

pub(super) fn union(a: Span, b: Span) -> Span {
    Span::new(a.start.min(b.start), a.end.max(b.end))
}

fn name_index(output: &[FromCol]) -> HashMap<Symbol, u32> {
    let mut index = HashMap::with_capacity(output.len());
    for (i, col) in output.iter().enumerate() {
        index.entry(col.name).or_insert(i as u32);
    }
    index
}
