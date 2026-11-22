//! `PhysicalFacts` from the manifest and catalog, one entry per table
//! the plan scans. A table with no known key or row count gets no entry,
//! so backends report `Unknown` for it rather than `NotFiltered`.

use cogitator_ir::common::ModelConfig;
use cogitator_ir::lir::{KeyKind, PhysColumn, PhysicalFacts, TableFacts};
use cogitator_ir::mir::{Mir, TableRef};
use cogitator_ir::schema::{ModelSchema, ProjectSchemas};
use cogitator_ir::{Interner, SqlType, Symbol};

use crate::artifacts::catalog::CatalogTable;
use crate::artifacts::manifest::ManifestConfig;

use super::facts::{clustering_columns, index_columns, key_columns, partition_columns};
use super::PlanInput;

/// The facts built, and what they support.
#[derive(Debug, Default)]
pub(super) struct BuiltPhys {
    pub(super) facts: PhysicalFacts,
    /// A table has a key the backend can use, so `Need::Phys` holds.
    pub(super) has_keys: bool,
    /// A table has a row count, so `Need::Stats` holds.
    pub(super) has_stats: bool,
}

/// Keys in configured order, which backends read as `CLUSTER BY` order.
/// A column has one key, the first found, except that a partition column
/// can also be clustered: it then takes its place in the cluster order too.
#[derive(Default)]
struct Keys(Vec<(String, KeyKind)>);

impl Keys {
    fn add(&mut self, columns: impl IntoIterator<Item = String>, kind: KeyKind) {
        for column in columns {
            let taken = self.0.iter().any(|(name, existing)| {
                *name == column && !(*existing == KeyKind::Partition && kind == KeyKind::Cluster)
            });
            if !taken {
                self.0.push((column, kind));
            }
        }
    }

    fn from_manifest(config: &ManifestConfig, input: &PlanInput<'_>) -> Self {
        let mut keys = Keys::default();
        if let Some(raw) = &config.partition_by {
            keys.add(partition_columns(raw, input.dialect), KeyKind::Partition);
        }
        keys.add(
            config
                .cluster_by
                .iter()
                .flat_map(|key| key_columns(key, input.dialect)),
            KeyKind::Cluster,
        );
        if let Some(raw) = &config.indexes {
            keys.add(
                index_columns(raw)
                    .into_iter()
                    .filter_map(|cols| cols.into_iter().next())
                    .flat_map(|head| key_columns(&head, input.dialect)),
                KeyKind::Sort,
            );
        }
        keys
    }

    fn from_config(config: &ModelConfig, syms: &Interner) -> Self {
        let text = |list: &[Symbol]| {
            list.iter()
                .map(|s| syms.text(*s).to_string())
                .collect::<Vec<_>>()
        };
        let mut keys = Keys::default();
        keys.add(text(&config.partition_by), KeyKind::Partition);
        keys.add(text(&config.cluster_by), KeyKind::Cluster);
        keys.add(
            config
                .indexes
                .iter()
                .filter_map(|index| index.first())
                .map(|s| syms.text(*s).to_string()),
            KeyKind::Sort,
        );
        keys
    }
}

/// One entry per scanned table with anything to say. Interns into the
/// plan's own table so the names compare equal to scan columns.
pub(super) fn physical_facts(
    mir: &mut Mir,
    schemas: &ProjectSchemas,
    usable: &[KeyKind],
    model: Option<&str>,
    input: &PlanInput<'_>,
) -> BuiltPhys {
    let mut built = BuiltPhys::default();
    let tables: Vec<(u32, TableRef)> = mir
        .tables
        .iter()
        .map(|(id, def)| (id.raw(), def.target.clone()))
        .collect();
    for (id, target) in tables {
        let (mut keys, schema, catalog) = match target {
            TableRef::Model { name, .. } => {
                let name = mir.syms.text(name).to_string();
                let keys = input
                    .manifest
                    .and_then(|m| m.models.get(&name))
                    .map_or_else(Keys::default, |n| Keys::from_manifest(&n.config, input));
                let catalog = input.catalog.and_then(|c| c.models.get(&name));
                (keys, schemas.model(&name).cloned(), catalog)
            }
            TableRef::Source { source, table } => {
                let (source, table) = (
                    mir.syms.text(source).to_string(),
                    mir.syms.text(table).to_string(),
                );
                let schema = schemas.source(&source, &table).cloned();
                let catalog = input.catalog.and_then(|c| c.sources.get(&(source, table)));
                (Keys::default(), schema, catalog)
            }
            TableRef::This => {
                let keys = Keys::from_config(&mir.config, &mir.syms);
                let schema = model.and_then(|m| schemas.model(m)).cloned();
                let catalog = model.and_then(|m| input.catalog?.models.get(m));
                (keys, schema, catalog)
            }
            TableRef::Physical(_) | TableRef::Jinja => continue,
        };
        add_catalog_keys(&mut keys, catalog, input);
        // perf.catalog_stats off: the rows are never seen as a fact
        let row_count = catalog
            .filter(|_| input.catalog_stats)
            .and_then(|c| c.stats.row_count);
        let columns = phys_columns(&keys, schema.as_deref(), &mut mir.syms);
        if columns.is_empty() && row_count.is_none() {
            continue;
        }
        built.has_keys |= columns
            .iter()
            .any(|c| c.key.is_some_and(|k| usable.contains(&k)));
        built.has_stats |= row_count.is_some();
        built
            .facts
            .tables
            .insert(id, TableFacts { columns, row_count });
    }
    built
}

fn add_catalog_keys(keys: &mut Keys, catalog: Option<&CatalogTable>, input: &PlanInput<'_>) {
    if let Some(raw) = catalog.and_then(|c| c.stats.clustering.as_deref()) {
        keys.add(clustering_columns(raw, input.dialect), KeyKind::Cluster);
    }
}

fn phys_columns(keys: &Keys, schema: Option<&ModelSchema>, syms: &mut Interner) -> Vec<PhysColumn> {
    keys.0
        .iter()
        .map(|(name, kind)| PhysColumn {
            name: syms.intern(name),
            ty: column_type(schema, name),
            key: Some(*kind),
        })
        .collect()
}

fn column_type(schema: Option<&ModelSchema>, name: &str) -> SqlType {
    schema
        .and_then(|s| s.columns.iter().find(|c| c.name.eq_ignore_ascii_case(name)))
        .map_or(SqlType::Unknown, |c| c.ty.ty.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn keys_keep_their_configured_cluster_order() {
        let mut keys = Keys::default();
        keys.add(names(&["b", "a"]), KeyKind::Cluster);
        let order: Vec<&str> = keys.0.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(order, ["b", "a"]);
    }

    #[test]
    fn a_partition_column_also_takes_its_cluster_place() {
        let mut keys = Keys::default();
        keys.add(names(&["ts"]), KeyKind::Partition);
        keys.add(names(&["ts", "a"]), KeyKind::Cluster);
        keys.add(names(&["ts"]), KeyKind::Cluster);
        assert_eq!(
            keys.0,
            [
                ("ts".to_string(), KeyKind::Partition),
                ("ts".to_string(), KeyKind::Cluster),
                ("a".to_string(), KeyKind::Cluster),
            ]
        );
    }

    #[test]
    fn other_second_keys_are_ignored() {
        let mut keys = Keys::default();
        keys.add(names(&["a"]), KeyKind::Cluster);
        keys.add(names(&["a"]), KeyKind::Sort);
        assert_eq!(keys.0, [("a".to_string(), KeyKind::Cluster)]);
    }
}
