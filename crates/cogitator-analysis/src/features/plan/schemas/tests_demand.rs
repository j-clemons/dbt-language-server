//! Downstream demand through tables, views and models it cannot read.

use super::tests::{run, Files, Project};
use super::JobState;

fn downstream(project: &Project, model: &str) -> Option<Vec<(String, Vec<String>)>> {
    let (ctx, files) = (project.ctx(), Files::default());
    let snapshot = run(&ctx, &files, &mut JobState::default());
    let facts = snapshot.dag.facts(model)?;
    Some(
        facts
            .downstream_columns
            .iter()
            .map(|(down, cols)| {
                (
                    down.to_string(),
                    cols.iter().map(|c| c.to_string()).collect(),
                )
            })
            .collect(),
    )
}

#[test]
fn a_table_consumer_reads_what_it_selects() {
    let project = Project::new()
        .sql("stg", "select 1 as a, 2 as b, 3 as c")
        .sql(
            "orders",
            "{{ config(materialized='table') }}\nselect a, b from {{ ref('stg') }} where c > 0",
        );
    assert_eq!(
        downstream(&project, "stg"),
        Some(vec![(
            "orders".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        )]),
        "the filter on c is a read"
    );
}

#[test]
fn a_view_passes_on_only_what_its_consumer_asks() {
    let project = Project::new()
        .sql("stg", "select 1 as a, 2 as b, 3 as c")
        .sql(
            "v",
            "{{ config(materialized='view') }}\nselect a, b as bee, c from {{ ref('stg') }}",
        )
        .sql(
            "t",
            "{{ config(materialized='table') }}\nselect bee from {{ ref('v') }}",
        );
    assert_eq!(
        downstream(&project, "stg"),
        Some(vec![("v".to_string(), vec!["b".to_string()])]),
        "the engine prunes through the view"
    );
}

#[test]
fn an_unconsumed_view_counts_as_reading_everything() {
    let project = Project::new().sql("stg", "select 1 as a, 2 as b").sql(
        "v",
        "{{ config(materialized='view') }}\nselect a, b from {{ ref('stg') }}",
    );
    assert_eq!(
        downstream(&project, "stg"),
        Some(vec![(
            "v".to_string(),
            vec!["a".to_string(), "b".to_string()]
        )])
    );
}

#[test]
fn a_consumer_that_cannot_be_read_makes_demand_unknown() {
    let project = Project::new().sql("stg", "select 1 as a").sql(
        "dynamic",
        "select {{ dbt_utils.star(ref('stg')) }} from {{ ref('stg') }}",
    );
    assert_eq!(downstream(&project, "stg"), None);
}

#[test]
fn an_exposure_makes_demand_unknown() {
    let project = Project::new()
        .sql("stg", "select 1 as a, 2 as b")
        .sql("t", "select a from {{ ref('stg') }}")
        .yaml(concat!(
            "version: 2\nexposures:\n  - name: dash\n    type: dashboard\n",
            "    owner: {name: x}\n    depends_on:\n      - ref('stg')\n",
        ));
    assert_eq!(downstream(&project, "stg"), None);
}

#[test]
fn a_model_with_no_downstream_has_empty_known_demand() {
    let project = Project::new().sql("alone", "select 1 as a");
    assert_eq!(downstream(&project, "alone"), Some(vec![]));
}

#[test]
fn dbt_tests_count_as_reads_without_being_consumers() {
    let project = Project::new().sql("m", "select 1 as id").yaml(
        "version: 2\nmodels:\n  - name: m\n    columns:\n      - name: id\n        tests: [unique]\n",
    );
    let (ctx, files) = (project.ctx(), Files::default());
    let snapshot = run(&ctx, &files, &mut JobState::default());
    let facts = snapshot.dag.facts("m").expect("known");
    assert!(facts.downstream_columns.is_empty());
    assert_eq!(facts.tested_columns.len(), 1);
}
