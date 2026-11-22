//! `subq.not_in_nullable`: `x NOT IN (subquery)` is never true once the
//! subquery returns a NULL (DataFusion `rewrite_set_comparison.rs`).
//!
//! A column known `NotNull` (a `not_null` test, constraint or key) is
//! silent, and so is one the subquery itself filters NULLs out of. A
//! nullable column is a Warning; a column whose nullability is unknown is
//! at most a Hint worded "may", because most columns are undocumented.

use std::collections::BTreeSet;

use cogitator_ir::common::{Lit, UnaryOp};
use cogitator_ir::mir::{ColId, HirNode, Mir, RelId, RelOp, ScalarId, ScalarKind, SubqueryKind};
use cogitator_ir::{Level, Nullability, RuleId, Span};

use super::not_in_fix;
use super::scan::{plan_scalars, touches_opaque};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings, Fix};
use crate::meta::{FixSafety, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};
use crate::rules::join::null_reject::rejects;
use crate::rules::join::support::{hir_ref, no_opaque};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("subq.not_in_nullable"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE,
    severity: Severity::Warning,
    fix: FixSafety::ChangesIntent,
};

pub struct NotInNullable;

impl Rule for NotInNullable {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for NotInNullable {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        let mut spans: BTreeSet<Span> = BTreeSet::new();
        for (_, scalar) in plan_scalars(ir) {
            if let Some(finding) = examine(ir, scalar, cx) {
                if spans.insert(finding.span) {
                    out.push(finding);
                }
            }
        }
    }
}

fn examine(mir: &Mir, id: ScalarId, cx: &Cx<'_>) -> Option<Finding> {
    let scalar = mir.scalars.get(id)?;
    let span = mir.prov.span(scalar.prov).unwrap_or_default();
    match &scalar.kind {
        ScalarKind::InList {
            e,
            list,
            negated: true,
        } => list_with_null(mir, *e, list, span),
        ScalarKind::Subquery {
            kind: SubqueryKind::In { e, negated: true },
            rel,
            ..
        } => {
            let verdict = over_subquery(mir, *e, *rel)?;
            let fixes = fix_for(mir, cx, id).into_iter().collect();
            Some(finding(verdict, span, fixes))
        }
        // `NOT x IN (…)`: the same semantics, a different shape to rewrite.
        ScalarKind::Unary {
            op: UnaryOp::Not,
            e,
        } => {
            let inner = mir.scalars.get(*e)?;
            let ScalarKind::Subquery {
                kind: SubqueryKind::In { e, negated: false },
                rel,
                ..
            } = &inner.kind
            else {
                return None;
            };
            let verdict = over_subquery(mir, *e, *rel)?;
            Some(finding(verdict, span, Vec::new()))
        }
        _ => None,
    }
}

struct Verdict {
    severity: Severity,
    evidence: Evidence,
    message: String,
}

fn finding(verdict: Verdict, span: Span, fixes: Vec<Fix>) -> Finding {
    Finding {
        id: META.id,
        span,
        level: Level::Mir,
        severity: verdict.severity,
        message: verdict.message,
        evidence: verdict.evidence,
        fixes,
    }
}

fn fix_for(mir: &Mir, cx: &Cx<'_>, id: ScalarId) -> Option<Fix> {
    let (hir, text) = (cx.hir?, cx.text?);
    let HirNode::Expr(expr) = hir_ref(mir, mir.scalars.get(id)?.prov)?.node else {
        return None;
    };
    not_in_fix::not_exists(hir, text, expr)
}

fn list_with_null(mir: &Mir, e: ScalarId, list: &[ScalarId], span: Span) -> Option<Finding> {
    if touches_opaque(mir, e) {
        return None;
    }
    let is_null = |s: &ScalarId| {
        matches!(
            mir.scalars.get(*s).map(|n| &n.kind),
            Some(ScalarKind::Lit(Lit::Null))
        )
    };
    list.iter().any(is_null).then(|| {
        finding(
            Verdict {
                severity: Severity::Warning,
                evidence: Evidence::Exact,
                message: "This NOT IN list holds a NULL, so the predicate is never true".to_owned(),
            },
            span,
            Vec::new(),
        )
    })
}

/// The verdict on `e NOT IN (rel)`, or `None` when no NULL can appear or
/// the shape cannot be judged.
fn over_subquery(mir: &Mir, e: ScalarId, rel: RelId) -> Option<Verdict> {
    if touches_opaque(mir, e) || !no_opaque(mir, rel) {
        return None;
    }
    let body = mir.rels.get(rel)?;
    let [col] = body.out.as_slice() else {
        return None;
    };
    let null = mir.cols.get(*col)?.ty.null;
    if null == Nullability::NotNull || nulls_filtered_out(mir, rel, *col) {
        return None;
    }
    let name = column_label(mir, *col);
    Some(match null {
        Nullability::Nullable => Verdict {
            severity: Severity::Warning,
            evidence: Evidence::Exact,
            message: format!(
                "NOT IN is never true once this subquery returns a NULL, and {name} can be NULL; \
                 NOT EXISTS is not affected"
            ),
        },
        _ => Verdict {
            severity: Severity::Hint,
            evidence: Evidence::Heuristic,
            message: format!(
                "NOT IN may never be true: {name} is not known to be NOT NULL, and one NULL from \
                 this subquery stops the predicate passing"
            ),
        },
    })
}

fn column_label(mir: &Mir, col: ColId) -> String {
    match mir.cols.get(col).and_then(|c| c.name) {
        Some(name) => format!("`{}`", mir.syms.text(name)),
        None => "the subquery's column".to_owned(),
    }
}

/// True when a filter under `rel` already rejects NULLs of `col`, so the
/// subquery cannot return one.
fn nulls_filtered_out(mir: &Mir, rel: RelId, col: ColId) -> bool {
    let mut col = col;
    let mut at = rel;
    for _ in 0..=mir.rels.len() {
        let Some(node) = mir.rels.get(at) else {
            return false;
        };
        match &node.op {
            RelOp::Filter { input, conjuncts } => {
                let side = BTreeSet::from([col]);
                if conjuncts.iter().any(|c| rejects(mir, *c, &side)) {
                    return true;
                }
                at = *input;
            }
            RelOp::Sort { input, .. } | RelOp::Limit { input, .. } | RelOp::Distinct(input) => {
                at = *input;
            }
            RelOp::Project { input, items } => {
                let Some((_, scalar)) = items.iter().find(|(c, _)| *c == col) else {
                    return false;
                };
                let Some(ScalarKind::Col(from)) = mir.scalars.get(*scalar).map(|s| &s.kind) else {
                    return false;
                };
                col = *from;
                at = *input;
            }
            _ => return false,
        }
    }
    false
}
