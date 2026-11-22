//! The staged driver: text → HIR → MIR → passes (with LIR lowered through
//! `PassInput::lower_lir`) → report. Each stage stops the ones below it
//! when it produces nothing; the report says which and why.

use cogitator_error::guidance::Guidance;
use cogitator_ir::common::Level;
use cogitator_ir::hir::{print_hir, Hir};
use cogitator_ir::lir::{print_lir, Backend};
use cogitator_ir::mir::{print_mir, Mir};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::{DegradationKind, Fidelity, Lowered, Violation};
use cogitator_lower::DialectBackend;
use cogitator_rules::meta::Needs;
use cogitator_sql_front::{build_hir, HirInput, SchemaEnv};

use super::compiled_findings::place_findings;
use super::compiled_route::{self, Failure, Route};
use super::env::{bare_env, for_plan, BuiltEnv, Upstreams};
use super::facts::merge_manifest;
use super::passes::{Passes, Run};
use super::report::{LevelReport, OriginView, PlanLevels, PlanStatus, QueryPlan};
use super::trace::{NeedsView, TraceView};
use super::PlanInput;

pub(super) type MirLowering<'a> = &'a dyn Fn(&Hir) -> Lowered<Mir>;

/// Runs the whole pipeline. `lower` is the HIR→MIR seam tests use to force
/// a violation; everywhere else it is `lower_to_mir`.
pub(super) fn drive(
    input: &PlanInput<'_>,
    backend: &dyn DialectBackend,
    lower: MirLowering<'_>,
) -> QueryPlan {
    let template = built_hir(input);
    let (route_input, built, banner, failure) =
        if compiled_route::declined_on_control_flow(&template.hir) {
            match compiled_route::route(input) {
                Ok(Route {
                    input,
                    built,
                    banner,
                }) => (input, built, banner, None),
                Err(failure) => (*input, template, None, Some(failure)),
            }
        } else {
            (*input, template, None, None)
        };
    let input = &route_input;
    let mut hir_lowered = built.hir;
    if let Some(hir) = hir_lowered.value.as_mut() {
        merge_manifest_config(hir, input);
    }
    log_violations(Level::Hir, &hir_lowered.violations);
    let hir_text = hir_lowered.value.as_ref().map(print_hir);
    let hir_report = LevelReport::of(&hir_lowered, hir_text, "HIR");
    let backend_kind = backend.capabilities().backend;

    let Some(hir) = hir_lowered.value.as_ref() else {
        return declined(input, &hir_lowered, hir_report, backend_kind, failure);
    };

    let mir_lowered = lower(hir);
    log_violations(Level::Mir, &mir_lowered.violations);
    let Lowered {
        value: mir,
        fidelity: mir_fidelity,
        degraded: mir_degraded,
        violations: mir_violations,
    } = mir_lowered;
    let Some(mir) = mir else {
        let lowered: Lowered<Mir> = Lowered {
            value: None,
            fidelity: mir_fidelity,
            degraded: mir_degraded,
            violations: mir_violations,
        };
        return without_mir(input, hir, hir_report, &lowered, backend_kind, banner);
    };

    let mut run = Passes {
        input,
        backend,
        env: built.env,
    }
    .run(hir, mir);
    place_findings(&mut run.findings, &hir.origin);

    let mir_report = LevelReport {
        available: true,
        fidelity: Some(mir_fidelity),
        text: Some(print_mir(&run.mir)),
        unavailable: None,
        degradations: mir_degraded,
        violations: mir_violations,
    };
    let lir_report = lir_report(&run);
    let status = status_of(&hir_report, &mir_report, &lir_report, run.trace.incomplete);
    QueryPlan {
        model: input.model.map(str::to_string),
        dialect: input.dialect.as_str().to_string(),
        backend: backend_kind,
        schema_revision: input.schema_revision,
        status,
        guidance: None,
        banner,
        origin: Some(OriginView::of(&hir.origin)),
        levels: PlanLevels {
            hir: hir_report,
            mir: mir_report,
            lir: lir_report,
        },
        findings: run.findings,
        trace: run.trace,
        needs: run.needs,
    }
}

pub(super) struct BuiltHir {
    pub(super) hir: Lowered<Hir>,
    pub(super) env: BuiltEnv,
}

fn hir_input<'a>(input: &PlanInput<'a>) -> HirInput<'a> {
    HirInput {
        text: input.text,
        dialect: input.dialect,
        origin: SourceOrigin::Template {
            variant: TemplateVariant::Plain,
        },
        config: Default::default(),
    }
}

fn built_hir(input: &PlanInput<'_>) -> BuiltHir {
    let front = hir_input(input);
    two_pass(input, |env| build_hir(&front, env))
}

/// HIR is built once to learn the direct upstreams from the text, and
/// again only when those upstreams have schema facts to bind against.
pub(super) fn two_pass(
    input: &PlanInput<'_>,
    build: impl Fn(&SchemaEnv) -> Lowered<Hir>,
) -> BuiltHir {
    let first = build(&bare_env(input));
    let Some(hir) = first.value.as_ref() else {
        return BuiltHir {
            hir: first,
            env: BuiltEnv::default(),
        };
    };
    let env = for_plan(&Upstreams::of(hir), input);
    if env.is_empty() {
        return BuiltHir { hir: first, env };
    }
    BuiltHir {
        hir: build(&env.env),
        env,
    }
}

fn merge_manifest_config(hir: &mut Hir, input: &PlanInput<'_>) {
    let manifest = input
        .model
        .and_then(|m| input.manifest?.models.get(m))
        .map(|node| &node.config);
    let contract = input
        .model
        .and_then(|m| input.models.get(m))
        .is_some_and(|details| details.contract_enforced);
    merge_manifest(
        &mut hir.config,
        &mut hir.syms,
        manifest,
        contract,
        input.dialect,
    );
}

pub(super) fn log_violations(level: Level, violations: &[Violation]) {
    for violation in violations {
        tracing::error!(?level, invariant = ?violation.invariant, detail = %violation.detail,
            "IR verifier violation; level dropped");
    }
}

/// HIR produced nothing: control flow the template cannot express, or a
/// verifier violation.
fn declined(
    input: &PlanInput<'_>,
    lowered: &Lowered<Hir>,
    hir: LevelReport,
    backend: Backend,
    failure: Option<Failure<'_>>,
) -> QueryPlan {
    QueryPlan {
        model: input.model.map(str::to_string),
        dialect: input.dialect.as_str().to_string(),
        backend,
        schema_revision: input.schema_revision,
        status: PlanStatus::Declined,
        guidance: Some(compiled_route::declined_guidance(input, lowered, failure).to_message()),
        banner: None,
        origin: None,
        levels: PlanLevels {
            hir,
            mir: LevelReport::skipped("the HIR is unavailable"),
            lir: LevelReport::skipped("the HIR is unavailable"),
        },
        findings: Vec::new(),
        trace: TraceView::unavailable(Level::Hir),
        needs: NeedsView::of(Needs::NONE),
    }
}

pub(super) fn guidance(input: &PlanInput<'_>, lowered: &Lowered<Hir>) -> Guidance {
    let model = input.model.unwrap_or("this model");
    if let Some(violation) = lowered.violations.first() {
        return Guidance::new(format!(
            "No query plan for {model}: the verifier rejected it."
        ))
        .because(format!("{:?}: {}", violation.invariant, violation.detail))
        .fix("this is a cogitator bug; please report it with the model's SQL.");
    }
    Guidance::new(format!("No query plan for {model} from its template."))
        .because(decline_reason(lowered).to_string())
        .fix("run `dbt compile` to plan the model from its compiled SQL.")
}

pub(super) fn decline_reason(lowered: &Lowered<Hir>) -> &str {
    lowered
        .degraded
        .iter()
        .find(|d| d.kind == DegradationKind::JinjaControlFlow)
        .or(lowered.degraded.first())
        .map_or("the template could not be lowered", |d| d.detail.as_str())
}

/// MIR lowering produced nothing. HIR is shown; everything below is not.
fn without_mir(
    input: &PlanInput<'_>,
    hir: &Hir,
    hir_report: LevelReport,
    lowered: &Lowered<Mir>,
    backend: Backend,
    banner: Option<String>,
) -> QueryPlan {
    QueryPlan {
        model: input.model.map(str::to_string),
        dialect: input.dialect.as_str().to_string(),
        backend,
        schema_revision: input.schema_revision,
        status: PlanStatus::Partial,
        guidance: None,
        banner,
        origin: Some(OriginView::of(&hir.origin)),
        levels: PlanLevels {
            hir: hir_report,
            mir: LevelReport::of(lowered, None, "MIR"),
            lir: LevelReport::skipped("the MIR is unavailable"),
        },
        findings: Vec::new(),
        trace: TraceView::unavailable(Level::Mir),
        needs: NeedsView::of(Needs::NONE),
    }
}

fn lir_report(run: &Run) -> LevelReport {
    let Some(outcome) = &run.lir_outcome else {
        return LevelReport::skipped("the pass manager stopped before LIR");
    };
    let lowered = Lowered {
        value: run.lir.as_ref(),
        fidelity: outcome.fidelity,
        degraded: outcome.degraded.clone(),
        violations: outcome.violations.clone(),
    };
    let text = run.lir.as_ref().map(|lir| print_lir(lir, &run.mir));
    LevelReport::of(&lowered, text, "LIR")
}

fn status_of(
    hir: &LevelReport,
    mir: &LevelReport,
    lir: &LevelReport,
    incomplete: bool,
) -> PlanStatus {
    let clean = |level: &LevelReport| {
        level.available
            && level.fidelity == Some(Fidelity::Complete)
            && level.degradations.is_empty()
            && level.violations.is_empty()
    };
    if clean(hir) && clean(mir) && clean(lir) && !incomplete {
        PlanStatus::Complete
    } else {
        PlanStatus::Partial
    }
}
