//! The `columns:` list of a model or source table: generated struct fields
//! joined positionally with what only the marked tree holds (08 §2.4).

use cogitator_lsp::Position;

use super::{name_position, ColumnEntry, ColumnTest};
use crate::generated::dbt_yml_files::ColumnProperties;
use crate::yaml;

/// What the marked tree contributes to one column.
#[derive(Debug, Clone, Default)]
pub(super) struct ColumnSide {
    name_pos: Position,
    data_type: Option<String>,
    tests: Vec<ColumnTest>,
}

pub(super) fn columns_of(columns: &[ColumnProperties], sides: &[ColumnSide]) -> Vec<ColumnEntry> {
    columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let side = sides.get(i).cloned().unwrap_or_default();
            ColumnEntry {
                name: c.name.clone(),
                description: c.description.clone().unwrap_or_default(),
                name_pos: side.name_pos,
                data_type: side.data_type,
                tests: side.tests,
            }
        })
        .collect()
}

/// One `ColumnSide` per `columns[j]` of a model/table entry. `data_type`
/// comes from the tree rather than the generated type, so a non-string
/// value reads as absent instead of rejecting the file.
pub(super) fn column_sides(entry: &saphyr::MarkedYaml<'_>) -> Vec<ColumnSide> {
    yaml::get(entry, "columns")
        .map(|columns| {
            yaml::items(columns)
                .map(|column| ColumnSide {
                    name_pos: name_position(column),
                    data_type: yaml::get(column, "data_type")
                        .and_then(yaml::as_str)
                        .filter(|text| !text.trim().is_empty())
                        .map(str::to_string),
                    tests: column_tests(column),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `tests:` and `data_tests:` of one column, tolerating any malformed
/// entry by skipping it: tests are evidence, never required.
fn column_tests(column: &saphyr::MarkedYaml<'_>) -> Vec<ColumnTest> {
    ["tests", "data_tests"]
        .iter()
        .filter_map(|key| yaml::get(column, key))
        .flat_map(yaml::items)
        .filter_map(column_test)
        .collect()
}

/// `- unique`, `- not_null: {config: {where: ...}}`, or the long form
/// `- name: x, test_name: unique`.
fn column_test(item: &saphyr::MarkedYaml<'_>) -> Option<ColumnTest> {
    if let Some(name) = yaml::as_str(item) {
        return Some(ColumnTest {
            name: test_name(name),
            strict: true,
        });
    }
    let serde_json::Value::Object(map) = yaml::to_json(item) else {
        return None;
    };
    let long = map.get("test_name").and_then(serde_json::Value::as_str);
    let (name, args) = match long {
        Some(name) => (name.to_string(), serde_json::Value::Object(map.clone())),
        None => {
            let (name, args) = map.iter().next()?;
            (name.clone(), args.clone())
        }
    };
    Some(ColumnTest {
        name: test_name(&name),
        strict: is_strict(&args),
    })
}

fn test_name(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    match lowered.strip_prefix("dbt.") {
        Some(rest) => rest.to_string(),
        None => lowered,
    }
}

/// Reads `where`, `severity` and `enabled` from a test's arguments or
/// their `config:` block. Anything limiting the test weakens the claim.
fn is_strict(args: &serde_json::Value) -> bool {
    let setting = |key: &str| {
        args.get(key)
            .or_else(|| args.get("config").and_then(|c| c.get(key)))
    };
    let filtered = setting("where").is_some_and(|w| !w.is_null());
    let warns = setting("severity")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("warn"));
    let disabled = setting("enabled") == Some(&serde_json::Value::Bool(false));
    !filtered && !warns && !disabled
}

pub(super) fn prune_columns(map: &mut serde_json::Map<String, serde_json::Value>) {
    use serde_json::Value;
    if let Some(Value::Array(columns)) = map.get_mut("columns") {
        for c in columns {
            if let Value::Object(cmap) = c {
                cmap.retain(|k, _| k == "name" || k == "description");
            }
        }
    }
}
