//! The hot-loadable server config: `<root>/cogitator.yml` (or
//! `.cogitator.yml`), every key optional. The pure core — parse, warn
//! about unknown keys with did-you-mean, DIFF two versions and APPLY
//! only the changed keys (so a file edit never clobbers runtime
//! switches it did not touch) — lives here; the watcher task is the
//! shell; the diff/apply half lives in `apply`. `startup:` keys are
//! honored only from `--config` at launch
//! (the tracing subscriber and parser backend are install-once).

use std::path::Path;

use cogitator_error::guidance::{closest_matches, Guidance};
use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub profiles: ProfilesSection,
    #[serde(default)]
    pub limits: LimitsSection,
    #[serde(default)]
    pub refresh: RefreshSection,
    #[serde(default)]
    pub startup: StartupSection,
    #[serde(default)]
    pub lint: LintSection,
    #[serde(default)]
    pub warehouse: WarehouseSection,
    #[serde(default)]
    pub log: LogSection,
    #[serde(default)]
    pub perf: PerfSection,
    /// Rule id → off | hint | info | warning. Sorted, so applying it is
    /// deterministic; ids and values are validated, not assumed.
    #[serde(default)]
    pub perf_rules: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ProfilesSection {
    pub dir: Option<String>,
    pub profile: Option<String>,
    pub target: Option<String>,
    pub allow: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct LimitsSection {
    /// dbt.compiledSql wire cap in MiB.
    pub compiled_sql_mib: Option<u64>,
    /// target/ artifact files past this size are skipped.
    pub artifact_mib: Option<u64>,
}

/// Opt-in style checks (03 §10); absent keys fall back to the launch
/// baseline, so the changed-keys-only semantics hold per flag.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct LintSection {
    pub select_star: Option<bool>,
    pub unaliased_columns: Option<bool>,
    pub trailing_comma: Option<bool>,
    pub undocumented_columns: Option<bool>,
    pub dead_models: Option<bool>,
}

pub use cogitator_analysis::config_schema::lint_flags;

impl LintSection {
    /// Resolve to bitflags; a key absent from the file keeps the bit
    /// it has in `base` (the launch value).
    pub fn flags(&self, base: u32) -> u32 {
        let mut flags = base;
        let mut bit = |value: Option<bool>, mask: u32| match value {
            Some(true) => flags |= mask,
            Some(false) => flags &= !mask,
            None => {}
        };
        bit(self.select_star, lint_flags::SELECT_STAR);
        bit(self.unaliased_columns, lint_flags::UNALIASED_COLUMNS);
        bit(self.trailing_comma, lint_flags::TRAILING_COMMA);
        bit(self.undocumented_columns, lint_flags::UNDOCUMENTED_COLUMNS);
        bit(self.dead_models, lint_flags::DEAD_MODELS);
        flags
    }
}

/// Query-plan performance diagnostics (specs/ir-pipeline/08).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct PerfSection {
    /// off (default) | recommended | strict.
    pub profile: Option<String>,
    pub catalog_stats: Option<bool>,
}

/// The editor-visible log stream (05 §6.3 addendum).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct LogSection {
    /// off | error | warn | info | debug.
    pub client_level: Option<String>,
}

/// Warehouse introspection switches (duckdb builds only; harmless and
/// inert elsewhere).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WarehouseSection {
    pub columns: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RefreshSection {
    /// Context-refresh coalescing window.
    pub debounce_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct StartupSection {
    pub debug: Option<bool>,
    pub otlp: Option<String>,
    pub tree_sitter: Option<bool>,
    pub fusion: Option<String>,
}

impl StartupSection {
    pub fn is_empty(&self) -> bool {
        self == &StartupSection::default()
    }
}

/// Parse `text` (the content of `path`). Errors come back as Guidance
/// carrying the syntax position where saphyr knows one.
pub fn parse(text: &str, path: &Path) -> Result<FileConfig, Guidance> {
    if text.trim().is_empty() {
        return Ok(FileConfig::default()); // an empty file means defaults
    }
    if let Some((line, character, message)) = cogitator_schemas::yaml::syntax_error(text) {
        return Err(Guidance::new("cogitator.yml does not parse.")
            .at(format!("{}:{}:{}", path.display(), line + 1, character + 1))
            .because(message)
            .fix("fix the YAML; the previous configuration stays active."));
    }
    let marked = cogitator_schemas::yaml::load_marked(text, path).map_err(|err| {
        Guidance::new("cogitator.yml does not parse.")
            .at(path.display().to_string())
            .because(err.to_string())
            .fix("fix the YAML; the previous configuration stays active.")
    })?;
    let value = cogitator_schemas::yaml::to_json(&marked);
    serde_json::from_value(value).map_err(|err| {
        Guidance::new("cogitator.yml has a value of the wrong type.")
            .at(path.display().to_string())
            .because(err.to_string())
            .fix("compare against the documented schema in the README.")
    })
}

// kept in lockstep with cogitator_analysis::config_schema (test-pinned)
const ROOT_KEYS: &[&str] = &[
    "profiles",
    "limits",
    "refresh",
    "startup",
    "lint",
    "log",
    "warehouse",
    "perf",
    "perf_rules",
];
const PROFILES_KEYS: &[&str] = &["dir", "profile", "target", "allow"];
const LIMITS_KEYS: &[&str] = &["compiled_sql_mib", "artifact_mib"];
const REFRESH_KEYS: &[&str] = &["debounce_ms"];
const STARTUP_KEYS: &[&str] = &["debug", "otlp", "tree_sitter", "fusion"];
const LINT_KEYS: &[&str] = &[
    "select_star",
    "unaliased_columns",
    "trailing_comma",
    "undocumented_columns",
    "dead_models",
];
const LOG_KEYS: &[&str] = &["client_level"];
const WAREHOUSE_KEYS: &[&str] = &["columns"];
const PERF_KEYS: &[&str] = &["profile", "catalog_stats"];
// dotted ids stay plain keys here: did-you-mean works unchanged
const PERF_RULE_KEYS: &[&str] = &[
    "join.left_nullified",
    "join.cartesian",
    "join.distinct_fanout",
    "model.unused_cte",
    "cross.select_star_chain",
    "filter.non_sargable",
    "incr.missing_guard",
    "incr.subquery_watermark",
    "set.union_distinct",
    "cross.unused_column",
    "cross.filter_late",
    "cross.ephemeral_repeat",
    "cross.duplicate_logic",
    "order.in_cte",
    "subq.not_in_nullable",
    "subq.correlated",
    "join.redundant_left",
    "join.key_type_mismatch",
    "filter.partition_or",
    "filter.cluster_order",
    "filter.constant_false",
    "limit.no_scan_reduction",
    "incr.no_unique_key",
];

/// Unknown keys warn with did-you-mean — never a hard error, so a
/// typo'd key cannot take the rest of the file down.
pub fn unknown_key_warnings(text: &str, path: &Path) -> Vec<Guidance> {
    use cogitator_schemas::yaml::{as_str, entries, get, position_of};
    use saphyr::{LoadableYamlNode, MarkedYaml};

    let Ok(docs) = MarkedYaml::load_from_str(text) else {
        return Vec::new();
    };
    let Some(root) = docs.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut flag = |key: &MarkedYaml<'_>, known: &[&str], section: &str| {
        let Some(name) = as_str(key) else { return };
        if known.contains(&name) {
            return;
        }
        let position = position_of(key);
        out.push(
            Guidance::new(format!("'{name}' is not a cogitator.yml key{section}."))
                .at(format!("{}:{}", path.display(), position.line + 1))
                .suggest(closest_matches(name, known.iter().copied()))
                .fix("the full schema is documented in the README."),
        );
    };
    for (key, _) in entries(root) {
        flag(key, ROOT_KEYS, "");
    }
    for (section, keys) in [
        ("profiles", PROFILES_KEYS),
        ("limits", LIMITS_KEYS),
        ("refresh", REFRESH_KEYS),
        ("startup", STARTUP_KEYS),
        ("lint", LINT_KEYS),
        ("log", LOG_KEYS),
        ("warehouse", WAREHOUSE_KEYS),
        ("perf", PERF_KEYS),
        ("perf_rules", PERF_RULE_KEYS),
    ] {
        if let Some(node) = get(root, section) {
            for (key, _) in entries(node) {
                flag(key, keys, &format!(" under {section}:"));
            }
        }
    }
    out
}

mod apply;
mod perf;

pub use apply::{apply, diff, launch_filter, Baseline};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_lists_match_the_shared_schema() {
        use cogitator_analysis::config_schema::{section, COGITATOR_SCHEMA};
        let names: Vec<&str> = COGITATOR_SCHEMA.iter().map(|s| s.name).collect();
        assert_eq!(ROOT_KEYS, names.as_slice());
        for (keys, name) in [
            (PROFILES_KEYS, "profiles"),
            (LIMITS_KEYS, "limits"),
            (REFRESH_KEYS, "refresh"),
            (STARTUP_KEYS, "startup"),
            (LINT_KEYS, "lint"),
            (LOG_KEYS, "log"),
            (WAREHOUSE_KEYS, "warehouse"),
            (PERF_KEYS, "perf"),
            (PERF_RULE_KEYS, "perf_rules"),
        ] {
            let schema: Vec<&str> = section(name)
                .expect(name)
                .keys
                .iter()
                .map(|k| k.key)
                .collect();
            assert_eq!(keys, schema.as_slice(), "{name}");
        }
    }

    #[test]
    fn parses_partial_and_empty_files() {
        let path = Path::new("cogitator.yml");
        let full = parse(
            "profiles:\n  dir: /etc/dbt\n  target: prod\nlimits:\n  compiled_sql_mib: 4\n",
            path,
        )
        .unwrap();
        assert_eq!(full.profiles.dir.as_deref(), Some("/etc/dbt"));
        assert_eq!(full.limits.compiled_sql_mib, Some(4));
        assert!(full.startup.is_empty());

        let empty = parse("", path).unwrap();
        assert_eq!(empty, FileConfig::default());
    }

    #[test]
    fn syntax_errors_carry_a_position() {
        let err = parse("profiles:\n  dir: [unclosed\n", Path::new("cogitator.yml"))
            .expect_err("must fail");
        let message = err.to_message();
        assert!(message.contains("cogitator.yml:"), "{message}");
        assert!(
            message.contains("previous configuration stays active"),
            "{message}"
        );
    }

    #[test]
    fn unknown_keys_suggest() {
        let warnings = unknown_key_warnings(
            "profils:\n  dir: /x\nprofiles:\n  targett: dev\n",
            Path::new("cogitator.yml"),
        );
        let texts: Vec<String> = warnings.iter().map(|w| w.to_message()).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("'profils'") && t.contains("profiles")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("'targett'") && t.contains("target")),
            "{texts:?}"
        );
    }
}
