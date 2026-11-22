//! End-to-end suite (specs/rust-port/05 §7) — this file's theme only;
//! the framed-LSP harness lives in tests/harness/.

mod harness;
use harness::{jaffle, Lsp};

/// diagnostic carries typed data; echoed back through the codeAction
/// context it yields an edit-bearing quickfix, and `only: ["quickfix"]`
/// now filters instead of blanking the response.
#[test]
fn dangling_ref_quickfix_carries_an_edit() {
    let project = jaffle();
    let (mut lsp, _uri) = project.booted();
    let root = project.root.clone();
    let bad_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/quickfix_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": bad_uri, "languageId": "sql", "version": 1,
            "text": "select * from {{ ref('customer') }}\n"}}),
    );
    let pulled = lsp.request(
        "textDocument/diagnostic",
        serde_json::json!({"textDocument": {"uri": bad_uri}}),
    );
    let items = pulled["result"]["items"].as_array().expect("items").clone();
    let diag = items
        .iter()
        .find(|d| d["code"] == "dangling-ref")
        .expect("dangling-ref diagnostic")
        .clone();
    assert_eq!(diag["data"]["fix"], "replace_token", "{diag}");
    assert_eq!(diag["data"]["suggestions"][0], "customers", "{diag}");

    let actions = lsp.request(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": bad_uri},
            "range": diag["range"],
            "context": {"diagnostics": [diag], "only": ["quickfix"]}
        }),
    );
    let list = actions["result"].as_array().expect("actions");
    assert!(!list.is_empty(), "{actions}");
    assert!(list.iter().all(|a| a["kind"] == "quickfix"), "{actions}");
    let first = &list[0];
    assert_eq!(first["title"], "Change to 'customers'", "{first}");
    assert_eq!(
        first["edit"]["documentChanges"][0]["edits"][0]["newText"], "customers",
        "{first}"
    );

    lsp.shutdown();
}

/// doc('…') end to end over the booted server: hover shows the block's
/// markdown, definition jumps into docs.md, a misspelled name pulls a
/// dangling-doc diagnostic with suggestions, and `doc('` completes.
#[test]
fn doc_blocks_hover_definition_diagnostic_completion() {
    let project = jaffle();
    let (mut lsp, _uri) = project.booted();
    let root = project.root.clone();

    let probe = cogitator_lsp::fs_path_to_uri(&root.join("models/doc_probe.yml"));
    let text = concat!(
        "version: 2\n",
        "models:\n",
        "  - name: orders\n",
        "    description: '{{ doc(\"orders_status\") }}'\n",
        "    columns:\n",
        "      - name: status\n",
        "        description: '{{ doc(\"orders_statuss\") }}'\n",
    );
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": probe, "languageId": "yaml", "version": 1,
            "text": text}}),
    );

    let hover = lsp.request(
        "textDocument/hover",
        serde_json::json!({"textDocument": {"uri": probe},
                           "position": {"line": 3, "character": 30}}),
    );
    assert!(
        hover["result"]["contents"]
            .as_str()
            .is_some_and(|c| c.contains("placed")),
        "{hover}"
    );

    let def = lsp.request(
        "textDocument/definition",
        serde_json::json!({"textDocument": {"uri": probe},
                           "position": {"line": 3, "character": 30}}),
    );
    assert!(
        def["result"]["uri"]
            .as_str()
            .is_some_and(|u| u.ends_with("docs.md")),
        "{def}"
    );

    let pulled = lsp.request(
        "textDocument/diagnostic",
        serde_json::json!({"textDocument": {"uri": probe}}),
    );
    let items = pulled["result"]["items"].as_array().expect("items");
    let diag = items
        .iter()
        .find(|d| d["code"] == "dangling-doc")
        .expect("dangling-doc diagnostic");
    assert_eq!(diag["data"]["suggestions"][0], "orders_status", "{diag}");

    let sql_probe = cogitator_lsp::fs_path_to_uri(&root.join("models/doc_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": sql_probe, "languageId": "sql", "version": 1,
            "text": "{{ doc('"}}),
    );
    let completion = lsp.request(
        "textDocument/completion",
        serde_json::json!({"textDocument": {"uri": sql_probe},
                           "position": {"line": 0, "character": 8}}),
    );
    let labels: Vec<&str> = completion["result"]
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|i| i["label"].as_str())
        .collect();
    assert!(labels.contains(&"orders_status"), "{labels:?}");

    lsp.shutdown();
}

/// Inlay hints, document links and linked editing (01 §6.3) over the
/// shared fixture — read-only requests, no fixture mutation.
#[test]
fn inlay_links_and_linked_editing() {
    let project = jaffle();
    let (mut lsp, customers_uri) = project.booted();

    // document links: every ref in customers.sql targets its model file
    let links = lsp.request(
        "textDocument/documentLink",
        serde_json::json!({"textDocument": {"uri": customers_uri}}),
    );
    let targets: Vec<&str> = links["result"]
        .as_array()
        .expect("links")
        .iter()
        .filter_map(|l| l["target"].as_str())
        .collect();
    assert!(
        targets
            .iter()
            .any(|t| t.ends_with("staging/stg_customers.sql")),
        "{targets:?}"
    );

    // inlay hints: the ref resolves to its warehouse relation, and a
    // macro call site names its positional parameters
    let probe_uri = cogitator_lsp::fs_path_to_uri(&project.root.join("models/inlay_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": probe_uri, "languageId": "sql", "version": 1,
            "text": "select {{ full_name('a', 'b') }} from {{ ref('stg_customers') }}\n"}}),
    );
    let hints = lsp.request(
        "textDocument/inlayHint",
        serde_json::json!({"textDocument": {"uri": probe_uri},
                           "range": {"start": {"line": 0, "character": 0},
                                     "end": {"line": 2, "character": 0}}}),
    );
    let labels: Vec<&str> = hints["result"]
        .as_array()
        .expect("hints")
        .iter()
        .filter_map(|h| h["label"].as_str())
        .collect();
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with("→ jaffle_shop.main.stg_customers")),
        "{labels:?}"
    );
    assert!(labels.contains(&"first_name:"), "{labels:?}");
    assert!(labels.contains(&"last_name:"), "{labels:?}");

    // linked editing: a CTE name edits together with its uses
    let le_uri = cogitator_lsp::fs_path_to_uri(&project.root.join("models/le_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": le_uri, "languageId": "sql", "version": 1,
            "text": "with t as (select 1 as x) select * from t\n"}}),
    );
    let linked = lsp.request(
        "textDocument/linkedEditingRange",
        serde_json::json!({"textDocument": {"uri": le_uri},
                           "position": {"line": 0, "character": 5}}),
    );
    assert_eq!(
        linked["result"]["ranges"].as_array().map(|r| r.len()),
        Some(2),
        "{linked}"
    );
    // a plain column is NOT linked-editable
    let not_linked = lsp.request(
        "textDocument/linkedEditingRange",
        serde_json::json!({"textDocument": {"uri": le_uri},
                           "position": {"line": 0, "character": 23}}),
    );
    assert!(not_linked["result"].is_null(), "{not_linked}");

    lsp.shutdown();
}

/// Formatting (05 §9): one whole-document edit, Jinja expressions
/// byte-identical after the reflow; a doc with inline {% %} declines
/// with an empty edit list.
#[test]
fn formatting_reflows_sql_and_keeps_jinja() {
    let project = jaffle();
    let (mut lsp, _uri) = project.booted();
    let root = project.root.clone();
    let fmt_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/fmt_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": fmt_uri, "languageId": "sql", "version": 1,
            "text": "select a,b,   c from {{ ref('stg_customers') }} where a>1\n"}}),
    );
    let formatted = lsp.request(
        "textDocument/formatting",
        serde_json::json!({"textDocument": {"uri": fmt_uri},
                           "options": {"tabSize": 4, "insertSpaces": true}}),
    );
    let edits = formatted["result"].as_array().expect("edits");
    assert_eq!(edits.len(), 1, "{formatted}");
    let new_text = edits[0]["newText"].as_str().unwrap();
    assert!(
        new_text.contains("{{ ref('stg_customers') }}"),
        "{new_text}"
    );
    assert!(new_text.contains('\n'), "reflowed: {new_text}");

    let inline_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/fmt_inline.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": inline_uri, "languageId": "sql", "version": 1,
            "text": "select 1 {% if x %} ,2 {% endif %} from t\n"}}),
    );
    let declined = lsp.request(
        "textDocument/formatting",
        serde_json::json!({"textDocument": {"uri": inline_uri},
                           "options": {"tabSize": 4, "insertSpaces": true}}),
    );
    assert_eq!(
        declined["result"].as_array().map(Vec::len),
        Some(0),
        "{declined}"
    );

    lsp.shutdown();
}

/// dbt.expandJinja (05 §9): loops unroll, refs resolve to relations,
/// fixture macros render; unsupported constructs answer Guidance.
#[test]
fn expand_jinja_renders_the_stubbed_subset() {
    let project = jaffle();
    let (mut lsp, _uri) = project.booted();
    let root = project.root.clone();
    let probe_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/expand_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": probe_uri, "languageId": "sql", "version": 1,
            "text": "select {% for i in range(2) %}{{ times_five(i) }} as c{{ i }}{{ ', ' if not loop.last }}{% endfor %}\nfrom {{ ref('stg_customers') }}\n"}}),
    );
    let expanded = lsp.request(
        "workspace/executeCommand",
        serde_json::json!({"command": "dbt.expandJinja", "arguments": [{"uri": probe_uri}]}),
    );
    let sql = expanded["result"]["sql"].as_str().expect("sql");
    assert!(sql.contains("0 as c0, 5 as c1"), "{sql}");
    assert!(sql.contains("from jaffle_shop.main.stg_customers"), "{sql}");

    lsp.shutdown();
}

/// The editing-action trio (03 §10): convert literal relation →
/// ref()/source(), scaffold key-column tests, inherit an upstream
/// description — all edit-bearing, all through textDocument/codeAction.
#[test]
fn editing_actions_convert_scaffold_inherit() {
    let project = jaffle();
    let root = project.root.clone();
    std::fs::write(
        root.join("models/probe_actions.sql"),
        "select customer_id, order_count from {{ ref('orders') }}\n",
    )
    .unwrap();
    let probe_yml = root.join("models/probe_actions.yml");
    let yml_text = "version: 2\n\nmodels:\n  - name: probe_actions\n    columns:\n      - name: customer_id\n      - name: something_id\n";
    std::fs::write(&probe_yml, yml_text).unwrap();

    let (mut lsp, _uri) = project.booted();

    // 1. convert literal relation (duckdb target: jaffle_shop.main.*)
    let convert_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/convert_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": convert_uri, "languageId": "sql", "version": 1,
            "text": "select * from main.stg_customers\n"}}),
    );
    let actions = lsp.request(
        "textDocument/codeAction",
        serde_json::json!({"textDocument": {"uri": convert_uri},
                           "range": {"start": {"line": 0, "character": 0},
                                     "end": {"line": 0, "character": 0}},
                           "context": {"diagnostics": []}}),
    );
    let convert = actions["result"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["kind"] == "refactor.rewrite").cloned())
        .expect("convert action");
    assert_eq!(
        convert["title"], "Convert to ref('stg_customers')",
        "{convert}"
    );
    assert_eq!(
        convert["edit"]["documentChanges"][0]["edits"][0]["newText"], "{{ ref('stg_customers') }}",
        "{convert}"
    );

    // 2+3. YAML entry: scaffold tests + inherit description
    let yml_uri = cogitator_lsp::fs_path_to_uri(&probe_yml);
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": yml_uri, "languageId": "yaml", "version": 1,
            "text": yml_text.replace("\\n", "\n")}}),
    );
    let actions = lsp.request(
        "textDocument/codeAction",
        serde_json::json!({"textDocument": {"uri": yml_uri},
                           "range": {"start": {"line": 3, "character": 4},
                                     "end": {"line": 3, "character": 4}},
                           "context": {"diagnostics": []}}),
    );
    let list = actions["result"].as_array().expect("actions");
    let scaffold = list
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .is_some_and(|t| t.starts_with("Add unique + not_null"))
        })
        .expect("scaffold action");
    assert!(
        scaffold["title"]
            .as_str()
            .unwrap()
            .contains("2 key column(s)"),
        "{scaffold}"
    );
    let scaffold_text = scaffold["edit"]["documentChanges"][0]["edits"][0]["newText"]
        .as_str()
        .unwrap();
    assert!(scaffold_text.contains("data_tests:"), "{scaffold_text}");
    assert!(scaffold_text.contains("- unique"), "{scaffold_text}");

    let inherit = list
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .is_some_and(|t| t.starts_with("Inherit description"))
        })
        .expect("inherit action");
    assert!(
        inherit["title"]
            .as_str()
            .unwrap()
            .contains("from orders.customer_id"),
        "{inherit}"
    );
    assert!(
        inherit["edit"]["documentChanges"][0]["edits"][0]["newText"]
            .as_str()
            .is_some_and(|t| t.contains("description: \"Foreign key")),
        "{inherit}"
    );

    lsp.shutdown();
}

/// The definition actions through textDocument/codeAction: create the
/// model a dangling ref() names, define an unknown var() in
/// dbt_project.yml, and add the columns a model produces but its
/// properties YAML does not document.
#[test]
fn editing_actions_create_define_document() {
    let project = jaffle();
    let root = project.root.clone();
    let cols_sql = "select customer_id, 1 as extra_col from {{ ref('orders') }}\n";
    std::fs::write(root.join("models/probe_cols.sql"), cols_sql).unwrap();
    std::fs::write(
        root.join("models/probe_cols.yml"),
        "version: 2\n\nmodels:\n  - name: probe_cols\n    columns:\n      - name: customer_id\n",
    )
    .unwrap();

    let (mut lsp, _uri) = project.booted();
    fn actions_at(lsp: &mut Lsp, uri: &str, character: usize) -> Vec<serde_json::Value> {
        let reply = lsp.request(
            "textDocument/codeAction",
            serde_json::json!({"textDocument": {"uri": uri},
                               "range": {"start": {"line": 0, "character": character},
                                         "end": {"line": 0, "character": character}},
                               "context": {"diagnostics": []}}),
        );
        reply["result"].as_array().cloned().expect("actions")
    }
    let titled = |list: &[serde_json::Value], title: &str| -> serde_json::Value {
        list.iter()
            .find(|a| a["title"] == title)
            .cloned()
            .unwrap_or_else(|| panic!("no '{title}' in {list:?}"))
    };

    // 1+2. a dangling ref() and an unknown var()
    let dangling = "select * from {{ ref('not_yet_written') }} where id = {{ var('fresh_var') }}\n";
    let dangling_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/dangling_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": dangling_uri, "languageId": "sql", "version": 1,
            "text": dangling}}),
    );
    let at_ref = dangling.find("not_yet").unwrap() + 2;
    let create = titled(
        &actions_at(&mut lsp, &dangling_uri, at_ref),
        "Create model 'not_yet_written'",
    );
    assert_eq!(create["kind"], "quickfix");
    assert_eq!(
        create["edit"]["documentChanges"][0]["kind"], "create",
        "{create}"
    );
    assert_eq!(
        create["edit"]["documentChanges"][0]["uri"].as_str(),
        Some(cogitator_lsp::fs_path_to_uri(&root.join("models/not_yet_written.sql")).as_str())
    );
    let at_var = dangling.find("fresh_var").unwrap() + 2;
    let define = titled(
        &actions_at(&mut lsp, &dangling_uri, at_var),
        "Define var 'fresh_var' in dbt_project.yml",
    );
    assert_eq!(
        define["edit"]["documentChanges"][0]["edits"][0]["newText"], "  fresh_var: null\n",
        "{define}"
    );

    // 3. the produced column the YAML lacks
    let cols_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/probe_cols.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": cols_uri, "languageId": "sql", "version": 1,
            "text": cols_sql}}),
    );
    let add = titled(
        &actions_at(&mut lsp, &cols_uri, 0),
        "Add 1 missing column(s) of 'probe_cols' to its schema YAML",
    );
    let change = &add["edit"]["documentChanges"][0];
    assert!(
        change["textDocument"]["uri"]
            .as_str()
            .is_some_and(|u| u.ends_with("models/probe_cols.yml")),
        "{add}"
    );
    assert_eq!(
        change["edits"][0]["newText"],
        "      - name: extra_col\n        description: \"\"\n"
    );

    lsp.shutdown();
}

/// Go-to-definition on a built-in SQL function (03 §9): the bundled
/// documentation materializes under target/.cogitator/docs/ and the
/// Location jumps into it; plain unknown words keep the self-location
/// fallback.
#[test]
fn function_definition_opens_materialized_docs() {
    let project = jaffle();
    let root = project.root.clone();
    // a snowflake target: the only dialects with bundled function docs
    std::fs::write(
        root.join(".dbt/profiles.yml"),
        "jaffle_shop:\n  target: dev\n  outputs:\n    dev:\n      type: snowflake\n      account: a\n      user: u\n      password: p\n      database: d\n      schema: s\n",
    )
    .unwrap();

    let (mut lsp, _uri) = project.booted();
    let probe_uri = cogitator_lsp::fs_path_to_uri(&root.join("models/fn_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": probe_uri, "languageId": "sql", "version": 1,
            "text": "select datediff(day, a, b), somethingweird from t\n"}}),
    );
    // the dialect lands with a refresh; definition converges on it
    let location = lsp.wait_until("the snowflake dialect backs definition", |lsp| {
        let definition = lsp.request(
            "textDocument/definition",
            serde_json::json!({"textDocument": {"uri": probe_uri},
                               "position": {"line": 0, "character": 10}}),
        );
        let location = definition["result"].clone();
        location["uri"]
            .as_str()
            .is_some_and(|u| u.contains(".cogitator/docs/"))
            .then_some(location)
    });
    let uri = location["uri"].as_str().expect("uri");
    assert!(
        uri.ends_with("target/.cogitator/docs/snowflake/datediff.md"),
        "{location}"
    );
    let path = cogitator_lsp::uri_to_fs_path(uri).expect("fs path");
    let content = std::fs::read_to_string(&path).expect("materialized file exists");
    assert!(content.to_lowercase().contains("datediff"), "{content}");

    // an unknown word still answers the plain self-location fallback
    let character = "select datediff(day, a, b), so".len() as u32;
    let fallback = lsp.request(
        "textDocument/definition",
        serde_json::json!({"textDocument": {"uri": probe_uri},
                           "position": {"line": 0, "character": character}}),
    );
    assert_eq!(fallback["result"]["uri"], probe_uri, "{fallback}");

    lsp.shutdown();
}

/// prepareRename + the macro call hierarchy over the live server:
/// ref tokens prepare with a placeholder, non-targets answer null,
/// and incomingCalls locates the fixture's full_name callers.
#[test]
fn prepare_rename_and_call_hierarchy() {
    let project = jaffle();
    let (mut lsp, _uri) = project.booted();
    let scratch = "file:///tmp/prepare.sql";
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": scratch, "languageId": "sql", "version": 1,
            "text": "select {{ full_name('a', 'b') }} from {{ ref('customers') }}"}}),
    );

    let prep = lsp.request(
        "textDocument/prepareRename",
        serde_json::json!({"textDocument": {"uri": scratch},
                           "position": {"line": 0, "character": 47}}),
    );
    assert_eq!(prep["result"]["placeholder"], "customers", "{prep}");

    let null = lsp.request(
        "textDocument/prepareRename",
        serde_json::json!({"textDocument": {"uri": scratch},
                           "position": {"line": 0, "character": 2}}),
    );
    assert!(null["result"].is_null(), "{null}");

    let items = lsp.request(
        "textDocument/prepareCallHierarchy",
        serde_json::json!({"textDocument": {"uri": scratch},
                           "position": {"line": 0, "character": 12}}),
    );
    let item = items["result"][0].clone();
    assert_eq!(item["name"], "full_name", "{items}");

    let calls = lsp.request(
        "callHierarchy/incomingCalls",
        serde_json::json!({"item": item}),
    );
    assert!(
        calls["result"]
            .as_array()
            .is_some_and(|c| c.iter().any(|x| x["from"]["name"] == "customers")),
        "{calls}"
    );

    let outgoing = lsp.request(
        "callHierarchy/outgoingCalls",
        serde_json::json!({"item": item}),
    );
    assert_eq!(outgoing["result"], serde_json::json!([]), "{outgoing}");

    lsp.shutdown();
}
