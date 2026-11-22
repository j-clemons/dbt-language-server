//! The schema job over real project files in a temp directory.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::schema::Completeness;
use cogitator_ir::SqlType;

use super::*;
use crate::artifacts::catalog::{CatalogColumn, CatalogIndex, CatalogTable};
use crate::{build_context, DbtContext};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct Project {
    pub(super) root: PathBuf,
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Project {
    pub(super) fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("cogitator_schemas_{}_{n}", std::process::id()));
        std::fs::create_dir_all(root.join("models")).expect("temp project");
        std::fs::write(
            root.join("dbt_project.yml"),
            "name: p\nversion: '1.0'\nprofile: p\nmodel-paths: ['models']\n",
        )
        .expect("project file");
        Project { root }
    }

    pub(super) fn sql(self, name: &str, text: &str) -> Self {
        self.write_sql(name, text);
        self
    }

    pub(super) fn write_sql(&self, name: &str, text: &str) {
        std::fs::write(self.root.join(format!("models/{name}.sql")), text).expect("model");
    }

    pub(super) fn yaml(self, text: &str) -> Self {
        std::fs::write(self.root.join("models/schema.yml"), text).expect("yaml");
        self
    }

    pub(super) fn ctx(&self) -> DbtContext {
        build_context(self.root.to_str().expect("utf-8 path"))
    }
}

/// Reads through to disk with a per-path stamp the test bumps to force a
/// change without depending on timestamp granularity.
#[derive(Default)]
pub(super) struct Files {
    bumped: std::cell::RefCell<HashMap<PathBuf, u64>>,
    pub(super) reads: Cell<usize>,
}

impl Files {
    pub(super) fn bump(&self, path: &Path) {
        *self
            .bumped
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_default() += 1;
    }
}

impl SavedFiles for Files {
    fn stamp(&self, path: &Path) -> Option<u64> {
        path.is_file()
            .then(|| self.bumped.borrow().get(path).copied().unwrap_or(0))
    }

    fn read(&self, path: &Path) -> Option<String> {
        self.reads.set(self.reads.get() + 1);
        std::fs::read_to_string(path).ok()
    }
}

pub(super) fn run(ctx: &DbtContext, files: &Files, state: &mut JobState) -> Arc<SchemaSnapshot> {
    run_with(ctx, files, state, None, JobLimits::default())
}

pub(super) fn run_with(
    ctx: &DbtContext,
    files: &Files,
    state: &mut JobState,
    catalog: Option<&CatalogIndex>,
    limits: JobLimits,
) -> Arc<SchemaSnapshot> {
    let dialect = Dialect::new("duckdb");
    let never = || false;
    let input = JobInput {
        dialect: &dialect,
        models: &ctx.model_detail_map,
        sources: &ctx.source_detail_map,
        graph: &ctx.graph,
        manifest: None,
        catalog,
        files,
        limits,
        stale: &never,
    };
    run_schema_job(&input, state).expect("not superseded")
}

pub(super) fn names(snapshot: &SchemaSnapshot, model: &str) -> Vec<String> {
    snapshot
        .schemas
        .model(model)
        .expect("schema")
        .columns
        .iter()
        .map(|c| c.name.to_string())
        .collect()
}

pub(super) fn completeness(snapshot: &SchemaSnapshot, model: &str) -> Completeness {
    snapshot.schemas.model(model).expect("schema").completeness
}

#[test]
fn named_columns_flow_through_a_star_chain_and_stay_closed() {
    let project = Project::new()
        .sql(
            "base",
            "select 1 as id, 'x' as name, cast(2 as bigint) as n",
        )
        .sql("mid", "select * from {{ ref('base') }}")
        .sql("top", "select id, n from {{ ref('mid') }}");
    let (ctx, files) = (project.ctx(), Files::default());
    let snapshot = run(&ctx, &files, &mut JobState::default());
    assert_eq!(names(&snapshot, "mid"), vec!["id", "name", "n"]);
    assert_eq!(names(&snapshot, "top"), vec!["id", "n"]);
    assert_eq!(completeness(&snapshot, "top"), Completeness::Closed);
    let mid = snapshot.schemas.model("mid").expect("schema");
    assert_eq!(mid.columns[2].ty.ty, SqlType::Int { bits: Some(64) });
    assert_eq!(snapshot.report.rebuilt, 3);
}

#[test]
fn a_star_over_an_unknown_relation_is_open_even_with_named_siblings() {
    let project = Project::new()
        .sql("a", "select * from {{ source('s', 't') }}")
        .sql("b", "select x, y from {{ source('s', 't') }}")
        .sql(
            "c",
            "select {{ dbt_utils.star(ref('b')) }} from {{ ref('b') }}",
        );
    let (ctx, files) = (project.ctx(), Files::default());
    let snapshot = run(&ctx, &files, &mut JobState::default());
    assert_eq!(completeness(&snapshot, "a"), Completeness::Open);
    assert!(names(&snapshot, "a").is_empty());
    // Named columns over an open source are exactly those columns.
    assert_eq!(completeness(&snapshot, "b"), Completeness::Closed);
    assert_eq!(names(&snapshot, "b"), vec!["x", "y"]);
    // Jinja expanding to columns leaves the list unknown.
    assert_eq!(completeness(&snapshot, "c"), Completeness::Open);
}

#[test]
fn quoted_and_unnamed_outputs_stay_open() {
    let project = Project::new()
        .sql("anon", "select count(*) from {{ ref('up') }}")
        .sql("up", "select 1 as a");
    let (ctx, files) = (project.ctx(), Files::default());
    let snapshot = run(&ctx, &files, &mut JobState::default());
    assert_eq!(completeness(&snapshot, "anon"), Completeness::Open);
    assert_eq!(completeness(&snapshot, "up"), Completeness::Closed);
}

fn source_details() -> Project {
    let yaml = "version: 2\nsources:\n  - name: s\n    tables:\n      - name: t\n        columns:\n          - name: a\n";
    Project::new()
        .sql("m", "select * from {{ source('s','t') }}")
        .yaml(yaml)
}

#[test]
fn a_yaml_only_source_stays_open_and_a_catalog_source_closes() {
    let project = source_details();
    let ctx = project.ctx();
    let files = Files::default();
    let yaml_only = run(&ctx, &files, &mut JobState::default());
    let schema = yaml_only.schemas.source("s", "t").expect("source");
    assert_eq!(schema.completeness, Completeness::Open);
    assert_eq!(completeness(&yaml_only, "m"), Completeness::Open);

    let catalog = catalog_with(&["a", "b"]);
    let closed = run_with(
        &ctx,
        &files,
        &mut JobState::default(),
        Some(&catalog),
        JobLimits::default(),
    );
    let schema = closed.schemas.source("s", "t").expect("source");
    assert_eq!(schema.completeness, Completeness::Closed);
    assert_eq!(schema.columns.len(), 2);
    assert_eq!(schema.columns[0].ty.ty, SqlType::Int { bits: Some(32) });
    assert_eq!(
        names(&closed, "m"),
        vec!["a", "b"],
        "the star expands over the catalog's closed list"
    );
    assert_eq!(completeness(&closed, "m"), Completeness::Closed);
}

fn catalog_with(columns: &[&str]) -> CatalogIndex {
    let mut catalog = CatalogIndex {
        generated_at: Some("2026-01-01T00:00:00Z".to_string()),
        ..CatalogIndex::default()
    };
    catalog.sources.insert(
        ("s".to_string(), "t".to_string()),
        CatalogTable {
            columns: columns
                .iter()
                .enumerate()
                .map(|(i, name)| CatalogColumn {
                    name: name.to_string(),
                    data_type: "integer".to_string(),
                    comment: None,
                    index: Some(i as i64),
                })
                .collect(),
            ..CatalogTable::default()
        },
    );
    catalog
}

#[test]
fn an_unchanged_file_is_reused_without_a_read() {
    let project = Project::new()
        .sql("base", "select 1 as id")
        .sql("next", "select * from {{ ref('base') }}");
    let (ctx, files) = (project.ctx(), Files::default());
    let mut state = JobState::default();
    run(&ctx, &files, &mut state);
    assert_eq!(files.reads.get(), 2);
    let again = run(&ctx, &files, &mut state);
    assert_eq!(files.reads.get(), 2, "cached: nothing read twice");
    assert_eq!(again.report.reused, 2);
    assert_eq!(again.revision, 1, "same content keeps the revision");
}

#[test]
fn early_cut_off_stops_at_an_unchanged_fingerprint() {
    let project = Project::new()
        .sql("base", "select 1 as id")
        .sql("next", "select * from {{ ref('base') }}");
    let (ctx, files) = (project.ctx(), Files::default());
    let mut state = JobState::default();
    let first = run(&ctx, &files, &mut state);
    // base changes on disk but keeps its schema
    project.write_sql("base", "select 1 as id -- comment");
    files.bump(&project.root.join("models/base.sql"));
    let second = run(&ctx, &files, &mut state);
    assert_eq!(second.report.rebuilt, 1, "only base re-reads");
    assert_eq!(second.report.reused, 1, "next is cut off");
    assert_eq!(second.revision, first.revision);
}

#[test]
fn a_changed_schema_propagates_and_bumps_the_revision() {
    let project = Project::new()
        .sql("base", "select 1 as id")
        .sql("next", "select * from {{ ref('base') }}");
    let (ctx, files) = (project.ctx(), Files::default());
    let mut state = JobState::default();
    let first = run(&ctx, &files, &mut state);
    project.write_sql("base", "select 1 as id, 2 as extra");
    files.bump(&project.root.join("models/base.sql"));
    let second = run(&ctx, &files, &mut state);
    assert_eq!(second.report.rebuilt, 2, "next depends on the new columns");
    assert_eq!(names(&second, "next"), vec!["id", "extra"]);
    assert_eq!(second.revision, first.revision + 1);
}
