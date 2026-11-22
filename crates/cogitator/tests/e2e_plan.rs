//! `dbt.queryPlan` end to end (specs/ir-pipeline/09) on per-test copies
//! of the jaffle fixture.

mod harness;
use harness::jaffle;
use serde_json::{json, Value};

fn plan(lsp: &mut harness::Lsp, uri: &str) -> Value {
    lsp.request(
        "workspace/executeCommand",
        json!({"command": "dbt.queryPlan", "arguments": [{"uri": uri}]}),
    )
}

#[test]
fn query_plan_shows_every_level_for_a_jinja_model() {
    let project = jaffle();
    // customers.sql is what `booted` opens: live text, no compiled file
    let (mut lsp, uri) = project.booted();
    let response = plan(&mut lsp, &uri);
    let result = &response["result"];
    assert_eq!(result["model"], "customers", "{response}");
    assert_eq!(result["backend"], "DuckDb", "{response}");
    assert_ne!(result["status"], "declined", "{response}");
    assert_eq!(result["guidance"], Value::Null, "{response}");

    for level in ["hir", "mir", "lir"] {
        let report = &result["levels"][level];
        assert_eq!(report["available"], true, "{level}: {report}");
        assert_eq!(report["violations"], json!([]), "{level}: {report}");
        assert!(
            report["text"].as_str().is_some_and(|t| !t.is_empty()),
            "{level}: {report}"
        );
    }
    let entries = result["trace"]["entries"].as_array().expect("trace");
    assert!(
        entries.iter().any(|e| e["rule"] == "canon.simplify"),
        "{response}"
    );
    // no manifest or catalog in the copy: the facts the rules need are absent
    assert!(
        entries.iter().any(|e| e["outcome"] == "skipped"
            && e["reason"]
                .as_str()
                .is_some_and(|r| r.starts_with("unmet:"))),
        "{response}"
    );
    assert!(
        result["needs"]["missing"]
            .as_array()
            .is_some_and(|m| m.contains(&json!("phys"))),
        "{response}"
    );

    lsp.shutdown();
}

/// What `dbt compile` writes for models/orders.sql, reduced to two payment
/// methods and rendered with the fixture's relation names.
const ORDERS_COMPILED: &str = "\n\nwith orders as (\n\n    select * from \"jaffle\".\"main\".\"stg_orders\"\n\n),\n\npayments as (\n\n    select * from \"jaffle\".\"main\".\"stg_payments\"\n\n),\n\norder_payments as (\n\n    select\n        order_id,\n\n        sum(case when payment_method = 'credit_card' then amount else 0 end) as credit_card_amount,\n        sum(case when payment_method = 'coupon' then amount else 0 end) as coupon_amount,\n        sum(amount) as total_amount\n\n    from payments\n\n    group by order_id\n\n),\n\nfinal as (\n\n    select\n        orders.order_id,\n        orders.customer_id,\n        orders.order_date,\n        orders.status,\n\n        order_payments.credit_card_amount,\n\n        order_payments.coupon_amount,\n\n        order_payments.total_amount as amount\n\n    from orders\n\n\n    left join order_payments\n        on orders.order_id = order_payments.order_id\n\n)\n\nselect * from final\n";

fn write_orders_compiled(project: &harness::fixture::TestProject) {
    let dir = project.root.join("target/compiled/jaffle_shop/models");
    std::fs::create_dir_all(&dir).expect("compiled dir");
    std::fs::write(dir.join("orders.sql"), ORDERS_COMPILED).expect("compiled file");
}

#[test]
fn query_plan_falls_back_to_compiled_sql_for_control_flow() {
    let project = jaffle();
    write_orders_compiled(&project);
    let (mut lsp, _uri) = project.booted();
    let orders = cogitator_lsp::fs_path_to_uri(&project.root.join("models/orders.sql"));
    let response = plan(&mut lsp, &orders);
    let result = &response["result"];
    assert_eq!(result["origin"]["kind"], "compiled", "{response}");
    assert_eq!(result["origin"]["staleness"], "Fresh", "{response}");
    assert_eq!(result["banner"], Value::Null, "{response}");
    assert_ne!(result["status"], "declined", "{response}");
    let hir = result["levels"]["hir"]["text"].as_str().expect("hir");
    assert!(hir.contains("model stg_orders"), "{hir}");
    assert_eq!(
        result["levels"]["hir"]["violations"],
        json!([]),
        "{response}"
    );
    assert_eq!(result["levels"]["mir"]["available"], true, "{response}");
    lsp.shutdown();
}

#[test]
fn query_plan_marks_a_compiled_plan_stale_after_an_unsaved_edit() {
    let project = jaffle();
    write_orders_compiled(&project);
    let (mut lsp, _uri) = project.booted();
    let path = project.root.join("models/orders.sql");
    let orders = cogitator_lsp::fs_path_to_uri(&path);
    let saved = std::fs::read_to_string(&path).expect("orders");
    let edited = saved.replace("group by order_id", "group by order_id, customer_id");
    assert_ne!(saved, edited);
    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": orders, "languageId": "sql", "version": 1, "text": edited}}),
    );
    let response = plan(&mut lsp, &orders);
    let result = &response["result"];
    assert_eq!(result["origin"]["staleness"], "Stale", "{response}");
    assert!(
        result["banner"]
            .as_str()
            .is_some_and(|banner| banner.contains("dbt compile")),
        "{response}"
    );
    let findings = result["findings"].as_array().expect("findings");
    assert!(
        findings.iter().all(|f| f["diagnosticsEligible"] == false),
        "{response}"
    );
    lsp.shutdown();
}

#[test]
fn query_plan_declines_control_flow_with_guidance_when_nothing_was_compiled() {
    let project = jaffle();
    let (mut lsp, _uri) = project.booted();
    let orders = cogitator_lsp::fs_path_to_uri(&project.root.join("models/orders.sql"));
    // a `{% set %}` + `{% for %}` model, read from disk: not opened
    let response = plan(&mut lsp, &orders);
    let result = &response["result"];
    assert_eq!(result["model"], "orders", "{response}");
    assert_eq!(result["status"], "declined", "{response}");
    let guidance = result["guidance"].as_str().expect("guidance");
    assert!(guidance.contains("dbt compile"), "{guidance}");
    assert!(guidance.contains("no compiled file"), "{guidance}");
    assert_eq!(result["levels"]["hir"]["available"], false, "{response}");
    assert_eq!(result["levels"]["lir"]["available"], false, "{response}");
    assert!(
        result["trace"]["entries"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "every rule still reports why it did not run: {response}"
    );

    lsp.shutdown();
}

#[test]
fn query_plan_is_cached_per_generation_and_replans_after_an_edit() {
    let project = jaffle();
    let (mut lsp, uri) = project.booted();
    let probe_uri = cogitator_lsp::fs_path_to_uri(&project.root.join("models/plan_probe.sql"));
    let text = "select id, name from {{ ref('stg_customers') }} where id > 1\n";
    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": probe_uri, "languageId": "sql", "version": 1, "text": text}}),
    );

    // the schema job runs after the refresh; compare plans once it has
    let first = lsp.wait_until("the schema job publishes a revision", |lsp| {
        let response = plan(lsp, &probe_uri);
        (response["result"]["schemaRevision"]
            .as_u64()
            .is_some_and(|r| r > 0))
        .then_some(response)
    });
    let second = plan(&mut lsp, &probe_uri);
    assert_eq!(first["result"], second["result"], "same plan");
    let cache = lsp.request(
        "workspace/executeCommand",
        json!({"command": "dbt.cache", "arguments": [{"uri": uri}]}),
    );
    let plans = &cache["result"]["plans"];
    assert_eq!(plans["entries"], 1, "{cache}");
    assert!(plans["hits"].as_u64().is_some_and(|h| h >= 1), "{cache}");

    // an edit bumps the generation: the plan reflects the new text
    lsp.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": probe_uri, "version": 2},
            "contentChanges": [{"text": "select id from {{ ref('stg_customers') }}\n"}]}),
    );
    let after = plan(&mut lsp, &probe_uri);
    let mir = after["result"]["levels"]["mir"]["text"]
        .as_str()
        .expect("mir");
    let project_line = mir.lines().next().unwrap_or_default();
    assert!(
        project_line.contains("id#") && !project_line.contains("name"),
        "the edited text was planned: {mir}"
    );

    lsp.shutdown();
}

#[test]
fn query_plan_rejects_a_non_model_uri_with_guidance() {
    let project = jaffle();
    let (mut lsp, _uri) = project.booted();
    let schema = cogitator_lsp::fs_path_to_uri(&project.root.join("models/schema.yml"));
    let response = plan(&mut lsp, &schema);
    let message = response["error"]["message"].as_str().expect("error");
    assert!(message.contains("needs a model"), "{message}");
    lsp.shutdown();
}

#[test]
fn schema_job_gives_the_plan_a_revision_closed_schemas_and_demand() {
    let project = jaffle();
    let (star, reader) = (
        project.root.join("models/star_orders.sql"),
        project.root.join("models/reads_star.sql"),
    );
    std::fs::write(
        &star,
        "{{ config(materialized='table') }}\nselect * from {{ ref('stg_orders') }}\n",
    )
    .expect("star model");
    std::fs::write(&reader, "select order_id from {{ ref('star_orders') }}\n")
        .expect("reader model");
    let (mut lsp, _uri) = project.booted();
    let star_uri = cogitator_lsp::fs_path_to_uri(&star);

    let result = lsp.wait_until("the schema job publishes a revision", |lsp| {
        let response = plan(lsp, &star_uri);
        let result = response["result"].clone();
        (result["schemaRevision"].as_u64().is_some_and(|r| r > 0)).then_some(result)
    });
    let needs = result["needs"]["available"].as_array().expect("needs");
    assert!(needs.contains(&json!("schema")), "{result}");
    assert!(needs.contains(&json!("dag")), "{result}");
    let findings = result["findings"].as_array().expect("findings");
    let finding = findings
        .iter()
        .find(|f| f["id"] == "cross.select_star_chain")
        .expect("select-star finding");
    assert!(
        finding["message"]
            .as_str()
            .is_some_and(|m| m.contains("1 of 4")),
        "{finding}"
    );
    lsp.shutdown();
}

fn column_lineage(lsp: &mut harness::Lsp, uri: &str) -> Value {
    lsp.request(
        "workspace/executeCommand",
        json!({"command": "dbt.columnLineage", "arguments": [{"uri": uri}]}),
    )
}

#[test]
fn column_lineage_answers_control_flow_from_fresh_compiled_sql() {
    let project = jaffle();
    write_orders_compiled(&project);
    let (mut lsp, _uri) = project.booted();
    let orders = cogitator_lsp::fs_path_to_uri(&project.root.join("models/orders.sql"));

    let response = column_lineage(&mut lsp, &orders);
    let result = &response["result"];
    assert_eq!(result["origin"], "compiled", "{response}");
    let columns = result["columns"].as_array().expect("columns");
    let amount = columns
        .iter()
        .find(|c| c["name"] == "amount")
        .expect("amount column");
    assert!(
        amount["sources"]
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["name"] == "stg_payments")),
        "{amount}"
    );
    lsp.shutdown();
}

#[test]
fn column_lineage_refuses_stale_compiled_sql() {
    let project = jaffle();
    write_orders_compiled(&project);
    let model = project.root.join("models/orders.sql");
    let newer = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    std::fs::File::options()
        .write(true)
        .open(&model)
        .and_then(|f| f.set_modified(newer))
        .expect("touch model");
    let (mut lsp, _uri) = project.booted();
    let orders = cogitator_lsp::fs_path_to_uri(&model);

    let response = column_lineage(&mut lsp, &orders);
    assert_eq!(response["result"], Value::Null, "{response}");
    lsp.shutdown();
}

#[test]
fn column_lineage_refuses_compiled_sql_while_the_model_has_unsaved_edits() {
    let project = jaffle();
    write_orders_compiled(&project);
    let model = project.root.join("models/orders.sql");
    let text = std::fs::read_to_string(&model).expect("model");
    let (mut lsp, _uri) = project.booted();
    let orders = cogitator_lsp::fs_path_to_uri(&model);
    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": orders, "languageId": "sql", "version": 1, "text": text
        }}),
    );
    let response = column_lineage(&mut lsp, &orders);
    assert_eq!(response["result"]["origin"], "compiled", "{response}");
    // an edit too small for the coverage check: only the disk comparison sees it
    lsp.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": orders, "version": 2},
            "contentChanges": [{"text": format!("{text}\n-- unsaved")}]
        }),
    );
    let response = column_lineage(&mut lsp, &orders);
    assert_eq!(response["result"], Value::Null, "{response}");
    lsp.shutdown();
}
