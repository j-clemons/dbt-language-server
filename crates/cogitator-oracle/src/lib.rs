//! Differential test harness for the IR pipeline (specs/ir-pipeline/07).
//!
//! DuckDB and DataFusion act as engine oracles: a fix's before and after SQL
//! run against seeded tables and the results are compared as bags. Neither
//! engine is part of the default build. The DuckDB run sits behind the
//! `duckdb-oracle` feature of this crate; the DataFusion run lives in the
//! isolated workspace `oracles/datafusion/`. Without a feature this crate
//! holds only the engine-neutral corpus model and checks, which are shared
//! by both runs.
//!
//! **Scope.** A pass here proves equivalence on the SQL subset both engines
//! share. It says nothing about Snowflake or BigQuery semantics.
//!
//! # Adding a case
//!
//! Drop a directory into `testdata/oracle/<case>/`. [`corpus::discover`]
//! finds it; no code registers it, and both engine runs pick it up.
//!
//! | File | Content |
//! |---|---|
//! | `schema.sql` | `CREATE TABLE t (col TYPE, …)` for every table, typed and empty |
//! | `seed.sql`, other seeds | `CREATE OR REPLACE TABLE t AS SELECT * FROM (VALUES …) AS v(col, …)` |
//! | `model.sql` | the SQL before the fix |
//! | `after.sql` | the SQL after the fix (`after` in `expect.toml` renames it) |
//! | `expect.toml` | the rule, the safety label, the fixtures, the claims |
//!
//! A seed names only the tables it populates, so a table it omits stays
//! empty. Seeds must run on both engines: stay in the shared subset, and give
//! an all-NULL column an explicit `CAST` so its type is not inferred.
//!
//! `expect.toml` keys are listed on [`corpus::Expect`]. Every case declares
//! `covers` across its fixtures so that NULLs, duplicates and empty tables
//! are all exercised:
//!
//! - **`safety = "P"`** (bag-preserving): every fixture has `role =
//!   "preserve"`. `model EXCEPT ALL after` and `after EXCEPT ALL model` must
//!   both be empty, with matching column names and types.
//! - **`safety = "N"`** (intent-changing): at least one `role = "witness"`
//!   fixture must differ and at least one `role = "precondition"` fixture
//!   must be equal, which keeps the label honest in both directions.
//!
//! The optional `[plan]` table states which columns we call unused and which
//! we require, per base table. The check is one-way, see [`claims`]: an
//! unused column must not be read by the engine, and the required set must
//! cover the engine's projection. Join order and cardinalities are never
//! compared; on toy data they say nothing.
//!
//! # Extension points
//!
//! - **Fix engine (waves 3 and 4).** `after.sql` is hand-written for now. Once
//!   the fix engine exists, a case's `after` SQL can be generated and written
//!   to that file, or set directly on [`corpus::Case::after`] before calling
//!   [`run::run_case`]; the checks do not care where it came from.
//! - **MIR-to-SQL printer.** Canonicalisers are tested the same way: print
//!   the MIR before and after the rewrite into `model` and `after`.
//! - **Computed claims.** `[plan]` is hand-written for now. A pass-derived
//!   claim set fills [`claims::PlanClaims`] the same way.
//! - **Another engine.** Implement [`run::Engine`].

pub mod claims;
pub mod corpus;
pub mod duckdb_plan;
pub mod run;

#[cfg(feature = "duckdb-oracle")]
pub mod duckdb_engine;
