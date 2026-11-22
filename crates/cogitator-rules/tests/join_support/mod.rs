//! Runs SQL through the real front end, lowering and pass manager, for
//! the `join.*`, `set.*` and `model.*` rule tests.

use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::common::Materialization;
use cogitator_ir::common::ModelConfig;
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::schema::{Completeness, ModelSchema, ProjectSchemas, SchemaCol};
use cogitator_ir::{ColType, Fidelity, Nullability, SqlType, VerifyMode};
use cogitator_lower::lower_to_mir;
use cogitator_rules::passes::{run_passes, Budgets, PassInput, PassOutput};
use cogitator_rules::{Cx, DagFacts, Finding, Fix, Need, Needs};
use cogitator_sql_front::{build_hir, HirInput, SchemaEnv};

/// Every need a detect rule of this package reads, by default.
pub fn all_needs() -> Needs {
    Needs::NONE.with(Need::AllBranches).with(Need::Tests)
}

/// A closed model with the given `(column, unique)` columns.
pub fn model(columns: &[(&str, bool)]) -> Arc<ModelSchema> {
    let columns: Vec<SchemaCol> = columns
        .iter()
        .map(|(name, unique)| SchemaCol {
            name: Arc::from(*name),
            ty: ColType::new(SqlType::Int { bits: None }, Nullability::Unknown),
            unique: *unique,
        })
        .collect();
    Arc::new(ModelSchema {
        columns: columns.into(),
        completeness: Completeness::Closed,
        fingerprint: 0,
    })
}

/// The models the tests read: `a`, `b`, `c`, none with a unique test.
pub fn schemas() -> ProjectSchemas {
    let mut out = ProjectSchemas::default();
    for name in ["a", "b", "c"] {
        out.models.insert(
            Arc::from(name),
            model(&[
                ("id", false),
                ("k", false),
                ("state", false),
                ("flag", false),
            ]),
        );
    }
    out
}

/// Findings from running `sql` with the default schemas and needs.
pub fn findings(sql: &str, rule: &str) -> Vec<Finding> {
    run(sql, schemas(), all_needs(), rule)
}

/// Findings for one rule, with explicit schemas and needs.
pub fn run(sql: &str, schemas: ProjectSchemas, needs: Needs, rule: &str) -> Vec<Finding> {
    run_raw(sql, schemas, needs)
        .findings
        .into_iter()
        .filter(|f| f.id.as_str() == rule)
        .collect()
}

pub fn run_raw(sql: &str, schemas: ProjectSchemas, needs: Needs) -> PassOutput {
    run_raw_with(sql, schemas, needs, Extras::default())
}

/// `Cx` fields a test adds beyond schemas and text.
#[derive(Default)]
pub struct Extras<'a> {
    pub mat: Option<Materialization>,
    pub dag: Option<&'a DagFacts>,
    pub no_text: bool,
}

/// `run_raw` with extra `Cx` fields.
pub fn run_raw_with(
    sql: &str,
    schemas: ProjectSchemas,
    needs: Needs,
    extras: Extras<'_>,
) -> PassOutput {
    let dialect = Dialect::new("duckdb");
    let input = HirInput {
        text: sql,
        dialect: &dialect,
        origin: SourceOrigin::Template {
            variant: TemplateVariant::Plain,
        },
        config: ModelConfig::default(),
    };
    let schemas = Arc::new(schemas);
    let env = SchemaEnv {
        schemas: schemas.clone(),
        model: None,
    };
    let built = build_hir(&input, &env);
    assert!(built.violations.is_empty(), "{:?}", built.violations);
    assert_ne!(built.fidelity, Fidelity::Declined, "{:?}", built.degraded);
    let hir = built.value.expect("hir");
    let lowered = lower_to_mir(&hir);
    assert!(lowered.violations.is_empty(), "{:?}", lowered.violations);
    let mir = lowered.value.expect("mir");
    let mut cx = Cx::new(needs);
    cx.hir = Some(&hir);
    cx.schemas = Some(schemas.as_ref());
    cx.text = (!extras.no_text).then_some(sql);
    cx.mat = extras.mat;
    cx.dag = extras.dag;
    let pass = PassInput {
        hir: Some(&hir),
        mir,
        lir: None,
        lower_lir: None,
        verify: VerifyMode::EveryRewrite,
        budgets: Budgets::default(),
        snapshots: false,
    };
    run_passes(pass, &cx)
}

/// Applies a fix's edits to `sql`, last edit first.
pub fn apply(sql: &str, fix: &Fix) -> String {
    let mut out = sql.to_owned();
    let mut edits: Vec<_> = fix.edits.iter().collect();
    edits.sort_by_key(|e| std::cmp::Reverse(e.span.start));
    for edit in edits {
        out.replace_range(
            edit.span.start as usize..edit.span.end as usize,
            &edit.new_text,
        );
    }
    out
}
