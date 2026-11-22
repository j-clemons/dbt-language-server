//! `SchemaEnv` for one plan. With a [`SchemaSnapshot`] the direct upstreams
//! come from the project-wide job, `Closed` where it proved that. Before the
//! first job finishes, `fill` lists YAML and catalog columns as `Open`: a
//! listing is evidence of these columns, not proof of no others.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use cogitator_ir::hir::{FromKind, Hir, RelTarget};
use cogitator_ir::schema::{Completeness, ModelSchema, ProjectSchemas, SchemaCol};
use cogitator_ir::{ColType, Nullability, SqlType};
use cogitator_schemas::ColumnEntry;
use cogitator_sql_front::{parse_sql_type, SchemaEnv};

use crate::artifacts::catalog::CatalogTable;

use cogitator_rules::DagFacts;

use super::schemas::merge::with_tests;
use super::schemas::SchemaSnapshot;
use super::PlanInput;

/// Named relations the query reads directly.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Upstreams {
    models: BTreeSet<String>,
    sources: BTreeSet<(String, String)>,
    this: bool,
    /// A hard-coded table or dynamic Jinja relation: no facts can exist.
    unnamed: bool,
}

impl Upstreams {
    /// Walks every FROM item; a parse hole contributes nothing.
    pub(super) fn of(hir: &Hir) -> Self {
        let mut out = Upstreams::default();
        for (_, from) in hir.from.iter() {
            let FromKind::Base { target, .. } = &from.kind else {
                continue;
            };
            match target {
                RelTarget::Model { name, .. } => {
                    out.models.insert(hir.syms.text(*name).to_string());
                }
                RelTarget::Source { source, table } => {
                    out.sources.insert((
                        hir.syms.text(*source).to_string(),
                        hir.syms.text(*table).to_string(),
                    ));
                }
                RelTarget::This => out.this = true,
                RelTarget::Physical(_) | RelTarget::Jinja(_) => out.unnamed = true,
                RelTarget::Cte(_) => {}
            }
        }
        out
    }

    fn count(&self) -> usize {
        self.models.len() + self.sources.len() + usize::from(self.this)
    }
}

/// The environment built, and what it can honestly claim.
#[derive(Debug, Default)]
pub(super) struct BuiltEnv {
    pub(super) env: SchemaEnv,
    /// Every direct upstream has a schema, so `Need::Schema` holds.
    pub(super) schema_known: bool,
    /// At least one column has a stated type, so `Need::Types` holds.
    pub(super) types_known: bool,
    /// A direct upstream has a strict `unique` or `not_null` test.
    pub(super) tests_known: bool,
    /// Downstream demand for this model, when it is known.
    pub(super) dag: Option<DagFacts>,
}

impl BuiltEnv {
    pub(super) fn is_empty(&self) -> bool {
        self.env.schemas.models.is_empty() && self.env.schemas.sources.is_empty()
    }
}

/// An environment naming only the model: used for the first HIR pass.
pub(super) fn bare_env(input: &PlanInput<'_>) -> SchemaEnv {
    SchemaEnv {
        schemas: Arc::new(ProjectSchemas {
            revision: input.schema_revision,
            ..ProjectSchemas::default()
        }),
        model: input.model.map(Arc::from),
    }
}

/// The environment for one plan: the job's snapshot when there is one.
pub(super) fn for_plan(upstreams: &Upstreams, input: &PlanInput<'_>) -> BuiltEnv {
    match input.project {
        Some(snapshot) => from_snapshot(upstreams, input, snapshot),
        None => fill(upstreams, input),
    }
}

/// Copies the direct upstreams out of the project snapshot. A relation
/// the job has no schema for is left out, so the binder keeps it open.
fn from_snapshot(
    upstreams: &Upstreams,
    input: &PlanInput<'_>,
    snapshot: &SchemaSnapshot,
) -> BuiltEnv {
    let mut schemas = ProjectSchemas {
        revision: snapshot.revision,
        ..ProjectSchemas::default()
    };
    let mut found = 0;
    let mut types_known = false;
    let self_name = input.model.filter(|_| upstreams.this);
    for name in upstreams.models.iter().map(String::as_str).chain(self_name) {
        if let Some(schema) = snapshot.schemas.model(name) {
            found += 1;
            types_known |= has_types(schema);
            schemas.models.insert(Arc::from(name), schema.clone());
        }
    }
    for (source, table) in &upstreams.sources {
        if let Some(schema) = snapshot.schemas.source(source, table) {
            found += 1;
            types_known |= has_types(schema);
            schemas.sources.insert(
                (Arc::from(source.as_str()), Arc::from(table.as_str())),
                schema.clone(),
            );
        }
    }
    BuiltEnv {
        schema_known: upstreams.count() > 0 && !upstreams.unnamed && found == upstreams.count(),
        types_known,
        tests_known: tested_upstream(upstreams, input),
        dag: input.model.and_then(|m| snapshot.dag.facts(m)),
        env: SchemaEnv {
            schemas: Arc::new(schemas),
            model: input.model.map(Arc::from),
        },
    }
}

/// Whether YAML on a direct upstream states a proof-giving test.
fn tested_upstream(upstreams: &Upstreams, input: &PlanInput<'_>) -> bool {
    let tested = |columns: &[ColumnEntry]| {
        columns
            .iter()
            .any(|c| c.proven_by("unique") || c.proven_by("not_null"))
    };
    upstreams
        .models
        .iter()
        .any(|m| input.models.get(m).is_some_and(|d| tested(&d.columns)))
        || upstreams.sources.iter().any(|(source, table)| {
            input
                .sources
                .get(source)
                .and_then(|s| s.tables.get(table))
                .is_some_and(|t| tested(&t.columns))
        })
}

/// Fills schemas for `upstreams` from YAML and the catalog. A relation
/// with neither is left out, so the binder keeps it open.
pub(super) fn fill(upstreams: &Upstreams, input: &PlanInput<'_>) -> BuiltEnv {
    let mut schemas = ProjectSchemas {
        revision: input.schema_revision,
        ..ProjectSchemas::default()
    };
    let mut found = 0;
    let mut types_known = false;

    let self_name = input.model.filter(|_| upstreams.this);
    for name in upstreams.models.iter().map(String::as_str).chain(self_name) {
        let yaml = input
            .models
            .get(name)
            .map_or(&[][..], |d| d.columns.as_slice());
        let catalog = input.catalog.and_then(|c| c.models.get(name));
        if let Some(schema) = schema_of(yaml, catalog, input) {
            found += 1;
            types_known |= has_types(&schema);
            schemas.models.insert(Arc::from(name), Arc::new(schema));
        }
    }
    for (source, table) in &upstreams.sources {
        let yaml = input
            .sources
            .get(source)
            .and_then(|s| s.tables.get(table))
            .map_or(&[][..], |t| t.columns.as_slice());
        let catalog = input
            .catalog
            // boundary: the catalog's key is an owned tuple
            .and_then(|c| c.sources.get(&(source.clone(), table.clone())));
        if let Some(schema) = schema_of(yaml, catalog, input) {
            found += 1;
            types_known |= has_types(&schema);
            schemas.sources.insert(
                (Arc::from(source.as_str()), Arc::from(table.as_str())),
                Arc::new(schema),
            );
        }
    }
    BuiltEnv {
        schema_known: upstreams.count() > 0 && !upstreams.unnamed && found == upstreams.count(),
        types_known,
        tests_known: tested_upstream(upstreams, input),
        dag: None,
        env: SchemaEnv {
            schemas: Arc::new(schemas),
            model: input.model.map(Arc::from),
        },
    }
}

fn has_types(schema: &ModelSchema) -> bool {
    schema.columns.iter().any(|c| c.ty.ty != SqlType::Unknown)
}

/// Catalog columns first, in warehouse order, with their warehouse type;
/// YAML-only columns after, typed from `data_type` when stated.
fn schema_of(
    yaml: &[ColumnEntry],
    catalog: Option<&CatalogTable>,
    input: &PlanInput<'_>,
) -> Option<ModelSchema> {
    if yaml.is_empty() && catalog.is_none_or(|c| c.columns.is_empty()) {
        return None;
    }
    let mut columns: Vec<SchemaCol> = Vec::new();
    for column in catalog.map_or(&[][..], |c| c.columns.as_slice()) {
        columns.push(col(&column.name, Some(&column.data_type), input));
    }
    let typed_in_catalog = columns.len();
    for entry in yaml {
        let known = columns
            .iter()
            .take(typed_in_catalog)
            .position(|c| c.name.eq_ignore_ascii_case(&entry.name));
        match known {
            Some(i) => {
                let name = columns[i].name.clone();
                let mut ty = columns[i].ty.clone();
                if ty.ty == SqlType::Unknown {
                    ty = col(&entry.name, entry.data_type.as_deref(), input).ty;
                }
                columns[i] = with_tests(name, ty, Some(entry));
            }
            None => {
                let ty = col(&entry.name, entry.data_type.as_deref(), input).ty;
                columns.push(with_tests(Arc::from(entry.name.as_str()), ty, Some(entry)));
            }
        }
    }
    let mut hasher = DefaultHasher::new();
    columns.hash(&mut hasher);
    Some(ModelSchema {
        columns: Arc::from(columns),
        completeness: Completeness::Open,
        fingerprint: hasher.finish(),
    })
}

fn col(name: &str, raw_type: Option<&str>, input: &PlanInput<'_>) -> SchemaCol {
    let ty = raw_type.map_or(SqlType::Unknown, |raw| parse_sql_type(raw, input.dialect));
    SchemaCol {
        name: Arc::from(name),
        ty: ColType::new(ty, Nullability::Unknown),
        unique: false,
    }
}
