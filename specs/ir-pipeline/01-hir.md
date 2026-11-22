# 01 — HIR

Surface-faithful and name-resolved, with Jinja as first-class nodes.
Spans point at the user's document. Types in
`crates/cogitator-ir/src/hir/`; construction in `cogitator-sql-front`.

```rust
build_hir(input: &HirInput, env: &SchemaEnv) -> Lowered<Hir>
```

## Shape

Arena-based, ids are `Idx<T>` (u32), names are interned `Symbol`s per plan
(rust-port 08 §1: spans are indices). `Hir` has a `ctes` arena so `CteId` has
an owner; `Query.scope` indexes `scopes`. `From` carries `output` and `open`
so `Resolution::Bound { from, col }` indexes a known list.

`Body` is `Select | SetOp | Values | Hole(HoleKind)`. FROM is `Base`
(`RelTarget::{Model, Source, This, Cte, Physical, Jinja}`), `Derived`,
`Join`, `Unnest`, `TableFn`, `Hole`. Expressions add `Opaque { children }`
and `Hole` so broken or unknown SQL stays analysable.

`Resolution` is one of `Bound`, `Outer { depth, from, col }`, `OpenSchema`,
`Candidates`, `LateralAlias(item)`, `Unresolved`. `ColRef.res` is an
`Option` only so a binder bug is reportable: `None` is a violation, while
"no such column" is `Some(Unresolved)`.

## Jinja

`JinjaNode { span, kind, role }`. `JinjaRole` says how the region
participates in surrounding SQL:

| Role | Meaning | Consumers |
|---|---|---|
| `Relation` | A FROM target only known as Jinja: `{{ my_macro() }}`, `ref(var('x'))` | exactly one `RelTarget::Jinja` |
| `Scalar` | Expression position | exactly one `ExprKind::Jinja` or `SelectItem::Jinja` |
| `Composite` | Multi-role region, e.g. a macro expanding to `WHERE ...` | exactly one |
| `Blank` | Masked out: comments, control tags, regions inside a `Hole`, and static `ref`/`source`/`this` (see below) | zero or one |
| `ClausePosition` | Sits between clauses, no expression | zero or one |

A statically known `ref`, `source` or `this` lowers to
`RelTarget::{Model, Source, This}`, which carries the name a `JinjaId` cannot.
Its region stays in the `jinja` arena as an inventory entry with kind
`Ref`/`Source`/`This` and role `Blank`, because `Relation` requires a
`RelTarget::Jinja` consumer.

## Front-end decisions (binding for `feat/ir-hir`)

- **sqlparser has no error recovery.** Parse the whole statement first. On
  failure, split at paren depth 0 and parse each CTE and the final query via
  `with_tokens_with_locations` + `parse_query`; a failing part becomes a Hole.
- **Masking:** a new same-length mask that preserves newlines, with
  placeholders identified by byte offset. The existing `mask_jinja`
  overwrites newlines, so sqlparser line and column positions would drift.
- **Jinja classification:** `minijinja::machinery::parse_expr` only (no
  hand-written lexers, rust-port 05 §4). Do not trust `Span::empty()`-prone
  sqlparser spans; only `Ident`-derived spans may drive fixes.
- **Stack safety:** token and depth caps run before parsing, since left-deep
  chains (`a+b+…`) escape sqlparser's recursion limit.
- **`is_incremental()`:** a model whose only control flow is
  `{% if is_incremental() %}…{% endif %}` is lowered in template mode as two
  variants: `Incremental` (tags blanked, body kept) and `FullRefresh` (block
  blanked). Rules run on `Incremental`, and `incr.*` rules see the watermark.
  Other control flow takes the compiled fallback.

## Compiled fallback

`SourceOrigin::{Template { variant }, Compiled { staleness, map }}`.
`SpanMap` anchors template SQL segments to compiled text in order, ignoring
whitespace; `Mapped::{Exact, InJinja, Unmapped}` answers one span. Fixes only
for `Fresh` + `Exact`; `InJinja` highlights without a fix; `Unmapped` is plan
view only. A stale plan shows a banner and produces no diagnostics. If neither
source works, `Declined` with `Guidance` pointing at `dbt compile`.

For a compiled-origin plan the spans index the compiled text, and `verify_hir`
is given that text; `SpanMap::map` translates them for the editor.

## Types and schema

`SqlType` is a lattice with `Unknown` and `Other(raw)`, parsed through
sqlparser's `parse_data_type`, never failing. `ColType` carries nullability;
`not_null` tests and constraints give `NotNull`, outer joins force `Nullable`.

`ProjectSchemas { revision, models, sources }` is built by a background job
after refresh, walking `ProjectGraph` topologically on saved files only, with
early cut-off on unchanged fingerprints. "Column does not exist" requires a
`Closed` `ModelSchema`.

## Reused code

`cogitator_parser::backend::{sql_dialect, tokenize_template}`, `Branches` in
`sql_lex.rs`, `ProjectGraph`, `CatalogIndex`, `compiled_sql_path`,
`closest_matches`.
