//! The DataFusion side of the differential oracle (specs/ir-pipeline/07).
//!
//! A separate workspace, because this DataFusion release pins arrow and
//! sqlparser versions the server's lockfile has moved past. Cases and the
//! meaning of P and N come from `cogitator-oracle`; only the engine calls
//! live here. Results cover the shared SQL subset, not Snowflake or BigQuery.
//!
//! Tables are created by the fixtures' `CREATE TABLE` / `CREATE OR REPLACE
//! TABLE … AS SELECT * FROM (VALUES …)` statements. This build has no file
//! format features, so inline VALUES are the only seeding path.
//!
//! The bag comparison does not use SQL `EXCEPT ALL`. In 55.2.0 that operator
//! is an anti-join: `dup EXCEPT ALL once` comes back empty even when `dup`
//! holds a row twice, so it would pass a P fix that dropped duplicates. The
//! rows are compared as multisets here instead, with NULL as its own value.

use std::collections::HashMap;

use cogitator_oracle::claims::{EngineReads, TableRead};
use cogitator_oracle::run::{BagReport, Columns, Engine};
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::SessionContext;
use datafusion::sql::parser::DFParser;
use tokio::runtime::{Builder, Runtime};

/// Marks NULL in a formatted row; no SQL value in the corpus can contain it.
const NULL_CELL: &str = "\u{0}NULL";
/// Separates cells in a formatted row.
const CELL_SEPARATOR: char = '\u{1f}';

/// Runs cases against an in-memory DataFusion session, fresh for every call.
pub struct DataFusion {
    runtime: Runtime,
}

impl DataFusion {
    pub fn new() -> Result<Self, String> {
        let runtime = Builder::new_current_thread()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { runtime })
    }
}

impl Engine for DataFusion {
    fn name(&self) -> &str {
        "datafusion"
    }

    fn bag(&mut self, setup: &str, left: &str, right: &str) -> Result<BagReport, String> {
        self.runtime.block_on(async {
            let ctx = session(setup).await?;
            let (left_columns, left_rows) = run_side(&ctx, left).await?;
            let (right_columns, right_rows) = run_side(&ctx, right).await?;
            let except_all = (left_columns.len() == right_columns.len())
                .then(|| multiset_differences(&left_rows, &right_rows));
            Ok(BagReport {
                left_columns,
                right_columns,
                except_all,
            })
        })
    }

    fn reads(&mut self, setup: &str, query: &str) -> Result<EngineReads, String> {
        self.runtime.block_on(async {
            let ctx = session(setup).await?;
            let plan = ctx
                .sql(query)
                .await
                .and_then(|df| df.into_optimized_plan())
                .map_err(|e| e.to_string())?;
            let mut reads = EngineReads::new();
            collect_reads(&plan, &mut reads).map_err(|e| e.to_string())?;
            Ok(reads)
        })
    }
}

async fn session(setup: &str) -> Result<SessionContext, String> {
    let ctx = SessionContext::new();
    let statements = DFParser::parse_sql(setup).map_err(|e| format!("setup: {e}"))?;
    for statement in statements {
        let state = ctx.state();
        let plan = state
            .statement_to_plan(statement)
            .await
            .map_err(|e| format!("setup: {e}"))?;
        ctx.execute_logical_plan(plan)
            .await
            .map_err(|e| format!("setup: {e}"))?
            .collect()
            .await
            .map_err(|e| format!("setup: {e}"))?;
    }
    Ok(ctx)
}

async fn run_side(ctx: &SessionContext, query: &str) -> Result<(Columns, Vec<String>), String> {
    let df = ctx.sql(query).await.map_err(|e| e.to_string())?;
    let columns = df
        .schema()
        .fields()
        .iter()
        .map(|f| (f.name().clone(), f.data_type().to_string()))
        .collect();
    let batches = df.collect().await.map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for batch in &batches {
        format_rows(batch, &mut rows)?;
    }
    Ok((columns, rows))
}

fn format_rows(batch: &RecordBatch, rows: &mut Vec<String>) -> Result<(), String> {
    let options = FormatOptions::new().with_null(NULL_CELL);
    let formatters = batch
        .columns()
        .iter()
        .map(|column| ArrayFormatter::try_new(column.as_ref(), &options))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for row in 0..batch.num_rows() {
        let mut text = String::new();
        for formatter in &formatters {
            text.push_str(&formatter.value(row).to_string());
            text.push(CELL_SEPARATOR);
        }
        rows.push(text);
    }
    Ok(())
}

/// Rows present more often on one side, counted with multiplicity in each
/// direction: the two `EXCEPT ALL` results' sizes.
fn multiset_differences(left: &[String], right: &[String]) -> (u64, u64) {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for row in left {
        *counts.entry(row).or_default() += 1;
    }
    for row in right {
        *counts.entry(row).or_default() -= 1;
    }
    let mut left_only = 0;
    let mut right_only = 0;
    for count in counts.values() {
        if *count > 0 {
            left_only += count.unsigned_abs();
        } else {
            right_only += count.unsigned_abs();
        }
    }
    (left_only, right_only)
}

fn collect_reads(plan: &LogicalPlan, reads: &mut EngineReads) -> datafusion::error::Result<()> {
    plan.apply_with_subqueries(|node| {
        if let LogicalPlan::TableScan(scan) = node {
            let read: &mut TableRead = reads
                .entry(scan.table_name.table().to_lowercase())
                .or_default();
            read.columns.extend(
                scan.projected_schema
                    .fields()
                    .iter()
                    .map(|f| f.name().to_lowercase()),
            );
            for filter in &scan.filters {
                read.columns
                    .extend(filter.column_refs().iter().map(|c| c.name.to_lowercase()));
            }
        }
        Ok(TreeNodeRecursion::Continue)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiset_difference_counts_copies() {
        let rows = |r: &[&str]| r.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            multiset_differences(&rows(&["a", "a", "b"]), &rows(&["a", "b"])),
            (1, 0)
        );
        assert_eq!(multiset_differences(&rows(&["a"]), &rows(&["b"])), (1, 1));
        assert_eq!(multiset_differences(&rows(&[]), &rows(&[])), (0, 0));
    }
}
