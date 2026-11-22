//! Pins the answers over every model in `testdata/jaffle_shop_duckdb` and
//! the inputs the lineage tests use. These snapshots replaced the parity
//! test against the pre-HIR engine once it held across this corpus.
//!
//! Accepted differences from that engine, recorded here and in
//! specs/ir-pipeline/09: `__dbt_rel_N__` placeholder names never appear;
//! gap closures only add sources (no source the old engine found is lost);
//! a top-level set operation answers where the old engine declined.

use std::path::{Path, PathBuf};

use cogitator_dialects::Dialect;
use cogitator_parser::{DocumentParser, JinjaSqlParser, ParsedDocument};

use super::column_lineage;
use super::gap_tests::render;

/// (snapshot name, dialect, input).
const INPUTS: &[(&str, &str, &str)] = &[
    (
        "ref_and_source_join",
        "duckdb",
        "select a.x, b.y, a.x + b.y as z\nfrom {{ ref('a') }} as a\n\
         join {{ source('s', 't') }} b on a.id = b.id",
    ),
    (
        "qualified_star",
        "duckdb",
        "select x.* from {{ ref('a') }} x",
    ),
    (
        "expressions",
        "duckdb",
        "select case when a > 1 then b else c end as v, cast(d as int) as e,\n\
         coalesce(f, g) as h, i between j and k as l, m like n as o,\n\
         p in (q, r) as s\nfrom {{ ref('z') }}",
    ),
    (
        "scalar_jinja",
        "duckdb",
        "select {{ var('x') }} as v, {{ my_macro() }} from {{ ref('m') }}",
    ),
    (
        "package_ref",
        "duckdb",
        "select * from {{ ref('pkg', 'model') }}",
    ),
    (
        "cte_missing_column",
        "duckdb",
        "with c as (select a from {{ ref('m') }}) select a, missing from c",
    ),
    (
        "star_cte",
        "duckdb",
        "with c as (select * from {{ ref('m') }}) select id, c.name from c",
    ),
    (
        "physical_table",
        "duckdb",
        "select a from raw_schema.raw_table",
    ),
    (
        "comma_join_mixed",
        "duckdb",
        "select t.a, o.b from {{ ref('m') }} t, raw.other o where o.b = t.a",
    ),
    (
        "ambiguous_join",
        "duckdb",
        "select x from {{ ref('a') }} join {{ ref('b') }} on true",
    ),
    (
        "rename",
        "duckdb",
        "select customer_id, amount as total from {{ ref('stg_payments') }}",
    ),
    (
        "ctes_with_join",
        "duckdb",
        "with orders as (\n    select order_id, customer_id from {{ ref('stg_orders') }}\n),\n\
         payments as (\n    select order_id, amount from {{ source('stripe', 'payment') }}\n)\n\
         select o.customer_id, p.amount\n\
         from orders o join payments p on o.order_id = p.order_id\n",
    ),
    (
        "star_and_expression",
        "duckdb",
        "select *, first_name || ' ' || last_name as full_name\n\
         from {{ ref('stg_customers') }}\n",
    ),
    (
        "config_and_comment",
        "duckdb",
        "{{ config(materialized='table') }}\n{# c #}\nselect a from {{ ref('b') }}",
    ),
    (
        "comment_and_raw",
        "duckdb",
        "{# {{ ref('dead') }} #}{% raw %}select a{% endraw %} from {{ ref('b') }}",
    ),
    (
        "new_column",
        "duckdb",
        "select customer_id, 1 as brand_new_col from {{ ref('stg_customers') }}",
    ),
    (
        "snowflake_qualify",
        "snowflake",
        "select id, amount\nfrom {{ ref('payments') }}\n\
         qualify row_number() over (partition by id order by amount desc) = 1\n",
    ),
];

fn parse(text: &str, dialect: &Dialect) -> ParsedDocument {
    JinjaSqlParser.parse(text, dialect).expect("parse")
}

fn answer(text: &str, dialect: &Dialect) -> String {
    let body = match column_lineage(&parse(text, dialect), dialect) {
        Some(lineage) => render(&lineage),
        None => "(no lineage)\n".to_string(),
    };
    format!("{text}\n---\n{body}")
}

/// Every `.sql` file under the fixture's `models/`, sorted.
fn fixture_models() -> Vec<PathBuf> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/jaffle_shop_duckdb/models");
    let mut out = Vec::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read models").flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "sql") {
                out.push(path);
            }
        }
    }
    out.sort();
    assert!(out.len() >= 5, "fixture models not found: {out:?}");
    out
}

#[test]
fn fixture_models_keep_their_answers() {
    let dialect = Dialect::new("duckdb");
    for path in fixture_models() {
        let text = std::fs::read_to_string(&path).expect("read model");
        let name = path.file_stem().and_then(|s| s.to_str()).expect("stem");
        insta::assert_snapshot!(format!("fixture_{name}"), answer(&text, &dialect));
    }
}

#[test]
fn test_inputs_keep_their_answers() {
    for (name, dialect, text) in INPUTS {
        insta::assert_snapshot!(*name, answer(text, &Dialect::new(*dialect)));
    }
}

#[test]
fn control_blocks_still_decline() {
    let dialect = Dialect::new("duckdb");
    for text in [
        "select 1 {% if flag %}, 2{% endif %} from {{ ref('a') }}",
        // HIR can build this as its Incremental variant; lineage still declines
        "select a from {{ ref('b') }}\n{% if is_incremental() %}where a > 1{% endif %}",
    ] {
        assert!(
            column_lineage(&parse(text, &dialect), &dialect).is_none(),
            "{text}"
        );
    }
}
