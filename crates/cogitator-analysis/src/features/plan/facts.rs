//! Raw facts into pipeline vocabulary: the free-form dialect string,
//! manifest config JSON, and catalog clustering text. Non-column
//! expressions yield nothing: an unknown key is not a wrong key.

use cogitator_dialects::Dialect;
use cogitator_ir::common::{Materialization, ModelConfig};
use cogitator_ir::lir::Backend;
use cogitator_ir::{Interner, Symbol};
use cogitator_parser::pipeline::sql_dialect;
use cogitator_sql_front::fold_schema_name;
use serde_json::Value;
use sqlparser::ast::{Expr, Function, FunctionArg, FunctionArgExpr, FunctionArguments};
use sqlparser::parser::Parser;

use crate::artifacts::manifest::ManifestConfig;

/// Date parts appear as bare identifiers in BigQuery trunc functions.
const DATE_PARTS: &[&str] = &[
    "year",
    "quarter",
    "month",
    "week",
    "day",
    "hour",
    "minute",
    "second",
    "millisecond",
    "microsecond",
];

pub(super) fn backend_for(dialect: &Dialect) -> Backend {
    match dialect.as_str() {
        "snowflake" => Backend::Snowflake,
        "bigquery" => Backend::BigQuery,
        "duckdb" => Backend::DuckDb,
        "postgres" => Backend::Postgres,
        _ => Backend::Generic,
    }
}

/// A raw `materialized` value; unknown strings are kept, not defaulted.
pub(super) fn materialization(raw: &str, syms: &mut Interner) -> Materialization {
    match raw.to_ascii_lowercase().as_str() {
        "table" => Materialization::Table,
        "view" => Materialization::View,
        "incremental" => Materialization::Incremental,
        "ephemeral" => Materialization::Ephemeral,
        "materialized_view" => Materialization::MaterializedView,
        "snapshot" => Materialization::Snapshot,
        _ => Materialization::Other(syms.intern(raw)),
    }
}

/// Columns of a raw `partition_by`: BigQuery's `{field: ...}`, or a
/// string or list on other adapters. Each may be an expression.
pub(super) fn partition_columns(value: &Value, dialect: &Dialect) -> Vec<String> {
    match value {
        Value::Object(map) => map
            .get("field")
            .and_then(Value::as_str)
            .map(|field| key_columns(field, dialect))
            .unwrap_or_default(),
        Value::String(text) => key_columns(text, dialect),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .flat_map(|item| key_columns(item, dialect))
            .collect(),
        _ => Vec::new(),
    }
}

/// Columns of raw clustering text: Snowflake's `LINEAR(a, b)` or
/// BigQuery's `a,b`.
pub(super) fn clustering_columns(raw: &str, dialect: &Dialect) -> Vec<String> {
    key_columns(raw, dialect)
}

/// The leading column of each raw `indexes` entry.
pub(super) fn index_columns(value: &Value) -> Vec<Vec<String>> {
    let Value::Array(items) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item.get("columns")? {
            Value::Array(cols) => Some(
                cols.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
            ),
            Value::String(col) => Some(vec![col.clone()]),
            _ => None,
        })
        .filter(|cols: &Vec<String>| !cols.is_empty())
        .collect()
}

/// Comma-separated key expressions, each reduced to its one column.
pub(super) fn key_columns(raw: &str, dialect: &Dialect) -> Vec<String> {
    let parsed = sql_dialect(dialect);
    let Ok(mut parser) = Parser::new(parsed.as_ref()).try_with_sql(raw) else {
        return Vec::new();
    };
    let Ok(exprs) = parser.parse_comma_separated(Parser::parse_expr) else {
        return Vec::new();
    };
    exprs
        .iter()
        .flat_map(|expr| match linear_args(expr) {
            Some(args) => args.iter().filter_map(|a| column_of(a)).collect(),
            None => column_of(expr).into_iter().collect::<Vec<_>>(),
        })
        .map(|name| fold_schema_name(&name, dialect).into_owned())
        .collect()
}

/// The arguments of `LINEAR(...)`, which lists keys rather than wraps one.
fn linear_args(expr: &Expr) -> Option<Vec<&Expr>> {
    let Expr::Function(function) = expr else {
        return None;
    };
    function
        .name
        .to_string()
        .eq_ignore_ascii_case("linear")
        .then(|| function_exprs(function))
}

fn function_exprs(function: &Function) -> Vec<&Expr> {
    let FunctionArguments::List(list) = &function.args else {
        return Vec::new();
    };
    list.args
        .iter()
        .filter_map(|arg| match arg {
            FunctionArg::Unnamed(FunctionArgExpr::Expr(expr))
            | FunctionArg::Named {
                arg: FunctionArgExpr::Expr(expr),
                ..
            }
            | FunctionArg::ExprNamed {
                arg: FunctionArgExpr::Expr(expr),
                ..
            } => Some(expr),
            _ => None,
        })
        .collect()
}

/// The single column a key expression reads, if it reads exactly one:
/// `date(ts)` and `timestamp_trunc(ts, day)` both partition on `ts`.
fn column_of(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(ident) => Some(ident.value.clone()),
        Expr::CompoundIdentifier(parts) => parts.last().map(|ident| ident.value.clone()),
        Expr::Nested(inner) => column_of(inner),
        Expr::Cast { expr, .. } => column_of(expr),
        Expr::Function(function) => {
            let mut columns = function_exprs(function)
                .into_iter()
                .filter(|arg| !is_date_part(arg))
                .filter_map(column_of);
            let first = columns.next()?;
            columns.next().is_none().then_some(first)
        }
        _ => None,
    }
}

fn is_date_part(expr: &Expr) -> bool {
    matches!(expr, Expr::Identifier(ident)
        if DATE_PARTS.contains(&ident.value.to_ascii_lowercase().as_str()))
}

/// Fills gaps in the template's own config from the manifest: `Hir.config`
/// is what the user wrote and wins.
pub(super) fn merge_manifest(
    config: &mut ModelConfig,
    syms: &mut Interner,
    manifest: Option<&ManifestConfig>,
    contract_enforced: bool,
    dialect: &Dialect,
) {
    config.contract_enforced |= contract_enforced;
    let Some(manifest) = manifest else {
        return;
    };
    if config.materialized.is_none() {
        config.materialized = manifest
            .materialized
            .as_deref()
            .map(|raw| materialization(raw, syms));
    }
    fill(&mut config.unique_key, syms, || {
        manifest
            .unique_key
            .iter()
            .map(|k| fold_schema_name(k, dialect).into_owned())
            .collect()
    });
    fill(&mut config.partition_by, syms, || {
        manifest
            .partition_by
            .as_ref()
            .map(|raw| partition_columns(raw, dialect))
            .unwrap_or_default()
    });
    fill(&mut config.cluster_by, syms, || {
        manifest
            .cluster_by
            .iter()
            .flat_map(|key| key_columns(key, dialect))
            .collect()
    });
    if config.incremental_strategy.is_none() {
        config.incremental_strategy = manifest
            .incremental_strategy
            .as_deref()
            .map(|raw| syms.intern(raw));
    }
    if config.indexes.is_empty() {
        if let Some(raw) = &manifest.indexes {
            config.indexes = index_columns(raw)
                .into_iter()
                .map(|cols| {
                    cols.iter()
                        .map(|c| syms.intern(&fold_schema_name(c, dialect)))
                        .collect()
                })
                .collect();
        }
    }
}

fn fill(slot: &mut Vec<Symbol>, syms: &mut Interner, names: impl FnOnce() -> Vec<String>) {
    if slot.is_empty() {
        *slot = names().iter().map(|name| syms.intern(name)).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn duckdb() -> Dialect {
        Dialect::new("duckdb")
    }

    #[test]
    fn backends_from_the_free_form_dialect() {
        assert_eq!(backend_for(&Dialect::new("bigquery")), Backend::BigQuery);
        assert_eq!(backend_for(&Dialect::new("redshift")), Backend::Generic);
        assert_eq!(backend_for(&Dialect::new("")), Backend::Generic);
    }

    #[test]
    fn bigquery_partition_dict_reads_its_field() {
        let value = json!({"field": "created_at", "data_type": "timestamp"});
        assert_eq!(
            partition_columns(&value, &Dialect::new("bigquery")),
            vec!["created_at"]
        );
    }

    #[test]
    fn partition_expressions_reduce_to_their_column() {
        assert_eq!(partition_columns(&json!("date(ts)"), &duckdb()), vec!["ts"]);
        assert_eq!(
            partition_columns(&json!(["TIMESTAMP_TRUNC(ts, day)"]), &duckdb()),
            vec!["ts"]
        );
        assert_eq!(
            partition_columns(&json!("date_trunc('month', order_date)"), &duckdb()),
            vec!["order_date"]
        );
    }

    #[test]
    fn multi_column_expressions_are_not_a_key() {
        assert!(partition_columns(&json!("coalesce(a, b)"), &duckdb()).is_empty());
        assert!(partition_columns(&json!(3), &duckdb()).is_empty());
        assert!(partition_columns(&json!({"data_type": "date"}), &duckdb()).is_empty());
    }

    #[test]
    fn unparseable_expressions_yield_nothing() {
        assert!(key_columns("date(", &duckdb()).is_empty());
    }

    #[test]
    fn snowflake_clustering_text() {
        let snowflake = Dialect::new("snowflake");
        assert_eq!(
            clustering_columns("LINEAR(a, to_date(b))", &snowflake),
            vec!["A", "B"]
        );
        assert_eq!(clustering_columns("a,b", &duckdb()), vec!["a", "b"]);
    }

    #[test]
    fn index_entries_keep_their_columns_in_order() {
        let value = json!([{"columns": ["x", "y"]}, {"columns": "z"}, {"unique": true}]);
        assert_eq!(
            index_columns(&value),
            vec![
                vec!["x".to_string(), "y".to_string()],
                vec!["z".to_string()]
            ]
        );
    }

    #[test]
    fn the_templates_config_wins_over_the_manifest() {
        let mut syms = Interner::new();
        let kept = syms.intern("kept");
        let mut config = ModelConfig {
            unique_key: vec![kept],
            ..ModelConfig::default()
        };
        let manifest = ManifestConfig {
            materialized: Some("incremental".to_string()),
            unique_key: vec!["other".to_string()],
            partition_by: Some(json!({"field": "day"})),
            ..ManifestConfig::default()
        };
        merge_manifest(&mut config, &mut syms, Some(&manifest), true, &duckdb());
        assert_eq!(config.unique_key, vec![kept], "the template wins per key");
        assert_eq!(config.materialized, Some(Materialization::Incremental));
        assert_eq!(syms.text(config.partition_by[0]), "day");
        assert!(config.contract_enforced);
    }

    #[test]
    fn unknown_materializations_are_kept() {
        let mut syms = Interner::new();
        match materialization("custom", &mut syms) {
            Materialization::Other(sym) => assert_eq!(syms.text(sym), "custom"),
            other => panic!("{other:?}"),
        }
        assert_eq!(materialization("TABLE", &mut syms), Materialization::Table);
    }
}
