//! `incr.missing_guard`: an incremental model with no `is_incremental()`.
//!
//! Such a model reads the same input on every run as on a full refresh.
//! The claim needs the whole template to be visible, so the rule stays
//! quiet on compiled SQL (the Jinja is gone), on the `FullRefresh` variant
//! (the block is blanked), on parse holes, and when a macro or computed
//! expression could render the guard. Other control flow never reaches
//! this rule: the front end declines the template, and the pass manager
//! records the missing HIR as `Skipped`.

use cogitator_ir::common::Materialization;
use cogitator_ir::hir::{Body, Hir, JinjaKind, JinjaNode, JinjaRole};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::{Level, RuleId, Span};

use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("incr.missing_guard"),
    level: RuleLevel::Hir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Mat),
    severity: Severity::Warning,
    fix: FixSafety::None,
};

pub struct MissingGuard;

impl Rule for MissingGuard {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Hir> for MissingGuard {
    fn check(&self, ir: &Hir, cx: &Cx<'_>, out: &mut Findings) {
        if cx.mat != Some(Materialization::Incremental) || !template_fully_visible(ir) {
            return;
        }
        if ir.jinja.values().any(may_supply_guard) {
            return;
        }
        out.push(Finding {
            id: META.id,
            span: anchor(ir),
            level: Level::Hir,
            severity: Severity::Warning,
            message: "This incremental model has no `is_incremental()` block, so every run \
                      reads the same input as a full refresh. Restrict new rows inside \
                      `{% if is_incremental() %}`, or use another materialization."
                .to_string(),
            evidence: Evidence::Exact,
            fixes: Vec::new(),
        });
    }
}

fn template_fully_visible(hir: &Hir) -> bool {
    let visible_variant = matches!(
        hir.origin,
        SourceOrigin::Template {
            variant: TemplateVariant::Plain | TemplateVariant::Incremental
        }
    );
    let has_hole = hir
        .queries
        .values()
        .any(|q| matches!(q.body, Body::Hole(_)));
    visible_variant && !has_hole
}

/// The guard itself, or Jinja whose output is unknown. A `var` in scalar
/// position yields a value, not SQL. Macros stay on the unknown side
/// wherever they sit: one in a WHERE can render the whole guard.
fn may_supply_guard(node: &JinjaNode) -> bool {
    match node.kind {
        JinjaKind::IsIncremental | JinjaKind::Macro(_) | JinjaKind::Expression => true,
        JinjaKind::Var => node.role != JinjaRole::Scalar,
        JinjaKind::Ref
        | JinjaKind::Source
        | JinjaKind::This
        | JinjaKind::Config
        | JinjaKind::Statement
        | JinjaKind::Comment => false,
    }
}

/// The `config()` call that made the model incremental, or the query.
pub(super) fn anchor(hir: &Hir) -> Span {
    hir.jinja
        .values()
        .find(|node| node.kind == JinjaKind::Config)
        .map(|node| node.span)
        .or_else(|| {
            hir.root
                .and_then(|root| hir.queries.get(root))
                .map(|q| q.span)
        })
        .unwrap_or_default()
}
