//! Properties/schema `.yml` adapter (specs/rust-port/03 §6.2, 08 §2.3–2.4).
//!
//! One saphyr parse per file feeds both the typify-generated `DbtYmlFiles`
//! type (shape source of truth) and the name-position side-table.

use std::path::Path;

use cogitator_error::Result;
use cogitator_lsp::Position;

use crate::generated::dbt_yml_files::DbtYmlFiles;
use crate::yaml;

mod columns;

use columns::{column_sides, columns_of, prune_columns, ColumnSide};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PropertiesFile {
    pub models: Vec<ModelEntry>,
    pub sources: Vec<SourceEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelEntry {
    pub name: String,
    pub name_pos: Position,
    pub description: String,
    pub alias: Option<String>,
    pub columns: Vec<ColumnEntry>,
    /// `tags:` and `config.tags` of the entry (tag search, dbt.projectGraph).
    pub tags: Vec<String>,
    /// `access:` (entry or config level): private | protected | public.
    pub access: Option<String>,
    /// `group:` (entry or config level) — the governance group.
    pub group: Option<String>,
    /// `config.contract.enforced: true` (or entry-level `contract:`).
    pub contract_enforced: bool,
}

/// A documented column from the properties YAML (write-assist completion,
/// column hover/definition).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ColumnEntry {
    pub name: String,
    pub description: String,
    /// Position of the column's `name:` value node in the YAML file.
    pub name_pos: Position,
    /// `data_type:` as written, unparsed. `None` when absent, empty or not a
    /// string. Columns filled from the catalog or warehouse leave this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_type: Option<String>,
    /// Column tests (`tests:` or `data_tests:`), in file order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<ColumnTest>,
}

/// One generic test on a column. `strict` says the test passing proves the
/// property for every row: no `where:` filter, not `severity: warn`, not
/// disabled.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ColumnTest {
    /// Lower-cased test name, without a `dbt.` prefix.
    pub name: String,
    pub strict: bool,
}

impl ColumnEntry {
    /// True when a strict test named `name` covers this column.
    pub fn proven_by(&self, name: &str) -> bool {
        self.tests.iter().any(|t| t.strict && t.name == name)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceEntry {
    pub name: String,
    pub name_pos: Position,
    pub description: String,
    /// Relation coordinates, raw YAML strings — `env_var()` text is kept
    /// verbatim here and resolved downstream (analysis layer).
    pub database: Option<String>,
    pub schema: Option<String>,
    pub loader: Option<String>,
    pub tables: Vec<TableEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableEntry {
    pub name: String,
    pub name_pos: Position,
    pub description: String,
    /// Raw, unresolved — see `SourceEntry`.
    pub identifier: Option<String>,
    pub loader: Option<String>,
    pub columns: Vec<ColumnEntry>,
}

/// Parse one properties YAML file. Failures degrade per file (Go returned an
/// empty `PropertiesYaml` on decode errors).
pub fn parse_properties(text: &str, path: &Path) -> Result<PropertiesFile> {
    let marked = yaml::load_marked(text, path)?;
    let mut value = yaml::to_json(&marked);
    // Projection before deserialization, same rationale as in project.rs:
    // the server reads names/descriptions/tables only (03 §6.2); columns,
    // tests and configs are dropped so a strict generated union cannot
    // reject an otherwise fine file.
    prune_for_server(&mut value);
    let parsed: DbtYmlFiles = serde_json::from_value(value)?;

    // Side-table: positions of the `name:` *value* nodes, keyed by list order
    // so entries join positionally with the generated structs. The alias also
    // comes from the tree: dbt-jsonschema does not model `config.alias` on
    // properties-file models, but dbt (and the Go server) honor it (03 §6.3).
    type ModelSide = (
        Position,
        Option<String>,
        Vec<String>,
        Vec<ColumnSide>,
        Governance,
    );
    let model_positions: Vec<ModelSide> = yaml::get(&marked, "models")
        .map(|models| {
            yaml::items(models)
                .map(|m| {
                    let alias = yaml::get(m, "config")
                        .and_then(|c| yaml::get(c, "alias"))
                        .and_then(|a| yaml::as_str(a).map(str::to_string));
                    (
                        name_position(m),
                        alias,
                        tags_of(m),
                        column_sides(m),
                        governance_of(m),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    type TableSide = (Position, Vec<ColumnSide>);
    let source_nodes: Vec<(Position, Vec<TableSide>)> = yaml::get(&marked, "sources")
        .map(|sources| {
            yaml::items(sources)
                .map(|s| {
                    let tables = yaml::get(s, "tables")
                        .map(|t| {
                            yaml::items(t)
                                .map(|table| (name_position(table), column_sides(table)))
                                .collect()
                        })
                        .unwrap_or_default();
                    (name_position(s), tables)
                })
                .collect()
        })
        .unwrap_or_default();

    let models = parsed
        .models
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (name_pos, alias, tags, column_side, governance) =
                model_positions.get(i).cloned().unwrap_or_default();
            ModelEntry {
                name: m.name.clone(),
                name_pos,
                description: m.description.clone().unwrap_or_default(),
                alias,
                columns: columns_of(&m.columns, &column_side),
                tags,
                access: governance.access,
                group: governance.group,
                contract_enforced: governance.contract_enforced,
            }
        })
        .collect();

    let sources = parsed
        .sources
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (source_pos, table_positions) = source_nodes
                .get(i)
                .cloned()
                .unwrap_or((Position::default(), Vec::new()));
            SourceEntry {
                name: s.name.clone(),
                name_pos: source_pos,
                description: s.description.clone().unwrap_or_default(),
                database: s.database.clone(),
                schema: s.schema.clone(),
                loader: s.loader.clone(),
                tables: s
                    .tables
                    .iter()
                    .enumerate()
                    .map(|(j, t)| {
                        let (name_pos, column_side) =
                            table_positions.get(j).cloned().unwrap_or_default();
                        TableEntry {
                            name: t.name.clone(),
                            name_pos,
                            description: t.description.clone().unwrap_or_default(),
                            identifier: t.identifier.clone(),
                            loader: t.loader.clone(),
                            columns: columns_of(&t.columns, &column_side),
                        }
                    })
                    .collect(),
            }
        })
        .collect();

    Ok(PropertiesFile { models, sources })
}

/// Governance keys of one model entry, entry- or config-level
/// (dbt accepts both spellings).
#[derive(Debug, Clone, Default)]
struct Governance {
    access: Option<String>,
    group: Option<String>,
    contract_enforced: bool,
}

fn governance_of(entry: &saphyr::MarkedYaml<'_>) -> Governance {
    let either = |key: &str| {
        yaml::get(entry, key)
            .or_else(|| yaml::get(entry, "config").and_then(|config| yaml::get(config, key)))
    };
    let text = |key: &str| either(key).and_then(|n| yaml::as_str(n).map(str::to_string));
    let contract_enforced = either("contract")
        .and_then(|contract| yaml::get(contract, "enforced"))
        .map(|node| yaml::to_json(node))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Governance {
        access: text("access"),
        group: text("group"),
        contract_enforced,
    }
}

/// `tags:` on the entry plus `config.tags`, each a string or a list.
fn tags_of(entry: &saphyr::MarkedYaml<'_>) -> Vec<String> {
    let mut tags = Vec::new();
    let nodes = [
        yaml::get(entry, "tags"),
        yaml::get(entry, "config").and_then(|c| yaml::get(c, "tags")),
    ];
    for node in nodes.into_iter().flatten() {
        if let Some(tag) = yaml::as_str(node) {
            tags.push(tag.to_string());
        } else {
            tags.extend(yaml::items(node).filter_map(|t| yaml::as_str(t).map(str::to_string)));
        }
    }
    tags.sort();
    tags.dedup();
    tags
}

fn name_position(entry: &saphyr::MarkedYaml<'_>) -> Position {
    yaml::get(entry, "name")
        .map(|n| yaml::position_of(n))
        .unwrap_or_default()
}

fn prune_for_server(value: &mut serde_json::Value) {
    use serde_json::Value;
    let Value::Object(root) = value else { return };
    root.retain(|k, _| k == "models" || k == "sources" || k == "version");
    if let Some(Value::Array(models)) = root.get_mut("models") {
        for m in models {
            if let Value::Object(map) = m {
                map.retain(|k, _| matches!(k.as_str(), "name" | "description" | "columns"));
                prune_columns(map);
            }
        }
    }
    if let Some(Value::Array(sources)) = root.get_mut("sources") {
        for s in sources {
            if let Value::Object(map) = s {
                map.retain(|k, _| {
                    matches!(
                        k.as_str(),
                        "name" | "description" | "database" | "schema" | "loader" | "tables"
                    )
                });
                if let Some(Value::Array(tables)) = map.get_mut("tables") {
                    for t in tables {
                        if let Value::Object(tmap) = t {
                            tmap.retain(|k, _| {
                                matches!(
                                    k.as_str(),
                                    "name" | "description" | "identifier" | "loader" | "columns"
                                )
                            });
                            prune_columns(tmap);
                        }
                    }
                }
            }
        }
    }
}
