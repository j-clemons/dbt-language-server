use std::collections::HashMap;

use cogitator_dialects::Dialect;
use cogitator_ir::lir::{
    Backend, Capabilities, JoinHint, Lir, PhysColumn, PhysicalFacts, Pred, Pruning,
};
use cogitator_ir::mir::Mir;
use cogitator_ir::{Fidelity, Invariant, Lowered, Violation};
use cogitator_lower::{lower_plan, lower_to_mir, DialectBackend};
use cogitator_schemas::ColumnEntry;

use super::*;
use crate::artifacts::catalog::CatalogIndex;
use crate::artifacts::manifest::{ManifestConfig, ManifestIndex, ManifestNode};

/// Owns what a `PlanInput` borrows.
pub(super) struct Facts {
    dialect: Dialect,
    models: HashMap<String, ModelDetails>,
    sources: HashMap<String, SourceDetails>,
    manifest: Option<ManifestIndex>,
    pub(super) catalog: Option<CatalogIndex>,
}

impl Facts {
    pub(super) fn new(dialect: &str) -> Self {
        Facts {
            dialect: Dialect::new(dialect),
            models: HashMap::new(),
            sources: HashMap::new(),
            manifest: None,
            catalog: None,
        }
    }

    pub(super) fn yaml(mut self, model: &str, columns: &[(&str, Option<&str>)]) -> Self {
        self.models.insert(
            model.to_string(),
            ModelDetails {
                columns: columns.iter().map(|(name, ty)| column(name, *ty)).collect(),
                ..ModelDetails::default()
            },
        );
        self
    }

    pub(super) fn manifest(mut self, model: &str, config: ManifestConfig) -> Self {
        self.manifest
            .get_or_insert_with(ManifestIndex::default)
            .models
            .insert(
                model.to_string(),
                ManifestNode {
                    unique_id: format!("model.p.{model}"),
                    original_file_path: None,
                    relation_name: None,
                    config,
                },
            );
        self
    }

    pub(super) fn input<'a>(&'a self, text: &'a str, model: Option<&'a str>) -> PlanInput<'a> {
        PlanInput {
            text,
            dialect: &self.dialect,
            model,
            models: &self.models,
            sources: &self.sources,
            manifest: self.manifest.as_ref(),
            catalog: self.catalog.as_ref(),
            schema_revision: 7,
            catalog_stats: true,
            compiled: CompiledInput::NotChecked,
            project: None,
        }
    }

    pub(super) fn plan(&self, text: &str) -> QueryPlan {
        query_plan(&self.input(text, Some("m")))
    }
}

fn column(name: &str, data_type: Option<&str>) -> ColumnEntry {
    ColumnEntry {
        name: name.to_string(),
        description: String::new(),
        name_pos: Default::default(),
        data_type: data_type.map(str::to_string),
        tests: Vec::new(),
    }
}

fn rows_for<'a>(plan: &'a QueryPlan, rule: &str) -> Vec<&'a TraceRow> {
    plan.trace
        .entries
        .iter()
        .filter(|r| r.rule == rule)
        .collect()
}

#[test]
fn a_plain_model_renders_every_level_and_the_trace() {
    let plan = Facts::new("duckdb").plan(
        "with a as (select id, amount from {{ ref('up') }}) \
         select id from a where amount > 1",
    );
    assert_eq!(plan.status, PlanStatus::Complete, "{plan:#?}");
    assert!(plan.guidance.is_none());
    assert_eq!(plan.backend, Backend::DuckDb);
    assert_eq!(plan.schema_revision, 7);
    assert_eq!(
        plan.origin,
        Some(OriginView::Template {
            variant: cogitator_ir::origin::TemplateVariant::Plain
        })
    );
    for level in [&plan.levels.hir, &plan.levels.mir, &plan.levels.lir] {
        assert!(level.available, "{level:#?}");
        assert_eq!(level.fidelity, Some(Fidelity::Complete));
        assert!(level.text.as_deref().is_some_and(|t| !t.is_empty()));
    }
    assert!(!plan.trace.entries.is_empty());
    assert!(!plan.trace.incomplete, "{:#?}", plan.trace);
    // canonicalisers ran and were recorded
    assert!(!rows_for(&plan, "canon.simplify").is_empty());
}

#[test]
fn unmet_needs_are_skipped_with_a_reason() {
    let plan = Facts::new("duckdb").plan("select id from {{ ref('up') }} where date(ts) = d");
    assert!(plan.needs.missing.contains(&"phys"));
    assert!(plan.needs.missing.contains(&"schema"));
    assert!(plan.needs.available.contains(&"dialect"));
    assert!(plan.needs.available.contains(&"allBranches"));
    let rows = rows_for(&plan, "filter.non_sargable");
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0].outcome, "skipped");
    assert_eq!(rows[0].reason.as_deref(), Some("unmet:types"));
}

#[test]
fn a_model_with_a_hole_still_produces_levels() {
    let plan = Facts::new("duckdb").plan("with a as (select from from where) select id from a");
    assert_eq!(plan.status, PlanStatus::Partial, "{plan:#?}");
    assert!(plan.levels.hir.available);
    assert!(
        !plan.levels.hir.degradations.is_empty(),
        "{:#?}",
        plan.levels.hir
    );
    assert!(plan.levels.mir.available, "{:#?}", plan.levels.mir);
    assert!(plan.levels.lir.available, "{:#?}", plan.levels.lir);
}

#[test]
fn control_flow_declines_with_guidance_and_skips_every_rule() {
    let plan = Facts::new("duckdb")
        .plan("select {% for c in cols %}{{ c }},{% endfor %} 1 from {{ ref('up') }}");
    assert_eq!(plan.status, PlanStatus::Declined);
    let guidance = plan.guidance.as_deref().expect("guidance");
    assert!(guidance.contains("dbt compile"), "{guidance}");
    assert!(!plan.levels.hir.available);
    assert!(plan
        .levels
        .hir
        .unavailable
        .as_deref()
        .is_some_and(|n| n.contains("declined")));
    assert!(!plan.levels.mir.available && !plan.levels.lir.available);
    assert!(plan.trace.incomplete);
    assert!(plan
        .trace
        .entries
        .iter()
        .all(|r| r.outcome == "skipped" && r.reason.as_deref() == Some("unavailable:hir")));
    assert!(!plan.trace.entries.is_empty());
}

#[test]
fn a_lone_is_incremental_block_plans_the_incremental_variant() {
    let plan = Facts::new("duckdb").plan(
        "select id from {{ ref('up') }}\n\
         {% if is_incremental() %}where id > (select max(id) from {{ this }}){% endif %}",
    );
    assert_ne!(plan.status, PlanStatus::Declined, "{plan:#?}");
    assert_eq!(
        plan.origin,
        Some(OriginView::Template {
            variant: cogitator_ir::origin::TemplateVariant::Incremental
        })
    );
    assert!(plan.levels.lir.available);
}

#[test]
fn a_forced_mir_violation_drops_mir_and_lir_but_not_hir() {
    let facts = Facts::new("duckdb");
    let input = facts.input("select 1 as x", Some("m"));
    let forced = |_: &cogitator_ir::hir::Hir| -> Lowered<Mir> {
        Lowered::declined(Vec::new())
            .with_violations(vec![Violation::new(Invariant::MirIdRange, "forced")])
    };
    let backend = cogitator_lower::backends::for_backend(Backend::DuckDb).expect("backend");
    let plan = drive::drive(&input, backend, &forced);
    assert_eq!(plan.status, PlanStatus::Partial);
    assert!(plan.levels.hir.available);
    assert!(!plan.levels.mir.available);
    let note = plan.levels.mir.unavailable.as_deref().expect("note");
    assert!(note.contains("MirIdRange"), "{note}");
    assert_eq!(plan.levels.mir.violations.len(), 1);
    assert!(!plan.levels.lir.available);
    assert!(plan
        .trace
        .entries
        .iter()
        .all(|r| r.reason.as_deref() == Some("unavailable:mir")));
}

/// A backend whose capability table permits no CTE policy at all, so the
/// `Auto` every CTE lowers to must fail `verify_lir`.
struct NoCtePolicies;

static NO_CTE_CAPS: Capabilities = Capabilities {
    backend: Backend::Generic,
    join_hints: &[JoinHint::None],
    cte_policies: &[],
    pruning_keys: &[],
    doc: "",
};

impl DialectBackend for NoCtePolicies {
    fn capabilities(&self) -> &'static Capabilities {
        &NO_CTE_CAPS
    }
    fn lower(&self, mir: &Mir, phys: &PhysicalFacts) -> Lir {
        lower_plan(mir, phys, self)
    }
    fn pruning(&self, _pred: &Pred, _col: &PhysColumn) -> Pruning {
        Pruning::Unknown
    }
}

#[test]
fn a_forced_lir_violation_drops_only_lir() {
    let facts = Facts::new("duckdb");
    let input = facts.input("with a as (select 1 as x) select x from a", Some("m"));
    let plan = drive::drive(&input, &NoCtePolicies, &lower_to_mir);
    assert!(plan.levels.hir.available);
    assert!(plan.levels.mir.available, "{:#?}", plan.levels.mir);
    assert!(!plan.levels.lir.available, "{:#?}", plan.levels.lir);
    assert!(plan
        .levels
        .lir
        .violations
        .iter()
        .any(|v| v.invariant == Invariant::LirHintSupported));
    assert!(plan
        .levels
        .lir
        .unavailable
        .as_deref()
        .is_some_and(|n| n.contains("LirHintSupported")));
    assert_eq!(plan.status, PlanStatus::Partial);
    // the rules that read LIR say so instead of disappearing
    assert!(plan
        .trace
        .entries
        .iter()
        .any(|r| r.phase == "detectLir" && r.reason.as_deref() == Some("unavailable:lir")));
}

#[test]
fn an_unknown_dialect_is_generic_and_lacks_the_dialect_need() {
    let plan = Facts::new("redshift").plan("select 1 as x");
    assert_eq!(plan.backend, Backend::Generic);
    assert!(plan.needs.missing.contains(&"dialect"));
}

#[test]
fn the_plan_is_deterministic_json() {
    let facts = Facts::new("duckdb").yaml("up", &[("id", Some("int"))]);
    let text = "with a as (select id from {{ ref('up') }}) select id from a";
    let once = serde_json::to_string(&facts.plan(text)).expect("serialize");
    let twice = serde_json::to_string(&facts.plan(text)).expect("serialize");
    assert_eq!(once, twice);
    let value: serde_json::Value = serde_json::from_str(&once).expect("json");
    assert_eq!(value["levels"]["hir"]["available"], true);
    assert_eq!(value["schemaRevision"], 7);
    assert_eq!(value["status"], "complete");
}

#[test]
fn a_verifier_violation_gets_bug_guidance_not_dbt_compile() {
    let facts = Facts::new("duckdb");
    let input = facts.input("select 1", Some("m"));
    let lowered: Lowered<cogitator_ir::hir::Hir> = Lowered::declined(Vec::new())
        .with_violations(vec![Violation::new(Invariant::HirScopeTree, "forced")]);
    let message = drive::guidance(&input, &lowered).to_message();
    assert!(message.contains("HirScopeTree"), "{message}");
    assert!(message.contains("cogitator bug"), "{message}");
    assert!(!message.contains("dbt compile"), "{message}");
}
