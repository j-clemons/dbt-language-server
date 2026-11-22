# 06 — Cost honesty

The pipeline reports structure and evidence, not predicted cost.

## Evidence tags

Each finding is tagged `Evidence::{Exact, Catalog { generated_at }, Heuristic,
Unknown}`:

| Tag | Meaning |
|---|---|
| `Exact` | Derived from the query structure alone, on a `Fresh` plan |
| `Catalog(generated_at)` | Backed by `catalog.json` stats, with their age |
| `Heuristic` | A structural guess; say so in the message |
| `Unknown` | The rule could not establish it |

## Rules

- Never dollars, and never selectivity percentages.
- Statistics may reorder and enrich findings but never raise severity (05).
- `Estimate::Heuristic(Bound)` is a bound, not a point estimate.
- `perf.catalog_stats` (08) switches off catalog enrichment entirely.
