//! The compiled fallback through the driver (specs/ir-pipeline/01, 09).

use cogitator_ir::origin::{SourceOrigin, SpanMap, Staleness};
use cogitator_ir::Span;

use super::compiled_findings::place_findings;
use super::tests::Facts;
use super::*;
use crate::features::plan::report::{EvidenceView, FixEditView, FixView};

/// `{% for %}` is the control flow; `orders` is literal SQL around it.
const TEMPLATE: &str = "with orders as (\n    select * from {{ ref('stg_orders') }}\n)\n\nselect\n    id,\n    {% for c in cols %}\n    {{ c }},\n    {% endfor %}\n    amount\nfrom orders\nwhere amount > 0\n";

const COMPILED: &str = "with orders as (\n    select * from stg_orders\n)\n\nselect\n    id,\n    \n    a,\n    \n    b,\n    \n    amount\nfrom orders\nwhere amount > 0\n";

fn facts() -> Facts {
    Facts::new("duckdb").yaml("stg_orders", &[("id", None), ("amount", None)])
}

fn available(text: &str, mtime: Staleness) -> CompiledInput<'_> {
    CompiledInput::Available {
        text,
        path: "target/compiled/p/models/m.sql",
        mtime,
    }
}

fn plan_with(facts: &Facts, text: &str, compiled: CompiledInput<'_>) -> QueryPlan {
    query_plan(&facts.input(text, Some("m")).with_compiled(compiled))
}

fn staleness_of(plan: &QueryPlan) -> Staleness {
    match plan.origin {
        Some(OriginView::Compiled { staleness, .. }) => staleness,
        ref other => panic!("not a compiled plan: {other:?}"),
    }
}

#[test]
fn control_flow_falls_back_to_compiled_sql() {
    let facts = facts();
    let plan = plan_with(&facts, TEMPLATE, available(COMPILED, Staleness::Fresh));
    assert_ne!(plan.status, PlanStatus::Declined, "{plan:?}");
    assert_eq!(staleness_of(&plan), Staleness::Fresh);
    assert_eq!(plan.banner, None);
    assert!(plan.levels.mir.available, "{:?}", plan.levels.mir);
    let hir = plan.levels.hir.text.as_deref().expect("hir");
    assert!(hir.contains("stg_orders"), "the ref resolved: {hir}");
}

#[test]
fn unused_cte_is_skipped_on_a_compiled_plan_of_control_flow() {
    let facts = facts();
    let plan = plan_with(&facts, TEMPLATE, available(COMPILED, Staleness::Fresh));
    let row = plan
        .trace
        .entries
        .iter()
        .find(|row| row.rule == "model.unused_cte")
        .expect("unused_cte row");
    assert_eq!(row.outcome, "skipped", "{row:?}");
    assert!(
        plan.needs.missing.contains(&"allBranches"),
        "{:?}",
        plan.needs
    );
}

#[test]
fn no_compiled_file_declines_with_compile_guidance() {
    let facts = facts();
    let plan = plan_with(
        &facts,
        TEMPLATE,
        CompiledInput::Missing {
            path: "target/compiled/p/models/m.sql",
        },
    );
    assert_eq!(plan.status, PlanStatus::Declined);
    let guidance = plan.guidance.expect("guidance");
    assert!(guidance.contains("no compiled file"), "{guidance}");
    assert!(guidance.contains("dbt compile"), "{guidance}");
    assert!(guidance.contains("target/compiled"), "{guidance}");
}

#[test]
fn an_older_compiled_file_plans_with_a_banner_and_no_eligible_findings() {
    let facts = facts();
    let plan = plan_with(&facts, TEMPLATE, available(COMPILED, Staleness::Stale));
    assert_ne!(plan.status, PlanStatus::Declined);
    assert_eq!(staleness_of(&plan), Staleness::Stale);
    let banner = plan.banner.as_deref().expect("banner");
    assert!(banner.contains("older"), "{banner}");
    assert!(banner.contains("dbt compile"), "{banner}");
    assert!(plan
        .findings
        .iter()
        .all(|f| !f.diagnostics_eligible && f.fixes.is_empty()));
}

#[test]
fn an_unsaved_edit_goes_stale_through_coverage_even_with_a_fresh_mtime() {
    let facts = facts();
    let edited = TEMPLATE.replace("amount > 0", "amount > 100 and status = 'done'");
    let plan = plan_with(&facts, &edited, available(COMPILED, Staleness::Fresh));
    assert_eq!(staleness_of(&plan), Staleness::Stale);
    let banner = plan.banner.as_deref().expect("banner");
    assert!(banner.contains("changed"), "{banner}");
}

#[test]
fn unknown_mtimes_are_not_fresh() {
    let facts = facts();
    let plan = plan_with(&facts, TEMPLATE, available(COMPILED, Staleness::Unknown));
    assert_eq!(staleness_of(&plan), Staleness::Unknown);
    assert!(plan.banner.is_some());
    assert!(plan.findings.iter().all(|f| !f.diagnostics_eligible));
}

#[test]
fn an_oversized_file_says_so_and_points_at_the_cap() {
    let plan = plan_with(
        &facts(),
        TEMPLATE,
        CompiledInput::TooLarge {
            path: "target/compiled/p/models/m.sql",
            bytes: 9,
            cap: 3,
        },
    );
    assert_eq!(plan.status, PlanStatus::Declined);
    let guidance = plan.guidance.expect("guidance");
    assert!(guidance.contains("compiled_sql_mib"), "{guidance}");
}

#[test]
fn unparsable_compiled_sql_declines() {
    let plan = plan_with(
        &facts(),
        TEMPLATE,
        available("definitely not sql (", Staleness::Fresh),
    );
    assert_eq!(plan.status, PlanStatus::Declined, "{plan:?}");
    assert!(plan.guidance.expect("guidance").contains("dbt compile"));
}

#[test]
fn a_template_that_does_not_need_it_ignores_compiled_sql() {
    let facts = facts();
    let plan = plan_with(
        &facts,
        "select id from {{ ref('stg_orders') }}",
        available("select junk from junk", Staleness::Fresh),
    );
    assert!(matches!(plan.origin, Some(OriginView::Template { .. })));
    assert_eq!(plan.banner, None);
}

const JOIN_TEMPLATE: &str = "select\n    a.id,\n    {% for c in cols %}\n    {{ c }},\n    {% endfor %}\n    b.amount\nfrom {{ ref('stg_orders') }} a\njoin {{ ref('stg_items') }} b on b.flag = 1\n";

const JOIN_COMPILED: &str = "select\n    a.id,\n    \n    x,\n    \n    b.amount\nfrom stg_orders a\njoin stg_items b on b.flag = 1\n";

fn join_facts() -> Facts {
    Facts::new("duckdb")
        .yaml("stg_orders", &[("id", None)])
        .yaml("stg_items", &[("amount", None), ("flag", None)])
}

fn cartesian(plan: &QueryPlan) -> &FindingView {
    plan.findings
        .iter()
        .find(|f| f.id == "join.cartesian")
        .unwrap_or_else(|| panic!("no join.cartesian: {:?}", plan.findings))
}

#[test]
fn a_real_finding_on_a_fresh_compiled_plan_lands_on_the_template() {
    let facts = join_facts();
    let plan = plan_with(
        &facts,
        JOIN_TEMPLATE,
        available(JOIN_COMPILED, Staleness::Fresh),
    );
    let finding = cartesian(&plan);
    assert!(finding.diagnostics_eligible, "{finding:?}");
    assert_ne!(finding.placement, Placement::Unmapped, "{finding:?}");
    assert!(
        (finding.span.end as usize) <= JOIN_TEMPLATE.len(),
        "span indexes the template: {finding:?}"
    );
}

#[test]
fn a_real_finding_on_a_stale_compiled_plan_is_not_eligible() {
    let facts = join_facts();
    let plan = plan_with(
        &facts,
        JOIN_TEMPLATE,
        available(JOIN_COMPILED, Staleness::Stale),
    );
    assert!(plan.banner.is_some());
    assert!(!cartesian(&plan).diagnostics_eligible);
}

fn finding(id: &'static str, span: Span, edits: Vec<Span>) -> FindingView {
    FindingView {
        id,
        span,
        placement: Placement::Template,
        diagnostics_eligible: true,
        level: cogitator_ir::Level::Mir,
        severity: "hint",
        message: String::new(),
        evidence: EvidenceView::Exact,
        fixes: vec![FixView {
            title: "fix".to_string(),
            safety: "preserving",
            edits: edits
                .into_iter()
                .map(|span| FixEditView {
                    span,
                    new_text: String::new(),
                })
                .collect(),
        }],
        suppressed: false,
    }
}

fn origin(staleness: Staleness) -> SourceOrigin {
    SourceOrigin::Compiled {
        staleness,
        map: SpanMap {
            anchors: vec![
                (Span::new(0, 10), Span::new(100, 110)),
                (Span::new(20, 30), Span::new(200, 210)),
            ],
            jinja_gaps: vec![(Span::new(10, 20), Span::new(150, 160))],
            coverage: 9000,
        },
    }
}

#[test]
fn exact_findings_move_to_the_template_and_keep_their_fixes() {
    let mut findings = vec![finding("r", Span::new(2, 4), vec![Span::new(2, 4)])];
    place_findings(&mut findings, &origin(Staleness::Fresh));
    let found = &findings[0];
    assert_eq!(found.span, Span::new(102, 104));
    assert_eq!(found.placement, Placement::Exact);
    assert!(found.diagnostics_eligible);
    assert_eq!(found.fixes[0].edits[0].span, Span::new(102, 104));
}

#[test]
fn a_fix_whose_edit_is_not_exact_is_withheld_whole() {
    let mut findings = vec![finding(
        "r",
        Span::new(2, 4),
        vec![Span::new(2, 4), Span::new(12, 14)],
    )];
    place_findings(&mut findings, &origin(Staleness::Fresh));
    assert!(findings[0].fixes.is_empty());
    assert!(findings[0].diagnostics_eligible, "the finding itself stays");
}

#[test]
fn jinja_findings_highlight_without_a_fix() {
    let mut findings = vec![finding("r", Span::new(12, 14), vec![Span::new(12, 14)])];
    place_findings(&mut findings, &origin(Staleness::Fresh));
    assert_eq!(findings[0].placement, Placement::InJinja);
    assert_eq!(findings[0].span, Span::new(150, 160));
    assert!(findings[0].fixes.is_empty());
    assert!(findings[0].diagnostics_eligible);
}

#[test]
fn unmapped_findings_stay_out_of_diagnostics_and_sort_last() {
    let mut findings = vec![
        finding("a", Span::new(25, 40), vec![]),
        finding("b", Span::new(2, 4), vec![]),
    ];
    place_findings(&mut findings, &origin(Staleness::Fresh));
    assert_eq!(findings[0].id, "b");
    assert_eq!(findings[1].placement, Placement::Unmapped);
    assert_eq!(
        findings[1].span,
        Span::new(25, 40),
        "keeps compiled offsets"
    );
    assert!(!findings[1].diagnostics_eligible);
}

#[test]
fn a_span_crossing_jinja_highlights_the_covering_range_without_a_fix() {
    let mut findings = vec![finding("r", Span::new(5, 25), vec![Span::new(5, 25)])];
    place_findings(&mut findings, &origin(Staleness::Fresh));
    assert_eq!(findings[0].placement, Placement::InJinja);
    assert_eq!(findings[0].span, Span::new(105, 205));
    assert!(findings[0].fixes.is_empty());
    assert!(findings[0].diagnostics_eligible);
}

#[test]
fn a_stale_plan_offers_no_diagnostics_and_no_fixes() {
    let mut findings = vec![finding("r", Span::new(2, 4), vec![Span::new(2, 4)])];
    place_findings(&mut findings, &origin(Staleness::Stale));
    assert!(!findings[0].diagnostics_eligible);
    assert!(findings[0].fixes.is_empty());
    assert_eq!(findings[0].span, Span::new(102, 104), "still locatable");
}

#[test]
fn template_origin_findings_are_untouched() {
    let mut findings = vec![finding("r", Span::new(2, 4), vec![Span::new(2, 4)])];
    let before = findings.clone();
    place_findings(
        &mut findings,
        &SourceOrigin::Template {
            variant: cogitator_ir::origin::TemplateVariant::Plain,
        },
    );
    assert_eq!(findings, before);
}
