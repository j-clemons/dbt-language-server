# 09 — Plan view

`dbt.queryPlan` (one argument object: `{uri, position?, …}`) shows each IR
level and which rules fired, for one model. Always available, independent of
`perf.profile`. Neovim wrapper `:DbtPlan`.

## Content

Per level: the deterministic text from `print_hir`, `print_mir`, `print_lir`,
or a level-unavailable note naming the violations or the degradation that
declined it. Plus the trace (`TraceEntry` per rule visit, including `Skipped`
with reasons) and any `incomplete` budget overrun.

Stale compiled plans show a banner and produce no diagnostics (01). `Unmapped`
spans appear only here.

## Compiled plans

A model whose control flow the template cannot express is planned from
`target/compiled` when the shell found a file (`PlanInput.compiled`, read off
the dispatcher thread and capped by `limits.compiled_sql_mib`). The result
reports:

| Field | Meaning |
|---|---|
| `origin` | `{kind: "compiled", staleness, coverage}`; `coverage` is the basis points of compiled text anchored to the template |
| `banner` | Present when the plan is not known fresh: older than the model, or the template's SQL no longer matches (unsaved edits count), or timestamps unavailable. Absent on template plans |
| `guidance` | Set on `declined`: distinguishes no compiled file, a file over the cap, and an unusable file; each points at `dbt compile` |

A stale file still produces a plan with a banner; only a missing, oversized or
unparsable file declines.

Each finding carries:

| Field | Values |
|---|---|
| `span` | Template offsets, except `placement: unmapped`, which keeps compiled offsets |
| `placement` | `template`, `exact`, `inJinja`, `unmapped` |
| `diagnosticsEligible` | The one field a consumer needs. False on a stale or unknown-age compiled plan and on `unmapped` spans; true for template plans and for fresh `exact`/`inJinja` spans |

`fixes` is non-empty only for `Fresh` + `exact`; a fix with any edit that is not
`exact` is withheld whole. Degradation spans and the printed levels stay in
compiled offsets. `model.unused_cte` and other `AllBranches` rules record
`Skipped` on compiled plans.

## Compute

Offloaded (`offload_blocking`), cached in a `DerivedCache` slot keyed by
(uri, generation, plan key). The plan key moves with the schema revision,
the context snapshot, the manifest and catalog, `perf.catalog_stats` and the
stamps of the model and its compiled file. The push-diagnostics path reuses
a cached plan and never computes one.

## Lineage

`lineage/from_hir.rs` is column lineage over HIR; `column_lineage` and the
`ModelLineage` JSON are unchanged. The pre-HIR engine has been removed: parity
held across the fixtures and the lineage test inputs, and `corpus_tests.rs` now
pins those answers. Accepted differences from the old engine: `__dbt_rel_N__`
placeholder names never appear, gap closures only add sources, and a
top-level set operation answers where the old engine declined.

`lineage/typed.rs` adds `typed_column_lineage`, over the project
`SchemaSnapshot`: a star expands into its columns when every input is `Closed`
(an `Open` or unknown input keeps the `*` row), and a template with control
flow answers from `target/compiled` only when the file is known `Fresh`, its
coverage reaches `MIN_LITERAL_COVERAGE` and the open document has no unsaved
edits. Stale SQL yields no answer, never a banner. `dbt.columnLineage` reports
`origin: "compiled"`, `dbt.impact` lists such models in `compiledLineage`, and
`dbt.showLineage` and the contract and undocumented diagnostics say so in
their text; a response built from the template omits the field.

Freshness compares the model's own file with its compiled file; a change only
in a macro it calls is not detectable that way and is accepted as staleness.
Hover reads the cached lineage at the document's generation and never computes.
