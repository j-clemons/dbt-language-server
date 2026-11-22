//! A model's `config` object from manifest.json. Its shape varies by adapter
//! and dbt version, so it arrives as a `serde_json::Value`; anything unexpected
//! degrades the one field, never the manifest.

use serde_json::{Map, Value};

/// The config keys the optimiser reads, as the manifest holds them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManifestConfig {
    pub materialized: Option<String>,
    /// dbt accepts one column or a list; both read as a list.
    pub unique_key: Vec<String>,
    pub incremental_strategy: Option<String>,
    /// The raw value: a dict on BigQuery (`{field, data_type, granularity}`),
    /// a string or list on other adapters. `None` when absent or null.
    pub partition_by: Option<Value>,
    /// dbt accepts one column or a list; both read as a list.
    pub cluster_by: Vec<String>,
    /// The raw value, normally a list of `{columns, unique, type}` dicts.
    /// `None` when absent or null.
    pub indexes: Option<Value>,
}

pub(super) fn parse_config(config: Value) -> ManifestConfig {
    let Value::Object(mut map) = config else {
        return ManifestConfig::default();
    };
    ManifestConfig {
        materialized: text(&mut map, "materialized"),
        unique_key: keys(&mut map, "unique_key"),
        incremental_strategy: text(&mut map, "incremental_strategy"),
        partition_by: raw(&mut map, "partition_by"),
        cluster_by: keys(&mut map, "cluster_by"),
        indexes: raw(&mut map, "indexes"),
    }
}

fn raw(map: &mut Map<String, Value>, key: &str) -> Option<Value> {
    map.remove(key).filter(|value| !value.is_null())
}

fn text(map: &mut Map<String, Value>, key: &str) -> Option<String> {
    match raw(map, key)? {
        Value::String(text) if !text.trim().is_empty() => Some(text),
        _ => None,
    }
}

fn keys(map: &mut Map<String, Value>, key: &str) -> Vec<String> {
    let as_key = |value: Value| match value {
        Value::String(text) if !text.trim().is_empty() => Some(text),
        _ => None,
    };
    match raw(map, key) {
        Some(Value::Array(items)) => items.into_iter().filter_map(as_key).collect(),
        Some(value) => as_key(value).into_iter().collect(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_every_key() {
        let config = parse_config(json!({
            "materialized": "incremental",
            "unique_key": "id",
            "incremental_strategy": "merge",
            "partition_by": {"field": "ts", "data_type": "date"},
            "cluster_by": ["a", "b"],
            "indexes": [{"columns": ["a"], "unique": true}],
            "alias": null,
        }));
        assert_eq!(config.materialized.as_deref(), Some("incremental"));
        assert_eq!(config.unique_key, ["id"]);
        assert_eq!(config.incremental_strategy.as_deref(), Some("merge"));
        assert_eq!(
            config.partition_by,
            Some(json!({"field": "ts", "data_type": "date"}))
        );
        assert_eq!(config.cluster_by, ["a", "b"]);
        assert_eq!(
            config.indexes,
            Some(json!([{"columns": ["a"], "unique": true}]))
        );
    }

    #[test]
    fn list_and_string_forms_both_read_as_lists() {
        let config = parse_config(json!({"unique_key": ["a", "b"], "cluster_by": "c"}));
        assert_eq!(config.unique_key, ["a", "b"]);
        assert_eq!(config.cluster_by, ["c"]);
    }

    #[test]
    fn partition_by_keeps_whatever_shape_it_has() {
        for value in [json!("date(ts)"), json!(["a", "b"]), json!({"field": "ts"})] {
            let config = parse_config(json!({"partition_by": value.clone()}));
            assert_eq!(config.partition_by, Some(value));
        }
    }

    #[test]
    fn missing_and_null_read_as_absent() {
        let expected = ManifestConfig::default();
        assert_eq!(parse_config(json!({})), expected);
        let nulls = parse_config(json!({
            "materialized": null, "unique_key": null, "incremental_strategy": null,
            "partition_by": null, "cluster_by": null, "indexes": null,
        }));
        assert_eq!(nulls, expected);
    }

    #[test]
    fn malformed_values_degrade_one_field() {
        let config = parse_config(json!({
            "materialized": 3,
            "unique_key": {"a": 1},
            "incremental_strategy": ["merge"],
            "cluster_by": [1, "kept", null, ""],
            "partition_by": "ts",
        }));
        assert_eq!(config.materialized, None);
        assert!(config.unique_key.is_empty());
        assert_eq!(config.incremental_strategy, None);
        assert_eq!(config.cluster_by, ["kept"]);
        assert_eq!(config.partition_by, Some(json!("ts")));
    }

    #[test]
    fn a_non_object_config_is_empty() {
        for raw in [json!(null), json!("table"), json!([1]), json!(3)] {
            assert_eq!(parse_config(raw), ManifestConfig::default());
        }
    }
}
