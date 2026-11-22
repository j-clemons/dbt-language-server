//! The job's public vocabulary and its between-runs state.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::schema::ProjectSchemas;

use super::dag::DagIndex;
use super::infer::Inferred;
use crate::artifacts::catalog::CatalogIndex;
use crate::artifacts::manifest::ManifestIndex;
use crate::context::types::{ModelDetails, SourceDetails};
use crate::graph::ProjectGraph;

/// Saved file access, injected so the core stays pure.
pub trait SavedFiles {
    /// A value that changes when the file does; `None` if unreadable.
    fn stamp(&self, path: &Path) -> Option<u64>;
    /// The saved text, bounded by the implementation.
    fn read(&self, path: &Path) -> Option<String>;
}

/// Counts that bound the job; passing one degrades explicitly.
#[derive(Debug, Clone, Copy)]
pub struct JobLimits {
    pub max_models: usize,
    pub max_columns: usize,
    pub max_read_bytes: u64,
    /// Canonical subplan text the duplicate check keeps across models.
    pub max_subplan_bytes: u64,
}

impl Default for JobLimits {
    fn default() -> Self {
        JobLimits {
            max_models: 20_000,
            max_columns: 5_000,
            max_read_bytes: 1 << 30,
            max_subplan_bytes: 64 << 20,
        }
    }
}

/// Why a model's schema rests on less than its SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobIssue {
    Unreadable,
    Declined,
    Cycle,
    MissingUpstream(String),
    ModelCap,
    ReadCap,
    ColumnCap,
    /// This model and later ones were not compared for duplicate logic.
    SubplanBudget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDegradation {
    pub model: Arc<str>,
    pub issue: JobIssue,
}

impl JobDegradation {
    pub(super) fn new(model: &Arc<str>, issue: JobIssue) -> Self {
        JobDegradation {
            model: model.clone(),
            issue,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobReport {
    pub models: usize,
    pub rebuilt: usize,
    pub reused: usize,
    pub degraded: Vec<JobDegradation>,
}

/// The published result of one job.
#[derive(Debug)]
pub struct SchemaSnapshot {
    pub revision: u64,
    pub schemas: Arc<ProjectSchemas>,
    pub dag: DagIndex,
    pub report: JobReport,
    pub(super) content: u64,
}

pub(super) struct Cached {
    pub(super) key: (Option<u64>, u64),
    pub(super) inferred: Option<Arc<Inferred>>,
}

/// What survives between jobs.
#[derive(Default)]
pub struct JobState {
    pub(super) entries: HashMap<Arc<str>, Cached>,
    pub(super) previous: Option<Arc<SchemaSnapshot>>,
}

pub struct JobInput<'a> {
    pub dialect: &'a Dialect,
    pub models: &'a HashMap<String, ModelDetails>,
    pub sources: &'a HashMap<String, SourceDetails>,
    pub graph: &'a ProjectGraph,
    pub manifest: Option<&'a ManifestIndex>,
    pub catalog: Option<&'a CatalogIndex>,
    pub files: &'a dyn SavedFiles,
    pub limits: JobLimits,
    /// True once a newer job has been requested.
    pub stale: &'a dyn Fn() -> bool,
}
