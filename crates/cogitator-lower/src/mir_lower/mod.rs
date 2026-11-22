//! HIR → MIR (specs/ir-pipeline/02).
//!
//! The walk is depth-first and appends bottom-up, so every operand and
//! subquery precedes its user. Each FROM item gets a `Binding` mapping
//! `Resolution` indexes to `ColId`s; columns of open schemas are found by
//! the `demand` pre-pass, because a `Scan`'s column list is fixed when
//! it is allocated. The HIR is read with `get`: lowering must stay total
//! even on HIR that was never verified.

mod aggregate;
mod column;
mod cte;
mod demand;
mod equal;
mod expr;
mod from;
mod join;
mod order;
mod order_key;
mod query;
mod select;
mod set_op;
mod star;
mod window;

use std::collections::BTreeMap;

use cogitator_ir::common::{HoleKind, HoleRef};
use cogitator_ir::hir::{self, ExprId, FromId, Hir};
use cogitator_ir::mir::{
    verify_mir, ColId, HirNode, HirRef, Mir, Origin, ProvId, RelId, RelOp, ScalarId, ScalarKind,
};
use cogitator_ir::{
    ColType, Degradation, DegradationKind, Invariant, Lowered, Span, Symbol, Violation,
};

use demand::Demand;

/// Work budget, above the front end's 200k-node cap: star expansion and
/// re-lowered lateral aliases add nodes the HIR does not count.
const MAX_STEPS: usize = 400_000;
/// Nested queries; each level costs several large stack frames.
const MAX_QUERY_DEPTH: usize = 48;
/// Nested join trees.
const MAX_FROM_DEPTH: usize = 256;
/// Nested expressions, just above the front end's 256.
const MAX_EXPR_DEPTH: usize = 300;

/// What one FROM item contributes to name resolution.
struct Binding {
    /// By `From.output` index, then any columns the demand pass added.
    cols: Vec<ColId>,
    /// How many correlation frames were open when it was lowered.
    depth: usize,
    /// On the null-supplying side of an outer join.
    nullable: bool,
}

/// The expression context of one SELECT.
#[derive(Default)]
struct Env {
    /// Group keys as HIR expressions, with their output and type.
    keys: Vec<(ExprId, ColId, ColType)>,
    /// Columns that passed through the Aggregate as keys.
    key_cols: Vec<ColId>,
    aggs: BTreeMap<ExprId, ColId>,
    wins: BTreeMap<ExprId, ColId>,
    /// Above the Aggregate: a plain column must be a key.
    grouped: bool,
    /// `select.items` index → aliased expression, for `LateralAlias`.
    aliases: Vec<Option<ExprId>>,
    /// Aliases being expanded; stops `select b as b` recursing.
    expanding: Vec<u32>,
}

struct Ctx<'h> {
    hir: &'h Hir,
    mir: Mir,
    degraded: Vec<Degradation>,
    violations: Vec<Violation>,
    bindings: BTreeMap<FromId, Binding>,
    ctes: BTreeMap<hir::CteId, cogitator_ir::mir::CteId>,
    demand: Demand,
    /// One per open correlation boundary; collects outer columns read.
    frames: Vec<Vec<ColId>>,
    env: Env,
    steps: usize,
    query_depth: usize,
    from_depth: usize,
    expr_depth: usize,
    over_budget: bool,
}

pub(crate) fn lower(hir: &Hir) -> Lowered<Mir> {
    let Some(root) = hir.root else {
        return Lowered::declined(vec![Degradation::new(
            DegradationKind::ParseHole,
            "the model has no query to lower",
        )]);
    };
    let mut cx = Ctx::new(hir);
    let out = cx.query(root, &[]);
    cx.mir.root = out.rel;
    // boundary: the Mir owns its copy of the model config
    cx.mir.config = hir.config.clone();
    let Ctx {
        mir,
        degraded,
        violations,
        ..
    } = cx;
    let verified = verify_mir(&mir, Some(hir));
    let lowered = if degraded.is_empty() {
        Lowered::complete(mir)
    } else {
        Lowered::partial(mir, degraded)
    };
    lowered
        .with_violations(violations)
        .with_violations(verified)
}

impl<'h> Ctx<'h> {
    fn new(hir: &'h Hir) -> Self {
        Self {
            hir,
            // boundary: the Mir owns its interner so it prints without HIR
            mir: Mir::new(hir.syms.clone()),
            degraded: Vec::new(),
            violations: Vec::new(),
            bindings: BTreeMap::new(),
            ctes: BTreeMap::new(),
            demand: Demand::of(hir),
            frames: Vec::new(),
            env: Env::default(),
            steps: 0,
            query_depth: 0,
            from_depth: 0,
            expr_depth: 0,
            over_budget: false,
        }
    }

    fn prov(&mut self, node: HirNode, span: Span) -> ProvId {
        self.mir.prov.add(Origin::Hir(HirRef { node, span }))
    }

    fn degrade(&mut self, kind: DegradationKind, span: Span, detail: impl Into<String>) {
        self.degraded.push(Degradation::new(kind, detail).at(span));
    }

    fn bug(&mut self, detail: impl Into<String>) {
        self.violations
            .push(Violation::new(Invariant::HirIdRange, detail));
    }

    /// Counts one unit of work; false once the budget is spent.
    fn tick(&mut self) -> bool {
        self.steps += 1;
        if self.steps > MAX_STEPS && !self.over_budget {
            self.over_budget = true;
            self.degraded.push(Degradation::new(
                DegradationKind::CapExceeded,
                format!("lowering stopped after {MAX_STEPS} steps"),
            ));
        }
        !self.over_budget
    }

    fn cap(&mut self, span: Span, what: &str) {
        self.degrade(
            DegradationKind::CapExceeded,
            span,
            format!("{what} exceeds the lowering depth limit"),
        );
    }

    fn col_type(&self, col: ColId) -> ColType {
        self.mir
            .cols
            .get(col)
            .map_or(ColType::UNKNOWN, |def| def.ty.clone())
    }

    fn col_name(&self, col: ColId) -> Option<Symbol> {
        self.mir.cols.get(col).and_then(|def| def.name)
    }

    fn col_scalar(&mut self, col: ColId, ty: ColType, prov: ProvId) -> ScalarId {
        self.mir.add_scalar(ScalarKind::Col(col), ty, prov)
    }

    fn opaque_scalar(
        &mut self,
        children: Vec<ScalarId>,
        kind: HoleKind,
        span: Span,
        prov: ProvId,
    ) -> ScalarId {
        let hole = HoleRef { kind, span };
        self.mir.add_scalar(
            ScalarKind::Opaque { children, hole },
            ColType::UNKNOWN,
            prov,
        )
    }

    /// The row source of a `SELECT` without FROM. Not a gap in the
    /// input, so it carries no `Degradation`.
    fn singleton(&mut self, prov: ProvId) -> RelId {
        let hole = HoleRef {
            kind: HoleKind::Unsupported,
            span: Span::default(),
        };
        self.mir.add_rel(
            RelOp::Opaque {
                inputs: Vec::new(),
                out: Vec::new(),
                open: false,
                hole,
            },
            prov,
        )
    }

    /// An `Opaque` with fresh output columns; `inputs` stay analysable.
    fn opaque_rel(
        &mut self,
        inputs: Vec<RelId>,
        spec: OpaqueSpec,
        prov: ProvId,
    ) -> (RelId, Vec<ColId>) {
        let out: Vec<ColId> = spec
            .cols
            .into_iter()
            .map(|(name, ty)| self.mir.add_col(name, ty))
            .collect();
        let hole = HoleRef {
            kind: spec.kind,
            span: spec.span,
        };
        let rel = self.mir.add_rel(
            RelOp::Opaque {
                inputs,
                out: out.clone(),
                open: spec.open,
                hole,
            },
            prov,
        );
        (rel, out)
    }
}

/// The columns and hole of an `Opaque` relation being built.
struct OpaqueSpec {
    cols: Vec<(Option<Symbol>, ColType)>,
    open: bool,
    kind: HoleKind,
    span: Span,
}
