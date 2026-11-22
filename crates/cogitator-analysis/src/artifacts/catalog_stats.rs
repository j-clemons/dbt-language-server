//! The `stats` object of one catalog.json table. dbt writes
//! `{"<id>": {"id", "label", "value", "include", "description"}}` with
//! adapter-specific ids; a value of an unexpected type reads as absent.

use serde_json::Value;

/// The table facts the optimiser can use, under adapter-neutral names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogStats {
    /// `row_count` (Snowflake) or `num_rows` (BigQuery).
    pub row_count: Option<u64>,
    /// `bytes` (Snowflake) or `num_bytes` (BigQuery).
    pub byte_count: Option<u64>,
    /// `partitioning_type` (BigQuery), as written.
    pub partitioning: Option<String>,
    /// `clustering_key` (Snowflake, e.g. `LINEAR(a, b)`) or
    /// `clustering_fields` (BigQuery), as written.
    pub clustering: Option<String>,
}

pub(super) fn parse_stats(stats: &Value) -> CatalogStats {
    CatalogStats {
        row_count: count(stats, &["row_count", "num_rows"]),
        byte_count: count(stats, &["bytes", "num_bytes"]),
        partitioning: text(stats, &["partitioning_type"]),
        clustering: text(stats, &["clustering_key", "clustering_fields"]),
    }
}

fn stat<'a>(stats: &'a Value, id: &str) -> Option<&'a Value> {
    stats.get(id)?.get("value")
}

fn count(stats: &Value, ids: &[&str]) -> Option<u64> {
    ids.iter().find_map(|id| stat(stats, id).and_then(as_count))
}

/// Text owned by the `CatalogIndex` snapshot (08 §1.5).
fn text(stats: &Value, ids: &[&str]) -> Option<String> {
    ids.iter().find_map(|id| {
        stat(stats, id)?
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    })
}

/// A whole, non-negative count. Warehouses may hand these over as floats
/// (`1200.0`) or numeric strings.
fn as_count(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64().or_else(|| number.as_f64().and_then(whole)),
        Value::String(text) => {
            let text = text.trim();
            text.parse::<u64>()
                .ok()
                .or_else(|| text.parse::<f64>().ok().and_then(whole))
        }
        _ => None,
    }
}

fn whole(value: f64) -> Option<u64> {
    // 2^64 is the first f64 past u64::MAX; the range also rejects NaN and inf
    let fits = (0.0..18_446_744_073_709_551_616.0).contains(&value);
    (fits && value.fract() == 0.0).then_some(value as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(value: Value) -> Value {
        json!({"id": "x", "label": "X", "value": value, "include": true})
    }

    #[test]
    fn reads_snowflake_ids() {
        let stats = parse_stats(&json!({
            "row_count": entry(json!(1200)),
            "bytes": entry(json!(4096)),
            "clustering_key": entry(json!("LINEAR(a, b)")),
            "has_stats": {"id": "has_stats", "value": true, "include": false},
        }));
        assert_eq!(stats.row_count, Some(1200));
        assert_eq!(stats.byte_count, Some(4096));
        assert_eq!(stats.clustering.as_deref(), Some("LINEAR(a, b)"));
        assert_eq!(stats.partitioning, None);
    }

    #[test]
    fn reads_bigquery_ids() {
        let stats = parse_stats(&json!({
            "num_rows": entry(json!(7)),
            "num_bytes": entry(json!(70)),
            "partitioning_type": entry(json!("DAY")),
            "clustering_fields": entry(json!("a, b")),
        }));
        assert_eq!(stats.row_count, Some(7));
        assert_eq!(stats.byte_count, Some(70));
        assert_eq!(stats.partitioning.as_deref(), Some("DAY"));
        assert_eq!(stats.clustering.as_deref(), Some("a, b"));
    }

    #[test]
    fn counts_accept_floats_and_numeric_strings() {
        for value in [
            json!(1200.0),
            json!("1200"),
            json!(" 1200 "),
            json!("1200.0"),
        ] {
            let stats = parse_stats(&json!({"row_count": entry(value.clone())}));
            assert_eq!(stats.row_count, Some(1200), "{value}");
        }
        let empty = parse_stats(&json!({"row_count": entry(json!(0))}));
        assert_eq!(empty.row_count, Some(0), "an empty table is a real count");
    }

    #[test]
    fn malformed_counts_read_as_absent() {
        for value in [
            json!(null),
            json!(true),
            json!(-1),
            json!(1.5),
            json!(1e30),
            json!("many"),
            json!("NaN"),
            json!([1]),
            json!({"n": 1}),
        ] {
            let stats = parse_stats(&json!({"row_count": entry(value.clone())}));
            assert_eq!(stats.row_count, None, "{value}");
        }
    }

    #[test]
    fn malformed_text_reads_as_absent() {
        for value in [json!(null), json!(3), json!(""), json!("  "), json!(["a"])] {
            let stats = parse_stats(&json!({"clustering_key": entry(value.clone())}));
            assert_eq!(stats.clustering, None, "{value}");
        }
    }

    #[test]
    fn a_malformed_id_falls_through_to_the_other_spelling() {
        let stats = parse_stats(&json!({
            "row_count": entry(json!("unknown")),
            "num_rows": entry(json!(5)),
        }));
        assert_eq!(stats.row_count, Some(5));
    }

    #[test]
    fn unexpected_shapes_yield_empty_stats() {
        for raw in [
            json!(null),
            json!("stats"),
            json!([1, 2]),
            json!({}),
            json!({"row_count": 9}),
            json!({"row_count": {"id": "row_count"}}),
        ] {
            assert_eq!(parse_stats(&raw), CatalogStats::default(), "{raw}");
        }
    }
}
