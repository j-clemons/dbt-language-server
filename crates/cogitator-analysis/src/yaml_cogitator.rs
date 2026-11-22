//! cogitator.yml intelligence (03 §10): diagnostics, completion and
//! hover for the server's OWN config file, driven entirely by the
//! `config_schema` table — before this, an open cogitator.yml fell
//! through to the dbt-properties pass and had its own keys flagged.

use cogitator_error::guidance::{closest_matches, Guidance};
use cogitator_lsp::{
    completion_kind, diagnostic_severity, CompletionItem, Diagnostic, Position, Range,
};

use crate::config_schema::{section, COGITATOR_SCHEMA};

/// Is this URI the server's config file? Filename-based on purpose: a
/// cogitator.yml anywhere gets config intelligence, while the watcher's
/// root-scoped check stays separate.
pub fn is_cogitator_config(uri: &str) -> bool {
    let path = cogitator_lsp::uri_to_fs_path(uri).unwrap_or_else(|| uri.to_string());
    std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "cogitator.yml" || n == ".cogitator.yml")
}

/// Unknown keys (with did-you-mean) and light value-type checks from
/// the schema table. Warnings only — the watcher applies what it can.
pub fn cogitator_diagnostics(text: &str) -> Vec<Diagnostic> {
    use cogitator_schemas::yaml::{as_str, entries, get, position_of};
    use saphyr::{LoadableYamlNode, MarkedYaml};

    if let Some((line, character, message)) = cogitator_schemas::yaml::syntax_error(text) {
        return vec![Diagnostic::plain(
            Range::zero_width(Position { line, character }),
            format!("yaml syntax error: {message}"),
            diagnostic_severity::ERROR,
            "cogitator-yml",
            "cogitator",
        )];
    }
    let Ok(docs) = MarkedYaml::load_from_str(text) else {
        return Vec::new();
    };
    let Some(root) = docs.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let warn = |position: Position, guidance: Guidance| {
        Diagnostic::plain(
            Range::zero_width(position),
            guidance.to_diagnostic(),
            diagnostic_severity::WARNING,
            "cogitator-yml",
            "cogitator",
        )
    };
    let section_names = COGITATOR_SCHEMA.iter().map(|s| s.name);
    for (key, _) in entries(root) {
        let Some(name) = as_str(key) else { continue };
        if section(name).is_none() {
            out.push(warn(
                position_of(key),
                Guidance::new(format!("'{name}' is not a cogitator.yml section"))
                    .suggest(closest_matches(name, section_names.clone())),
            ));
        }
    }
    for schema in COGITATOR_SCHEMA {
        let Some(node) = get(root, schema.name) else {
            continue;
        };
        for (key, value) in entries(node) {
            let Some(name) = as_str(key) else { continue };
            let Some(doc) = schema.keys.iter().find(|k| k.key == name) else {
                out.push(warn(
                    position_of(key),
                    Guidance::new(format!(
                        "'{name}' is not a cogitator.yml key under {}:",
                        schema.name
                    ))
                    .suggest(closest_matches(name, schema.keys.iter().map(|k| k.key))),
                ));
                continue;
            };
            if let Some(guidance) = value_problem(schema.name, name, doc.value_hint, value) {
                out.push(warn(position_of(value), guidance));
            }
        }
    }
    out
}

/// A value the schema's hint rejects: a closed enum names its choices
/// (with did-you-mean); other hints use the light type check.
fn value_problem(
    section_name: &str,
    key: &str,
    hint: &str,
    value: &saphyr::MarkedYaml<'_>,
) -> Option<Guidance> {
    if hint.contains('|') {
        return enum_problem(section_name, key, hint, value);
    }
    value_type_problem(hint, value)
        .map(|problem| Guidance::new(format!("{section_name}.{key} {problem}")))
}

fn enum_problem(
    section_name: &str,
    key: &str,
    hint: &str,
    value: &saphyr::MarkedYaml<'_>,
) -> Option<Guidance> {
    use cogitator_schemas::yaml::as_str;
    let choices = hint.split('|');
    if as_str(value).is_some_and(|v| choices.clone().any(|c| c == v)) {
        return None;
    }
    let shown = as_str(value).unwrap_or("this value");
    Some(
        Guidance::new(format!(
            "{section_name}.{key} expects one of {hint}, not '{shown}'"
        ))
        .suggest(closest_matches(shown, choices)),
    )
}

/// Light value typing from the schema's value hint; `None` = fine.
fn value_type_problem(hint: &str, value: &saphyr::MarkedYaml<'_>) -> Option<&'static str> {
    use saphyr::{Scalar, YamlData};
    match (hint, &value.data) {
        ("bool", YamlData::Value(Scalar::Boolean(_))) => None,
        ("bool", _) => Some("expects true or false"),
        ("MiB" | "ms", YamlData::Value(Scalar::Integer(n))) if *n >= 0 => None,
        ("MiB" | "ms", _) => Some("expects a non-negative integer"),
        _ => None, // paths/names/lists stay unchecked
    }
}

/// Section names at indent 0; a section's keys (with docs) inside it.
pub fn cogitator_completion(text: &str, pos: Position) -> Vec<CompletionItem> {
    let lines: Vec<&str> = text.lines().collect();
    let cursor_line = pos.line as usize;
    let indent = lines
        .get(cursor_line)
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(0);
    let item = |label: &str, doc: &str, detail: &str, insert: String| CompletionItem {
        label: label.to_string(),
        detail: detail.to_string(),
        documentation: doc.to_string(),
        kind: completion_kind::VARIABLE,
        insert_text: insert,
        sort_text: label.to_string(),
        data: None,
    };
    if indent == 0 {
        return COGITATOR_SCHEMA
            .iter()
            .map(|s| {
                item(
                    s.name,
                    s.doc,
                    "cogitator.yml section",
                    format!("{}:\n  ", s.name),
                )
            })
            .collect();
    }
    let section_above = lines[..cursor_line.min(lines.len())]
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.starts_with(' ') && l.trim_end().ends_with(':'))
        .map(|l| l.trim_end().trim_end_matches(':'))
        .and_then(section);
    let Some(schema) = section_above else {
        return Vec::new();
    };
    schema
        .keys
        .iter()
        .map(|k| item(k.key, k.doc, k.value_hint, format!("{}: ", k.key)))
        .collect()
}

/// The documentation of the section or key under the cursor.
pub fn cogitator_hover(text: &str, pos: Position) -> Option<String> {
    let line = text.lines().nth(pos.line as usize)?;
    let name = line.trim_start().split(':').next()?.trim();
    if name.is_empty() {
        return None;
    }
    if !line.starts_with(' ') {
        let schema = section(name)?;
        return Some(format!("cogitator.yml `{name}:` — {}", schema.doc));
    }
    let section_above = text
        .lines()
        .take(pos.line as usize)
        .filter(|l| !l.trim().is_empty() && !l.starts_with(' ') && l.trim_end().ends_with(':'))
        .last()
        .map(|l| l.trim_end().trim_end_matches(':'))
        .and_then(section)?;
    let key = section_above.keys.iter().find(|k| k.key == name)?;
    Some(format!(
        "`{}.{}` ({}) — {}",
        section_above.name, key.key, key.value_hint, key.doc
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_and_types_warn_with_positions() {
        let diags = cogitator_diagnostics(
            "profils:\n  dir: /x\nlint:\n  select_star: yes\n  bogus: 1\nlimits:\n  compiled_sql_mib: much\n",
        );
        let texts: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("'profils'") && t.contains("profiles")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("'bogus'") && t.contains("under lint:")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("compiled_sql_mib expects a non-negative integer")),
            "{texts:?}"
        );
        // YAML 1.2: `yes` is a STRING, not a boolean — the warning saves
        // users from a silently ignored flag
        assert!(
            texts
                .iter()
                .any(|t| t.contains("select_star expects true or false")),
            "{texts:?}"
        );
        assert!(diags
            .iter()
            .all(|d| d.range.start.line > 0 || d.range.start.character == 0));
    }

    #[test]
    fn perf_sections_validate_rule_ids_and_values() {
        let diags = cogitator_diagnostics(
            "perf:\n  profile: loud\nperf_rules:\n  join.cartesan: warning\n  filter.non_sargable: nope\n  join.cartesian: warning\n",
        );
        let texts: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("perf.profile") && t.contains("off|recommended|strict")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("'join.cartesan'") && t.contains("join.cartesian")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("perf_rules.filter.non_sargable")),
            "{texts:?}"
        );
        assert_eq!(texts.len(), 3, "the valid entry is silent: {texts:?}");
    }

    #[test]
    fn completion_is_section_aware() {
        let text = "lint:\n  \n";
        let items = cogitator_completion(
            text,
            Position {
                line: 1,
                character: 2,
            },
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"select_star"), "{labels:?}");
        assert!(labels.contains(&"dead_models"), "{labels:?}");
        let top = cogitator_completion("", Position::default());
        assert!(top.iter().any(|i| i.label == "warehouse"), "{top:?}");
    }

    #[test]
    fn hover_reads_the_schema_table() {
        let text = "refresh:\n  debounce_ms: 200\n";
        let hover = cogitator_hover(
            text,
            Position {
                line: 1,
                character: 4,
            },
        )
        .unwrap();
        assert!(hover.contains("Coalescing window"), "{hover}");
        let section_hover = cogitator_hover(
            text,
            Position {
                line: 0,
                character: 1,
            },
        )
        .unwrap();
        assert!(section_hover.contains("refresh"), "{section_hover}");
    }
}
