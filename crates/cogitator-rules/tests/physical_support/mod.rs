//! The physical rules need what the filter harness leaves out of `Cx`:
//! the document text, the model config, and facts that may be absent.
//! This drives the same pipeline, as `features/plan` does.
#![allow(dead_code, unused_imports)]

#[path = "../filter_support/mod.rs"]
pub mod base;

use cogitator_ir::hir::Hir;
use cogitator_ir::lir::{KeyKind, PhysColumn, PhysicalFacts, TableFacts};
use cogitator_ir::mir::{Mir, TableRef};
use cogitator_ir::types::SqlType;
use cogitator_ir::VerifyMode;
use cogitator_lower::backends::for_backend;
use cogitator_lower::{lower_to_lir, lower_to_mir};
use cogitator_rules::passes::{run_passes, LirLowering, PassInput};
use cogitator_rules::{Budgets, Cx, Need, Needs};

pub use base::{no_findings, only, Case, Run, INPUT_MODEL};

#[derive(Clone, Copy)]
pub struct Options {
    pub with_text: bool,
    pub with_facts: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            with_text: true,
            with_facts: true,
        }
    }
}

fn facts(mir: &Mir, keys: &[(&str, KeyKind)]) -> PhysicalFacts {
    let mut phys = PhysicalFacts::default();
    let target = mir.tables.iter().find(|(_, table)| {
        matches!(table.target, TableRef::Model { name, .. } if mir.syms.text(name) == INPUT_MODEL)
    });
    let Some((id, _)) = target else {
        return phys;
    };
    let columns = keys
        .iter()
        .filter_map(|(name, kind)| {
            let symbol = mir
                .syms
                .get(name)
                .or_else(|| mir.syms.get(&name.to_ascii_uppercase()))?;
            Some(PhysColumn {
                name: symbol,
                ty: SqlType::Unknown,
                key: Some(*kind),
            })
        })
        .collect();
    phys.tables.insert(
        id.raw(),
        TableFacts {
            columns,
            row_count: None,
        },
    );
    phys
}

fn needs(without: Option<Need>) -> Needs {
    [Need::Dialect, Need::Types, Need::Phys, Need::Mat]
        .into_iter()
        .filter(|need| Some(*need) != without)
        .fold(Needs::NONE, Needs::with)
}

pub fn hir_of(case: &Case<'_>) -> Hir {
    base::hir_of(case).expect("a HIR")
}

pub fn run_with(case: Case<'_>, options: Options) -> Run {
    run_hir(&case, &hir_of(&case), options)
}

pub fn run(case: Case<'_>) -> Run {
    run_with(case, Options::default())
}

pub fn run_hir(case: &Case<'_>, hir: &Hir, options: Options) -> Run {
    let mir = lower_to_mir(hir).value.expect("a MIR");
    let phys = if options.with_facts {
        facts(&mir, case.keys)
    } else {
        PhysicalFacts::default()
    };
    let backend = for_backend(case.backend).expect("every backend is registered");
    let lower = |plan: &Mir| lower_to_lir(plan, backend, &phys).value;
    let mut cx = Cx::new(needs(case.without));
    cx.hir = Some(hir);
    cx.text = options.with_text.then_some(case.sql);
    cx.dialect = Some(case.backend);
    cx.mat = hir.config.materialized;
    cx.config = Some(&hir.config);
    cx.phys = Some(&phys);
    let out = run_passes(
        PassInput {
            hir: Some(hir),
            mir,
            lir: None,
            lower_lir: Some(LirLowering(&lower)),
            verify: VerifyMode::EveryRewrite,
            budgets: Budgets::STANDARD,
            snapshots: false,
        },
        &cx,
    );
    Run { out }
}
