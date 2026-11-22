//! Relation coordinates survive the properties prune: database/schema/loader
//! per source, identifier/loader per table — raw strings, `env_var()` text
//! kept verbatim (resolution happens in the analysis layer).

use std::path::Path;

use cogitator_schemas::parse_properties;

const YAML: &str = r#"
version: 2
sources:
  - name: warehouse
    database: "{{ env_var('WH_DB', 'raw') }}"
    schema: landing
    loader: fivetran
    tables:
      - name: payments
        identifier: payments_v2
        loader: stripe
      - name: orders
  - name: bare
    tables:
      - name: events
"#;

#[test]
fn source_relation_fields_survive_the_prune() {
    let parsed = parse_properties(YAML, Path::new("schema.yml")).expect("parses");
    let warehouse = &parsed.sources[0];
    assert_eq!(
        warehouse.database.as_deref(),
        Some("{{ env_var('WH_DB', 'raw') }}"),
        "env_var text stays raw at this layer"
    );
    assert_eq!(warehouse.schema.as_deref(), Some("landing"));
    assert_eq!(warehouse.loader.as_deref(), Some("fivetran"));
    let payments = &warehouse.tables[0];
    assert_eq!(payments.identifier.as_deref(), Some("payments_v2"));
    assert_eq!(payments.loader.as_deref(), Some("stripe"));
    assert_eq!(warehouse.tables[1].identifier, None);

    let bare = &parsed.sources[1];
    assert_eq!(bare.database, None);
    assert_eq!(bare.schema, None);
    assert_eq!(bare.loader, None);
    assert_eq!(bare.tables[0].identifier, None);
}

const COLUMNS: &str = r#"
version: 2
models:
  - name: orders
    columns:
      - name: amount
        data_type: numeric(10, 2)
        description: money
      - name: templated
        data_type: "{{ var('id_type') }}"
      - name: absent
      - name: blank
        data_type: ""
      - name: number
        data_type: 123
      - name: list
        data_type: [a, b]
      - name: "null"
        data_type: null
sources:
  - name: raw
    tables:
      - name: payments
        columns:
          - name: id
            data_type: BIGINT
          - name: note
"#;

#[test]
fn column_data_type_is_the_raw_string_or_absent() {
    let parsed = parse_properties(COLUMNS, Path::new("schema.yml")).expect("parses");
    let types: Vec<(&str, Option<&str>)> = parsed.models[0]
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.data_type.as_deref()))
        .collect();
    assert_eq!(
        types,
        [
            ("amount", Some("numeric(10, 2)")),
            ("templated", Some("{{ var('id_type') }}")),
            ("absent", None),
            ("blank", None),
            ("number", None),
            ("list", None),
            ("null", None),
        ]
    );
    assert_eq!(parsed.models[0].columns[0].description, "money");

    let table = &parsed.sources[0].tables[0];
    assert_eq!(table.columns[0].data_type.as_deref(), Some("BIGINT"));
    assert_eq!(table.columns[1].data_type, None);
}

#[test]
fn column_entries_without_a_data_type_serialize_as_before() {
    let parsed = parse_properties(COLUMNS, Path::new("schema.yml")).expect("parses");
    let columns = &parsed.models[0].columns;
    let absent = serde_json::to_value(&columns[2]).unwrap();
    assert!(absent.get("data_type").is_none(), "{absent}");
    let typed = serde_json::to_value(&columns[0]).unwrap();
    assert_eq!(typed["data_type"], "numeric(10, 2)");
    // a cache written before the field existed still loads
    let old = r#"{"name": "a", "description": "", "name_pos": {"line": 0, "character": 0}}"#;
    let back: cogitator_schemas::ColumnEntry = serde_json::from_str(old).unwrap();
    assert_eq!(back.data_type, None);
}

#[test]
fn column_tests_are_read_from_both_keys_with_their_conditions() {
    let yaml = r#"
version: 2
models:
  - name: m
    columns:
      - name: id
        tests: [unique, not_null]
      - name: legacy
        data_tests:
          - dbt.unique
      - name: scoped
        tests:
          - not_null:
              config:
                where: "x > 1"
          - relationships:
              to: ref('b')
              field: id
      - name: warned
        tests:
          - unique:
              severity: warn
      - name: long_form
        data_tests:
          - name: my_check
            test_name: not_null
"#;
    let parsed = parse_properties(yaml, Path::new("schema.yml")).expect("parses");
    let columns = &parsed.models[0].columns;
    assert!(columns[0].proven_by("unique") && columns[0].proven_by("not_null"));
    assert!(columns[1].proven_by("unique"), "dbt. prefix and data_tests");
    assert!(!columns[2].proven_by("not_null"), "where weakens the test");
    assert_eq!(columns[2].tests.len(), 2, "every test is recorded");
    assert!(
        !columns[3].proven_by("unique"),
        "severity warn weakens the test"
    );
    assert!(columns[4].proven_by("not_null"), "test_name is the test");
}
