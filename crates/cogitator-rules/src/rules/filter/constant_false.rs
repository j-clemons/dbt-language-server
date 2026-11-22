//! `filter.constant_false`: a filter that keeps no rows (DuckDB
//! `empty_result_pullup.cpp`; DataFusion `eliminate_filter.rs`,
//! `propagate_empty_relation.rs`).
//!
//! `canon.simplify` keeps `false` and NULL conjuncts. `where false` and
//! `where 1 = 0` as the only filter on a query are the deliberate
//! schema-only idioms and stay silent; `LIMIT 0` is not a filter at all.
//! A contradiction such as `a = 1 and a = 2` is flagged on the second
//! comparison. Number and boolean literals compare exactly; two strings
//! count as different only when no common collation could equate them.

use std::collections::{BTreeMap, BTreeSet};

use cogitator_ir::common::{BinaryOp, Lit};
use cogitator_ir::hir::{Body, ExprId, ExprKind, Hir};
use cogitator_ir::mir::{ColId, HirNode, Mir, RelId, RelOp, ScalarId, ScalarKind};
use cogitator_ir::{Level, RuleId, Span};

use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};
use crate::rules::join::support::hir_ref;
use crate::rules::subq::scan::touches_opaque;

pub static META: RuleMeta = RuleMeta {
    id: RuleId("filter.constant_false"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE,
    severity: Severity::Hint,
    fix: FixSafety::None,
};

pub struct ConstantFalse;

impl Rule for ConstantFalse {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for ConstantFalse {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        let mut spans: BTreeSet<Span> = BTreeSet::new();
        for id in ir.reachable() {
            if let Some(finding) = examine(ir, id, cx) {
                if spans.insert(finding.span) {
                    out.push(finding);
                }
            }
        }
    }
}

fn examine(mir: &Mir, id: RelId, cx: &Cx<'_>) -> Option<Finding> {
    let RelOp::Filter { conjuncts, .. } = &mir.rels.get(id)?.op else {
        return None;
    };
    // an opaque conjunct may hold more of the authored predicate than the
    // plan shows, so nothing is claimed about this filter
    if conjuncts.iter().any(|c| touches_opaque(mir, *c)) {
        return None;
    }
    constant_conjunct(mir, conjuncts, cx).or_else(|| contradiction(mir, conjuncts))
}

fn finding(mir: &Mir, id: ScalarId, message: String) -> Option<Finding> {
    let scalar = mir.scalars.get(id)?;
    Some(Finding {
        id: META.id,
        span: mir.prov.span(scalar.prov).unwrap_or_default(),
        level: Level::Mir,
        severity: META.severity,
        message,
        evidence: Evidence::Exact,
        fixes: Vec::new(),
    })
}

/// A conjunct `canon.simplify` reduced to `false` or NULL.
fn constant_conjunct(mir: &Mir, conjuncts: &[ScalarId], cx: &Cx<'_>) -> Option<Finding> {
    conjuncts.iter().find_map(|c| {
        match mir.scalars.get(*c).map(|s| &s.kind)? {
            ScalarKind::Lit(Lit::Bool(false)) => {
                // without the HIR the deliberate idioms cannot be told apart
                let exempt = cx.hir.is_none_or(|hir| is_schema_only_idiom(mir, hir, *c));
                (!exempt).then(|| {
                    finding(
                        mir,
                        *c,
                        "This filter is always false, so the query returns no rows".to_owned(),
                    )
                })?
            }
            ScalarKind::Lit(Lit::Null) => finding(
                mir,
                *c,
                "This filter evaluates to NULL, so the query returns no rows".to_owned(),
            ),
            _ => None,
        }
    })
}

/// True when the conjunct is the entire WHERE of its SELECT, written as
/// `false` or as a comparison of two literals, and that SELECT has no
/// other filter.
fn is_schema_only_idiom(mir: &Mir, hir: &Hir, conjunct: ScalarId) -> bool {
    let Some(prov) = mir.scalars.get(conjunct).map(|s| s.prov) else {
        return false;
    };
    let Some(HirNode::Expr(expr)) = hir_ref(mir, prov).map(|r| r.node) else {
        return false;
    };
    is_literal_false_shape(hir, expr)
        && hir.queries.iter().any(|(_, query)| {
            matches!(&query.body, Body::Select(select)
                if select.filter == Some(expr)
                    && select.having.is_none()
                    && select.qualify.is_none())
        })
}

fn is_literal_false_shape(hir: &Hir, expr: ExprId) -> bool {
    let kind = |id: ExprId| hir.exprs.get(id).map(|e| &e.kind);
    match kind(expr) {
        Some(ExprKind::Lit(Lit::Bool(false))) => true,
        Some(ExprKind::Binary { l, r, .. }) => {
            matches!(kind(*l), Some(ExprKind::Lit(_))) && matches!(kind(*r), Some(ExprKind::Lit(_)))
        }
        _ => false,
    }
}

/// Two `col = literal` conjuncts pinning one column to different values.
fn contradiction(mir: &Mir, conjuncts: &[ScalarId]) -> Option<Finding> {
    let mut pinned: BTreeMap<ColId, Lit> = BTreeMap::new();
    for conjunct in conjuncts {
        let Some((col, lit)) = col_equals_literal(mir, *conjunct) else {
            continue;
        };
        match pinned.get(&col) {
            Some(first) if literals_differ(mir, first, &lit) => {
                let name = mir
                    .cols
                    .get(col)
                    .and_then(|c| c.name)
                    .map_or("this column", |n| mir.syms.text(n));
                return finding(
                    mir,
                    *conjunct,
                    format!(
                        "`{name}` cannot equal two different values, so this filter keeps no rows"
                    ),
                );
            }
            Some(_) => {}
            None => {
                pinned.insert(col, lit);
            }
        }
    }
    None
}

fn col_equals_literal(mir: &Mir, id: ScalarId) -> Option<(ColId, Lit)> {
    let ScalarKind::Binary {
        op: BinaryOp::Eq,
        l,
        r,
    } = &mir.scalars.get(id)?.kind
    else {
        return None;
    };
    let kind = |s: ScalarId| mir.scalars.get(s).map(|n| &n.kind);
    match (kind(*l)?, kind(*r)?) {
        (ScalarKind::Col(c), ScalarKind::Lit(lit)) | (ScalarKind::Lit(lit), ScalarKind::Col(c)) => {
            Some((*c, *lit))
        }
        _ => None,
    }
}

/// True only when the two literals provably differ: an exact number or
/// boolean comparison, or ASCII strings that differ under case folding
/// and trailing-space padding (no collation in use equates those).
fn literals_differ(mir: &Mir, a: &Lit, b: &Lit) -> bool {
    match (a, b) {
        (Lit::Bool(x), Lit::Bool(y)) => x != y,
        (Lit::Number(x), Lit::Number(y)) => {
            match (
                decimal_key(mir.syms.text(*x)),
                decimal_key(mir.syms.text(*y)),
            ) {
                (Some(x), Some(y)) => x != y,
                _ => false,
            }
        }
        (Lit::Str(x), Lit::Str(y)) => {
            let (x, y) = (mir.syms.text(*x), mir.syms.text(*y));
            x.is_ascii() && y.is_ascii() && !x.trim_end().eq_ignore_ascii_case(y.trim_end())
        }
        _ => false,
    }
}

/// `(integer digits, fraction digits)` with redundant zeros removed, so
/// `1` and `1.0` compare equal. `None` for anything but plain decimals.
fn decimal_key(text: &str) -> Option<(&str, &str)> {
    let (int, frac) = text.split_once('.').unwrap_or((text, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || !digits(frac) || (int.is_empty() && frac.is_empty()) {
        return None;
    }
    Some((int.trim_start_matches('0'), frac.trim_end_matches('0')))
}
