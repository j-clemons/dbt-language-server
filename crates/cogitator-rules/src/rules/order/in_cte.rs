//! `order.in_cte`: an ORDER BY in a CTE or subquery that nothing above
//! it can observe (DuckDB `remove_redundant_order.cpp`).
//!
//! The model's outermost ORDER BY is never flagged. A Sort is silent when
//! anything above it may read its order: a LIMIT (or OFFSET), a window, an
//! order-sensitive or unknown aggregate, a set operation, a scalar
//! subquery (`ARRAY(SELECT … ORDER BY …)` is one) or anything opaque.

use std::collections::{BTreeMap, BTreeSet};

use cogitator_ir::hir::{ExprKind, Hir, Query, QueryId};
use cogitator_ir::lir::Backend;
use cogitator_ir::mir::{AggCall, HirNode, Mir, RelId, RelOp, ScalarId, ScalarKind, SubqueryKind};
use cogitator_ir::{Level, RuleId, Span};

use super::remove;
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings, Fix};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};
use crate::rules::join::support::{expr, hir_ref, no_opaque};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("order.in_cte"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    // a Jinja branch not taken can hide a LIMIT
    needs: Needs::NONE.with(Need::AllBranches),
    severity: Severity::Info,
    // P where order is unobservable, N on DuckDB and unknown dialects
    fix: FixSafety::Mixed,
};

/// Aggregates whose result cannot depend on input order.
const ORDER_INSENSITIVE: &[&str] = &[
    "avg",
    "bit_and",
    "bit_or",
    "bit_xor",
    "bool_and",
    "bool_or",
    "count",
    "max",
    "mean",
    "median",
    "min",
    "stddev",
    "stddev_pop",
    "stddev_samp",
    "sum",
    "var_pop",
    "var_samp",
    "variance",
];

pub struct InCte;

impl Rule for InCte {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for InCte {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        // the authored shape and the text decide what may be claimed
        let (Some(hir), Some(text)) = (cx.hir, cx.text) else {
            return;
        };
        let Some(root) = hir.root else {
            return;
        };
        let parents = Parents::of(ir);
        let mut spans = BTreeSet::new();
        for id in ir.reachable() {
            let Some(finding) = examine(ir, hir, text, cx, id, root, &parents) else {
                continue;
            };
            if spans.insert(finding.span) {
                out.push(finding);
            }
        }
    }
}

fn examine(
    mir: &Mir,
    hir: &Hir,
    text: &str,
    cx: &Cx<'_>,
    id: RelId,
    root: QueryId,
    parents: &Parents,
) -> Option<Finding> {
    let rel = mir.rels.get(id)?;
    let RelOp::Sort { keys, .. } = &rel.op else {
        return None;
    };
    let HirNode::Query(q) = hir_ref(mir, rel.prov)?.node else {
        return None;
    };
    if q == root || !no_opaque(mir, id) || keys_opaque(mir, keys) || parents.observed(mir, id) {
        return None;
    }
    let query = hir.queries.get(q)?;
    if query.order_by.is_empty()
        || query.limit.is_some()
        || keys_unmodelled(hir, query)
        || has_offset(hir, query, text)
    {
        return None;
    }
    let span =
        order_span(hir, query).unwrap_or_else(|| mir.prov.span(rel.prov).unwrap_or_default());
    Some(Finding {
        id: META.id,
        span,
        level: Level::Mir,
        severity: META.severity,
        message: "This ORDER BY has no effect: nothing here limits or reads the order, and SQL \
                  does not guarantee it survives the enclosing query"
            .to_owned(),
        evidence: Evidence::Exact,
        fixes: fix(hir, text, query, cx).into_iter().collect(),
    })
}

fn keys_opaque(mir: &Mir, keys: &[cogitator_ir::mir::SortKey]) -> bool {
    keys.iter()
        .any(|k| crate::rules::subq::scan::touches_opaque(mir, k.expr))
}

/// True when an authored ORDER BY key is Jinja, opaque or unparsed. The
/// plan's key is then a hidden column and hides that from the Sort.
fn keys_unmodelled(hir: &Hir, query: &Query) -> bool {
    query.order_by.iter().any(|key| {
        let mut stack = vec![key.expr];
        let mut budget = hir.exprs.len() + 1;
        while let Some(id) = stack.pop() {
            if budget == 0 {
                return true;
            }
            budget -= 1;
            match expr(hir, id).map(|e| &e.kind) {
                None | Some(ExprKind::Jinja(_) | ExprKind::Opaque { .. } | ExprKind::Hole) => {
                    return true;
                }
                Some(kind) => stack.extend(kind.children()),
            }
        }
        false
    })
}

fn fix(hir: &Hir, text: &str, query: &Query, cx: &Cx<'_>) -> Option<Fix> {
    let span = remove::order_by_span(hir, text, query)?;
    let safety = match cx.dialect {
        Some(Backend::Snowflake | Backend::BigQuery | Backend::Postgres) => FixSafety::Preserving,
        // DuckDB preserves insertion order by default, and an unknown
        // engine might, so deleting the sort can be observable there
        _ => FixSafety::ChangesIntent,
    };
    Some(Fix {
        title: "Remove this ORDER BY".to_owned(),
        safety,
        edits: vec![crate::findings::FixEdit {
            span,
            new_text: String::new(),
        }],
    })
}

/// The written order keys, from the first's start to the last's end.
fn order_span(hir: &Hir, query: &Query) -> Option<Span> {
    let first = expr(hir, query.order_by.first()?.expr)?.span;
    let last = expr(hir, query.order_by.last()?.expr)?.span;
    (first.start < last.end).then_some(Span::new(first.start, last.end))
}

/// HIR drops a bare `OFFSET`, which depends on the order, so the text
/// after the last key is the only place left to see one.
fn has_offset(hir: &Hir, query: &Query, text: &str) -> bool {
    let Some(end) = query
        .order_by
        .last()
        .and_then(|k| expr(hir, k.expr))
        .map(|e| e.span.end)
    else {
        return true;
    };
    let Some(rest) = text.get(end as usize..) else {
        return true;
    };
    // an expression key's span can stop inside its own parens, so skip
    // any closing ones first; running on into the outer query at worst
    // silences the rule
    let rest = rest.trim_start_matches(|c: char| c == ')' || c.is_whitespace());
    let head = rest.split(')').next().unwrap_or(rest);
    head.to_ascii_lowercase().contains("offset")
}

/// What consumes a relation's output.
#[derive(Clone, Copy)]
enum Parent {
    Rel(RelId),
    Sub(SubqueryKind),
}

struct Parents(BTreeMap<RelId, Vec<Parent>>);

impl Parents {
    fn of(mir: &Mir) -> Self {
        let mut map: BTreeMap<RelId, Vec<Parent>> = BTreeMap::new();
        for id in mir.reachable() {
            let Some(rel) = mir.rels.get(id) else {
                continue;
            };
            for input in rel.op.inputs() {
                map.entry(input).or_default().push(Parent::Rel(id));
            }
            if let RelOp::CteRef { cte, .. } = rel.op {
                if let Some(def) = mir.ctes.get(cte) {
                    map.entry(def.body).or_default().push(Parent::Rel(id));
                }
            }
            for scalar in rel.op.scalars() {
                note_subqueries(mir, scalar, &mut map);
            }
        }
        Self(map)
    }

    /// True when something above `start` may read its order.
    fn observed(&self, mir: &Mir, start: RelId) -> bool {
        let mut seen = BTreeSet::new();
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            for parent in self.0.get(&id).map_or(&[][..], Vec::as_slice) {
                let rel_id = match parent {
                    Parent::Sub(SubqueryKind::In { .. } | SubqueryKind::Exists) => continue,
                    Parent::Sub(SubqueryKind::Scalar) => return true,
                    Parent::Rel(r) => *r,
                };
                let Some(rel) = mir.rels.get(rel_id) else {
                    return true;
                };
                match &rel.op {
                    RelOp::Sort { .. } => {}
                    RelOp::Limit { .. }
                    | RelOp::Window { .. }
                    | RelOp::SetOp { .. }
                    | RelOp::Opaque { .. } => return true,
                    RelOp::Aggregate { aggs, .. } => {
                        if aggs.iter().any(|(_, a)| !order_insensitive(mir, a)) {
                            return true;
                        }
                    }
                    _ => stack.push(rel_id),
                }
            }
        }
        false
    }
}

fn order_insensitive(mir: &Mir, call: &AggCall) -> bool {
    let name = mir.syms.text(call.func);
    ORDER_INSENSITIVE
        .iter()
        .any(|f| f.eq_ignore_ascii_case(name))
}

/// Records every subquery under `root` as read by its kind.
fn note_subqueries(mir: &Mir, root: ScalarId, map: &mut BTreeMap<RelId, Vec<Parent>>) {
    let mut stack = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = mir.scalars.get(id) else {
            continue;
        };
        if let ScalarKind::Subquery { kind, rel, .. } = node.kind {
            map.entry(rel).or_default().push(Parent::Sub(kind));
        }
        stack.extend(node.kind.children());
    }
}
