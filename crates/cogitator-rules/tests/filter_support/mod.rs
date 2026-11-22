//! Runs SQL through the whole pipeline for the filter and incr rules:
//! `build_hir`, `lower_to_mir`, `lower_to_lir`, then `run_passes`.
#![allow(dead_code)]

use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::common::ModelConfig;
use cogitator_ir::hir::Hir;
use cogitator_ir::lir::{Backend, KeyKind, Lir, PhysColumn, PhysicalFacts, TableFacts};
use cogitator_ir::mir::{Mir, TableRef};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::schema::{Completeness, ModelSchema, ProjectSchemas, SchemaCol};
use cogitator_ir::types::{ColType, Nullability, SqlType, Tz};
use cogitator_ir::VerifyMode;
use cogitator_lower::backends::for_backend;
use cogitator_lower::{lower_to_lir, lower_to_mir};
use cogitator_rules::passes::{run_passes, LirLowering, PassInput, PassOutput};
use cogitator_rules::{Budgets, Cx, Finding, Need, Needs};
use cogitator_sql_front::{build_hir, HirInput, SchemaEnv};

/// The dbt model the incremental fixtures build; `{{ this }}` resolves here.
pub const SELF_MODEL: &str = "inc_model";
/// The model every fixture reads and every key is declared on.
pub const INPUT_MODEL: &str = "events";

pub fn timestamp() -> SqlType {
    SqlType::Timestamp { tz: Tz::Unknown }
}

pub fn datetime() -> SqlType {
    SqlType::Timestamp { tz: Tz::Without }
}

/// One pipeline run. `ts_type` is the `events.ts` column's type.
#[derive(Clone, Copy)]
pub struct Case<'a> {
    pub sql: &'a str,
    pub dialect: &'a str,
    pub backend: Backend,
    pub keys: &'a [(&'a str, KeyKind)],
    pub ts_type: &'a SqlType,
    pub variant: TemplateVariant,
    pub origin: Option<&'a SourceOrigin>,
    /// Drops one need from `Cx::available`, to test `Skipped`.
    pub without: Option<Need>,
    /// Leave `events` out of the schema, so its scan is open.
    pub open_schema: bool,
}

impl<'a> Case<'a> {
    pub fn new(sql: &'a str, backend: Backend, keys: &'a [(&'a str, KeyKind)]) -> Self {
        static TIMESTAMP: SqlType = SqlType::Timestamp { tz: Tz::Unknown };
        Self {
            sql,
            dialect: match backend {
                Backend::BigQuery => "bigquery",
                Backend::Snowflake => "snowflake",
                Backend::DuckDb => "duckdb",
                Backend::Postgres => "postgres",
                Backend::Generic => "generic",
            },
            backend,
            keys,
            ts_type: &TIMESTAMP,
            variant: TemplateVariant::Plain,
            origin: None,
            without: None,
            open_schema: false,
        }
    }
}

pub struct Run {
    pub out: PassOutput,
}

impl Run {
    pub fn findings(&self, id: &str) -> Vec<&Finding> {
        self.out
            .findings
            .iter()
            .filter(|f| f.id.as_str() == id)
            .collect()
    }

    pub fn lir(&self) -> &Lir {
        self.out.lir.as_ref().expect("a LIR")
    }
}

pub fn only<'a>(run: &'a Run, id: &str) -> &'a Finding {
    let found = run.findings(id);
    assert_eq!(found.len(), 1, "{:?}", run.out.findings);
    found[0]
}

pub fn no_findings(run: &Run, id: &str) {
    assert_eq!(run.findings(id).len(), 0, "{:?}", run.out.findings);
}

fn column(name: &str, ty: SqlType) -> SchemaCol {
    SchemaCol {
        name: Arc::from(name),
        ty: ColType::new(ty, Nullability::Unknown),
        unique: false,
    }
}

fn schema(columns: Vec<SchemaCol>) -> Arc<ModelSchema> {
    Arc::new(ModelSchema {
        columns: Arc::from(columns),
        completeness: Completeness::Closed,
        fingerprint: 0,
    })
}

fn env(case: &Case<'_>) -> SchemaEnv {
    let int = SqlType::Int { bits: Some(64) };
    let mut schemas = ProjectSchemas::default();
    if !case.open_schema {
        schemas.models.insert(
            Arc::from(INPUT_MODEL),
            schema(vec![
                column("id", int.clone()),
                column("ts", case.ts_type.clone()),
                column("a", int.clone()),
                column("b", int.clone()),
            ]),
        );
    }
    schemas.models.insert(
        Arc::from(SELF_MODEL),
        schema(vec![column("id", int), column("ts", timestamp())]),
    );
    SchemaEnv {
        schemas: Arc::new(schemas),
        model: Some(Arc::from(SELF_MODEL)),
    }
}

/// Facts for `events`. Columns use the plan's own symbols; cluster keys
/// are listed in `CLUSTER BY` order, which `keys` must already be in.
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
            // Snowflake folds unquoted names up; the rest leave them.
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

/// The HIR for `case`, or `None` when the front end declines it.
pub fn hir_of(case: &Case<'_>) -> Option<Hir> {
    let dialect = Dialect::new(case.dialect);
    let origin = case.origin.cloned().unwrap_or(SourceOrigin::Template {
        variant: case.variant,
    });
    let input = HirInput {
        text: case.sql,
        dialect: &dialect,
        origin,
        config: ModelConfig::default(),
    };
    build_hir(&input, &env(case)).value
}

pub fn run(case: Case<'_>) -> Run {
    let hir = hir_of(&case).expect("a HIR");
    run_on(&case, &hir)
}

pub fn run_on(case: &Case<'_>, hir: &Hir) -> Run {
    let mir = lower_to_mir(hir).value.expect("a MIR");
    let phys = facts(&mir, case.keys);
    let backend = for_backend(case.backend).expect("every backend is registered");
    let lower = |plan: &Mir| lower_to_lir(plan, backend, &phys).value;
    let mut cx = Cx::new(needs(case.without));
    cx.hir = Some(hir);
    cx.dialect = Some(case.backend);
    cx.mat = hir.config.materialized;
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
