//! Columns the model produces (per lineage) that its properties YAML
//! never documents — a Hint, pairing with the inherit-description
//! action.

use cogitator_error::guidance::Guidance;
use cogitator_lsp::{diagnostic_severity, Diagnostic, Range};

use super::super::{model_name_from_uri, DbtContext, Document};
use crate::context::ModelDetails;
use crate::lineage::ModelLineage;

pub fn check(
    ctx: &DbtContext,
    _doc: &Document,
    uri: &str,
    lineage: &ModelLineage,
) -> Vec<Diagnostic> {
    let Some(model) = model_name_from_uri(uri) else {
        return Vec::new();
    };
    let Some(details) = ctx.model_detail_map.get(&model) else {
        return Vec::new();
    };
    if details.schema_uri.is_none() {
        return Vec::new(); // no YAML entry at all: extract scaffold territory
    }
    let missing = missing_columns(details, lineage);
    if missing.is_empty() {
        return Vec::new();
    }
    vec![Diagnostic::plain(
        Range::default(),
        Guidance::new(format!(
            "{} produced column(s) missing from the schema YAML: {}{}",
            missing.len(),
            missing.join(", "),
            lineage.origin_note()
        ))
        .fix("document them, or inherit the upstream descriptions.")
        .to_diagnostic(),
        diagnostic_severity::HINT,
        "lint-undocumented-column",
        "cogitator",
    )]
}

/// The produced columns (per lineage, in output order) the YAML entry
/// does not document; a `*` production is no column. Shared with the
/// "Add missing columns" action, so the hint and its fix agree.
pub(crate) fn missing_columns<'a>(
    details: &ModelDetails,
    lineage: &'a ModelLineage,
) -> Vec<&'a str> {
    lineage
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .filter(|name| !name.contains('*'))
        .filter(|name| {
            !details
                .columns
                .iter()
                .any(|d| d.name.eq_ignore_ascii_case(name))
        })
        .collect()
}
