//! The DuckDB side of [`crate::run::Engine`] (feature `duckdb-oracle`).

use cogitator_duckdb::oracle;

use crate::claims::EngineReads;
use crate::duckdb_plan::reads_from_explain;
use crate::run::{BagReport, Engine};

/// Runs cases against the bundled DuckDB engine.
#[derive(Debug, Default)]
pub struct DuckDb;

impl Engine for DuckDb {
    fn name(&self) -> &str {
        "duckdb"
    }

    fn bag(&mut self, setup: &str, left: &str, right: &str) -> Result<BagReport, String> {
        let diff = oracle::bag_equal(setup, left, right)?;
        Ok(BagReport {
            left_columns: diff.left_columns,
            right_columns: diff.right_columns,
            except_all: diff.except_all,
        })
    }

    fn reads(&mut self, setup: &str, query: &str) -> Result<EngineReads, String> {
        reads_from_explain(&oracle::explain_json(setup, query)?)
    }
}
