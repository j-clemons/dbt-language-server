//! `cross.select_star_chain` through the real front end and pass manager.

#[allow(dead_code)] // shared with the other rule test crates
mod join_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use cogitator_ir::common::Materialization;
use cogitator_ir::schema::{Completeness, ModelSchema, ProjectSchemas};
use cogitator_rules::{DagFacts, Evidence, Finding, FixSafety, Need, Needs};
use join_support::{apply, model, run_raw_with, Extras};

const RULE: &str = "cross.select_star_chain";

const STAR: &str = "select * from {{ ref('up') }}";

fn dag(columns: &[&str]) -> DagFacts {
    DagFacts {
        downstream_columns: BTreeMap::from([(
            Arc::from("orders"),
            columns.iter().map(|c| Arc::from(*c)).collect(),
        )]),
        ..DagFacts::default()
    }
}

fn needs() -> Needs {
    Needs::NONE
        .with(Need::Schema)
        .with(Need::Dag)
        .with(Need::Mat)
}

fn up() -> ProjectSchemas {
    let mut out = ProjectSchemas::default();
    out.models.insert(
        Arc::from("up"),
        model(&[("a", false), ("b", false), ("c", false), ("d", false)]),
    );
    out
}

fn run_with(
    sql: &str,
    schemas: ProjectSchemas,
    mat: Materialization,
    facts: &DagFacts,
    text: bool,
) -> Vec<Finding> {
    run_raw_with(
        sql,
        schemas,
        needs(),
        Extras {
            mat: Some(mat),
            dag: Some(facts),
            no_text: !text,
        },
    )
    .findings
    .into_iter()
    .filter(|f| f.id.as_str() == RULE)
    .collect()
}

fn run(sql: &str, mat: Materialization, facts: &DagFacts) -> Vec<Finding> {
    run_with(sql, up(), mat, facts, true)
}

fn only(mut findings: Vec<Finding>) -> Finding {
    assert_eq!(findings.len(), 1, "{findings:#?}");
    findings.pop().expect("one")
}

#[test]
fn a_table_reports_how_little_is_read_and_offers_an_intent_changing_expand() {
    let facts = dag(&["a", "c"]);
    let finding = only(run(STAR, Materialization::Table, &facts));
    assert_eq!(finding.evidence, Evidence::Exact);
    assert!(
        finding.message.contains("2 of 4") && finding.message.contains("in this project"),
        "{}",
        finding.message
    );
    assert_eq!(finding.fixes.len(), 1);
    assert_eq!(finding.fixes[0].safety, FixSafety::ChangesIntent);
    assert_eq!(
        apply(STAR, &finding.fixes[0]),
        "select a, c from {{ ref('up') }}"
    );
}

#[test]
fn an_incremental_model_is_also_a_stored_boundary() {
    let facts = dag(&["a"]);
    assert_eq!(
        run(STAR, Materialization::Incremental, &facts).len(),
        1,
        "incremental models store every column they select"
    );
}

#[test]
fn views_and_ephemerals_stay_silent() {
    let facts = dag(&["a"]);
    for mat in [Materialization::View, Materialization::Ephemeral] {
        assert!(run(STAR, mat, &facts).is_empty(), "{mat:?}");
    }
}

#[test]
fn no_downstream_reader_makes_no_claim() {
    let silent = run(STAR, Materialization::Table, &DagFacts::default());
    assert!(silent.is_empty(), "no reader means no comparison");
}

#[test]
fn every_column_read_is_silent() {
    let facts = dag(&["a", "b", "c", "d"]);
    assert!(run(STAR, Materialization::Table, &facts).is_empty());
}

#[test]
fn a_dbt_test_counts_as_a_read() {
    let mut facts = dag(&["a", "b", "c"]);
    facts.tested_columns.push(Arc::from("d"));
    assert!(run(STAR, Materialization::Table, &facts).is_empty());
}

#[test]
fn a_star_over_an_open_upstream_cannot_count_columns() {
    let mut schemas = up();
    let open = ModelSchema {
        completeness: Completeness::Open,
        ..(*schemas.models["up"]).clone()
    };
    schemas.models.insert(Arc::from("up"), Arc::new(open));
    let facts = dag(&["a"]);
    assert!(run_with(STAR, schemas, Materialization::Table, &facts, true).is_empty());
}

#[test]
fn catalog_backed_columns_carry_their_age() {
    let mut facts = dag(&["a"]);
    facts.catalog_generated_at = Some(Arc::from("2026-01-01T00:00:00Z"));
    let finding = only(run(STAR, Materialization::Table, &facts));
    assert_eq!(
        finding.evidence,
        Evidence::Catalog {
            generated_at: Arc::from("2026-01-01T00:00:00Z")
        }
    );
}

#[test]
fn only_exact_stars_get_an_edit() {
    let facts = dag(&["a"]);
    let qualified = "select u.* from {{ ref('up') }} as u";
    let finding = only(run(qualified, Materialization::Table, &facts));
    assert!(
        finding.fixes.is_empty(),
        "qualified stars are not rewritten"
    );
    let excluded = "select * exclude (b) from {{ ref('up') }}";
    assert!(only(run(excluded, Materialization::Table, &facts))
        .fixes
        .is_empty());
    let no_text = run_with(STAR, up(), Materialization::Table, &facts, false);
    assert!(only(no_text).fixes.is_empty(), "no text, no edit");
}

#[test]
fn a_used_reserved_word_withholds_the_edit() {
    let mut schemas = ProjectSchemas::default();
    schemas.models.insert(
        Arc::from("up"),
        model(&[("order", false), ("b", false), ("c", false)]),
    );
    let facts = dag(&["order"]);
    let finding = only(run_with(
        STAR,
        schemas,
        Materialization::Table,
        &facts,
        true,
    ));
    assert!(finding.fixes.is_empty(), "{finding:#?}");
}

#[test]
fn nothing_used_reports_but_offers_no_empty_select() {
    let facts = dag(&["unrelated"]);
    let finding = only(run(STAR, Materialization::Table, &facts));
    assert!(finding.message.contains("0 of 4"), "{}", finding.message);
    assert!(finding.fixes.is_empty());
}

#[test]
fn a_named_select_list_is_not_a_star() {
    let facts = dag(&["a"]);
    assert!(run(
        "select a, b from {{ ref('up') }}",
        Materialization::Table,
        &facts
    )
    .is_empty());
}

#[test]
fn a_cte_star_chain_counts_the_final_select() {
    let chain = "with x as (select * from {{ ref('up') }}) select * from x";
    let facts = dag(&["a", "b"]);
    let finding = only(run(chain, Materialization::Table, &facts));
    assert!(finding.message.contains("2 of 4"), "{}", finding.message);
}

#[test]
fn a_jinja_bearing_model_is_handled() {
    let facts = dag(&["a"]);
    let sql = "{{ config(materialized='table') }}\nselect * from {{ ref('up') }} where a > {{ var('min') }}";
    let finding = only(run(sql, Materialization::Table, &facts));
    assert_eq!(finding.fixes.len(), 1, "{finding:#?}");
}
