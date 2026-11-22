# 05 — Rules

## Upstream references

Paths are relative to DuckDB `src/optimizer/` (D, unless given in full) and
DataFusion `datafusion/optimizer/src/` (DF). Every cited path was checked
against the git tree at these commits:

- DuckDB: `duckdb/duckdb@a3ece3d143abe0e6dc136885eb0880e5be99606a`
- DataFusion: `apache/datafusion@6e0e0dabd52124e427cb2c5c1ab17001627fc5b2`

Fix safety: **P** preserves bag semantics; **N** changes intent and is never
preferred or included in `source.fixAll`.

## Tranche 1 — enablers

| Rule | Upstream |
|---|---|
| `canon.simplify` | D `rule/constant_folding.cpp`; DF `simplify_expressions/` |
| `canon.predicates` | D `filter_pushdown.cpp`; DF `push_down_filter.rs` |
| `canon.required_columns` | D `remove_unused_columns.cpp`; DF `optimize_projections/` |

## Tranche 1 — advice

| Rule | Level | Developer sees | Upstream / evidence | Needs | Fix |
|---|---|---|---|---|---|
| `join.left_nullified` | MIR | WHERE makes this LEFT JOIN inner (W) | D `outer_join_simplification.cpp`; DF `eliminate_outer_join.rs` | none | INNER (P) or move to ON (N) |
| `join.cartesian` | MIR | Join has no key (W) | D `pushdown/pushdown_cross_product.cpp`; DF `eliminate_cross_join.rs` | none | no |
| `join.distinct_fanout` | MIR | DISTINCT may hide join fan-out (H) | D `join_elimination.cpp`; DF `replace_distinct_aggregate.rs` | none (`unique` tests are optional evidence) | no |
| `model.unused_cte` | HIR | CTE never used (W) | none found | AllBranches | delete (P) |
| `cross.select_star_chain` | MIR | Only 4 of 31 columns used downstream (I) | D `remove_unused_columns.cpp`; DF `optimize_projections/` | schema, dag, mat | expand (N) |
| `filter.non_sargable` | LIR | Wrapped column may block pruning (W BigQuery, H others) | D `rule/monotone_preimage.cpp`, `rule/date_trunc_simplification.cpp` | dialect, types, phys | `date(ts)=d` to range (P); BigQuery documents `DATE()` and `CAST` forms that may still prune, so those report as unknown (Hint), never blocked |
| `incr.missing_guard` | HIR | Reprocesses all input each run (W) | dbt-specific | mat | no |
| `incr.subquery_watermark` | HIR+LIR | Subquery filter doesn't prune (W BigQuery, I Snowflake) | BigQuery and Snowflake docs | dialect, phys | no |
| `set.union_distinct` | MIR | UNION dedupes; UNION ALL? (I) | DF `replace_distinct_aggregate.rs`, `optimize_unions.rs` | none | P if disjointness proven, else N |

## Tranche 2

| Rule | Upstream |
|---|---|
| `cross.unused_column` | D `remove_unused_columns.cpp` |
| `cross.filter_late` | D `filter_pushdown.cpp`; DF `push_down_filter.rs` |
| `cross.ephemeral_repeat` | D `cte_inlining.cpp` |
| `cross.duplicate_logic` | D `common_subplan_optimizer.cpp`; DF `common_subexpr_eliminate.rs` |
| `order.in_cte` | D `remove_redundant_order.cpp` |
| `subq.not_in_nullable` | DF `rewrite_set_comparison.rs` |
| `subq.correlated` | D `src/planner/subquery/flatten_dependent_join.cpp`, `deliminator.cpp`; DF `decorrelate_predicate_subquery.rs`, `scalar_subquery_to_join.rs` |
| `join.redundant_left` | D `join_elimination.cpp` |
| `join.key_type_mismatch` | DF `analyzer/` — the directory exists at the pinned SHA; the specific file is **unverified** |
| `filter.partition_or`, `filter.cluster_order` | BigQuery docs (URL to be pinned when the rule is written) |
| `limit.no_scan_reduction` | D `limit_pushdown.cpp`; DF `push_down_limit.rs` |
| `incr.no_unique_key` | dbt-specific |
| `filter.constant_false` | D `empty_result_pullup.cpp`; DF `eliminate_filter.rs`, `propagate_empty_relation.rs` |

## Deliberately no advice

Engines do these better: join order, build/probe side, common aggregates,
IN-clause rewriting, top-N, compressed materialisation, statistics propagation.

## Rule conventions

`RuleMeta { id, level, class, needs, severity, fix }`. `severity` is the
highest the rule reports; a rule may report lower per dialect (W on BigQuery,
H elsewhere), and statistics never raise it. `needs` covers `Schema, Types,
Stats, Dialect, Mat, Dag, Tests, Phys, AllBranches`.

`AllBranches`: rules such as `model.unused_cte` would false-positive on a
compiled plan, since a Jinja branch not taken can legitimately hide a use.
Those rules are recorded `Skipped` on compiled-origin plans of templates with
control flow.

Every rule ships with one positive, one negative and one Jinja-bearing test.
`Finding`s carry `Evidence` (06) and zero or more `Fix`es, each with its own
title, safety and edit set (rendered through `FixKind::MultiEdit`, 08).

## Unverified upstream facts

Marked here so nobody cites them as settled:

- DataFusion's default `max_passes` value.
- Stability of DuckDB `EXPLAIN (FORMAT JSON)` output keys across releases.
- Any Snowflake statement that function-wrapped predicates block pruning. The
  claim needs a doc citation before `filter.non_sargable` ships above Hint
  severity on Snowflake.
