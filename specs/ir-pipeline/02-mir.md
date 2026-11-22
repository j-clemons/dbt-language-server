# 02 — MIR

Normalised, typed relational algebra with stable `ColId`s. Types in
`crates/cogitator-ir/src/mir/`; lowering (`feat/ir-lowering`) in
`cogitator-lower`.

```rust
lower_to_mir(hir: &Hir) -> Lowered<Mir>
```

## Shape

```rust
pub struct Mir { syms, rels, scalars, cols, ctes, tables, prov, root }
pub struct Rel { op: RelOp, out: Vec<ColId>, prov: ProvId }
```

`RelOp`: `Scan, Filter, Project, Join, Aggregate, Window, Distinct, SetOp,
Sort, Limit, Unnest, CteRef, Opaque`. `Mir` carries its own `Interner` so a
plan can be printed without its HIR; lowering clones the HIR's.

**Append-only.** A rewrite adds nodes and moves `root`, so the previous plan
stays available for rollback and before/after views. The invariants apply to
the plan reachable from `root`.

**Operand order.** Each operand and subquery precedes its user in its arena.
This lets the verifier walk bottom-up by id and rules out cycles through
inputs.

**Stable ColIds.** A column keeps its id from definition to output. A
`(c, Col(c))` Project or Aggregate item is a pass-through, not a redefinition.
Filters and projections can therefore move without renumbering columns.

**Correlation is explicit.** A column from an enclosing query is read with
`ScalarKind::Outer(c)`; the enclosing `Subquery { correlated }` must list
every outer column its relation reads. Bottom-up, a node's free columns are
its inputs' free columns, plus its own `Outer`s, plus any correlated column
the node cannot bind from its inputs. None may escape the plan root or a CTE
body.

**Recursive CTEs** lower to `Opaque` (Degradation `RecursiveCte`) so the CTE
graph stays acyclic.

## Provenance

`ProvTable` is an arena of `Origin::{Hir(HirRef), Derived { rule, from }}`.
Every reachable `Rel` and `Scalar` has a `prov`. `Derived.from` is non-empty
and points at earlier entries; `ProvTable::span` follows it to the authored
span, so a pushed-down filter can still report where it was written.

`HirRef { node: HirNode, span }` keeps the span on the reference so findings
never need to re-walk the HIR; `HirNode::None` is for lowering scaffolding and
hand-built test plans.

## Rewrite verification

`verify_rewrite(&mir, before)` compares `mir.root` with the plan at `before`:
root width, order and names are preserved; ColIds may differ, since a rewrite
may introduce a fresh id; each root column's `ColType` may only refine.

`refines` is the order `self ⊔ old == old`. `SqlType::join` is the least upper
bound: `Int ⊔ Decimal = Decimal`, numeric ⊔ `Float` = `Float`, `Null` is the
identity, incompatible pairs are `Unknown`. Nullability orders
`NotNull < Nullable < Unknown`.

A column reference's base type must refine the column's definition. Its
nullability is exempt, because outer joins widen nullability above the
definition while the `ColId` stays the same.

## Lowering notes

`BETWEEN` (kept in HIR) lowers to two conjuncts. `DISTINCT` stays a `Distinct`
node. Set operations stay `SetOp`, including INTERSECT and EXCEPT.

`Mir::add_rel` computes `out` from the op, and the testkit `MirBuilder` uses
it, so a hand-built plan starts out verifier-clean.
