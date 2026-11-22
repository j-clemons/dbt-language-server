//! Model governance diagnostics (dbt groups/access/contracts, 03 §10
//! addendum): a `ref()` into another group's private model warns at
//! the ref token; an enforced contract whose YAML columns drift from
//! the columns the SQL produces warns at the top of the file (the
//! pull path only — it needs lineage).

use cogitator_error::guidance::Guidance;
use cogitator_lsp::{diagnostic_severity, Diagnostic, Range};
use cogitator_parser::TokenKind;

use super::{model_name_from_uri, token_range, DbtContext, Document};
use crate::lineage::ModelLineage;

/// Refs into `access: private` models of a *different* group. A
/// private model without a group restricts nothing (dbt's own rule:
/// access is scoped by group), and intra-group refs stay silent.
pub fn private_ref_diagnostics(ctx: &DbtContext, doc: &Document, uri: &str) -> Vec<Diagnostic> {
    let own_group = model_name_from_uri(uri)
        .and_then(|model| ctx.model_detail_map.get(&model))
        .and_then(|details| details.group.clone());
    let mut out = Vec::new();
    for token in &doc.parsed.tokens {
        if token.kind != TokenKind::Ref {
            continue;
        }
        let literal = doc.parsed.literal(token);
        let Some(target) = ctx.model_detail_map.get(literal) else {
            continue;
        };
        if target.access.as_deref() != Some("private") {
            continue;
        }
        let Some(group) = target.group.as_deref() else {
            continue;
        };
        if own_group.as_deref() == Some(group) {
            continue;
        }
        out.push(Diagnostic::plain(
            token_range(token),
            Guidance::new(format!(
                "model '{literal}' is private to group '{group}' — \
                 ref it from inside the group, or raise its access"
            ))
            .to_diagnostic(),
            diagnostic_severity::WARNING,
            "private-ref",
            "cogitator",
        ));
    }
    out
}

/// Contract drift: `contract.enforced: true` promises the YAML columns
/// are exactly what the model produces. Column sets are compared by
/// name, case-insensitively; a `select *` the project's schemas could
/// not expand disables the check rather than guessing.
pub fn contract_diagnostics(
    ctx: &DbtContext,
    uri: &str,
    lineage: &ModelLineage,
) -> Vec<Diagnostic> {
    let Some(model) = model_name_from_uri(uri) else {
        return Vec::new();
    };
    let Some(details) = ctx.model_detail_map.get(&model) else {
        return Vec::new();
    };
    if !details.contract_enforced || details.columns.is_empty() {
        return Vec::new();
    }
    let produced: Vec<&str> = lineage.columns.iter().map(|c| c.name.as_str()).collect();
    if produced.iter().any(|name| name.contains('*')) {
        return Vec::new(); // star expansion unknown: never guess drift
    }
    let declared: Vec<&str> = details.columns.iter().map(|c| c.name.as_str()).collect();
    let missing: Vec<&str> = declared
        .iter()
        .filter(|name| !produced.iter().any(|p| p.eq_ignore_ascii_case(name)))
        .copied()
        .collect();
    let undeclared: Vec<&str> = produced
        .iter()
        .filter(|name| !declared.iter().any(|d| d.eq_ignore_ascii_case(name)))
        .copied()
        .collect();
    let note = lineage.origin_note();
    let mut out = Vec::new();
    if !missing.is_empty() {
        out.push(drift(format!(
            "contracted column(s) the model never produces: {} — \
             dbt build will fail this contract{note}",
            missing.join(", ")
        )));
    }
    if !undeclared.is_empty() {
        out.push(drift(format!(
            "produced column(s) missing from the enforced contract: {} — \
             add them to the YAML entry{note}",
            undeclared.join(", ")
        )));
    }
    out
}

fn drift(message: String) -> Diagnostic {
    Diagnostic::plain(
        Range::default(),
        Guidance::new(message).to_diagnostic(),
        diagnostic_severity::WARNING,
        "contract-drift",
        "cogitator",
    )
}
