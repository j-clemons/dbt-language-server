//! `model.unused_cte`: a CTE nothing reaches from the model's root query.
//!
//! The claim is about the whole template, so anything that could hide a
//! reference (a hole, opaque SQL, a dynamic relation, or a macro in clause
//! position) blocks it. A macro that yields a scalar could in principle
//! hold a subquery, so it only lowers the finding to an unfixable Hint.

use std::collections::BTreeSet;

use cogitator_ir::hir::{
    Body, CteId, ExprKind, FromKind, Hir, JinjaKind, JinjaRole, Query, RelTarget,
};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::{Level, RuleId, Span};

use super::reach::{reachable_ctes, referenced_anywhere};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings, Fix, FixEdit};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("model.unused_cte"),
    level: RuleLevel::Hir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::AllBranches),
    severity: Severity::Warning,
    fix: FixSafety::Preserving,
};

pub struct UnusedCte;

impl Rule for UnusedCte {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Hir> for UnusedCte {
    fn check(&self, ir: &Hir, cx: &Cx<'_>, out: &mut Findings) {
        let Some(root) = ir.root else {
            return;
        };
        let visibility = jinja_visibility(ir);
        if visibility == Visibility::Blocked || hides_sql(ir) || !every_branch_visible(ir) {
            return;
        }
        let used = reachable_ctes(ir, root);
        let mentioned = referenced_anywhere(ir);
        for (_, query) in ir.queries.iter() {
            for (at, cte) in query.ctes.iter().enumerate() {
                if used.contains(cte) {
                    continue;
                }
                if let Some(finding) = finding(ir, cx, query, at, &mentioned, visibility) {
                    out.push(finding);
                }
            }
        }
    }
}

fn finding(
    hir: &Hir,
    cx: &Cx<'_>,
    query: &Query,
    at: usize,
    mentioned: &BTreeSet<CteId>,
    visibility: Visibility,
) -> Option<Finding> {
    let id = *query.ctes.get(at)?;
    let cte = hir.ctes.get(id)?;
    let name = hir.syms.text(cte.name.folded);
    let (severity, evidence, message, fixes) = match visibility {
        Visibility::Macros => (
            Severity::Hint,
            Evidence::Heuristic,
            format!("CTE `{name}` looks unused; a macro in this model could still reference it"),
            Vec::new(),
        ),
        _ => (
            Severity::Warning,
            Evidence::Exact,
            format!("CTE `{name}` is never used"),
            deletion(hir, cx, query, at, mentioned)
                .into_iter()
                .collect(),
        ),
    };
    Some(Finding {
        id: META.id,
        span: cte.span,
        level: Level::Hir,
        severity,
        message,
        evidence,
        fixes,
    })
}

/// Deleting a CTE removes its name through the next CTE's name, comma
/// included, so the list stays valid. The last or only CTE is left alone:
/// its comma or the WITH keyword have no exact span. A CTE another CTE
/// names (even an unused one) stays until that reference is gone.
fn deletion(
    hir: &Hir,
    cx: &Cx<'_>,
    query: &Query,
    at: usize,
    mentioned: &BTreeSet<CteId>,
) -> Option<Fix> {
    let text = cx.text?;
    if !matches!(hir.origin, SourceOrigin::Template { .. }) {
        return None;
    }
    let id = *query.ctes.get(at)?;
    let cte = hir.ctes.get(id)?;
    if mentioned.contains(&id) || cte.recursive || cte.name.span.is_empty() {
        return None;
    }
    let next = hir.ctes.get(*query.ctes.get(at + 1)?)?;
    let span = Span::new(cte.name.span.start, next.name.span.start);
    if !span.is_valid_in(text) || span.is_empty() || !comma_precedes(text, next.name.span.start) {
        return None;
    }
    Some(Fix {
        title: format!("Delete CTE `{}`", hir.syms.text(cte.name.folded)),
        safety: FixSafety::Preserving,
        edits: vec![FixEdit {
            span,
            new_text: String::new(),
        }],
    })
}

/// The next CTE's name must follow `), `: a comment in between belongs
/// to it and would be deleted with the CTE.
fn comma_precedes(text: &str, next: u32) -> bool {
    let Some(before) = text.get(..next as usize) else {
        return false;
    };
    before
        .trim_end()
        .strip_suffix(',')
        .is_some_and(|s| s.trim_end().ends_with(')'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Full,
    /// Scalar-position macros only.
    Macros,
    Blocked,
}

/// The worst any Jinja region does to the reference analysis. `ref`,
/// `source`, `this` and `var` name relations or scalars, never a CTE.
fn jinja_visibility(hir: &Hir) -> Visibility {
    let mut out = Visibility::Full;
    for (_, node) in hir.jinja.iter() {
        let opaque = matches!(
            node.kind,
            JinjaKind::Macro(_) | JinjaKind::Expression | JinjaKind::Statement
        );
        if opaque {
            match node.role {
                JinjaRole::Blank => {}
                JinjaRole::Scalar => out = Visibility::Macros,
                _ => return Visibility::Blocked,
            }
        }
    }
    out
}

/// An `is_incremental` branch not taken can use a CTE this build cannot
/// see, so the template must have been built with every branch visible.
fn every_branch_visible(hir: &Hir) -> bool {
    let plain = matches!(
        hir.origin,
        SourceOrigin::Template {
            variant: TemplateVariant::Plain
        } | SourceOrigin::Compiled { .. }
    );
    plain
        && !hir
            .jinja
            .iter()
            .any(|(_, n)| matches!(n.kind, JinjaKind::IsIncremental))
}

/// Holes, opaque expressions and dynamic relations: places a reference
/// could be written that the walk below cannot read.
fn hides_sql(hir: &Hir) -> bool {
    hir.queries
        .iter()
        .any(|(_, q)| matches!(q.body, Body::Hole(_)))
        || hir.from.iter().any(|(_, f)| {
            matches!(
                f.kind,
                FromKind::Hole
                    | FromKind::Base {
                        target: RelTarget::Jinja(_),
                        ..
                    }
            )
        })
        || hir
            .exprs
            .iter()
            .any(|(_, e)| matches!(e.kind, ExprKind::Opaque { .. } | ExprKind::Hole))
}
