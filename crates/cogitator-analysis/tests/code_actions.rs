//! textDocument/codeAction over the fixture: the shared model command set,
//! the schema-gated tail, the kind filter, ref-targeted actions, and the
//! YAML-entry actions.

use std::path::PathBuf;

use cogitator_analysis::{build_context, features, Documents};
use cogitator_lsp::{Position, Range};
use pretty_assertions::assert_eq;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/jaffle_shop_duckdb")
}

fn ctx() -> cogitator_analysis::DbtContext {
    build_context(fixture_root().to_str().unwrap())
}

fn model_uri(rel: &str) -> String {
    cogitator_lsp::fs_path_to_uri(&fixture_root().join(rel))
}

fn titles(actions: &[cogitator_lsp::CodeAction]) -> Vec<String> {
    actions.iter().map(|a| a.title.clone()).collect()
}

/// Every action in these tests is command-only; unwrap accordingly.
fn cmd(a: &cogitator_lsp::CodeAction) -> &cogitator_lsp::Command {
    a.command.as_ref().expect("command-only action")
}

#[test]
fn model_file_offers_the_lens_command_set() {
    let ctx = ctx();
    let uri = model_uri("models/customers.sql");
    let actions = features::code_actions(&ctx, None, &uri, Range::default(), None, None, &[]);
    let titles = titles(&actions);
    assert!(titles.contains(&"Go to schema".to_string()), "{titles:?}");
    assert!(titles.contains(&"dbt run".to_string()), "{titles:?}");
    // customers has a schema entry with tests: the goToTests tail, not the
    // extract scaffold
    assert!(
        actions.iter().any(|a| cmd(a).command == "dbt.goToTests"),
        "{titles:?}"
    );
    assert!(
        !titles.contains(&"Extract schema YAML".to_string()),
        "{titles:?}"
    );
    for a in &actions {
        assert!(cmd(a).command.starts_with("dbt."), "{}", cmd(a).command);
        let arg = serde_json::to_value(&cmd(a).arguments[0]).unwrap();
        assert_eq!(arg["uri"], serde_json::Value::String(uri.clone()));
    }
}

#[test]
fn model_without_schema_entry_offers_the_scaffold() {
    let ctx = ctx();
    let uri = model_uri("models/staging/stg_stripe_payments.sql");
    let actions = features::code_actions(&ctx, None, &uri, Range::default(), None, None, &[]);
    let titles = titles(&actions);
    assert!(
        titles.contains(&"Extract schema YAML".to_string()),
        "{titles:?}"
    );
    assert!(
        !actions.iter().any(|a| cmd(a).command == "dbt.goToTests"),
        "{titles:?}"
    );
}

#[test]
fn kind_filter_yields_nothing() {
    let ctx = ctx();
    let uri = model_uri("models/customers.sql");
    let only = vec!["quickfix".to_string()];
    let actions =
        features::code_actions(&ctx, None, &uri, Range::default(), Some(&only), None, &[]);
    assert_eq!(titles(&actions), Vec::<String>::new());
}

#[test]
fn ref_token_prepends_the_target_schema_action() {
    let ctx = ctx();
    let uri = "file:///tmp/model.sql".to_string();
    let text = "select * from {{ ref('customers') }}";
    let mut docs = Documents::default();
    docs.open(uri.clone(), text, &ctx.dialect);
    let range = Range::zero_width(Position {
        line: 0,
        character: 24,
    });
    let actions = features::code_actions(&ctx, docs.get(&uri), &uri, range, None, None, &[]);
    assert_eq!(actions[0].title, "Open schema of 'customers'");
    assert_eq!(cmd(&actions[0]).command, "dbt.openSchema");
    let arg = serde_json::to_value(&cmd(&actions[0]).arguments[0]).unwrap();
    assert!(
        arg["uri"]
            .as_str()
            .is_some_and(|u| u.ends_with("customers.sql")),
        "{arg}"
    );
}

#[test]
fn yaml_entry_offers_model_and_tests_jumps() {
    let ctx = ctx();
    let entry_line = ctx
        .model_detail_map
        .get("customers")
        .expect("customers")
        .schema_range
        .start
        .line;
    let uri = model_uri("models/schema.yml");
    let range = Range::zero_width(Position {
        line: entry_line,
        character: 0,
    });
    let actions = features::code_actions(&ctx, None, &uri, range, None, None, &[]);
    assert_eq!(
        titles(&actions),
        vec![
            "Open model 'customers'".to_string(),
            "Go to tests of 'customers'".to_string()
        ]
    );
    assert_eq!(cmd(&actions[0]).command, "dbt.openModel");
    let arg = serde_json::to_value(&cmd(&actions[0]).arguments[0]).unwrap();
    assert!(
        arg["uri"]
            .as_str()
            .is_some_and(|u| u.ends_with("customers.sql")),
        "{arg}"
    );
}

#[test]
fn profiles_yaml_offers_target_switch_actions() {
    use cogitator_analysis::features::TargetChoice;
    let ctx = ctx();
    let choices = vec![
        TargetChoice {
            profile: "jaffle_shop".into(),
            target: "dev".into(),
            active: true,
        },
        TargetChoice {
            profile: "jaffle_shop".into(),
            target: "prod".into(),
            active: false,
        },
    ];
    let uri = "file:///home/me/.dbt/profiles.yml";
    let actions =
        features::code_actions(&ctx, None, uri, Range::default(), None, Some(&choices), &[]);
    assert_eq!(
        titles(&actions),
        vec![
            "Switch target: jaffle_shop/dev (active)".to_string(),
            "Switch target: jaffle_shop/prod".to_string()
        ]
    );
    assert_eq!(cmd(&actions[1]).command, "dbt.setTarget");
    let arg = serde_json::to_value(&cmd(&actions[1]).arguments[0]).unwrap();
    assert_eq!(arg["target"], "prod");
    assert_eq!(arg["profile"], "jaffle_shop");
}

#[test]
fn a_ref_to_no_model_offers_to_create_it_beside_the_referencing_one() {
    let ctx = ctx();
    let uri = model_uri("models/probe.sql");
    let text = "select * from {{ ref('brand_new_model') }}";
    let mut docs = Documents::default();
    docs.open(uri.clone(), text, &ctx.dialect);
    let range = Range::zero_width(Position {
        line: 0,
        character: 25,
    });
    let read = |_: &std::path::Path| -> Option<String> { None };
    let actions =
        features::define_missing_actions(&ctx, docs.get(&uri).unwrap(), &uri, range, &read);
    assert_eq!(
        titles(&actions),
        vec!["Create model 'brand_new_model'".to_string()]
    );
    let wire = serde_json::to_value(actions[0].edit.as_ref().unwrap()).unwrap();
    assert_eq!(
        wire["documentChanges"][0]["uri"].as_str(),
        Some(model_uri("models/brand_new_model.sql").as_str())
    );

    // a ref to a model that exists offers nothing of the kind
    let known = "select * from {{ ref('customers') }}";
    docs.open(uri.clone(), known, &ctx.dialect);
    let range = Range::zero_width(Position {
        line: 0,
        character: 24,
    });
    assert!(
        features::define_missing_actions(&ctx, docs.get(&uri).unwrap(), &uri, range, &read)
            .is_empty()
    );
}

#[test]
fn an_unknown_var_offers_to_define_it_in_dbt_project_yml() {
    let ctx = ctx();
    let uri = model_uri("models/probe.sql");
    let text = "select {{ var('brand_new_var') }} as v";
    let mut docs = Documents::default();
    docs.open(uri.clone(), text, &ctx.dialect);
    let range = Range::zero_width(Position {
        line: 0,
        character: 18,
    });
    let read = |path: &std::path::Path| std::fs::read_to_string(path).ok();
    let actions =
        features::define_missing_actions(&ctx, docs.get(&uri).unwrap(), &uri, range, &read);
    assert_eq!(
        titles(&actions),
        vec!["Define var 'brand_new_var' in dbt_project.yml".to_string()]
    );
    let wire = serde_json::to_value(actions[0].edit.as_ref().unwrap()).unwrap();
    let change = &wire["documentChanges"][0];
    assert_eq!(
        change["textDocument"]["uri"].as_str(),
        Some(model_uri("dbt_project.yml").as_str())
    );
    // at the top level of vars:, after its last line (the fixture's
    // project-scoped jaffle_shop block)
    let edit = &change["edits"][0];
    assert_eq!(edit["newText"], "  brand_new_var: null\n");
    let project = std::fs::read_to_string(fixture_root().join("dbt_project.yml")).unwrap();
    let line = edit["range"]["start"]["line"].as_u64().unwrap() as usize;
    let before: Vec<&str> = project.lines().take(line).collect();
    assert_eq!(before.last(), Some(&"    jaffle_number: 1"));
}
