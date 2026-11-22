//! Tests for `plan::diagnostics`, split out to keep the module small.

use cogitator_ir::lir::Backend;
use cogitator_ir::origin::TemplateVariant;
use cogitator_ir::Level;

use super::super::diagnostics_config::{PerfProfile, RuleOverride};
use super::super::report::{
    FixEditView, LevelReport, OriginView, Placement, PlanLevels, PlanStatus,
};
use super::super::suppress::scan;
use super::super::trace::{NeedsView, TraceView};
use super::*;
use cogitator_dialects::Dialect;
use cogitator_rules::meta::Needs;

fn finding(id: &'static str, span: Span, severity: &'static str) -> FindingView {
    FindingView {
        id,
        span,
        level: Level::Mir,
        severity,
        message: "Join has no key".to_string(),
        evidence: EvidenceView::Exact,
        fixes: Vec::new(),
        suppressed: false,
        diagnostics_eligible: true,
        placement: Placement::Template,
    }
}

fn plan(findings: Vec<FindingView>, origin: Option<OriginView>) -> QueryPlan {
    QueryPlan {
        model: Some("m".to_string()),
        dialect: "duckdb".to_string(),
        backend: Backend::DuckDb,
        schema_revision: 1,
        status: PlanStatus::Complete,
        guidance: None,
        banner: None,
        origin,
        levels: PlanLevels {
            hir: LevelReport::skipped("test"),
            mir: LevelReport::skipped("test"),
            lir: LevelReport::skipped("test"),
        },
        findings,
        trace: TraceView::unavailable(Level::Hir),
        needs: NeedsView::of(Needs::NONE),
    }
}

fn template() -> Option<OriginView> {
    Some(OriginView::Template {
        variant: TemplateVariant::Plain,
    })
}

fn settings(profile: PerfProfile) -> PerfSettings {
    PerfSettings {
        profile,
        ..PerfSettings::default()
    }
}

fn run(text: &str, findings: Vec<FindingView>, cfg: &PerfSettings) -> Vec<Diagnostic> {
    let plan = plan(findings, template());
    plan_diagnostics(text, &plan, &scan(text, &Dialect::new("duckdb")), cfg)
}

#[test]
fn profile_off_produces_nothing() {
    let found = vec![finding("join.cartesian", Span::new(0, 6), "warning")];
    assert!(run("select 1\n", found, &PerfSettings::default()).is_empty());
}

#[test]
fn recommended_produces_a_diagnostic() {
    let found = vec![finding("join.cartesian", Span::new(0, 6), "warning")];
    let diags = run("select 1\n", found, &settings(PerfProfile::Recommended));
    assert_eq!(diags.len(), 1);
    let d = &diags[0];
    assert_eq!(d.code, "join.cartesian");
    assert_eq!(d.source, "cogitator");
    assert_eq!(d.severity, diagnostic_severity::WARNING);
    assert_eq!(d.message, "Join has no key");
    assert_eq!(d.range.end.character, 6);
}

#[test]
fn evidence_tags_the_message_unless_exact() {
    let mut heuristic = finding("join.cartesian", Span::new(0, 6), "warning");
    heuristic.evidence = EvidenceView::Heuristic;
    let mut catalog = finding("join.left_nullified", Span::new(0, 6), "warning");
    catalog.evidence = EvidenceView::Catalog {
        generated_at: "2024".to_string(),
    };
    let diags = run(
        "select 1\n",
        vec![heuristic, catalog],
        &settings(PerfProfile::Strict),
    );
    assert_eq!(diags[0].message, "Join has no key [heuristic]");
    assert_eq!(diags[1].message, "Join has no key [catalog 2024]");
}

#[test]
fn dialect_severity_stays_and_overrides_win() {
    // filter.non_sargable reports Hint off BigQuery
    let found = vec![finding("filter.non_sargable", Span::new(0, 6), "hint")];
    let on = run(
        "select 1\n",
        found.clone(),
        &settings(PerfProfile::Recommended),
    );
    assert_eq!(on[0].severity, diagnostic_severity::HINT);
    let mut cfg = settings(PerfProfile::Recommended);
    cfg.rules.insert(
        "filter.non_sargable".to_string(),
        RuleOverride::Set(Severity::Warning),
    );
    assert_eq!(run("select 1\n", found.clone(), &cfg)[0].severity, 2);
    cfg.rules
        .insert("filter.non_sargable".to_string(), RuleOverride::Off);
    assert!(run("select 1\n", found, &cfg).is_empty());
}

#[test]
fn stats_evidence_cannot_raise_severity() {
    // a catalog-backed finding at the rule's ceiling stays there; a
    // stale value claiming more is capped, never reported above it
    let mut boosted = finding("join.distinct_fanout", Span::new(0, 6), "warning");
    boosted.evidence = EvidenceView::Catalog {
        generated_at: "2024".to_string(),
    };
    let diags = run("select 1\n", vec![boosted], &settings(PerfProfile::Strict));
    assert_eq!(diags[0].severity, diagnostic_severity::INFO);
}

#[test]
fn ineligible_findings_produce_nothing() {
    let mut ineligible = finding("join.cartesian", Span::new(0, 6), "warning");
    ineligible.diagnostics_eligible = false;
    let plan = plan(vec![ineligible], template());
    assert!(plan_diagnostics(
        "select 1\n",
        &plan,
        &Suppressions::default(),
        &settings(PerfProfile::Strict),
    )
    .is_empty());
}

#[test]
fn multibyte_text_converts_to_utf16_columns() {
    // é is two bytes, one UTF-16 unit; 𝄞 is four bytes, two units
    let text = "select 'é𝄞', x\n";
    let start = text.find('x').unwrap() as u32;
    let found = vec![finding(
        "join.cartesian",
        Span::new(start, start + 1),
        "warning",
    )];
    let diags = run(text, found, &settings(PerfProfile::Recommended));
    assert_eq!(diags[0].range.start.character, 14, "utf-16 column of x");
    assert_eq!(diags[0].range.end.character, 15);
}

#[test]
fn spans_outside_the_text_are_dropped() {
    let found = vec![finding("join.cartesian", Span::new(0, 400), "warning")];
    assert!(run("select 1\n", found, &settings(PerfProfile::Strict)).is_empty());
}

#[test]
fn fixes_become_multi_edit_preserving_first() {
    let mut f = finding("set.union_distinct", Span::new(0, 6), "info");
    f.fixes = vec![
        FixView {
            title: "Move it".to_string(),
            safety: "changesIntent",
            edits: vec![FixEditView {
                span: Span::new(0, 6),
                new_text: "x".to_string(),
            }],
        },
        FixView {
            title: "Change it".to_string(),
            safety: "preserving",
            edits: vec![
                FixEditView {
                    span: Span::new(0, 0),
                    new_text: "a".to_string(),
                },
                FixEditView {
                    span: Span::new(1, 2),
                    new_text: String::new(),
                },
            ],
        },
    ];
    let diags = run("select 1\n", vec![f], &settings(PerfProfile::Strict));
    let data = diags[0].data.clone().expect("fixes");
    let FixKind::MultiEdit { fixes } = data.fix else {
        panic!("multi edit");
    };
    assert_eq!(fixes[0].title, "Change it");
    assert!(fixes[0].preserving);
    assert_eq!(fixes[0].edits.len(), 2, "edits apply together");
    assert_eq!(fixes[0].edits[0].range.start, fixes[0].edits[0].range.end);
    assert_eq!(fixes[1].title, "Move it (changes results)");
    assert!(!fixes[1].preserving);
}

#[test]
fn a_fix_with_a_bad_edit_is_dropped_alone() {
    let mut f = finding("join.cartesian", Span::new(0, 6), "warning");
    f.fixes = vec![FixView {
        title: "t".to_string(),
        safety: "preserving",
        edits: vec![FixEditView {
            span: Span::new(0, 400),
            new_text: String::new(),
        }],
    }];
    let diags = run("select 1\n", vec![f], &settings(PerfProfile::Strict));
    assert_eq!(diags.len(), 1);
    assert!(diags[0].data.is_none());
}

#[test]
fn suppressed_findings_are_silent_and_unknown_ids_warn() {
    let text = "select 1 -- cogitator: ignore join.cartesian\n\
                select 2 -- cogitator: ignore join.cartesan\n";
    let mut suppressed = finding("join.cartesian", Span::new(0, 6), "warning");
    suppressed.suppressed = true;
    let diags = run(text, vec![suppressed], &settings(PerfProfile::Strict));
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, "cogitator-suppress");
    assert!(
        diags[0].message.contains("join.cartesian"),
        "{}",
        diags[0].message
    );
    assert_eq!(diags[0].severity, diagnostic_severity::WARNING);
}

#[test]
fn suppression_warnings_wait_for_opt_in() {
    let text = "select 1 -- cogitator: ignore join.cartesan\n";
    assert!(run(text, Vec::new(), &PerfSettings::default()).is_empty());
}
