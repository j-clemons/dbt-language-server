//! Column lineage with the project's knowledge: stars expand against
//! `Closed` upstream schemas, and a template with control flow answers
//! from its compiled SQL when that file is demonstrably fresh.
//!
//! Pure over [`PlanInput`]: the shell reads the compiled file and decides
//! whether unsaved edits rule it out. A compiled answer is never partly
//! stale: anything short of `Fresh` plus the coverage check declines.

use std::sync::Arc;

use cogitator_ir::origin::{SourceOrigin, Staleness};
use cogitator_parser::ParsedDocument;
use cogitator_sql_front::compiled::{
    align, build_compiled_hir, prepare, RelationTable, MIN_LITERAL_COVERAGE,
};
use cogitator_sql_front::{build_hir, HirInput, SchemaEnv};

use super::from_hir::{columns_of, has_control_block, template_input};
use super::{LineageOrigin, ModelLineage};
use crate::features::plan::{CompiledInput, PlanInput};

/// Whether this document can only be answered from compiled SQL; the shell
/// reads `target/compiled` for these and no others.
pub fn needs_compiled(doc: &ParsedDocument) -> bool {
    has_control_block(doc)
}

/// Lineage for `doc` over the project snapshot and, for a template with
/// control flow, fresh compiled SQL. `input.text` must be `doc`'s text.
///
/// `None` as for [`super::column_lineage`], and also when control flow has
/// no compiled file known fresh: stale SQL is no answer, not a worse one.
pub fn typed_column_lineage(doc: &ParsedDocument, input: &PlanInput<'_>) -> Option<ModelLineage> {
    lineage_over(doc, input, &project_env(input))
}

/// [`typed_column_lineage`] over an explicit environment.
pub(super) fn lineage_over(
    doc: &ParsedDocument,
    input: &PlanInput<'_>,
    env: &SchemaEnv,
) -> Option<ModelLineage> {
    if has_control_block(doc) {
        return compiled_lineage(doc, input, env);
    }
    let lowered = build_hir(&template_input(doc, input.dialect), env);
    Some(ModelLineage {
        columns: columns_of(&lowered, &doc.text, Some(doc), true)?,
        origin: None,
    })
}

/// The whole snapshot is the environment: the binder only looks names up.
fn project_env(input: &PlanInput<'_>) -> SchemaEnv {
    SchemaEnv {
        schemas: input
            .project
            .map_or_else(Default::default, |p| Arc::clone(&p.schemas)),
        model: input.model.map(Arc::from),
    }
}

fn compiled_lineage(
    doc: &ParsedDocument,
    input: &PlanInput<'_>,
    env: &SchemaEnv,
) -> Option<ModelLineage> {
    let CompiledInput::Available {
        text,
        mtime: Staleness::Fresh,
        ..
    } = input.compiled
    else {
        return None;
    };
    let relations = relation_table(input);
    let prepared = prepare(text, input.dialect, &relations, input.model).ok()?;
    let alignment = align(&doc.text, &prepared, text);
    if alignment.literal_coverage < MIN_LITERAL_COVERAGE {
        return None; // unsaved or uncompiled edits: the SQL no longer matches
    }
    let front = HirInput {
        text,
        dialect: input.dialect,
        origin: SourceOrigin::Compiled {
            staleness: Staleness::Fresh,
            map: alignment.map,
        },
        config: Default::default(),
    };
    let lowered = build_compiled_hir(&front, env, &prepared);
    Some(ModelLineage {
        columns: columns_of(&lowered, text, None, true)?,
        origin: Some(LineageOrigin::Compiled),
    })
}

/// Rendered relation name to model or source, as the query plan builds it
/// (`features/plan/compiled_route.rs`): names sorted first, so a collision
/// resolves the same way each time regardless of `HashMap` order.
fn relation_table(input: &PlanInput<'_>) -> RelationTable {
    let mut table = RelationTable::new(input.dialect);
    let manifest = input.manifest.map(|m| &m.models);
    let mut names: Vec<&str> = input
        .models
        .keys()
        .chain(manifest.into_iter().flat_map(|m| m.keys()))
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let relation = manifest
            .and_then(|m| m.get(name))
            .and_then(|node| node.relation_name.as_deref());
        table.add_model(name, relation);
    }
    let mut sources: Vec<(&str, &str)> = input
        .sources
        .iter()
        .flat_map(|(source, details)| {
            details
                .tables
                .keys()
                .map(move |table| (source.as_str(), table.as_str()))
        })
        .collect();
    sources.sort_unstable();
    for (source, name) in sources {
        let details = input.sources.get(source);
        let identifier = details
            .and_then(|d| d.tables.get(name))
            .and_then(|t| t.identifier.as_deref());
        let schema = details.and_then(|d| d.schema.as_deref());
        table.add_source(source, name, schema, identifier);
    }
    table
}
