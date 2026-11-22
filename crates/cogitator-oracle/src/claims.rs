//! The one-way plan comparison (specs/ir-pipeline/07).
//!
//! We never ask an engine's plan to match ours. Two directions only:
//! a column we call unused must not be read by the engine, and our required
//! set must cover what the engine projects. An engine reading less than we
//! require is fine: on toy data statistics can prune whole predicates.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

/// Column claims per base table, in `expect.toml` under `[plan]`.
///
/// For a claimed table, `required` and `unused` together should list every
/// column of the table, which is what a required-columns pass produces.
/// DuckDB reports pushed-down filters as text, so a filter column is only
/// recognised by its claimed name.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanClaims {
    #[serde(default)]
    pub unused: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub required: BTreeMap<String, Vec<String>>,
}

impl PlanClaims {
    pub fn is_empty(&self) -> bool {
        self.unused.is_empty() && self.required.is_empty()
    }
}

/// What an engine scans for one base table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableRead {
    /// Columns the scan outputs (lower-cased).
    pub columns: BTreeSet<String>,
    /// Pushed-down filter text where the engine exposes no structured form.
    pub filter_text: Vec<String>,
}

/// Per-table reads of one plan, keyed by lower-cased bare table name.
pub type EngineReads = BTreeMap<String, TableRead>;

/// Check `claims` against `reads`, returning one message per violation.
pub fn check_claims(reads: &EngineReads, claims: &PlanClaims) -> Vec<String> {
    let mut failures = Vec::new();
    for (table, columns) in &claims.unused {
        let Some(read) = reads.get(&table.to_lowercase()) else {
            continue;
        };
        for column in columns {
            if reads_column(read, column) {
                failures.push(format!(
                    "column {table}.{column} is claimed unused but the engine reads it"
                ));
            }
        }
    }
    for (table, columns) in &claims.required {
        let Some(read) = reads.get(&table.to_lowercase()) else {
            continue;
        };
        let required: BTreeSet<String> = columns.iter().map(|c| c.to_lowercase()).collect();
        for column in read.columns.difference(&required) {
            failures.push(format!(
                "the engine projects {table}.{column}, which the required set does not cover"
            ));
        }
    }
    failures
}

fn reads_column(read: &TableRead, column: &str) -> bool {
    let column = column.to_lowercase();
    read.columns.contains(&column) || read.filter_text.iter().any(|f| mentions(f, &column))
}

/// Whether `filter` names `column` outside a string literal, as a whole word.
fn mentions(filter: &str, column: &str) -> bool {
    filter
        .to_lowercase()
        .split('\'')
        .step_by(2)
        .any(|outside_literal| {
            outside_literal
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|word| word == column)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reads(table: &str, columns: &[&str], filter: Option<&str>) -> EngineReads {
        BTreeMap::from([(
            table.to_owned(),
            TableRead {
                columns: columns.iter().map(|c| (*c).to_owned()).collect(),
                filter_text: filter.iter().map(|f| (*f).to_owned()).collect(),
            },
        )])
    }

    fn claims(unused: &[(&str, &[&str])], required: &[(&str, &[&str])]) -> PlanClaims {
        let map = |pairs: &[(&str, &[&str])]| {
            pairs
                .iter()
                .map(|(t, cs)| {
                    (
                        (*t).to_owned(),
                        cs.iter().map(|c| (*c).to_owned()).collect(),
                    )
                })
                .collect()
        };
        PlanClaims {
            unused: map(unused),
            required: map(required),
        }
    }

    #[test]
    fn unused_column_read_by_engine_fails() {
        let failures = check_claims(
            &reads("orders", &["id", "note"], None),
            &claims(&[("orders", &["note"])], &[]),
        );
        assert_eq!(failures.len(), 1, "{failures:?}");
    }

    #[test]
    fn unused_column_in_a_pushed_filter_fails() {
        let failures = check_claims(
            &reads("orders", &["id"], Some("(note = 'x')")),
            &claims(&[("orders", &["note"])], &[]),
        );
        assert_eq!(failures.len(), 1, "{failures:?}");
    }

    #[test]
    fn a_literal_equal_to_a_column_name_is_not_a_read() {
        let failures = check_claims(
            &reads("orders", &["id"], Some("(status = 'note')")),
            &claims(&[("orders", &["note"])], &[]),
        );
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn engine_projection_outside_required_fails() {
        let failures = check_claims(
            &reads("orders", &["id", "status"], None),
            &claims(&[], &[("orders", &["id"])]),
        );
        assert_eq!(failures.len(), 1, "{failures:?}");
    }

    #[test]
    fn the_comparison_is_one_way() {
        // The engine reads less than required: fine (statistics may prune).
        let failures = check_claims(
            &reads("orders", &["id"], None),
            &claims(&[("orders", &["note"])], &[("orders", &["id", "status"])]),
        );
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn unscanned_tables_are_not_failures() {
        let failures = check_claims(
            &EngineReads::new(),
            &claims(&[("orders", &["note"])], &[("orders", &["id"])]),
        );
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn matching_is_case_insensitive() {
        let failures = check_claims(
            &reads("orders", &["note"], None),
            &claims(&[("ORDERS", &["NOTE"])], &[]),
        );
        assert_eq!(failures.len(), 1, "{failures:?}");
    }
}
