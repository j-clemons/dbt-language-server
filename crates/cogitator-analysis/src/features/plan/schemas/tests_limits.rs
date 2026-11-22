//! The job's degradations: cycles, missing upstreams and bounds.

use cogitator_ir::schema::Completeness;
use cogitator_ir::Nullability;

use super::tests::{completeness, names, run, run_with, Files, Project};
use super::*;

#[test]
fn a_cycle_degrades_without_hanging() {
    let project = Project::new()
        .sql("a", "select * from {{ ref('b') }}")
        .sql("b", "select * from {{ ref('a') }}")
        .sql("c", "select 1 as ok");
    let (ctx, files) = (project.ctx(), Files::default());
    let snapshot = run(&ctx, &files, &mut JobState::default());
    assert!(snapshot
        .report
        .degraded
        .iter()
        .any(|d| d.issue == JobIssue::Cycle));
    assert_eq!(completeness(&snapshot, "c"), Completeness::Closed);
    assert!(snapshot.dag.is_unknown("a") || snapshot.dag.is_unknown("b"));
}

#[test]
fn a_missing_upstream_degrades_and_stays_open() {
    let project = Project::new().sql("m", "select * from {{ ref('ghost') }}");
    let (ctx, files) = (project.ctx(), Files::default());
    let snapshot = run(&ctx, &files, &mut JobState::default());
    assert!(snapshot.report.degraded.iter().any(|d| matches!(
        &d.issue,
        JobIssue::MissingUpstream(name) if name == "ghost"
    )));
    assert_eq!(completeness(&snapshot, "m"), Completeness::Open);
}

#[test]
fn the_column_cap_degrades_to_open() {
    let project = Project::new().sql("m", "select 1 as a, 2 as b, 3 as c");
    let (ctx, files) = (project.ctx(), Files::default());
    let limits = JobLimits {
        max_columns: 2,
        ..JobLimits::default()
    };
    let snapshot = run_with(&ctx, &files, &mut JobState::default(), None, limits);
    assert_eq!(completeness(&snapshot, "m"), Completeness::Open);
    assert_eq!(names(&snapshot, "m").len(), 2);
    assert!(snapshot
        .report
        .degraded
        .iter()
        .any(|d| d.issue == JobIssue::ColumnCap));
}

#[test]
fn the_model_cap_stops_inference_explicitly() {
    let project = Project::new()
        .sql("a", "select 1 as a")
        .sql("b", "select 1 as b");
    let (ctx, files) = (project.ctx(), Files::default());
    let limits = JobLimits {
        max_models: 1,
        ..JobLimits::default()
    };
    let snapshot = run_with(&ctx, &files, &mut JobState::default(), None, limits);
    assert_eq!(snapshot.report.rebuilt, 1);
    assert!(snapshot
        .report
        .degraded
        .iter()
        .any(|d| d.issue == JobIssue::ModelCap));
}

#[test]
fn unique_and_not_null_tests_become_facts() {
    let project = Project::new()
        .sql("m", "select id, maybe, filtered, warned, dt from {{ source('s', 't') }}")
        .yaml(concat!(
            "version: 2\nmodels:\n  - name: m\n    columns:\n",
            "      - name: id\n        tests: [unique, not_null]\n",
            "      - name: maybe\n",
            "      - name: filtered\n        tests:\n          - not_null:\n              config: {where: \"x > 1\"}\n",
            "      - name: warned\n        tests:\n          - unique:\n              config: {severity: warn}\n",
            "      - name: dt\n        data_tests: [unique]\n",
        ));
    let (ctx, files) = (project.ctx(), Files::default());
    let snapshot = run(&ctx, &files, &mut JobState::default());
    let schema = snapshot.schemas.model("m").expect("schema");
    let col = |name: &str| {
        schema
            .columns
            .iter()
            .find(|c| &*c.name == name)
            .expect(name)
    };
    assert!(col("id").unique);
    assert_eq!(col("id").ty.null, Nullability::NotNull);
    assert!(!col("maybe").unique);
    assert_ne!(
        col("filtered").ty.null,
        Nullability::NotNull,
        "where: weakens it"
    );
    assert!(!col("warned").unique, "severity: warn does not prove it");
    assert!(col("dt").unique, "data_tests counts");
}
