# 03 — LIR and backends

LIR states only what is derivable without running the query. Types in
`crates/cogitator-ir/src/lir/`; backends in
`crates/cogitator-lower/src/backends/` (one module per dialect).

```rust
pub trait DialectBackend: Sync {
    fn capabilities(&self) -> &'static Capabilities;
    fn lower(&self, mir: &Mir, phys: &PhysicalFacts) -> Lir;
    fn pruning(&self, pred: &Pred, col: &PhysColumn) -> Pruning; // Prunes | Blocks{why, doc} | Unknown
}
lower_to_lir(mir: &Mir, backend: &dyn DialectBackend, phys: &PhysicalFacts) -> Lowered<Lir>
```

`Capabilities` and the `Pruning`/`Pred`/`PhysColumn`/`PhysicalFacts` inputs
live in `cogitator-ir` so the verifier and rules see them without depending on
`cogitator-lower`. `backends::for_backend(Backend)` returns the backend once
its module implements one; `feat/ir-lowering` fills all five (snowflake,
bigquery, duckdb, postgres, generic), each with a capability table and doc
links.

## Shape

`Lir { nodes, root, backend, sink }`. Each `Phys` has an `op`, the `mir: RelId`
it was lowered from, its `out`, and an `est`. A `Lir` is always read together
with the `Mir` it came from, since its operators hold MIR column and scalar ids.

`PhysOp`: `TableScan, Filter, Project, Join, Aggregate, Window, Sort, Limit,
Union, Cte, Opaque`. MIR's `Distinct` lowers to a key-only `Aggregate`;
INTERSECT, EXCEPT and UNNEST lower to `Opaque`.

`PruneKey { col, kind, usage }` with
`PruneUse::{Used(scalar), Blocked { by, reason }, NotFiltered, Unknown}`.
`Sink { mat, partition_by, cluster_by, contract }` models the materialisation.
`Join.hint` and `Cte.policy` must be supported by the backend's
`Capabilities`; `JoinHint::None` is always allowed.

Estimates are `Estimate::{Unknown, Stats { rows }, Heuristic(Bound)}`. They
report structure and evidence, never dollars or selectivity (06).

## Verification

`verify_lir(&lir, &mir, &caps)` checks the LIR rows of the table in 00 against
the `Mir` the plan came from and the backend's capability table.
