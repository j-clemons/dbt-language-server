//! End-to-end suite (specs/rust-port/05 §7) — this file's theme only;
//! the framed-LSP harness lives in tests/harness/.

mod harness;
use harness::jaffle;

/// Exposures as graph citizens (03 §10): symbol search, the project
/// DAG, and dangling-target warnings on the YAML entry.
#[test]
fn exposures_join_the_graph() {
    let project = jaffle();
    let root = project.root.clone();
    let exposures_yml = root.join("models/exposures.yml");
    std::fs::write(
        &exposures_yml,
        "version: 2\nexposures:\n  - name: weekly_kpis\n    type: dashboard\n    depends_on:\n      - ref('customers')\n      - ref('customrs')\n",
    )
    .unwrap();

    let (mut lsp, _uri) = project.booted();

    // workspace/symbol with the exposure: prefix
    let symbols = lsp.request(
        "workspace/symbol",
        serde_json::json!({"query": "exposure:"}),
    );
    assert!(
        symbols["result"]
            .as_array()
            .is_some_and(|s| s.iter().any(|x| x["name"] == "weekly_kpis")),
        "{symbols}"
    );

    // the DAG carries the exposure node and its edge
    let dag = lsp.request(
        "workspace/executeCommand",
        serde_json::json!({"command": "dbt.projectGraph", "arguments": [{}]}),
    );
    let node = dag["result"]["nodes"]
        .as_array()
        .and_then(|n| n.iter().find(|x| x["name"] == "weekly_kpis").cloned())
        .expect("exposure node");
    assert_eq!(node["kind"], "exposure", "{node}");
    assert!(
        dag["result"]["edges"].as_array().is_some_and(|e| e
            .iter()
            .any(|x| x["from"] == "model:customers" && x["to"] == "exposure:weekly_kpis")),
        "{dag}"
    );

    // the dangling target warns at the exposure's entry, with a hint
    let exposures_uri = cogitator_lsp::fs_path_to_uri(&exposures_yml);
    let text = std::fs::read_to_string(&exposures_yml).unwrap();
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": exposures_uri, "languageId": "yaml",
            "version": 1, "text": text}}),
    );
    let pulled = lsp.request(
        "textDocument/diagnostic",
        serde_json::json!({"textDocument": {"uri": exposures_uri}}),
    );
    let dangling = pulled["result"]["items"]
        .as_array()
        .and_then(|i| {
            i.iter()
                .find(|d| d["code"] == "dangling-exposure-ref")
                .cloned()
        })
        .expect("dangling exposure warning");
    assert!(
        dangling["message"]
            .as_str()
            .is_some_and(|m| m.contains("'customrs'") && m.contains("customers")),
        "{dangling}"
    );

    // impact: stg_customers feeds customers (and through it the
    // exposure); the column closure traces customer_id into customers
    let impact = lsp.request(
        "workspace/executeCommand",
        serde_json::json!({"command": "dbt.impact",
                           "arguments": [{"model": "stg_customers", "column": "customer_id"}]}),
    );
    let report = &impact["result"];
    assert!(
        report["models"].as_array().is_some_and(|m| m
            .iter()
            .any(|n| n["name"] == "customers" && n["depth"] == 1)),
        "{report}"
    );
    assert!(
        report["exposures"]
            .as_array()
            .is_some_and(|e| e.iter().any(|x| x == "weekly_kpis")),
        "{report}"
    );
    assert!(
        report["summary"]
            .as_str()
            .is_some_and(|t| t.starts_with("stg_customers.customer_id feeds")),
        "{report}"
    );
    assert!(
        report["columnHits"].as_array().is_some_and(|h| h
            .iter()
            .any(|x| x["model"] == "customers" && x["column"] == "customer_id")),
        "{report}"
    );

    // dead models: the exposure keeps 'customers' alive; customer_orders
    // has no dependents and no selector naming it
    let dead = lsp.request(
        "workspace/executeCommand",
        serde_json::json!({"command": "dbt.deadModels", "arguments": [{}]}),
    );
    let names: Vec<&str> = dead["result"]["models"]
        .as_array()
        .expect("models")
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect();
    assert!(names.contains(&"customer_orders"), "{names:?}");
    assert!(!names.contains(&"customers"), "{names:?}");

    lsp.shutdown();
}

/// Package awareness (03 §10): two-arg ref('pkg','model') resolves to
/// the package model (and never false-flags the package name), and a
/// declared-but-not-installed package warns in packages.yml.
#[test]
fn package_awareness_two_arg_ref_and_deps() {
    let project = jaffle();
    let root = project.root.clone();
    let packages_yml = root.join("packages.yml");
    std::fs::write(
        &packages_yml,
        "packages:\n  - package: dbt-labs/dbt_utils\n    version: 1.3.0\n  - local: dbt_packages/jaffle_package\n",
    )
    .unwrap();

    let (mut lsp, _uri) = project.booted();

    // a probe model using the two-arg form
    let probe_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/pkg_probe.sql"));
    let text = "select * from {{ ref('jaffle_package', 'stg_customer_status') }}\n";
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": probe_uri, "languageId": "sql",
            "version": 1, "text": text}}),
    );
    // no dangling-ref false positive on the package name
    let pulled = lsp.request(
        "textDocument/diagnostic",
        serde_json::json!({"textDocument": {"uri": probe_uri}}),
    );
    assert!(
        pulled["result"]["items"]
            .as_array()
            .is_some_and(|i| i.iter().all(|d| d["code"] != "dangling-ref")),
        "{pulled}"
    );
    // definition on the MODEL argument jumps into the package
    let character = text.find("stg_customer_status").unwrap() as u32 + 2;
    let definition = lsp.request(
        "textDocument/definition",
        serde_json::json!({"textDocument": {"uri": probe_uri},
                           "position": {"line": 0, "character": character}}),
    );
    assert!(
        definition["result"]["uri"].as_str().is_some_and(
            |u| u.ends_with("dbt_packages/jaffle_package/models/stg_customer_status.sql")
        ),
        "{definition}"
    );

    // packages.yml: the uninstalled hub package warns, the local one not
    let packages_uri = cogitator_lsp::fs_path_to_uri(&packages_yml);
    let text = std::fs::read_to_string(&packages_yml).unwrap();
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": packages_uri, "languageId": "yaml",
            "version": 1, "text": text}}),
    );
    let pulled = lsp.request(
        "textDocument/diagnostic",
        serde_json::json!({"textDocument": {"uri": packages_uri}}),
    );
    let items = pulled["result"]["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(
        items[0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("'dbt_utils'") && m.contains("dbt deps")),
        "{items:?}"
    );

    lsp.shutdown();
}

/// workspace/diagnostic (01 §6.4): project-wide dangling findings,
/// full → unchanged on the same revision, and stale clean files
/// answering an empty full report.
#[test]
fn workspace_diagnostics_full_then_unchanged() {
    let project = jaffle();
    let root = project.root.clone();
    std::fs::write(
        root.join("models/dangler.sql"),
        "select * from {{ ref('customrs') }}\n",
    )
    .unwrap();

    let (mut lsp, _uri) = project.booted();
    let report = lsp.request("workspace/diagnostic", serde_json::json!({}));
    let items = report["result"]["items"].as_array().expect("items").clone();
    let dangler = items
        .iter()
        .find(|i| {
            i["uri"]
                .as_str()
                .is_some_and(|u| u.ends_with("dangler.sql"))
        })
        .expect("dangler finding");
    assert_eq!(dangler["kind"], "full", "{dangler}");
    assert!(
        dangler["items"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("'customrs'") && m.contains("customers")),
        "{dangler}"
    );
    let result_id = dangler["resultId"].as_str().expect("resultId").to_string();

    // same revision → unchanged
    let second = lsp.request(
        "workspace/diagnostic",
        serde_json::json!({"previousResultIds": [
            {"uri": dangler["uri"], "value": result_id},
            {"uri": "file:///gone/clean.sql", "value": "0"}
        ]}),
    );
    let second_items = second["result"]["items"].as_array().expect("items");
    let again = second_items
        .iter()
        .find(|i| i["uri"] == dangler["uri"])
        .expect("tracked uri");
    assert_eq!(again["kind"], "unchanged", "{again}");
    // the clean tracked file clears with an empty full report
    let cleared = second_items
        .iter()
        .find(|i| i["uri"] == "file:///gone/clean.sql")
        .expect("stale uri");
    assert_eq!(cleared["kind"], "full", "{cleared}");
    assert_eq!(
        cleared["items"].as_array().map(Vec::len),
        Some(0),
        "{cleared}"
    );

    lsp.shutdown();
}

/// With the project schemas in hand a `select *` passes a column through
/// to a named output, so the column closure can trace it.
#[test]
fn impact_traces_a_column_through_a_star_over_a_closed_schema() {
    let project = jaffle();
    let star = project.root.join("models/star_reader.sql");
    std::fs::write(&star, "select * from {{ ref('stg_customers') }}\n").unwrap();
    let (mut lsp, _uri) = project.booted();

    let report = lsp.wait_until("the star expands in dbt.impact", |lsp| {
        let impact = lsp.request(
            "workspace/executeCommand",
            serde_json::json!({"command": "dbt.impact",
                               "arguments": [{"model": "stg_customers", "column": "customer_id"}]}),
        );
        let report = impact["result"].clone();
        report["columnHits"]
            .as_array()
            .is_some_and(|hits| {
                hits.iter()
                    .any(|h| h["model"] == "star_reader" && h["column"] == "customer_id")
            })
            .then_some(report)
    });
    assert!(report["compiledLineage"].is_null(), "{report}");
    lsp.shutdown();
}
