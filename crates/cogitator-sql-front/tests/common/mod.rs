//! Shared helpers: build a HIR, assert it is clean, and print everything
//! the snapshots pin (structure, Jinja roles, resolutions, degradations).
#![allow(dead_code)]

use std::fmt::Write as _;
use std::sync::Once;
use std::time::Duration;

use cogitator_dialects::Dialect;
use cogitator_ir::common::ModelConfig;
use cogitator_ir::hir::{print_hir, Hir, JinjaKind, Resolution};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::schema::{ModelSchema, ProjectSchemas};
use cogitator_ir::{Fidelity, Lowered};
use cogitator_sql_front::{build_hir, HirInput, SchemaEnv};
use std::sync::Arc;

static WATCHDOG: Once = Once::new();

/// Aborts this test binary at a deadline, measured from its first build,
/// so a runaway input cannot eat memory indefinitely. An abort kills the
/// test itself, which a timeout around `cargo` would not.
/// `COGITATOR_TEST_WATCHDOG_SECS` overrides the default of 300.
fn arm_watchdog() {
    WATCHDOG.call_once(|| {
        let secs = std::env::var("COGITATOR_TEST_WATCHDOG_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300u64);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            eprintln!("watchdog: test binary exceeded {secs}s, aborting");
            std::process::abort();
        });
    });
}

pub fn build_in(
    text: &str,
    dialect: &str,
    variant: TemplateVariant,
    env: &SchemaEnv,
) -> Lowered<Hir> {
    arm_watchdog();
    let dialect = Dialect::new(dialect);
    let input = HirInput {
        text,
        dialect: &dialect,
        origin: SourceOrigin::Template { variant },
        config: ModelConfig::default(),
    };
    let lowered = build_hir(&input, env);
    lowered.assert_clean();
    lowered
}

pub fn build(text: &str) -> Lowered<Hir> {
    build_in(
        text,
        "duckdb",
        TemplateVariant::Plain,
        &SchemaEnv::default(),
    )
}

pub fn hir_of(text: &str) -> Hir {
    let lowered = build(text);
    assert_eq!(
        lowered.fidelity,
        Fidelity::Complete,
        "degraded: {:?}",
        lowered.degraded
    );
    lowered.value.expect("hir")
}

/// `name` → `Resolution` of the first reference with that folded name.
pub fn res_of(hir: &Hir, name: &str) -> Resolution {
    hir.cols
        .values()
        .find(|c| hir.syms.text(c.name.folded) == name)
        .and_then(|c| c.res.clone())
        .unwrap_or_else(|| panic!("no column reference named {name}"))
}

/// Every reference named `name`, in arena order.
pub fn all_res_of(hir: &Hir, name: &str) -> Vec<Resolution> {
    hir.cols
        .values()
        .filter(|c| hir.syms.text(c.name.folded) == name)
        .filter_map(|c| c.res.clone())
        .collect()
}

/// The text a snapshot pins for one build.
pub fn report(text: &str, lowered: &Lowered<Hir>) -> String {
    let mut out = format!("fidelity {:?}\n", lowered.fidelity);
    let Some(hir) = &lowered.value else {
        write_degraded(lowered, &mut out);
        return out;
    };
    out.push_str(&print_hir(hir));
    let _ = writeln!(out, "--jinja--");
    for node in hir.jinja.values() {
        let span = text
            .get(node.span.start as usize..node.span.end as usize)
            .unwrap_or("?");
        let kind = match node.kind {
            JinjaKind::Macro(name) => format!("Macro({})", hir.syms.text(name)),
            other => format!("{other:?}"),
        };
        let _ = writeln!(out, "{kind} {:?} {span:?}", node.role);
    }
    let _ = writeln!(out, "--outputs--");
    if let Some(root) = hir.root.and_then(|r| hir.queries.get(r)) {
        for col in &root.outputs {
            let name = col.name.map_or("<unnamed>", |n| hir.syms.text(n));
            let _ = writeln!(out, "{name}");
        }
    }
    if hir.config != ModelConfig::default() {
        let _ = writeln!(out, "--config--\n{:?}", hir.config);
    }
    let _ = writeln!(out, "--resolutions--");
    for col in hir.cols.values() {
        let name = hir.syms.text(col.name.folded);
        let _ = writeln!(out, "{name} {:?}", col.res);
    }
    write_degraded(lowered, &mut out);
    out
}

fn write_degraded(lowered: &Lowered<Hir>, out: &mut String) {
    let _ = writeln!(out, "--degraded--");
    for d in &lowered.degraded {
        let _ = writeln!(out, "{:?} {}", d.kind, d.detail);
    }
}

/// A schema env with one model of the given completeness.
pub fn env_with(model: &str, columns: &[&str], closed: bool) -> SchemaEnv {
    use cogitator_ir::schema::{Completeness, SchemaCol};
    use cogitator_ir::types::ColType;
    let columns: Vec<SchemaCol> = columns
        .iter()
        .map(|name| SchemaCol {
            name: Arc::from(*name),
            ty: ColType::UNKNOWN,
            unique: false,
        })
        .collect();
    let schema = ModelSchema {
        columns: Arc::from(columns),
        completeness: if closed {
            Completeness::Closed
        } else {
            Completeness::Open
        },
        fingerprint: 0,
    };
    let mut schemas = ProjectSchemas::default();
    schemas.models.insert(Arc::from(model), Arc::new(schema));
    SchemaEnv {
        schemas: Arc::new(schemas),
        model: None,
    }
}
