//! The `dbt.queryPlan` payload (specs/ir-pipeline/09). The pass manager's
//! trace and findings are not `Serialize`, so these views own the wire
//! vocabulary.

use cogitator_ir::lir::Backend;
use cogitator_ir::origin::{SourceOrigin, Staleness, TemplateVariant};
use cogitator_ir::{Degradation, Fidelity, Level, Lowered, Span, Violation};
use cogitator_rules::findings::{Evidence, Finding, Fix};
use cogitator_rules::meta::{FixSafety, Needs, Severity};
use serde::Serialize;

use super::trace::{NeedsView, TraceView};
use super::PlanInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanStatus {
    Complete,
    Partial,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OriginView {
    Template {
        variant: TemplateVariant,
    },
    #[serde(rename_all = "camelCase")]
    Compiled {
        staleness: Staleness,
        /// Basis points of the compiled text anchored to the template.
        coverage: u16,
    },
}

impl OriginView {
    pub(super) fn of(origin: &SourceOrigin) -> Self {
        match origin {
            SourceOrigin::Template { variant } => OriginView::Template { variant: *variant },
            SourceOrigin::Compiled { staleness, map } => OriginView::Compiled {
                staleness: *staleness,
                coverage: map.coverage,
            },
        }
    }
}

/// One IR level: its printed form or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelReport {
    pub available: bool,
    /// `None` for a level the pipeline never reached.
    pub fidelity: Option<Fidelity>,
    pub text: Option<String>,
    /// Why the level is unavailable; names its violations or degradation.
    pub unavailable: Option<String>,
    pub degradations: Vec<Degradation>,
    pub violations: Vec<Violation>,
}

impl LevelReport {
    /// A level the pipeline never reached because `above` was dropped.
    pub(super) fn skipped(above: &str) -> Self {
        LevelReport {
            available: false,
            fidelity: None,
            text: None,
            unavailable: Some(format!("not lowered: {above}")),
            degradations: Vec::new(),
            violations: Vec::new(),
        }
    }

    /// `text` is the level's printer output when it survived verification.
    pub(super) fn of<T>(lowered: &Lowered<T>, text: Option<String>, name: &str) -> Self {
        let available = lowered.value.is_some();
        LevelReport {
            available,
            fidelity: Some(lowered.fidelity),
            text: if available { text } else { None },
            unavailable: (!available).then(|| unavailable_note(lowered, name)),
            degradations: lowered.degraded.clone(),
            violations: lowered.violations.clone(),
        }
    }
}

fn unavailable_note<T>(lowered: &Lowered<T>, name: &str) -> String {
    if lowered.violations.is_empty() {
        let detail = lowered
            .degraded
            .first()
            .map_or("no plan was produced", |d| d.detail.as_str());
        format!("{name} declined: {detail}")
    } else {
        let invariants: Vec<String> = lowered
            .violations
            .iter()
            .map(|v| format!("{:?}", v.invariant))
            .collect();
        format!("{name} dropped by the verifier: {}", invariants.join(", "))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanLevels {
    pub hir: LevelReport,
    pub mir: LevelReport,
    pub lir: LevelReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EvidenceView {
    Exact,
    #[serde(rename_all = "camelCase")]
    Catalog {
        generated_at: String,
    },
    #[serde(rename_all = "camelCase")]
    Tested {
        schema_revision: u64,
    },
    Heuristic,
    Unknown,
}

impl EvidenceView {
    fn of(evidence: &Evidence) -> Self {
        match evidence {
            Evidence::Exact => EvidenceView::Exact,
            Evidence::Catalog { generated_at } => EvidenceView::Catalog {
                generated_at: generated_at.to_string(),
            },
            Evidence::Tested { schema_revision } => EvidenceView::Tested {
                schema_revision: *schema_revision,
            },
            Evidence::Heuristic => EvidenceView::Heuristic,
            Evidence::Unknown => EvidenceView::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixEditView {
    pub span: Span,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FixView {
    pub title: String,
    pub safety: &'static str,
    pub edits: Vec<FixEditView>,
}

/// Where a finding's `span` points (specs/ir-pipeline/09).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Placement {
    /// The plan was built from the template; `span` indexes it directly.
    Template,
    /// Compiled SQL anchored to the template; `span` indexes the template.
    Exact,
    /// Compiled SQL produced by Jinja; `span` is that Jinja region.
    InJinja,
    /// No place in the template; `span` keeps its compiled offsets.
    Unmapped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingView {
    pub id: &'static str,
    pub span: Span,
    pub placement: Placement,
    /// False when the finding must not reach the editor as a diagnostic:
    /// a stale compiled plan, or a span with no place in the template.
    /// Consumers filter on this and need not know the plan's origin.
    pub diagnostics_eligible: bool,
    pub level: Level,
    pub severity: &'static str,
    pub message: String,
    pub evidence: EvidenceView,
    pub fixes: Vec<FixView>,
    /// A suppression comment covers this finding: it stays here but is
    /// not published as a diagnostic (specs/ir-pipeline/08).
    pub suppressed: bool,
}

fn severity(value: Severity) -> &'static str {
    match value {
        Severity::Off => "off",
        Severity::Hint => "hint",
        Severity::Info => "info",
        Severity::Warning => "warning",
    }
}

fn safety(value: FixSafety) -> &'static str {
    match value {
        FixSafety::None => "none",
        FixSafety::Preserving => "preserving",
        FixSafety::ChangesIntent => "changesIntent",
        FixSafety::Mixed => "mixed",
    }
}

fn fix_view(fix: &Fix) -> FixView {
    FixView {
        title: fix.title.clone(),
        safety: safety(fix.safety),
        edits: fix
            .edits
            .iter()
            .map(|edit| FixEditView {
                span: edit.span,
                new_text: edit.new_text.clone(),
            })
            .collect(),
    }
}

impl FindingView {
    pub(super) fn of(finding: &Finding) -> Self {
        FindingView {
            id: finding.id.as_str(),
            span: finding.span,
            placement: Placement::Template,
            diagnostics_eligible: true,
            level: finding.level,
            severity: severity(finding.severity),
            message: finding.message.clone(),
            evidence: EvidenceView::of(&finding.evidence),
            fixes: finding.fixes.iter().map(fix_view).collect(),
            suppressed: false,
        }
    }
}

/// The `dbt.queryPlan` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryPlan {
    pub model: Option<String>,
    pub dialect: String,
    pub backend: Backend,
    pub schema_revision: u64,
    pub status: PlanStatus,
    /// Set when `status` is `declined`: what to do about it.
    pub guidance: Option<String>,
    /// Set on a compiled plan of doubtful freshness: show this first.
    pub banner: Option<String>,
    pub origin: Option<OriginView>,
    pub levels: PlanLevels,
    pub findings: Vec<FindingView>,
    pub trace: TraceView,
    pub needs: NeedsView,
}

impl QueryPlan {
    pub(super) fn no_backend(input: &PlanInput<'_>, backend: Backend) -> Self {
        let skipped = LevelReport::skipped("no backend for this dialect");
        QueryPlan {
            model: input.model.map(str::to_string),
            dialect: input.dialect.as_str().to_string(),
            backend,
            schema_revision: input.schema_revision,
            status: PlanStatus::Declined,
            guidance: None,
            banner: None,
            origin: None,
            levels: PlanLevels {
                hir: skipped.clone(),
                mir: skipped.clone(),
                lir: skipped,
            },
            findings: Vec::new(),
            trace: TraceView::unavailable(Level::Hir),
            needs: NeedsView::of(Needs::NONE),
        }
    }
}
