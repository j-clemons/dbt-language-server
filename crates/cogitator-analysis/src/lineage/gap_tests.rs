//! One snapshot per gap the legacy engine had. Each is meant to be read
//! against the SQL it was recorded for.

use cogitator_dialects::Dialect;
use cogitator_parser::{DocumentParser, JinjaSqlParser};

use super::{column_lineage, ModelLineage, SourceRef};

pub(super) fn render(lineage: &ModelLineage) -> String {
    let mut out = String::new();
    for column in &lineage.columns {
        out.push_str(&column.name);
        out.push_str(" <-");
        if column.sources.is_empty() {
            out.push_str(" (nothing)");
        }
        out.push('\n');
        for source in &column.sources {
            out.push_str("  ");
            out.push_str(&match source {
                SourceRef::Model { name, column } => format!("model {name}.{column}"),
                SourceRef::Source {
                    source,
                    table,
                    column,
                } => format!("source {source}.{table}.{column}"),
                SourceRef::Unknown { name, column } => format!("unknown {name}.{column}"),
            });
            out.push('\n');
        }
    }
    out
}

fn lineage_in(dialect: &str, sql: &str) -> String {
    let dialect = Dialect::new(dialect);
    let doc = JinjaSqlParser.parse(sql, &dialect).expect("parse");
    let lineage = column_lineage(&doc, &dialect).expect(sql);
    format!("{sql}\n---\n{}", render(&lineage))
}

fn lineage_of(sql: &str) -> String {
    lineage_in("duckdb", sql)
}

#[test]
fn derived_table() {
    insta::assert_snapshot!(lineage_of(
        "select d.total, d.id from (select id, amount + tax as total from {{ ref('pay') }}) as d"
    ));
}

#[test]
fn derived_table_renamed_columns() {
    insta::assert_snapshot!(lineage_of(
        "select x, y from (select a, b from {{ ref('m') }}) as d(x, y)"
    ));
}

#[test]
fn unnest_source() {
    insta::assert_snapshot!(lineage_of(
        "select id, tag from {{ ref('m') }} as t, unnest(t.tags) as u(tag)"
    ));
}

#[test]
fn unnest_factor_outputs_its_alias() {
    // BigQuery parses a real UNNEST factor; `as tag` closes its columns.
    insta::assert_snapshot!(lineage_in(
        "bigquery",
        "select id, tag from {{ ref('m') }} as t, unnest(t.tags) as tag",
    ));
}

#[test]
fn nested_with() {
    insta::assert_snapshot!(lineage_of(concat!(
        "with outer_cte as (\n",
        "  with inner_cte as (select a from {{ ref('m') }})\n",
        "  select a as b from inner_cte\n",
        ")\n",
        "select b from outer_cte",
    )));
}

#[test]
fn subquery_expressions() {
    insta::assert_snapshot!(lineage_of(concat!(
        "select\n",
        "  (select max(v) from {{ ref('b') }} where b.id = a.id) as scalar_col,\n",
        "  exists (select * from {{ ref('c') }} c where c.id = a.id) as exists_col,\n",
        "  a.x in (select y from {{ ref('d') }}) as in_col\n",
        "from {{ ref('a') }} as a",
    )));
}

#[test]
fn over_and_filter() {
    insta::assert_snapshot!(lineage_of(concat!(
        "select\n",
        "  sum(amount) over (partition by region order by day) as running,\n",
        "  count(*) filter (where status = 'open') as open_count,\n",
        "  sum(amount) over w as named\n",
        "from {{ ref('m') }}\n",
        "window w as (partition by team)",
    )));
}

#[test]
fn star_exclude_and_replace() {
    insta::assert_snapshot!(lineage_of(concat!(
        "with c as (\n",
        "  select * exclude (secret) replace (upper(name) as name) from {{ ref('m') }}\n",
        ")\n",
        "select name, other, secret from c",
    )));
}

#[test]
fn top_level_star_replace_adds_its_sources() {
    insta::assert_snapshot!(lineage_of(
        "select * replace (lower(b) as a) from {{ ref('m') }}"
    ));
}

#[test]
fn top_level_set_operations() {
    insta::assert_snapshot!(lineage_of(concat!(
        "select a, b from {{ ref('x') }}\n",
        "union all\n",
        "select c, d from {{ source('s', 't') }}\n",
    )));
}

#[test]
fn set_operation_in_a_cte() {
    insta::assert_snapshot!(lineage_of(concat!(
        "with u as (select a from {{ ref('x') }} union select a from {{ ref('y') }})\n",
        "select a from u",
    )));
}

#[test]
fn union_by_name() {
    insta::assert_snapshot!(lineage_of(
        "select a, b from {{ ref('x') }} union all by name select b, c from {{ ref('y') }}"
    ));
}

#[test]
fn snowflake_case_folding() {
    insta::assert_snapshot!(lineage_in(
        "snowflake",
        "with Upper_Cte as (select ID as Key from {{ ref('m') }})\nselect key, UPPER_CTE.KEY as k2 from upper_cte",
    ));
}

#[test]
fn duckdb_case_folding() {
    insta::assert_snapshot!(lineage_of(
        "with C as (select Id from {{ ref('m') }}) select c.ID, ID as again from c"
    ));
}

#[test]
fn static_relations_this_and_opaque() {
    insta::assert_snapshot!(lineage_of(concat!(
        "select t.a, cast(s.f as int) as field, trim(s.g) as item, {{ my_macro() }} as m, u.b\n",
        "from {{ this }} as t, {{ source('raw', 'events') }} as s, {{ dyn(ref('up')) }} as u",
    )));
}

#[test]
fn lateral_alias_reads_the_item() {
    insta::assert_snapshot!(lineage_of(concat!(
        "with c as (select a from {{ ref('m') }})\n",
        "select a + 1 as b, b * 2 as d from c",
    )));
}

#[test]
fn struct_paths_and_subscripts_name_one_column() {
    insta::assert_snapshot!(lineage_of(concat!(
        "select s.g[1] as a, t.arr[2] as b, t.s.g[3] as c, arr[idx] as d, x[1].y as e ",
        "from {{ ref('m') }} t",
    )));
}

#[test]
fn deterministic_source_order_across_runs() {
    let sql =
        "select * from {{ ref('a') }} join {{ ref('b') }} on true join {{ ref('c') }} on true";
    let first = lineage_of(sql);
    for _ in 0..20 {
        assert_eq!(lineage_of(sql), first);
    }
}
