//! One `DialectBackend` per dialect. Each capability table links the
//! vendor documentation its claims come from; anything the docs do not
//! state is `Pruning::Unknown`.

pub mod bigquery;
pub mod duckdb;
pub mod generic;
pub mod postgres;
pub mod snowflake;

use cogitator_ir::lir::Backend;

use crate::DialectBackend;

static BIGQUERY: bigquery::BigQuery = bigquery::BigQuery;
static DUCKDB: duckdb::DuckDb = duckdb::DuckDb;
static GENERIC: generic::Generic = generic::Generic;
static POSTGRES: postgres::Postgres = postgres::Postgres;
static SNOWFLAKE: snowflake::Snowflake = snowflake::Snowflake;

/// The backend for `backend`. Every dialect has one; `Option` is the
/// established contract for callers.
pub fn for_backend(backend: Backend) -> Option<&'static dyn DialectBackend> {
    Some(match backend {
        Backend::BigQuery => &BIGQUERY,
        Backend::DuckDb => &DUCKDB,
        Backend::Generic => &GENERIC,
        Backend::Postgres => &POSTGRES,
        Backend::Snowflake => &SNOWFLAKE,
    })
}

#[cfg(test)]
mod table_tests;
