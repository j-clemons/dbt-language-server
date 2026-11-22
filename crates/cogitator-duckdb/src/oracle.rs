//! Test-only engine oracle (specs/ir-pipeline/07), consumed by
//! `crates/cogitator-oracle`. Behind the `duckdb-oracle` feature, so the
//! bundled C++ build never reaches the default build.
//!
//! Each call opens a fresh in-memory database and runs `setup` (schema and
//! seed SQL), so cases cannot leak state into each other. Results describe
//! DuckDB's semantics only: they prove the shared SQL subset, not Snowflake
//! or BigQuery behaviour.

use duckdb::Connection;

/// One relation's columns as `(name, type)`, in result order.
pub type Columns = Vec<(String, String)>;

/// The outcome of comparing two queries as bags of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BagDiff {
    pub left_columns: Columns,
    pub right_columns: Columns,
    /// `(left EXCEPT ALL right, right EXCEPT ALL left)` row counts. `None`
    /// when the column counts differ and `EXCEPT ALL` cannot be planned.
    pub except_all: Option<(u64, u64)>,
}

/// The physical plan DuckDB chooses for `query`, as `EXPLAIN (FORMAT JSON)`
/// reports it. The JSON's key shape is undocumented and pinned by a snapshot
/// in `cogitator-oracle`; a missing plan is an error, never a guess.
pub fn explain_json(setup: &str, query: &str) -> Result<serde_json::Value, String> {
    let conn = open(setup)?;
    let sql = format!("EXPLAIN (FORMAT JSON) {}", body(query));
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (key, value) = row.map_err(|e| e.to_string())?;
        if key == "physical_plan" {
            return serde_json::from_str(&value).map_err(|e| format!("plan JSON: {e}"));
        }
    }
    Err("EXPLAIN (FORMAT JSON) returned no physical_plan row".to_owned())
}

/// Compare `left` and `right` as bags with `EXCEPT ALL` in both directions,
/// reporting their column names and types.
pub fn bag_equal(setup: &str, left: &str, right: &str) -> Result<BagDiff, String> {
    let conn = open(setup)?;
    // A newline before the paren keeps a trailing `--` comment from eating it.
    let materialise = format!(
        "CREATE TEMP TABLE oracle_left AS (\n{}\n);\nCREATE TEMP TABLE oracle_right AS (\n{}\n);",
        body(left),
        body(right)
    );
    conn.execute_batch(&materialise)
        .map_err(|e| e.to_string())?;

    let left_columns = columns(&conn, "oracle_left")?;
    let right_columns = columns(&conn, "oracle_right")?;
    let except_all = if left_columns.len() == right_columns.len() {
        Some((
            except_all_count(&conn, "oracle_left", "oracle_right")?,
            except_all_count(&conn, "oracle_right", "oracle_left")?,
        ))
    } else {
        None
    };
    Ok(BagDiff {
        left_columns,
        right_columns,
        except_all,
    })
}

fn open(setup: &str) -> Result<Connection, String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    conn.execute_batch(setup)
        .map_err(|e| format!("setup: {e}"))?;
    Ok(conn)
}

/// A query body without the trailing `;` that would break embedding it.
fn body(query: &str) -> &str {
    query.trim_end().trim_end_matches(';').trim_end()
}

fn columns(conn: &Connection, table: &str) -> Result<Columns, String> {
    let sql = format!("SELECT column_name, column_type FROM (DESCRIBE {table})");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Columns, _>>()
        .map_err(|e| e.to_string())
}

fn except_all_count(conn: &Connection, minuend: &str, subtrahend: &str) -> Result<u64, String> {
    let sql = format!(
        "SELECT count(*) FROM (SELECT * FROM {minuend} EXCEPT ALL SELECT * FROM {subtrahend})"
    );
    let count: i64 = conn
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    u64::try_from(count).map_err(|e| e.to_string())
}
