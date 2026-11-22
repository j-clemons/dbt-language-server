//! Static `config(...)` arguments into [`ModelConfig`]. Non-constant
//! arguments are left alone: the value was not knowable, not wrong.

use cogitator_ir::common::{Materialization, ModelConfig};
use cogitator_ir::{Interner, Symbol};

use crate::dialect::DialectRules;
use crate::jinja::Raw;

/// Applies one `config(...)` call over `base`; later calls win per key.
pub(crate) fn apply_config(
    base: &mut ModelConfig,
    kwargs: &[(String, Raw)],
    syms: &mut Interner,
    rules: &DialectRules,
) {
    for (key, value) in kwargs {
        match key.as_str() {
            "materialized" => {
                if let Raw::Str(s) = value {
                    base.materialized = Some(materialization(s, syms));
                }
            }
            "unique_key" => replace(&mut base.unique_key, columns(value, syms, rules)),
            "partition_by" => replace(
                &mut base.partition_by,
                partition_columns(value, syms, rules),
            ),
            "cluster_by" => replace(&mut base.cluster_by, columns(value, syms, rules)),
            "incremental_strategy" => {
                if let Raw::Str(s) = value {
                    base.incremental_strategy = Some(syms.intern(s));
                }
            }
            "indexes" => {
                let indexes = index_columns(value, syms, rules);
                if !indexes.is_empty() {
                    base.indexes = indexes;
                }
            }
            "contract" => {
                if let Raw::Map(entries) = value {
                    if let Some((_, Raw::Bool(b))) = entries.iter().find(|(k, _)| k == "enforced") {
                        base.contract_enforced = *b;
                    }
                }
            }
            _ => {}
        }
    }
}

fn replace(slot: &mut Vec<Symbol>, value: Vec<Symbol>) {
    if !value.is_empty() {
        *slot = value;
    }
}

fn materialization(s: &str, syms: &mut Interner) -> Materialization {
    match s {
        "table" => Materialization::Table,
        "view" => Materialization::View,
        "incremental" => Materialization::Incremental,
        "ephemeral" => Materialization::Ephemeral,
        "materialized_view" => Materialization::MaterializedView,
        "snapshot" => Materialization::Snapshot,
        other => Materialization::Other(syms.intern(other)),
    }
}

/// A column name or a list of them.
fn columns(value: &Raw, syms: &mut Interner, rules: &DialectRules) -> Vec<Symbol> {
    match value {
        Raw::Str(s) => vec![syms.intern(&rules.fold_name(s))],
        Raw::List(items) => items
            .iter()
            .filter_map(|item| match item {
                Raw::Str(s) => Some(syms.intern(&rules.fold_name(s))),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// BigQuery's `{'field': 'x', ...}` or a plain column list.
fn partition_columns(value: &Raw, syms: &mut Interner, rules: &DialectRules) -> Vec<Symbol> {
    match value {
        Raw::Map(entries) => entries
            .iter()
            .find(|(k, _)| k == "field")
            .map(|(_, field)| columns(field, syms, rules))
            .unwrap_or_default(),
        other => columns(other, syms, rules),
    }
}

/// Postgres indexes: `[{'columns': ['a', 'b'], 'unique': true}]`.
fn index_columns(value: &Raw, syms: &mut Interner, rules: &DialectRules) -> Vec<Vec<Symbol>> {
    let Raw::List(items) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Raw::Map(entries) => entries
                .iter()
                .find(|(k, _)| k == "columns")
                .map(|(_, cols)| columns(cols, syms, rules)),
            _ => None,
        })
        .filter(|cols| !cols.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jinja::config_kwargs;
    use cogitator_dialects::Dialect;

    fn config_of(inner: &str, dialect: &str) -> (ModelConfig, Interner) {
        let mut base = ModelConfig::default();
        let mut syms = Interner::new();
        let rules = DialectRules::for_dialect(&Dialect::new(dialect));
        apply_config(&mut base, &config_kwargs(inner), &mut syms, &rules);
        (base, syms)
    }

    fn names(syms: &Interner, values: &[Symbol]) -> Vec<String> {
        values.iter().map(|s| syms.text(*s).to_string()).collect()
    }

    #[test]
    fn full_config() {
        let (cfg, syms) = config_of(
            "config(materialized='incremental', unique_key='ID', incremental_strategy='merge', \
             partition_by={'field': 'day', 'data_type': 'date'}, cluster_by=['a', 'b'], \
             indexes=[{'columns': ['x', 'y']}, {'columns': ['z']}], contract={'enforced': true})",
            "postgres",
        );
        assert_eq!(cfg.materialized, Some(Materialization::Incremental));
        assert_eq!(names(&syms, &cfg.unique_key), vec!["id"]);
        assert_eq!(names(&syms, &cfg.partition_by), vec!["day"]);
        assert_eq!(names(&syms, &cfg.cluster_by), vec!["a", "b"]);
        assert_eq!(cfg.indexes.len(), 2);
        assert_eq!(names(&syms, &cfg.indexes[0]), vec!["x", "y"]);
        assert!(cfg.contract_enforced);
        assert_eq!(
            syms.text(cfg.incremental_strategy.expect("strategy")),
            "merge"
        );
    }

    #[test]
    fn non_constant_values_are_skipped() {
        let (cfg, _) = config_of("config(materialized=var('m'), unique_key=x)", "duckdb");
        assert_eq!(cfg.materialized, None);
        assert!(cfg.unique_key.is_empty());
    }

    #[test]
    fn folding_applies_to_columns() {
        let (cfg, syms) = config_of("config(unique_key='id')", "snowflake");
        assert_eq!(names(&syms, &cfg.unique_key), vec!["ID"]);
    }

    #[test]
    fn unknown_materializations_are_kept() {
        let (cfg, syms) = config_of("config(materialized='custom_mat')", "duckdb");
        match cfg.materialized {
            Some(Materialization::Other(s)) => assert_eq!(syms.text(s), "custom_mat"),
            other => panic!("{other:?}"),
        }
    }
}
