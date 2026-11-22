//! The query-plan feature (specs/ir-pipeline/09): one model's text,
//! driven through HIR, MIR, the passes and LIR, reported level by level.
//!
//! Pure: every fact arrives through [`PlanInput`], the server shell does
//! the filesystem work and caches the JSON.
//!
//! Entry point: [`query_plan`]. Absent facts leave their `Need`
//! unavailable, so rules record `Skipped` rather than guess.

use std::collections::HashMap;

use cogitator_dialects::Dialect;
pub use cogitator_ir::origin::Staleness;
use cogitator_lower::backends::for_backend;
use cogitator_lower::lower_to_mir;

use crate::artifacts::catalog::CatalogIndex;
use crate::artifacts::manifest::ManifestIndex;
use crate::artifacts::ArtifactSet;
use crate::context::types::SourceDetails;
use crate::{DbtContext, ModelDetails};

mod compiled_findings;
mod compiled_route;
pub mod diagnostics;
pub mod diagnostics_config;
mod drive;
mod env;
mod facts;
mod passes;
mod phys;
mod report;
pub mod schemas;
pub mod suppress;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_compiled;
#[cfg(test)]
mod tests_facts;
mod trace;

pub use report::{
    EvidenceView, FindingView, FixEditView, FixView, LevelReport, OriginView, Placement,
    PlanLevels, PlanStatus, QueryPlan,
};
pub use trace::{NeedsView, TraceRow, TraceView};

/// What the shell found at the model's `target/compiled` path. The file
/// and its mtime are read off the dispatcher task, so this stays pure.
#[derive(Debug, Clone, Copy, Default)]
pub enum CompiledInput<'a> {
    /// The shell did not look, or the model has no compiled path.
    #[default]
    NotChecked,
    Missing {
        path: &'a str,
    },
    /// Over `limits.compiled_sql_mib`.
    TooLarge {
        path: &'a str,
        bytes: u64,
        cap: u64,
    },
    Unreadable {
        path: &'a str,
        detail: &'a str,
    },
    Available {
        text: &'a str,
        path: &'a str,
        /// `Fresh` when the file is at least as new as the model on disk.
        mtime: Staleness,
    },
}

/// One model's text and the facts known about it.
#[derive(Debug, Clone, Copy)]
pub struct PlanInput<'a> {
    pub text: &'a str,
    pub dialect: &'a Dialect,
    /// The model's name, for `{{ this }}` and its own manifest entry.
    pub model: Option<&'a str>,
    pub models: &'a HashMap<String, ModelDetails>,
    pub sources: &'a HashMap<String, SourceDetails>,
    pub manifest: Option<&'a ManifestIndex>,
    pub catalog: Option<&'a CatalogIndex>,
    /// Stamped into the plan's `ProjectSchemas` and echoed in the report.
    pub schema_revision: u64,
    /// `perf.catalog_stats`: catalog statistics count as a usable fact.
    pub catalog_stats: bool,
    /// Used when the template declines on control flow.
    pub compiled: CompiledInput<'a>,
    /// The project-wide schema job's result; before the first job finishes
    /// the plan lists YAML and catalog columns as `Open`.
    pub project: Option<&'a schemas::SchemaSnapshot>,
}

impl<'a> PlanInput<'a> {
    /// The input for `text` over a context snapshot and loaded artifacts.
    pub fn from_context(
        ctx: &'a DbtContext,
        text: &'a str,
        model: Option<&'a str>,
        artifacts: &'a ArtifactSet,
        schema_revision: u64,
    ) -> Self {
        PlanInput {
            text,
            dialect: &ctx.dialect,
            model,
            models: &ctx.model_detail_map,
            sources: &ctx.source_detail_map,
            manifest: artifacts.manifest.as_deref(),
            catalog: artifacts.catalog.as_deref(),
            schema_revision,
            catalog_stats: true,
            compiled: CompiledInput::NotChecked,
            project: None,
        }
    }

    /// The same input with the shell's reading of the compiled file.
    pub fn with_compiled(self, compiled: CompiledInput<'a>) -> Self {
        PlanInput { compiled, ..self }
    }

    /// Sets the project snapshot this plan binds against.
    pub fn with_project(mut self, project: Option<&'a schemas::SchemaSnapshot>) -> Self {
        self.project = project;
        self
    }
}

/// Drives `input` through every level. Never panics: gaps become
/// degradations, verifier violations drop their level and everything
/// below it, and a model the template cannot express comes back
/// `Declined` with guidance.
pub fn query_plan(input: &PlanInput<'_>) -> QueryPlan {
    let kind = facts::backend_for(input.dialect);
    // `for_backend` is total today; a future gap degrades to generic
    let Some(backend) =
        for_backend(kind).or_else(|| for_backend(cogitator_ir::lir::Backend::Generic))
    else {
        return QueryPlan::no_backend(input, kind);
    };
    drive::drive(input, backend, &lower_to_mir)
}
