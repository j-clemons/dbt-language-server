//! Assembles a `ModelSchema` from what is known: SQL inference, catalog
//! types and YAML. Column lookups are by name through hash maps, which are
//! never iterated, so column order always comes from the leading source.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::schema::{Completeness, ModelSchema, SchemaCol};
use cogitator_ir::{ColType, Nullability, SqlType};
use cogitator_schemas::ColumnEntry;
use cogitator_sql_front::parse_sql_type;

use super::infer::Inferred;
use crate::artifacts::catalog::{CatalogColumn, CatalogTable};

pub(super) struct BuiltSchema {
    pub(super) schema: ModelSchema,
    pub(super) capped: bool,
}

/// SQL inference when there is one; otherwise whatever YAML and the
/// catalog list, which is never a complete list.
pub(super) fn model_schema(
    inferred: Option<&Inferred>,
    yaml: &[ColumnEntry],
    catalog: Option<&CatalogTable>,
    dialect: &Dialect,
    cap: usize,
) -> Option<BuiltSchema> {
    let catalog_cols = catalog.map_or(&[][..], |c| c.columns.as_slice());
    let Some(inferred) = inferred else {
        return listed(yaml, catalog_cols, dialect, cap, Completeness::Open);
    };
    let by_catalog = catalog_index(catalog_cols);
    let by_yaml = yaml_index(yaml);
    let mut columns: Vec<SchemaCol> = Vec::with_capacity(inferred.columns.len());
    for col in &inferred.columns {
        let key = col.name.to_lowercase();
        let mut ty = col.ty.clone(); // boundary: the schema owns its types
        if ty.ty == SqlType::Unknown {
            if let Some(found) = by_catalog.get(&key) {
                ty.ty = parse_sql_type(&found.data_type, dialect);
            }
        }
        let entry = by_yaml.get(&key).copied();
        if ty.ty == SqlType::Unknown {
            if let Some(raw) = entry.and_then(|e| e.data_type.as_deref()) {
                ty.ty = parse_sql_type(raw, dialect);
            }
        }
        columns.push(with_tests(col.name.clone(), ty, entry));
    }
    let completeness = if inferred.open {
        append_listed(&mut columns, yaml, catalog_cols, dialect);
        Completeness::Open
    } else {
        Completeness::Closed
    };
    Some(finish(columns, completeness, cap))
}

/// Catalog columns are the whole list; YAML adds tests and type gaps but
/// cannot close a source on its own.
pub(super) fn source_schema(
    yaml: &[ColumnEntry],
    catalog: Option<&CatalogTable>,
    dialect: &Dialect,
    cap: usize,
) -> Option<BuiltSchema> {
    let catalog_cols = catalog.map_or(&[][..], |c| c.columns.as_slice());
    let completeness = if catalog_cols.is_empty() {
        Completeness::Open
    } else {
        Completeness::Closed
    };
    listed(yaml, catalog_cols, dialect, cap, completeness)
}

/// Catalog columns first, in warehouse order, then YAML-only columns.
fn listed(
    yaml: &[ColumnEntry],
    catalog: &[CatalogColumn],
    dialect: &Dialect,
    cap: usize,
    completeness: Completeness,
) -> Option<BuiltSchema> {
    if yaml.is_empty() && catalog.is_empty() {
        return None;
    }
    let by_yaml = yaml_index(yaml);
    let mut columns: Vec<SchemaCol> = catalog
        .iter()
        .map(|c| {
            let entry = by_yaml.get(&c.name.to_lowercase()).copied();
            let mut ty = parse_sql_type(&c.data_type, dialect);
            if ty == SqlType::Unknown {
                if let Some(raw) = entry.and_then(|e| e.data_type.as_deref()) {
                    ty = parse_sql_type(raw, dialect);
                }
            }
            with_tests(
                Arc::from(c.name.as_str()),
                ColType::new(ty, Nullability::Unknown),
                entry,
            )
        })
        .collect();
    if completeness == Completeness::Open {
        append_listed(&mut columns, yaml, &[], dialect);
    }
    Some(finish(columns, completeness, cap))
}

/// Appends listed columns not already present, for an `Open` schema: extra
/// names are evidence of columns, and an open list claims no completeness.
fn append_listed(
    columns: &mut Vec<SchemaCol>,
    yaml: &[ColumnEntry],
    catalog: &[CatalogColumn],
    dialect: &Dialect,
) {
    let mut seen: std::collections::HashSet<String> =
        columns.iter().map(|c| c.name.to_lowercase()).collect();
    for column in catalog {
        if seen.insert(column.name.to_lowercase()) {
            columns.push(SchemaCol {
                name: Arc::from(column.name.as_str()),
                ty: ColType::new(
                    parse_sql_type(&column.data_type, dialect),
                    Nullability::Unknown,
                ),
                unique: false,
            });
        }
    }
    for entry in yaml {
        if seen.insert(entry.name.to_lowercase()) {
            let ty = entry
                .data_type
                .as_deref()
                .map_or(SqlType::Unknown, |raw| parse_sql_type(raw, dialect));
            columns.push(with_tests(
                Arc::from(entry.name.as_str()),
                ColType::new(ty, Nullability::Unknown),
                Some(entry),
            ));
        }
    }
}

/// `unique` marks the column; `not_null` makes it `NotNull`.
pub(in crate::features::plan) fn with_tests(
    name: Arc<str>,
    mut ty: ColType,
    entry: Option<&ColumnEntry>,
) -> SchemaCol {
    let unique = entry.is_some_and(|e| e.proven_by("unique"));
    if entry.is_some_and(|e| e.proven_by("not_null")) {
        ty.null = Nullability::NotNull;
    }
    SchemaCol { name, ty, unique }
}

fn catalog_index(columns: &[CatalogColumn]) -> HashMap<String, &CatalogColumn> {
    columns.iter().map(|c| (c.name.to_lowercase(), c)).collect()
}

fn yaml_index(yaml: &[ColumnEntry]) -> HashMap<String, &ColumnEntry> {
    yaml.iter().map(|e| (e.name.to_lowercase(), e)).collect()
}

fn finish(mut columns: Vec<SchemaCol>, completeness: Completeness, cap: usize) -> BuiltSchema {
    let capped = columns.len() > cap;
    if capped {
        columns.truncate(cap);
    }
    let completeness = if capped {
        Completeness::Open
    } else {
        completeness
    };
    let mut hasher = DefaultHasher::new();
    columns.hash(&mut hasher);
    completeness.hash(&mut hasher);
    BuiltSchema {
        schema: ModelSchema {
            columns: Arc::from(columns),
            completeness,
            fingerprint: hasher.finish(),
        },
        capped,
    }
}
