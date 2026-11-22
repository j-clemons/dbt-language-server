//! The typed route: star expansion against known schemas, and the compiled
//! route's freshness gate.

use std::collections::HashMap;
use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::origin::Staleness;
use cogitator_ir::schema::{Completeness, ModelSchema, ProjectSchemas, SchemaCol};
use cogitator_ir::{ColType, Nullability, SqlType};
use cogitator_parser::{DocumentParser, JinjaSqlParser};
use cogitator_sql_front::SchemaEnv;

use super::gap_tests::render;
use super::typed::lineage_over;
use super::{column_lineage, LineageOrigin};
use crate::features::plan::{CompiledInput, PlanInput};
use crate::ModelDetails;

fn schema(names: &[&str], completeness: Completeness) -> Arc<ModelSchema> {
    let columns: Vec<SchemaCol> = names
        .iter()
        .map(|n| SchemaCol {
            name: Arc::from(*n),
            ty: ColType::new(SqlType::Unknown, Nullability::Unknown),
            unique: false,
        })
        .collect();
    Arc::new(ModelSchema {
        columns: Arc::from(columns),
        completeness,
        fingerprint: 1,
    })
}

fn env(models: &[(&str, &[&str], Completeness)]) -> SchemaEnv {
    let mut schemas = ProjectSchemas::default();
    for (name, columns, completeness) in models {
        schemas
            .models
            .insert(Arc::from(*name), schema(columns, *completeness));
    }
    SchemaEnv {
        schemas: Arc::new(schemas),
        model: Some(Arc::from("m")),
    }
}

/// A plan input with no compiled file; models named so relations resolve.
fn plan_input<'a>(
    text: &'a str,
    dialect: &'a Dialect,
    models: &'a HashMap<String, ModelDetails>,
    sources: &'a HashMap<String, crate::context::types::SourceDetails>,
) -> PlanInput<'a> {
    PlanInput {
        text,
        dialect,
        model: Some("m"),
        models,
        sources,
        manifest: None,
        catalog: None,
        schema_revision: 1,
        catalog_stats: false,
        compiled: CompiledInput::NotChecked,
        project: None,
    }
}

fn typed(sql: &str, schema_env: &SchemaEnv) -> String {
    let dialect = Dialect::new("duckdb");
    let (models, sources) = (HashMap::new(), HashMap::new());
    let doc = JinjaSqlParser.parse(sql, &dialect).expect("parse");
    let input = plan_input(sql, &dialect, &models, &sources);
    let lineage = lineage_over(&doc, &input, schema_env).expect(sql);
    format!("{sql}\n---\n{}", render(&lineage))
}

const CLOSED: Completeness = Completeness::Closed;
const OPEN: Completeness = Completeness::Open;

#[test]
fn a_star_over_a_closed_model_lists_its_columns() {
    let e = env(&[("a", &["id", "name"], CLOSED)]);
    insta::assert_snapshot!(typed("select * from {{ ref('a') }}", &e));
}

#[test]
fn exclude_and_replace_apply_to_the_expansion() {
    let e = env(&[("a", &["id", "name", "b"], CLOSED)]);
    insta::assert_snapshot!(typed(
        "select * exclude (b) replace (name || id as name) from {{ ref('a') }}",
        &e
    ));
}

#[test]
fn a_star_through_a_closed_cte_expands_and_renames() {
    let e = env(&[("a", &["id", "amount"], CLOSED)]);
    insta::assert_snapshot!(typed(
        "with c(x, y) as (select * from {{ ref('a') }}) select * from c",
        &e
    ));
}

#[test]
fn an_open_or_unknown_input_keeps_the_star_row() {
    let e = env(&[("a", &["id"], OPEN), ("b", &["k"], CLOSED)]);
    // `a` is Open and `u` unknown: the star cannot list everything
    insta::assert_snapshot!(typed(
        "select * from {{ ref('a') }} join {{ ref('b') }} on true join {{ ref('u') }} on true",
        &e
    ));
}

#[test]
fn a_qualified_star_expands_only_its_own_closed_item() {
    let e = env(&[("a", &["id", "x"], CLOSED)]);
    insta::assert_snapshot!(typed(
        "select a.*, u.* from {{ ref('a') }} a join {{ ref('u') }} u on true",
        &e
    ));
}

#[test]
fn closed_inputs_resolve_unqualified_names_precisely() {
    let e = env(&[("a", &["id", "x"], CLOSED), ("b", &["id", "y"], CLOSED)]);
    insta::assert_snapshot!(typed(
        "select x, y from {{ ref('a') }} join {{ ref('b') }} on a.id = b.id",
        &e
    ));
}

#[test]
fn the_plain_route_does_not_expand_closed_ctes() {
    let dialect = Dialect::new("duckdb");
    let sql = "with c as (select a, b from {{ ref('m') }}) select * from c";
    let doc = JinjaSqlParser.parse(sql, &dialect).expect("parse");
    let lineage = column_lineage(&doc, &dialect).expect("plain");
    assert_eq!(lineage.columns.len(), 1);
    assert_eq!(lineage.columns[0].name, "*");
    assert_eq!(lineage.origin, None);
}

const TEMPLATE: &str = "with orders as (\n    select * from {{ ref('stg_orders') }}\n)\n\nselect\n    id,\n    {% for c in cols %}\n    {{ c }},\n    {% endfor %}\n    amount\nfrom orders\nwhere amount > 0\n";

const COMPILED: &str = "with orders as (\n    select * from stg_orders\n)\n\nselect\n    id,\n    \n    a,\n    \n    b,\n    \n    amount\nfrom orders\nwhere amount > 0\n";

fn compiled_answer(text: &str, compiled: CompiledInput<'_>) -> Option<super::ModelLineage> {
    let dialect = Dialect::new("duckdb");
    let models = HashMap::from([("stg_orders".to_string(), ModelDetails::default())]);
    let sources = HashMap::new();
    let doc = JinjaSqlParser.parse(text, &dialect).expect("parse");
    let input = plan_input(text, &dialect, &models, &sources).with_compiled(compiled);
    lineage_over(
        &doc,
        &input,
        &env(&[("stg_orders", &["id", "a", "b", "amount"], CLOSED)]),
    )
}

fn available(text: &str, mtime: Staleness) -> CompiledInput<'_> {
    CompiledInput::Available {
        text,
        path: "target/compiled/p/models/m.sql",
        mtime,
    }
}

#[test]
fn fresh_compiled_sql_answers_for_control_flow_and_says_so() {
    let lineage = compiled_answer(TEMPLATE, available(COMPILED, Staleness::Fresh)).expect("fresh");
    assert_eq!(lineage.origin, Some(LineageOrigin::Compiled));
    let json = serde_json::to_value(&lineage).expect("json");
    assert_eq!(json["origin"], "compiled");
    insta::assert_snapshot!(render(&lineage));
}

#[test]
fn stale_or_unknown_compiled_sql_declines() {
    for mtime in [Staleness::Stale, Staleness::Unknown] {
        assert!(
            compiled_answer(TEMPLATE, available(COMPILED, mtime)).is_none(),
            "{mtime:?}"
        );
    }
}

#[test]
fn a_compiled_file_the_edited_template_no_longer_matches_declines() {
    let edited = TEMPLATE
        .replace(
            "where amount > 0",
            "where amount > 0 and id < 100000 and amount != 12345",
        )
        .replace("orders", "orders_v2");
    assert!(compiled_answer(&edited, available(COMPILED, Staleness::Fresh)).is_none());
}

#[test]
fn without_a_compiled_file_control_flow_declines() {
    assert!(compiled_answer(TEMPLATE, CompiledInput::NotChecked).is_none());
    let missing = CompiledInput::Missing {
        path: "target/compiled/m.sql",
    };
    assert!(compiled_answer(TEMPLATE, missing).is_none());
}

#[test]
fn the_template_route_omits_the_origin_field() {
    let e = env(&[("a", &["id"], CLOSED)]);
    let dialect = Dialect::new("duckdb");
    let (models, sources) = (HashMap::new(), HashMap::new());
    let sql = "select * from {{ ref('a') }}";
    let doc = JinjaSqlParser.parse(sql, &dialect).expect("parse");
    let input = plan_input(sql, &dialect, &models, &sources);
    let lineage = lineage_over(&doc, &input, &e).expect("lineage");
    let json = serde_json::to_value(&lineage).expect("json");
    assert_eq!(json.get("origin"), None);
}
