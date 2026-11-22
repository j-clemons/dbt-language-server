# Feature Requests — Missing LSP Features

Status of the Rust implementation (`crates/`) against the LSP specification.
The server currently implements exactly the original Go server's surface:

| Implemented today | Notes |
|---|---|
| `initialize` / `shutdown` / `exit` | capabilities per `specs/rust-port/01-protocol.md` §3.1 |
| `textDocument/didOpen` / `didChange` / `didSave` | incremental sync (`textDocumentSync: 2`) |
| `textDocument/hover` | refs, sources, vars, macros, dialect SQL functions |
| `textDocument/definition` | models, sources/tables, vars, macros, in-file CTEs |
| `textDocument/completion` | context-aware inside `ref('` / `source('` / `var('` / `{{` |
| `workspace/executeCommand` → `dbt.goToSchema` | jump to the model's properties-YAML entry |
| `textDocument/publishDiagnostics` (push) | dbt Fusion compile only (`--fusion`) |

Everything below is missing. Ordered by suggested priority; "building block"
names what already exists in the codebase.

## P1 — cheap wins, real gaps

- [x] **`textDocument/didClose`** — closed documents currently stay in the
      `Documents` store forever (leak inherited from the Go server).
      Building block: `Documents` (`crates/cogitator-analysis/src/lib.rs`) just
      needs a `close(uri)` + dispatch arm.
- [x] **Wire DuckDB parse diagnostics into the dispatcher** — the masking,
      confidence rule and `json_serialize_sql` engine exist
      (`crates/cogitator-duckdb`, feature `duckdb-parser`, spec 07 §3) but are
      not yet called on didOpen/didSave for duckdb projects.
- [x] **`textDocument/references`** — find all models referencing a model.
      Building block: the petgraph dependency graph already answers it
      (`ProjectGraph::dependents_of`, `crates/cogitator-analysis/src/graph.rs`);
      only dispatch + response wiring is missing.
- [x] **Dangling-ref diagnostics** — warn on `ref('nonexistent')` without
      Fusion. Building block: `ProjectGraph::dangling_refs` (spec 05 §5).
- [x] **`initialized` notification** — currently ignored; handle explicitly.

- [x] **Project scan cache** (05 §6.2 fast path): `FileCache` keyed by
      (mtime, size) in `cogitator-analysis/src/cache.rs`; the refresh task
      keeps one `ContextBuilder` alive, so unchanged YAML/macro/docs/model
      files are never re-read or re-parsed between refreshes.
- [x] **Column lineage**: `dbt.columnLineage` workspace command
      (`cogitator-analysis/src/lineage.rs`) — named-placeholder masking,
      sqlparser AST, CTE-transitive resolution incl. `select *` chains;
      declines on Jinja control blocks (07 §3.2 confidence rule).

- [x] **Column hover + go-to-definition**: hovering a column shows
      "Column of <relation>" with its YAML description and, when derivable,
      its lineage sources inline (dbt.columnLineage surfaced in the editor);
      definition jumps to the column's YAML entry (column positions now ride
      the saphyr side-table like model names).
- [x] **Model rename** (`textDocument/rename`): the graph knows every
      dependent, so the returned WorkspaceEdit rewrites each `ref('…')`,
      updates the properties-YAML entry and renames the model file
      (documentChanges with the file rename last). Caveat: dependents are
      read from disk; unsaved buffers of other files aren't visible here.
- [x] **CodeLens batch** on model files — "N downstream" / "N upstream"
      counts (graph; dbt terminology: `downstream_of`/`upstream_of`),
      "Go to schema" (navigates via window/showDocument), "Column lineage"
      (formatted into window/showMessage), and "dbt run / test / compile"
      executing per-model (`--select <selector>`, project-root cwd; fusion
      path when set, else `dbt`), with a successful compile opening
      target/compiled/<project>/<model>.sql in the editor. Macro files get
      per-macro "N call sites" lenses. Server→client requests use an
      outgoing-id counter; client responses are ignored by a response guard.
- [x] **rootUri / workspaceFolders** honored at initialize (modern clients
      that omit the deprecated rootPath find the project again).
- [x] **Actionable error feedback**: failures report through
      window/showMessage with reason + solution — missing dbt project,
      unresolved dialect/profiles.yml, invalid fusion binary, dbt executable
      missing or failing (exit code + stderr tail + exact retry command),
      missing schema entry, lineage declines, defused upgrade subcommand.
- [x] **Completion engine** (grammar-aware): the cursor position is
      classified from the token stream — member access (`alias.` → columns
      of the resolved relation via a token-based FROM/JOIN scope index),
      relation position after FROM/JOIN (CTEs plain, models/sources as
      `{{ ref(...) }}` snippets), column positions (SELECT/WHERE/ON/…),
      keyword continuations (`group `→`by`, `left `→`join`), with tiered
      ranking (context-exact < locals < columns < relations < functions <
      keywords). Fixes the inherited quirk where any earlier `{{ }}` on the
      line triggered macro completion.
- [x] **Write-assist completion** in plain SQL: local CTEs, identifiers
      already in the buffer (table aliases, columns in use), documented
      columns of every `ref`/`source` relation in the file (from the
      properties YAML, now carried through `cogitator-schemas`), and SQL
      keywords from sqlparser's table (covers the Snowflake set — QUALIFY,
      ILIKE, …), sorted local-first.
- [x] **Snowflake coverage closed out**: profiles.yml → dialect, keyword
      suggestions, function docs/completion (743 entries), and lineage
      parsing Snowflake syntax (QUALIFY) — all tested.

## P2 — quality of life

- [x] **`completionItem/resolve`** — send lean completion items, resolve
      documentation lazily (today every item carries full docs).
- [x] **`$/cancelRequest`** *(REAL since the offload batch: aborts in-flight work, answers -32800)* — ignored today; matters once any request gets
      slower than the current sub-millisecond handlers.
- [x] **`textDocument/signatureHelp`** for macros *(parameters split from the signature text)* — feasible once the
      minijinja-parsed macro parameters land (spec 06 §2.2 enrichment:
      `MacroParam { name, default }`).
- [x] **`workspace/didChangeWatchedFiles`** *(handled as refresh trigger; dynamic registration not requested yet, so some clients never send it)* — file watching as the context
      refresh trigger instead of the didOpen/didSave heuristic
      (pairs with the incremental-refresh fast path, spec 05 §6.2).
- [x] **Proper JSON-RPC error responses** *(MethodNotFound / InvalidParams on requests)* — malformed requests are silently
      dropped (faithful to Go); unknown methods should answer
      `MethodNotFound`, bad params `InvalidParams`.

## P3 — planned phases (already specced)

- [x] **Tree-sitter backend shipped** (`crates/cogitator-treesitter`, 06 §3): the
      tree-sitter-dbt grammar (sirh3e/tree-sitter-dbt, once a vendored dbt-labs
      jinja2 grammar) segments documents error-tolerantly —
      broken constructs degrade locally instead of staling the whole document
      — and is the default `DocumentParser` (fallback: `--no-tree-sitter`).
      06 §3.4 incremental `tree.edit()` reparse: DONE for documents past a
      32 KiB gate (equivalence-tested against full reparse). The grammar is
      full Jinja with dbt's blocks and lives in its own repository, pinned by
      `rev` in the workspace Cargo.toml.
- [x] **Tree-sitter batch — token-based v1 shipped** (06 §3.6, the tree can
      later upgrade fidelity: richer folding/symbols from real nodes):
      - [x] `textDocument/semanticTokens` (full)
      - [x] `textDocument/foldingRange` (multi-line paren pairs)
      - [x] `textDocument/documentSymbol` (CTEs + macros, flat)
      - [x] `textDocument/selectionRange` (token → line → document)
- [x] **dbt YAML intelligence — basic v1** (spec 08 §2.6):
      - [x] syntax + dbt-schema deserialize diagnostics for `dbt_project.yml`
            and properties files
      - [x] property-name completion in dbt `.yml` files (heuristic section
            detection; enum-value completion still open)
- [x] **Pull diagnostics** — `textDocument/diagnostic` (full report, same
      static set as the push pipeline); `workspace/diagnostic` still open.
- [ ] **AST-driven features for duckdb projects** (spec 07 §4,
      `json_serialize_sql` AST): column-name completion, AST-based graph
      edges, better CTE information.

## P4 — not planned / needs a Jinja renderer or major new scope

Implemented from this bucket (they turned out cheap on the existing data):
- [x] `textDocument/documentHighlight` (same literal + kind occurrences)
- [x] `workspace/symbol` (models, sources, macros)

Still open, with reasons:

- `textDocument/formatting` / `rangeFormatting` / `onTypeFormatting`
  (would mean embedding a SQL formatter)
- `textDocument/codeAction`, `prepareRename`,
  `inlayHint`, `inlineValue`, `documentLink`, `linkedEditingRange`,
  `documentColor`
- `textDocument/typeDefinition` / `implementation` / `declaration`
  (little meaning for dbt sources)
- Call/type hierarchy (could model macro call chains someday)
- workspace folders / multi-root (only the deprecated
  `rootPath` is read), `workspace/didChangeConfiguration`,
  `workspace/applyEdit`, file-operation notifications, `$/progress`

- [ ] **Quickfix code actions from did-you-mean suggestions** — purely
      additive on top of the Guidance diagnostics: needs Diagnostic.data +
      relatedInformation wire fields and a diagnostics-context codeAction
      path; the suggestions already ride every dangling-ref/-source/
      unknown-var message.
