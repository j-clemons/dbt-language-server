//! The pass-manager stage: builds `Cx` and `PhysicalFacts`, lowers LIR
//! through `PassInput::lower_lir`, and keeps what that hook reported.

use std::cell::RefCell;

use cogitator_ir::common::Level;
use cogitator_ir::hir::Hir;
use cogitator_ir::lir::{Backend, Lir};
use cogitator_ir::mir::Mir;
use cogitator_ir::origin::SourceOrigin;
use cogitator_ir::{Degradation, Fidelity, VerifyMode, Violation};
use cogitator_lower::{lower_to_lir, DialectBackend};
use cogitator_rules::cx::Cx;
use cogitator_rules::meta::{Need, Needs};
use cogitator_rules::passes::{run_passes, Budgets, LirLowering, PassInput};

use super::drive::log_violations;
use super::env::BuiltEnv;
use super::phys::{physical_facts, BuiltPhys};
use super::report::FindingView;
use super::trace::{NeedsView, TraceView};
use super::PlanInput;

/// The output of the pass manager, in report vocabulary.
pub(super) struct Run {
    pub(super) mir: Mir,
    pub(super) lir: Option<Lir>,
    /// What `lower_lir` reported; `None` when the hook never ran.
    pub(super) lir_outcome: Option<LirOutcome>,
    pub(super) findings: Vec<FindingView>,
    pub(super) trace: TraceView,
    pub(super) needs: NeedsView,
}

pub(super) struct LirOutcome {
    pub(super) fidelity: Fidelity,
    pub(super) degraded: Vec<Degradation>,
    pub(super) violations: Vec<Violation>,
}

pub(super) struct Passes<'a, 'i> {
    pub(super) input: &'a PlanInput<'i>,
    pub(super) backend: &'a dyn DialectBackend,
    pub(super) env: BuiltEnv,
}

impl Passes<'_, '_> {
    pub(super) fn run(self, hir: &Hir, mut mir: Mir) -> Run {
        let Passes {
            input,
            backend,
            env: built,
        } = self;
        let caps = backend.capabilities();
        let schemas = built.env.schemas.clone();
        let phys = physical_facts(&mut mir, &schemas, caps.pruning_keys, input.model, input);
        let available = needs(input, hir, &built, &phys, caps.backend);

        let outcome: RefCell<Option<LirOutcome>> = RefCell::new(None);
        // `lower_to_lir` runs `verify_lir` against this backend's table.
        let lower = |canonical: &Mir| -> Option<Lir> {
            let mut lowered = lower_to_lir(canonical, backend, &phys.facts);
            log_violations(Level::Lir, &lowered.violations);
            let lir = lowered.value.take();
            *outcome.borrow_mut() = Some(LirOutcome {
                fidelity: lowered.fidelity,
                degraded: std::mem::take(&mut lowered.degraded),
                violations: std::mem::take(&mut lowered.violations),
            });
            lir
        };
        let cx = Cx {
            available,
            hir: Some(hir),
            mir: None,
            text: Some(input.text),
            dialect: Some(caps.backend),
            mat: hir.config.materialized,
            config: Some(&hir.config),
            schemas: built.schema_known.then_some(&*schemas),
            phys: phys.has_keys.then_some(&phys.facts),
            dag: built.dag.as_ref(),
        };
        let output = run_passes(
            PassInput {
                hir: Some(hir),
                mir,
                lir: None,
                lower_lir: Some(LirLowering(&lower)),
                verify: VerifyMode::for_build(),
                budgets: Budgets::default(),
                snapshots: false,
            },
            &cx,
        );
        let trace = TraceView::of(&output.trace);
        Run {
            findings: output.findings.iter().map(FindingView::of).collect(),
            trace,
            mir: output.mir,
            lir: output.lir,
            lir_outcome: outcome.into_inner(),
            needs: NeedsView::of(available),
        }
    }
}

/// `Need::Dialect` is held only by a named backend: `generic` has no
/// capability table to reason with.
fn needs(
    input: &PlanInput<'_>,
    hir: &Hir,
    env: &BuiltEnv,
    phys: &BuiltPhys,
    backend: Backend,
) -> Needs {
    let mut needs = Needs::NONE;
    let mut set = |need: Need, holds: bool| {
        if holds {
            needs = needs.with(need);
        }
    };
    set(Need::Schema, env.schema_known);
    set(Need::Dag, env.dag.is_some());
    set(Need::Tests, env.tests_known);
    set(Need::Types, env.types_known);
    set(Need::Stats, phys.has_stats);
    set(Need::Phys, phys.has_keys);
    set(Need::Mat, hir.config.materialized.is_some());
    set(
        Need::Dialect,
        !input.dialect.as_str().is_empty() && backend != Backend::Generic,
    );
    // a template shows every branch; a compiled plan shows one
    set(
        Need::AllBranches,
        matches!(hir.origin, SourceOrigin::Template { .. }),
    );
    needs
}
