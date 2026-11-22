# 07 — Oracles

DuckDB and DataFusion are cited references and feature-gated test oracles,
never linked into the default build.

- **DuckDB:** feature `duckdb-oracle` on `crates/cogitator-duckdb`
  (`explain_json`, `bag_equal`), with tests in a new `crates/cogitator-oracle`.
- **DataFusion:** isolated workspace `oracles/datafusion/`, added to `exclude`
  beside `fuzz`, because 55.2.0 pins arrow 59 and sqlparser 0.62 against this
  lockfile's 60 and 0.63.

The commits cited for optimiser paths are pinned in 05. The oracle's DataFusion
version is a separate pin in `oracles/datafusion/Cargo.toml`.

## Differential tests (`testdata/oracle/<case>/`)

- P fixes must pass `EXCEPT ALL` in both directions with matching types.
- N fixes need a witness fixture that differs and a precondition fixture
  that matches.
- Canonicalisers are tested through a test-only MIR-to-SQL printer.
- Plan comparison is one-way: a column we call unused must not be read by the
  engine. Never compare join order or cardinalities on toy data.

## Limits

These prove only the shared SQL subset, not Snowflake or BigQuery semantics.
Oracle runs use `just oracle` locally and a separate CI job.

## Unverified

The DuckDB `EXPLAIN (FORMAT JSON)` key shape is not verified to be stable
across releases (05). The harness pins a DuckDB version and fails loudly on
parse errors rather than guessing.
