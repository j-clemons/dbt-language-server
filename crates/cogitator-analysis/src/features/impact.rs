//! dbt.impact (03 §10): the transitive downstream closure of a model,
//! with column granularity where per-model lineage resolves. Pure —
//! the shell fans out lineage computation and hands the map in.

use std::collections::HashMap;

use serde::Serialize;

use super::DbtContext;
use crate::graph::NodeKind;
use crate::lineage::{ModelLineage, SourceRef};

#[derive(Debug, Serialize)]
pub struct ImpactNode {
    pub name: String,
    pub uri: Option<String>,
    pub depth: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnHit {
    pub model: String,
    pub column: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImpactReport {
    pub model: String,
    pub column: Option<String>,
    /// "customer_id feeds 9 models, 2 exposures" — the editors' one-liner.
    pub summary: String,
    pub models: Vec<ImpactNode>,
    /// Exposures/metrics/semantic models in the closure.
    pub exposures: Vec<String>,
    pub column_hits: Vec<ColumnHit>,
    pub truncated: bool,
    /// Models whose column answer came from compiled SQL; omitted when
    /// none did (additive, like `ModelLineage.origin`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub compiled_lineage: Vec<String>,
}

/// How many downstream models the shell computes lineage for.
pub const IMPACT_LINEAGE_CAP: usize = 200;

/// The downstream models of `model`, depth-ordered — what the shell
/// needs lineage for before calling `impact`.
pub fn impact_targets(ctx: &DbtContext, model: &str) -> Vec<(String, std::path::PathBuf)> {
    ctx.graph
        .downstream_closure_with_depth(model)
        .into_iter()
        .filter(|(node, _)| node.kind == NodeKind::Model && !node.uri.as_os_str().is_empty())
        .take(IMPACT_LINEAGE_CAP)
        .map(|(node, _)| (node.name.clone(), node.uri.clone()))
        .collect()
}

pub fn impact(
    ctx: &DbtContext,
    model: &str,
    column: Option<&str>,
    lineages: &HashMap<String, ModelLineage>,
) -> ImpactReport {
    let closure = ctx.graph.downstream_closure_with_depth(model);
    let mut models = Vec::new();
    let mut exposures = Vec::new();
    for (node, depth) in &closure {
        match node.kind {
            NodeKind::Model => models.push(ImpactNode {
                name: node.name.clone(),
                uri: (!node.uri.as_os_str().is_empty())
                    .then(|| cogitator_lsp::fs_path_to_uri(&node.uri)),
                depth: *depth,
            }),
            NodeKind::Exposure | NodeKind::Metric | NodeKind::SemanticModel => {
                exposures.push(node.name.clone())
            }
            _ => {}
        }
    }
    let truncated = models.len() > IMPACT_LINEAGE_CAP;
    let compiled_lineage = models
        .iter()
        .filter(|node| lineages.get(&node.name).is_some_and(|l| l.origin.is_some()))
        .map(|node| node.name.clone())
        .collect();
    let column_hits = column
        .map(|column| column_closure(model, column, &models, lineages))
        .unwrap_or_default();
    let subject = column
        .map(|c| format!("{model}.{c}"))
        .unwrap_or_else(|| model.to_string());
    let summary = format!(
        "{subject} feeds {} model(s), {} exposure(s)",
        models.len(),
        exposures.len()
    );
    ImpactReport {
        model: model.to_string(),
        column: column.map(str::to_string),
        summary,
        models,
        exposures,
        column_hits,
        truncated,
        compiled_lineage,
    }
}

/// Taint propagation over per-model lineage, in closure (depth) order:
/// a downstream column is hit when any of its sources is a tainted
/// column of an upstream model. `*` and unknowns stay out — the hits
/// are an under-approximation on purpose.
fn column_closure(
    model: &str,
    column: &str,
    downstream: &[ImpactNode],
    lineages: &HashMap<String, ModelLineage>,
) -> Vec<ColumnHit> {
    let mut tainted: HashMap<&str, Vec<String>> =
        HashMap::from([(model, vec![column.to_string()])]);
    let mut hits = Vec::new();
    for node in downstream {
        let Some(lineage) = lineages.get(&node.name) else {
            continue;
        };
        let mut own: Vec<String> = Vec::new();
        for col in &lineage.columns {
            let star = col.name.contains('*');
            for source in &col.sources {
                let SourceRef::Model { name, column } = source else {
                    continue;
                };
                if column.starts_with("__dbt_rel_") {
                    continue; // internal relation placeholders
                }
                let is_tainted = tainted
                    .get(name.as_str())
                    .is_some_and(|cols| cols.iter().any(|c| c.eq_ignore_ascii_case(column)));
                if !is_tainted {
                    continue;
                }
                // through a star the column keeps its upstream name;
                // otherwise the select list renames it
                let produced = if star {
                    column.clone()
                } else {
                    col.name.clone()
                };
                if !own.iter().any(|c| c.eq_ignore_ascii_case(&produced)) {
                    own.push(produced.clone());
                    hits.push(ColumnHit {
                        model: node.name.clone(),
                        column: produced,
                    });
                }
            }
        }
        if !own.is_empty() {
            tainted.insert(node.name.as_str(), own);
        }
    }
    hits
}
