//! Reading DuckDB's `EXPLAIN (FORMAT JSON)` output. Pure, so it is tested
//! without the engine. The JSON's key shape is undocumented, so anything
//! unexpected is an error rather than a guess (specs/ir-pipeline/07), and a
//! snapshot in the feature-gated tests pins the shape of a real plan.

use serde_json::{json, Map, Value};

use crate::claims::EngineReads;

/// The base-table reads of a whole plan.
pub fn reads_from_explain(plan: &Value) -> Result<EngineReads, String> {
    let mut reads = EngineReads::new();
    for node in roots(plan)? {
        collect_reads(node, &mut reads)?;
    }
    Ok(reads)
}

/// The plan's structure with every value dropped: operator names, the sorted
/// `extra_info` keys and the children. Cardinalities and join order vary with
/// the data; only this skeleton is pinned.
pub fn plan_shape(plan: &Value) -> Result<Value, String> {
    roots(plan)?
        .iter()
        .map(shape_node)
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn roots(plan: &Value) -> Result<&Vec<Value>, String> {
    plan.as_array()
        .ok_or_else(|| format!("EXPLAIN JSON root is not an array: {plan}"))
}

fn collect_reads(node: &Value, reads: &mut EngineReads) -> Result<(), String> {
    let name = node_name(node)?;
    let extra = extra_info(node)?;
    match extra.and_then(|info| info.get("Table")) {
        Some(table) => {
            let table = table
                .as_str()
                .ok_or_else(|| format!("{name}: Table is not a string"))?;
            let bare = table.rsplit('.').next().unwrap_or(table).to_lowercase();
            let read = reads.entry(bare).or_default();
            if let Some(projections) = extra.and_then(|info| info.get("Projections")) {
                read.columns
                    .extend(strings(projections)?.iter().map(|c| c.to_lowercase()));
            }
            if let Some(filters) = extra.and_then(|info| info.get("Filters")) {
                read.filter_text.extend(strings(filters)?);
            }
        }
        None if name == "SEQ_SCAN" => return Err("SEQ_SCAN without a Table key".to_owned()),
        None => {}
    }
    for child in children(node)? {
        collect_reads(child, reads)?;
    }
    Ok(())
}

fn shape_node(node: &Value) -> Result<Value, String> {
    let mut keys: Vec<&str> = extra_info(node)?
        .map(|info| info.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort_unstable();
    let shaped: Result<Vec<_>, _> = children(node)?.iter().map(shape_node).collect();
    Ok(json!({ "name": node_name(node)?, "extra_info_keys": keys, "children": shaped? }))
}

fn node_name(node: &Value) -> Result<&str, String> {
    node.get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or_else(|| format!("plan node without a name: {node}"))
}

fn children(node: &Value) -> Result<&Vec<Value>, String> {
    node.get("children")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("plan node without children: {node}"))
}

fn extra_info(node: &Value) -> Result<Option<&Map<String, Value>>, String> {
    match node.get("extra_info") {
        None => Ok(None),
        Some(Value::Object(info)) => Ok(Some(info)),
        Some(other) => Err(format!("extra_info is not an object: {other}")),
    }
}

/// DuckDB writes a one-element list as a bare string.
fn strings(value: &Value) -> Result<Vec<String>, String> {
    match value {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("expected a string, got {item}"))
            })
            .collect(),
        other => Err(format!("expected a string or a list, got {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(table: &str, projections: Value, filters: Option<&str>) -> Value {
        let mut info = json!({ "Table": table, "Projections": projections });
        if let Some(f) = filters {
            info["Filters"] = json!(f);
        }
        json!({ "name": "SEQ_SCAN", "children": [], "extra_info": info })
    }

    #[test]
    fn reads_scans_of_every_depth() {
        let plan = json!([{
            "name": "HASH_JOIN",
            "children": [
                scan("memory.main.orders", json!(["customer_id", "ID"]), Some("(status = 'paid')")),
                scan("memory.main.customers", json!("id"), None),
            ],
            "extra_info": { "Join Type": "INNER" },
        }]);
        let reads = reads_from_explain(&plan).expect("plan parses");
        assert!(reads["orders"].columns.contains("customer_id"));
        assert!(reads["orders"].columns.contains("id"));
        assert_eq!(reads["orders"].filter_text, ["(status = 'paid')"]);
        assert_eq!(reads["customers"].columns.len(), 1);
    }

    #[test]
    fn self_joins_accumulate() {
        let plan = json!([{
            "name": "HASH_JOIN",
            "children": [scan("t", json!("a"), None), scan("t", json!("b"), None)],
        }]);
        let reads = reads_from_explain(&plan).expect("plan parses");
        assert_eq!(reads["t"].columns.len(), 2);
    }

    #[test]
    fn unexpected_shapes_fail_loudly() {
        assert!(reads_from_explain(&json!({"name": "X"})).is_err());
        assert!(reads_from_explain(&json!([{ "children": [] }])).is_err());
        assert!(reads_from_explain(&json!([{ "name": "X" }])).is_err());
        assert!(reads_from_explain(&json!([{ "name": "SEQ_SCAN", "children": [] }])).is_err());
        let bad_projection = json!([scan("t", json!(1), None)]);
        assert!(reads_from_explain(&bad_projection).is_err());
    }

    #[test]
    fn shape_drops_values_and_sorts_keys() {
        let plan = json!([scan("t", json!("a"), Some("(a > 1)"))]);
        let shape = plan_shape(&plan).expect("plan parses");
        assert_eq!(
            shape,
            json!([{
                "name": "SEQ_SCAN",
                "extra_info_keys": ["Filters", "Projections", "Table"],
                "children": [],
            }])
        );
    }
}
