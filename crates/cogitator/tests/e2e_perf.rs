//! Plan findings as diagnostics, end to end (specs/ir-pipeline/08): the
//! pull path computes, the push path only reuses, config hot-reloads.

mod harness;
use harness::{jaffle, Lsp, TestProject};
use serde_json::{json, Value};

const CARTESIAN: &str = "with a as (select 1 as x union all select 2 as x),\
     b as (select 3 as y union all select 4 as y)\
     select a.x from a, b\n";

fn project_with_probe(config: &str, text: &str) -> (TestProject, String) {
    let project = jaffle();
    let path = project.root.join("models/perf_probe.sql");
    std::fs::write(&path, text).unwrap();
    std::fs::write(project.root.join("cogitator.yml"), config).unwrap();
    let uri = cogitator_lsp::fs_path_to_uri(&path);
    (project, uri)
}

fn open(lsp: &mut Lsp, uri: &str, text: &str) {
    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "sql", "version": 1, "text": text}}),
    );
}

fn pull(lsp: &mut Lsp, uri: &str) -> Vec<Value> {
    let reply = lsp.request(
        "textDocument/diagnostic",
        json!({"textDocument": {"uri": uri}}),
    );
    reply["result"]["items"].as_array().expect("items").clone()
}

fn perf_codes(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter(|d| d["code"].as_str().is_some_and(|c| c.contains('.')))
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect()
}

fn plan(lsp: &mut Lsp, uri: &str) -> Value {
    lsp.request(
        "workspace/executeCommand",
        json!({"command": "dbt.queryPlan", "arguments": [{"uri": uri}]}),
    )
}

fn cache_stats(lsp: &mut Lsp, uri: &str) -> Value {
    lsp.request(
        "workspace/executeCommand",
        json!({"command": "dbt.cache", "arguments": [{"uri": uri}]}),
    )["result"]["plans"]
        .clone()
}

#[test]
fn profile_off_is_silent_and_plan_still_answers() {
    let (project, uri) = project_with_probe("", CARTESIAN);
    let (mut lsp, _) = project.booted();
    open(&mut lsp, &uri, CARTESIAN);
    let items = pull(&mut lsp, &uri);
    assert!(perf_codes(&items).is_empty(), "{items:?}");
    let response = plan(&mut lsp, &uri);
    let findings = response["result"]["findings"].as_array().expect("findings");
    assert!(
        findings.iter().any(|f| f["id"] == "join.cartesian"),
        "{response}"
    );
    assert!(
        findings.iter().all(|f| f["suppressed"] == false),
        "{response}"
    );
    lsp.shutdown();
}

#[test]
fn recommended_reports_with_rule_code_and_source() {
    let (project, uri) = project_with_probe("perf:\n  profile: recommended\n", CARTESIAN);
    let (mut lsp, _) = project.booted();
    lsp.wait_status(|s| s["config"]["loaded"] == true && s["perf"]["profile"] == "recommended");
    open(&mut lsp, &uri, CARTESIAN);
    let items = pull(&mut lsp, &uri);
    let found: Vec<&Value> = items
        .iter()
        .filter(|d| d["code"] == "join.cartesian")
        .collect();
    assert_eq!(found.len(), 1, "{items:?}");
    assert_eq!(found[0]["source"], "cogitator");
    assert_eq!(found[0]["severity"], 2, "warning");
    lsp.shutdown();
}

#[test]
fn perf_rules_override_and_hot_reload() {
    let (project, uri) = project_with_probe(
        "perf:\n  profile: strict\nperf_rules:\n  join.cartesian: hint\n",
        CARTESIAN,
    );
    let config = project.root.join("cogitator.yml");
    let (mut lsp, _) = project.booted();
    lsp.wait_status(|s| s["perf"]["profile"] == "strict");
    open(&mut lsp, &uri, CARTESIAN);
    let lowered = lsp.wait_until("the override lowers the severity", |lsp| {
        pull(lsp, &uri)
            .into_iter()
            .find(|d| d["code"] == "join.cartesian")
    });
    assert_eq!(lowered["severity"], 4, "hint: {lowered}");

    std::fs::write(
        &config,
        "perf:\n  profile: strict\nperf_rules:\n  join.cartesian: off\n",
    )
    .unwrap();
    lsp.reload_config();
    lsp.wait_until("the rule goes quiet", |lsp| {
        pull(lsp, &uri)
            .iter()
            .all(|d| d["code"] != "join.cartesian")
            .then_some(())
    });
    let doctor = lsp.request(
        "workspace/executeCommand",
        json!({"command": "dbt.doctor", "arguments": [{}]}),
    );
    assert_eq!(doctor["result"]["perf"]["profile"], "strict", "{doctor}");
    assert_eq!(doctor["result"]["perf"]["enabledRules"], 22, "{doctor}");
    lsp.shutdown();
}

#[test]
fn suppression_comment_keeps_the_finding_in_the_plan_only() {
    let text = "with a as (select 1 as x union all select 2 as x),\
                b as (select 3 as y union all select 4 as y)\
                -- cogitator: ignore join.cartesian -- demo\n\
                select a.x from a, b\n";
    let (project, uri) = project_with_probe("perf:\n  profile: strict\n", text);
    let (mut lsp, _) = project.booted();
    lsp.wait_status(|s| s["perf"]["profile"] == "strict");
    open(&mut lsp, &uri, text);
    let items = pull(&mut lsp, &uri);
    assert!(
        items.iter().all(|d| d["code"] != "join.cartesian"),
        "{items:?}"
    );
    let response = plan(&mut lsp, &uri);
    let finding = response["result"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|f| f["id"] == "join.cartesian")
        .cloned()
        .expect("still reported in the plan");
    assert_eq!(finding["suppressed"], true, "{finding}");
    lsp.shutdown();
}

#[test]
fn push_reuses_a_cached_plan_and_never_computes() {
    let (project, uri) = project_with_probe("perf:\n  profile: recommended\n", CARTESIAN);
    let (mut lsp, _) = project.booted();
    lsp.wait_status(|s| s["perf"]["profile"] == "recommended");

    // didOpen publishes before any plan exists: no finding, no compute
    open(&mut lsp, &uri, CARTESIAN);
    let opened = lsp.wait_notification("textDocument/publishDiagnostics", |n| {
        n["params"]["uri"] == uri
    });
    assert!(
        perf_codes(opened["params"]["diagnostics"].as_array().unwrap()).is_empty(),
        "{opened}"
    );
    assert_eq!(
        cache_stats(&mut lsp, &uri)["misses"],
        0,
        "push never computes"
    );

    // a pull computes; the plan lands in the cache
    let pulled = pull(&mut lsp, &uri);
    assert_eq!(perf_codes(&pulled), vec!["join.cartesian".to_string()]);
    let before = cache_stats(&mut lsp, &uri);

    // the next push serves it with no further computation
    lsp.notify(
        "textDocument/didSave",
        json!({"textDocument": {"uri": uri}}),
    );
    let saved = lsp.wait_notification("textDocument/publishDiagnostics", |n| {
        n["params"]["uri"] == uri
    });
    assert_eq!(
        perf_codes(saved["params"]["diagnostics"].as_array().unwrap()),
        vec!["join.cartesian".to_string()],
        "{saved}"
    );
    assert_eq!(
        cache_stats(&mut lsp, &uri)["misses"],
        before["misses"],
        "the push path only probes the cache"
    );
    lsp.shutdown();
}

#[test]
fn quickfix_prefers_preserving_fixes_and_fix_all_excludes_the_rest() {
    let text = "with unused as (select 1 as x),\nkept as (select 2 as y)\nselect y from kept\n";
    let (project, uri) = project_with_probe("perf:\n  profile: strict\n", text);
    let (mut lsp, _) = project.booted();
    lsp.wait_status(|s| s["perf"]["profile"] == "strict");
    open(&mut lsp, &uri, text);
    let items = pull(&mut lsp, &uri);
    let unused: Vec<Value> = items
        .into_iter()
        .filter(|d| d["code"] == "model.unused_cte")
        .collect();
    assert_eq!(unused.len(), 1, "{unused:?}");
    let range = unused[0]["range"].clone();

    let actions = lsp.request(
        "textDocument/codeAction",
        json!({"textDocument": {"uri": uri}, "range": range,
               "context": {"diagnostics": unused, "only": ["quickfix"]}}),
    );
    let quickfix = actions["result"].as_array().expect("actions");
    assert_eq!(quickfix.len(), 1, "{actions}");
    assert_eq!(quickfix[0]["isPreferred"], true, "{actions}");
    assert_eq!(quickfix[0]["kind"], "quickfix", "{actions}");

    let bundle = lsp.request(
        "textDocument/codeAction",
        json!({"textDocument": {"uri": uri}, "range": range,
               "context": {"diagnostics": unused, "only": ["source.fixAll"]}}),
    );
    let fix_all = bundle["result"].as_array().expect("actions");
    assert_eq!(fix_all.len(), 1, "{bundle}");
    assert_eq!(fix_all[0]["kind"], "source.fixAll.cogitator", "{bundle}");
    assert!(fix_all[0]["edit"].is_object(), "{bundle}");
    lsp.shutdown();
}

#[test]
fn unknown_ids_warn_through_the_pull_path() {
    let text = "select 1 as x -- cogitator: ignore join.cartesan\n";
    let (project, uri) = project_with_probe("perf:\n  profile: recommended\n", text);
    let (mut lsp, _) = project.booted();
    lsp.wait_status(|s| s["perf"]["profile"] == "recommended");
    open(&mut lsp, &uri, text);
    let items = pull(&mut lsp, &uri);
    let warning = items
        .iter()
        .find(|d| d["code"] == "cogitator-suppress")
        .unwrap_or_else(|| panic!("no warning: {items:?}"));
    assert_eq!(warning["severity"], 2);
    assert!(
        warning["message"]
            .as_str()
            .is_some_and(|m| m.contains("join.cartesian")),
        "{warning}"
    );
    lsp.shutdown();
}
