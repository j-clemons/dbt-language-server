//! End-to-end suite (specs/rust-port/05 §7) — this file's theme only;
//! the framed-LSP harness lives in tests/harness/.

mod harness;
use harness::jaffle;

/// live (the compiledSql wire cap), a parse failure keeps the old values
/// and surfaces as Guidance, and deleting the file reverts to launch
/// values — all driven through dbt.reloadConfig, never the 2 s poll.
#[test]
fn config_file_hot_reload() {
    let project = jaffle();
    let root = project.root.clone();
    let config = root.join("cogitator.yml");
    std::fs::write(&config, "profiles:\n  target: dev\n").unwrap();

    let (mut lsp, _uri) = project.booted();

    // 1. the boot pass loaded the file and applied profiles.target
    let status = lsp
        .wait_status(|s| s["config"]["loaded"] == true && s["profiles"]["activeTarget"] == "dev");
    assert!(
        status["config"]["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("cogitator.yml")),
        "{status}"
    );
    assert!(status["config"]["error"].is_null(), "{status}");

    // 2. hot re-tune: a 0 MiB wire cap flips compiledSql to uri-only
    let compiled_dir = root.join("target/compiled/jaffle_shop/models");
    std::fs::create_dir_all(&compiled_dir).unwrap();
    std::fs::write(compiled_dir.join("customers.sql"), "select 1 as id\n").unwrap();
    std::fs::write(
        &config,
        "profiles:\n  target: dev\nlimits:\n  compiled_sql_mib: 0\n",
    )
    .unwrap();
    lsp.reload_config();
    let capped = lsp.wait_until("the 0 MiB wire cap applies to compiledSql", |lsp| {
        let reply = lsp.request(
            "workspace/executeCommand",
            serde_json::json!({"command": "dbt.compiledSql",
                               "arguments": [{"model": "customers"}]}),
        );
        (reply["result"]["content"].is_null() && reply["result"]["uri"].is_string())
            .then_some(reply)
    });
    assert!(
        capped["result"]["uri"]
            .as_str()
            .is_some_and(|u| u.ends_with("customers.sql")),
        "{capped}"
    );
    // the cap explanation reached the client as a showMessage
    let cap_note = lsp.wait_notification("window/showMessage", |m| {
        m["params"]["message"]
            .as_str()
            .is_some_and(|t| t.contains("wire cap"))
    });
    assert_eq!(cap_note["params"]["type"], 2, "a warning: {cap_note}");

    // 3. a file that stops parsing: error surfaces, old values stay
    std::fs::write(&config, "profiles:\n  dir: [unclosed\n").unwrap();
    lsp.reload_config();
    let broken = lsp.wait_status(|s| s["config"]["error"].is_string());
    assert_eq!(broken["profiles"]["activeTarget"], "dev", "{broken}");

    // 4. deleting the file reverts its keys to the launch values
    std::fs::remove_file(&config).unwrap();
    lsp.reload_config();
    let reverted = lsp.wait_status(|s| s["config"]["loaded"] == false);
    assert!(reverted["config"]["path"].is_null(), "{reverted}");
    assert_eq!(reverted["profiles"]["activeTarget"], "", "{reverted}");

    lsp.shutdown();
}

/// cogitator.yml intelligence (03 §10): an open config file gets its
/// OWN diagnostics pass — unknown-key warnings from the shared schema,
/// never the dbt-properties pass that used to flag every root key.
#[test]
fn cogitator_yml_gets_config_intelligence() {
    let project = jaffle();
    let root = project.root.clone();
    let config = root.join("cogitator.yml");
    let text = "lint:\n  select_star: true\n  bogus: 1\n";
    std::fs::write(&config, text).unwrap();

    let (mut lsp, _uri) = project.booted();
    let config_uri = cogitator_lsp::fs_path_to_uri(&config);
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": config_uri, "languageId": "yaml",
            "version": 1, "text": text}}),
    );
    let pulled = lsp.request(
        "textDocument/diagnostic",
        serde_json::json!({"textDocument": {"uri": config_uri}}),
    );
    let items = pulled["result"]["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "exactly the unknown-key warning: {pulled}");
    assert!(
        items[0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("'bogus'") && m.contains("under lint:")),
        "{items:?}"
    );
    // completion inside lint: offers the checks, documented
    let completion = lsp.request(
        "textDocument/completion",
        serde_json::json!({"textDocument": {"uri": config_uri},
                           "position": {"line": 1, "character": 2}}),
    );
    let labels: Vec<&str> = completion["result"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|i| i["label"].as_str())
        .collect();
    assert!(labels.contains(&"dead_models"), "{labels:?}");
    // hover documents the key from the schema table
    let hover = lsp.request(
        "textDocument/hover",
        serde_json::json!({"textDocument": {"uri": config_uri},
                           "position": {"line": 1, "character": 4}}),
    );
    assert!(
        hover["result"]["contents"]
            .as_str()
            .is_some_and(|c| c.contains("SELECT *")),
        "{hover}"
    );

    lsp.shutdown();
}

/// Opt-in lint, hot: no items while `lint:` is absent; writing
/// cogitator.yml with select_star: true and forcing a reload makes the
/// next pull carry the warning (with its column-list quickfix data).
#[test]
fn lint_toggles_hot_through_cogitator_yml() {
    let project = jaffle();
    let root = project.root.clone();

    let (mut lsp, _uri) = project.booted();
    let probe_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/mart_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": probe_uri, "languageId": "sql", "version": 1,
            "text": "select * from {{ ref('stg_customers') }}\n"}}),
    );
    let quiet = lsp.request(
        "textDocument/diagnostic",
        serde_json::json!({"textDocument": {"uri": probe_uri}}),
    );
    assert!(
        quiet["result"]["items"]
            .as_array()
            .is_some_and(|i| i.iter().all(|d| d["code"] != "lint-select-star")),
        "{quiet}"
    );

    std::fs::write(root.join("cogitator.yml"), "lint:\n  select_star: true\n").unwrap();
    lsp.reload_config();
    let flagged = lsp.wait_until("the select_star lint switches on", |lsp| {
        let pulled = lsp.request(
            "textDocument/diagnostic",
            serde_json::json!({"textDocument": {"uri": probe_uri}}),
        );
        pulled["result"]["items"]
            .as_array()
            .and_then(|i| i.iter().find(|d| d["code"] == "lint-select-star").cloned())
    });
    assert!(
        flagged["message"]
            .as_str()
            .is_some_and(|m| m.contains("SELECT *")),
        "{flagged}"
    );
    assert!(
        flagged["data"]["suggestions"][0]
            .as_str()
            .is_some_and(|c| c.contains("customer_id")),
        "{flagged}"
    );

    lsp.shutdown();
}

/// Persisted scan cache (03 §10): the first session writes
/// target/.cogitator-cache; a fresh process seeds from it and its
/// first refresh serves stamp hits instead of re-reading the project.
#[test]
fn scan_cache_persists_across_sessions() {
    let project = jaffle();
    let root = project.root.clone();

    {
        let (lsp, _uri) = project.booted();
        assert_eq!(lsp.shutdown().code(), Some(0));
    }
    let cache = root.join("target/.cogitator-cache");
    assert!(cache.is_file(), "first session persisted the scan cache");
    let text = std::fs::read_to_string(&cache).unwrap();
    assert!(
        text.starts_with("{\"format\":4,"),
        "versioned header: {}",
        &text[..60.min(text.len())]
    );

    // second session: the first refresh hits instead of re-reading
    let (mut lsp, _uri) = project.booted();
    let status = lsp.request(
        "workspace/executeCommand",
        serde_json::json!({"command": "dbt.status", "arguments": [{}]}),
    );
    let hits = status["result"]["refresh"]["cacheHits"]
        .as_u64()
        .unwrap_or(0);
    assert!(hits > 0, "warm start served no scan-cache hits: {status}");

    lsp.shutdown();
}
