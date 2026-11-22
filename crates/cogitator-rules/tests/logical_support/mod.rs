//! Runs SQL through the real pipeline for the `order.*`, `subq.*`,
//! `join.redundant_left`, `join.key_type_mismatch` and
//! `filter.constant_false` rule tests.

#![allow(dead_code)]

use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::common::ModelConfig;
use cogitator_ir::lir::Backend;
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::schema::{Completeness, ModelSchema, ProjectSchemas, SchemaCol};
use cogitator_ir::{ColType, Fidelity, Nullability, SqlType, VerifyMode};
use cogitator_lower::lower_to_mir;
use cogitator_rules::passes::{run_passes, Budgets, PassInput, PassOutput};
use cogitator_rules::{Cx, Finding, Fix, Need, Needs};
use cogitator_sql_front::{build_hir, HirInput, SchemaEnv};

/// One column of a test model.
#[derive(Clone, Copy)]
pub struct Col {
    pub name: &'static str,
    pub ty: Type,
    pub null: Nullability,
    pub unique: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Int,
    Decimal,
    Text,
    Unknown,
}

impl Type {
    fn sql(self) -> SqlType {
        match self {
            Type::Int => SqlType::Int { bits: Some(64) },
            Type::Decimal => SqlType::Decimal {
                p: Some(12),
                s: Some(2),
            },
            Type::Text => SqlType::Text,
            Type::Unknown => SqlType::Unknown,
        }
    }
}

/// A plain nullable-unknown integer column.
pub const fn col(name: &'static str) -> Col {
    Col {
        name,
        ty: Type::Int,
        null: Nullability::Unknown,
        unique: false,
    }
}

impl Col {
    pub const fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    pub const fn not_null(mut self) -> Self {
        self.null = Nullability::NotNull;
        self
    }

    pub const fn nullable(mut self) -> Self {
        self.null = Nullability::Nullable;
        self
    }

    pub const fn typed(mut self, ty: Type) -> Self {
        self.ty = ty;
        self
    }
}

pub fn model(columns: &[Col]) -> Arc<ModelSchema> {
    let columns: Vec<SchemaCol> = columns
        .iter()
        .map(|c| SchemaCol {
            name: Arc::from(c.name),
            ty: ColType::new(c.ty.sql(), c.null),
            unique: c.unique,
        })
        .collect();
    Arc::new(ModelSchema {
        columns: columns.into(),
        completeness: Completeness::Closed,
        fingerprint: 0,
    })
}

/// `{name: columns}` as a closed project schema.
pub fn schemas(models: &[(&str, &[Col])]) -> ProjectSchemas {
    let mut out = ProjectSchemas::default();
    for (name, columns) in models {
        out.models.insert(Arc::from(*name), model(columns));
    }
    out
}

/// The default models: `a`, `b`, both `(id, k, v)` with unknown nulls.
pub fn default_schemas() -> ProjectSchemas {
    let cols = [col("id"), col("k"), col("v")];
    schemas(&[("a", &cols), ("b", &cols)])
}

/// Every need these rules may declare.
pub fn all_needs() -> Needs {
    Needs::NONE
        .with(Need::AllBranches)
        .with(Need::Tests)
        .with(Need::Types)
}

#[derive(Default)]
pub struct Options {
    pub dialect: Option<Backend>,
    pub no_text: bool,
}

pub fn run(sql: &str, schemas: ProjectSchemas, needs: Needs, options: Options) -> PassOutput {
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
    cx.text = (!options.no_text).then_some(sql);
    cx.dialect = options.dialect;
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

/// Findings for one rule with explicit schemas, needs and options.
pub fn run_rule(
    sql: &str,
    schemas: ProjectSchemas,
    needs: Needs,
    options: Options,
    rule: &str,
) -> Vec<Finding> {
    run(sql, schemas, needs, options)
        .findings
        .into_iter()
        .filter(|f| f.id.as_str() == rule)
        .collect()
}

/// Findings for one rule against the default schemas.
pub fn findings(sql: &str, rule: &str) -> Vec<Finding> {
    run_rule(
        sql,
        default_schemas(),
        all_needs(),
        Options::default(),
        rule,
    )
}

pub fn only(mut found: Vec<Finding>) -> Finding {
    assert_eq!(found.len(), 1, "{found:#?}");
    found.remove(0)
}

/// The text of `sql` under a finding's span.
pub fn at<'a>(sql: &'a str, finding: &Finding) -> &'a str {
    &sql[finding.span.start as usize..finding.span.end as usize]
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
