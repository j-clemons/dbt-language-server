# 04 — Pass manager

Fixed phases like DuckDB, with a DataFusion-style fixpoint inside
canonicalisation only:

```
lower → canonicalise to fixpoint → MIR detect → LIR detect
```

Code: `crates/cogitator-rules/src/passes/`. The driver
is `run_passes(PassInput, &Cx) -> PassOutput`. Callers lower LIR through
`PassInput::lower_lir`, because a pre-lowered `lir` goes stale once
canonicalisation moves `root`, and the hook verifies its own output. LIR detect
sees the canonical plan through `Cx::mir`, since LIR scalars and columns are
MIR ids. Detect rules see the canonical plan, so an empty `on` on a non-Cross
join may be a rewrite result rather than what the user wrote.

## Traits

```rust
trait Rule: Sync + 'static { fn meta(&self) -> &'static RuleMeta; }
trait Canonicalize: Rule { fn apply(&self, ir: Mir, cx: &Cx) -> Rewrite<Mir>; }
trait Detect<L>: Rule { fn check(&self, ir: &L, cx: &Cx, out: &mut Findings); }
```

`Rewrite::{Unchanged(ir), Changed(ir)}`: the MIR moves by value so a rewrite
can append nodes and move `root` without cloning.

## Fixpoint

Each canonicaliser runs in registry order, repeating until a pass changes
nothing or the plan repeats a stable hash (`plan_hash`, which ignores node
ids). Canonicalisers keep origin spans so a pushed filter does not hide the
authored problem.

## Budgets

`Budgets { max_passes: 8, max_nodes: 50_000, max_applications: 10_000 }`.
An overrun marks the trace `incomplete` and stops rewriting; results produced
so far stand.

Two node caps apply, and they measure different things:

| Cap | Limit | Where | Meaning |
|---|---|---|---|
| Front-end cap | 200,000 IR nodes | `cogitator-sql-front` (00) | Input size: larger plans degrade explicitly and are not built |
| Pass-manager budget | 50,000 nodes | `Budgets::max_nodes` | Rewriting: past this, canonicalisation stops and the trace is `incomplete` |

## Verification and rollback

With `VerifyMode::EveryRewrite` (debug) `verify_mir` + `verify_rewrite` run
after each `Changed`; with `Boundaries` (release) at the phase boundaries.
A failing rewrite resets `root` to the last verified node, is recorded as
`Outcome::RolledBack`, and rewriting continues with the next rule.

## Trace

One `TraceEntry` per rule visit, keyed by rule, phase and pass:
`Applied | NoChange | Findings { count } | Skipped { reason } | RolledBack { invariants }`.
A rule with unmet needs is recorded `Skipped { reason: Unmet(need) }` rather
than silently omitted.

## Registry

`registry::REGISTRY`, a static ordered slice: canonicalisers first, then
detectors by level, one `Registered { meta, canon, hir, mir, lir }` per rule.
A registry parity test asserts the tranche 1 id set and that each entry
implements exactly its declared levels.

## Determinism

Findings sort by (span, id) with a message tiebreak (`Findings::into_sorted`).
Budgets count, they do not time.
