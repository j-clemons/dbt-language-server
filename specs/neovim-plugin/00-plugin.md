# cogitator.nvim — implementation spec

A Neovim plugin that drives the `cogitator` dbt language server. This spec is
written against the server as of the repository head; every wire shape quoted
here is ground truth from `crates/cogitator-lsp/src/types/init.rs` and
`crates/cogitator/src/server/dispatcher.rs` — when in doubt, those files win.

Target: **Neovim ≥ 0.11** (`vim.lsp.config` / `vim.lsp.enable`). §10 notes the
0.10 fallback via `nvim-lspconfig`. Language: Lua only, no external deps.

---

## 1. Server contract (what the plugin talks to)

### 1.1 Transport and lifecycle

- stdio, standard `Content-Length` framing. Nothing but LSP frames on stdout.
- `initialize` → `initialized` → normal traffic → `shutdown` → `exit`.
  The server honors `workspaceFolders[0]`, then `rootUri`, then the
  deprecated `rootPath`.
- The server never requests dynamic registration. There is no `$/progress`.
- `exit` terminates the process immediately (exit 0). EOF on stdin is also a
  clean shutdown.

### 1.2 CLI flags the plugin may surface

| flag | meaning |
|---|---|
| `--profiles-dir <dir>` / `-p` | profiles.yml directory (`~` expanded) |
| `--fusion [path]` / `-f` | enable dbt Fusion static analysis; optional binary path (default `dbt`) |
| `--no-tree-sitter` | fall back to the phase-1 minijinja+sqlparser pipeline |
| `--debug` / `-d` | debug log to `log.txt` next to the executable |
| `--otlp [endpoint]` | OTLP trace export |
| `-v` / `--version` | print version and exit |

### 1.3 initializationOptions

```json
{ "profilesDir": "/data/dbt-profiles" }
```

`profilesDir` (also accepted: `profiles_dir`) replaces a `--profiles-dir`
launch flag. The server also accepts `profile`, `target` and
`profilesAllow` (`profiles_allow`) — the same overrides as the matching
CLI flags; the Neovim manager store injects profilesDir/profile/target
per project root at attach. Any other shape of `initializationOptions`
is ignored, never an error.

A fourth, **hot** configuration layer sits underneath these:
`<root>/cogitator.yml` (see the README's "Configuration file"), applied
at initialize and re-applied changed-keys-only while the server runs.
Precedence stays defaults < file < CLI < initializationOptions at
launch; later file edits win over earlier runtime switches (last
writer wins).

### 1.4 Advertised capabilities (initialize result)

- `textDocumentSync: 2` (incremental)
- `hoverProvider`, `definitionProvider` (incl. SQL built-ins → their
  bundled docs materialized under `target/.cogitator/docs/`),
  `referencesProvider`,
  `documentSymbolProvider`, `workspaceSymbolProvider`,
  `documentHighlightProvider`, `foldingRangeProvider`,
  `selectionRangeProvider`, `renameProvider`: `true`
- `completionProvider: { resolveProvider: true, triggerCharacters:
  [".", "'", "\"", " ", "("] }`; see §6.3
- `signatureHelpProvider: { triggerCharacters: ["(", ","] }`
- `inlayHintProvider: true` (relation after ref()/source(), macro
  parameter names), `documentLinkProvider: { resolveProvider: false }`
  (ref/source → definition targets), `linkedEditingRangeProvider:
  true` (CTE names only)
- `documentFormattingProvider: true` — sqlformat over Jinja-segmented
  SQL; full-line `{% %}` lines pass verbatim, `{{ }}` expressions are
  sentinel-masked, and ANY doubt (inline statements, sentinel
  miscount) declines with zero edits
- `codeActionProvider: true` — besides the executable-lens set, the
  response now carries edit-bearing actions: did-you-mean quickfixes
  from diagnostic data, "Convert to ref()/source()" on literal
  relations matching known coordinates (refactor.rewrite), and — at a
  properties-YAML model entry — key-column test scaffolds plus
  inherit-description fixes sourced from column lineage. Historically:
  **command-only** actions (no edits, no
  kinds, no resolve): the executable lens set at the cursor, so every
  stock LSP editor gets dbt workflows without client wiring. A non-empty
  `context.only` filter answers `[]` (the actions are kind-less). On a
  `ref('x')` token, "Open schema of 'x'" is prepended; in a properties
  YAML, the entry at/above the cursor offers "Open model" (via
  `dbt.openModel`) and "Go to tests". In the **active profiles.yml**
  the action set is instead one "Switch target: profile/target" per
  allowed profile×target (the active pair marked) running
  `dbt.setTarget` — target switching without a picker, usable from any
  stock LSP editor including Zed.
- `semanticTokensProvider`: `full: true`, legend token types (in index
  order): `keyword, function, macro, variable, string, number, operator,
  namespace`; no modifiers
- `diagnosticProvider: { interFileDependencies: false, workspaceDiagnostics: false }`
  (document pull diagnostics)
- `codeLensProvider: { resolveProvider: false }`
- `executeCommandProvider.commands`:
  `dbt.goToSchema`, `dbt.columnLineage`, `dbt.openSchema`, `dbt.showLineage`,
  `dbt.run`, `dbt.test`, `dbt.compile`, `dbt.modelGraph`, `dbt.profiles`,
  `dbt.setTarget`, `dbt.extractModelYaml`, `dbt.modelTests`,
  `dbt.goToTests`, `dbt.status`, `dbt.projectGraph`, `dbt.modelInfo`,
  `dbt.relation`, `dbt.compiledSql`, `dbt.parse`, `dbt.openModel`,
  `dbt.setProfilesDir`
  (append-only: clients may assume index 0 is `dbt.goToSchema`)
- `serverInfo.name == "cogitator"`

### 1.5 Server→client traffic the plugin must handle

- `window/showMessage` — all user feedback, including multi-line
  `Reason: … Solution: …` texts and async dbt run/test/compile outcomes.
- `window/showDocument` (a server→client **request**) — used by
  `dbt.openSchema` and by `dbt compile` success to open the compiled SQL.
  The server ignores the response body, but the client should still reply.
- `textDocument/publishDiagnostics` — push diagnostics while Fusion mode is
  active (pull diagnostics are suppressed server-side in that mode).

### 1.6 workspace/executeCommand argument shape

Every command takes **one argument object**, not positional values:

```json
{ "command": "dbt.run",
  "arguments": [ { "uri": "file:///…/models/customers.sql",
                   "position": { "line": 0, "character": 0 } } ] }
```

`position` is only read by `dbt.goToSchema` and `dbt.relation`. Results:

| command | result | side effect |
|---|---|---|
| `dbt.goToSchema` | `Location` or `null` | none (client jumps) |
| `dbt.openSchema` | `null` | `window/showDocument` or warning |
| `dbt.columnLineage` | lineage JSON or `null` | none |
| `dbt.showLineage` | `null` | formatted `window/showMessage` |
| `dbt.run` / `dbt.test` / `dbt.compile` | `{token}` (immediately) | async showMessage; compile success opens the compiled SQL via showDocument; the token feeds `dbt.cancel` |
| `dbt.parse` | `{token}` (immediately) | async showMessage/$\/progress; project-wide (no `--select`), success refreshes the server context |
| `dbt.modelGraph` | graph payload or `null` | none |
| `dbt.profiles` | redacted profiles inventory | warning when no profiles.yml |
| `dbt.setTarget` | `{profile, target}` or `null` | context refresh + confirmation message |
| `dbt.extractModelYaml` | `null` | workspace/applyEdit (CreateFile scaffold, or version TextEdits) + showDocument |
| `dbt.modelTests` | `{model, tests: [{name, column?, version?, uri, range}]}` or `null` | none |
| `dbt.goToTests` | `null` | showDocument to the first test + INFO list |
| `dbt.status` | server/project/profiles/refresh/caches/config snapshot | none |
| `dbt.projectGraph` | `{nodes: [{id, name, kind, layer, uri, tags, columns}], edges: [{from, to}]}` | none |
| `dbt.modelInfo` | `{model, uri, description, tags, upstream, downstream, versions, tests, compiledUri}` or `null` | none |
| `dbt.relation` | `{kind, name, source, database, schema, identifier, relation}` or `null` | none |
| `dbt.compiledSql` | `{model, uri, content}` or `null` | WARNING when the file is over the wire cap (2 MiB default; `limits.compiled_sql_mib`) |
| `dbt.openModel` | `null` | showDocument to the model's file (`{uri}` or `{model}`), or a warning |
| `dbt.setProfilesDir` | `{dir}` or `null` | live profiles-directory switch: profile/target overrides reset, context refresh, confirmation message — no client restart |
| `dbt.cache` | `{profiles, lineage, results, artifacts, plans}` each `{entries, hits, misses}` | `{action: "clear"}` empties the derived caches and refreshes the context |
| `dbt.reloadConfig` | `null` | forces an immediate cogitator.yml reload pass (otherwise: 2 s stamp poll / didSave fast-path) |
| `dbt.sourceFreshness` | `{token}` (immediately) | async `dbt source freshness` (project-wide); the refreshed sources.json feeds the freshness lenses on source YAML entries |
| `dbt.runSelection` | `{token}` or InvalidParams | runs dbt with an arbitrary validated selector (`{selector, kind?: run\|test\|build\|compile}`) |
| `dbt.cancel` | `{token, label}` or InvalidParams | aborts the running invocation (the child process is killed) + confirmation message |
| `dbt.deadModels` | `{models: [{name, uri, reasons}]}` | none (conservative: seeds/package models never flagged; selector mentions keep a model alive) |
| `dbt.impact` | `{model, column, summary, models: [{name, uri, depth}], exposures, columnHits, truncated}` | none (column hits are an under-approximation: `*` and unresolved lineage stay out; lineage fan-out capped at 200 models) |
| `dbt.expandJinja` | `{sql}` or InvalidParams Guidance | renders the open buffer with minijinja — project macros, ref/source → relations, var(), config()/is_incremental()/target stubs; adapter calls need a real compile |
| `dbt.doctor` | `{serverVersion, projectRoot/Name, dialect, dbtBinary(+Found), profiles, artifacts (ages incl. catalogAgeS + driftCount), caches, scanCachePersisted, graph, lintEnabled, runningJobs, config}` | none — the one health report :checkhealth/doctor render |
| `dbt.queryPlan` | `{model, dialect, backend, schemaRevision, status: complete\|partial\|declined, guidance, origin, levels: {hir, mir, lir: {available, fidelity, text, unavailable, degradations, violations}}, findings, trace: {entries, incomplete}, needs: {available, missing}}` | none (read-only): runs the IR pipeline on the open buffer or file; a level dropped by a verifier violation is shown as unavailable; models with control flow other than a lone `is_incremental()` come back `declined` with a Guidance pointing at `dbt compile` until compiled fallback lands — :DbtPlan |
| `dbt.preview` | `{token}` (job started) | `dbt show --select <model> --limit N` (N from `limit`, default 50) through the job registry; on success the result table lands in `target/.cogitator/preview/<model>.txt` and opens via showDocument — :DbtPreview [limit] |

On duckdb-feature builds with cogitator.yml `warehouse: {columns:
true}`, source tables whose YAML declares no columns get them from the
warehouse's information_schema during the context refresh (read-only,
never on a request path); completion/hover then show them with their
data type marked "(from warehouse)".

Two-argument `ref('package', 'model')` resolves like plain `ref()`
everywhere (hover, definition, completion, dangling checks): the first
string refines as the package, the second as the model. packages.yml /
dependencies.yml buffers warn per declared-but-not-installed package.

`dbt.relation` resolves the `source()` table or `ref()` under `position`
(otherwise the file's own model) to its warehouse coordinates. **dbt
defaults only**: a source without `schema:` uses the source name, a table
without `identifier:` its declared name, and model database/schema come
from the active profiles target (duckdb: the `path` file stem and `main`).
Custom `generate_schema_name`/`generate_database_name` macros and
`models:` config trees (`+schema`/`+database`/`+alias` in
dbt_project.yml) are **not** evaluated. Unresolvable parts are `null` and
left out of the dot-joined `relation` string.

`dbt.compiledSql` takes `{uri?, model?}` (`model` wins, like
`dbt.modelInfo`) and serves the compiled file's text so clients need no
filesystem path math; `uri`/`content` are `null` when dbt has not written
`target/compiled/…` yet, and `content` alone is `null` (with a WARNING
message) past the 2 MiB cap — open the `uri` directly then.

`workspace/symbol` additionally understands `tag:x` (models carrying tag
x — from YAML `tags:`/`config.tags` and `{{ config(tags=…) }}`) and
`selector:x` (selectors.yml entries) queries.

Deliberate non-goals of this surface: no `manifest.json` /
`run_results.json` fallback (the server's own analysis is always live;
manifest parsing is a specs/rust-port/00 non-goal), no full Jinja render
command (minijinja only tokenizes — 05 §4), no call hierarchy
(`dbt.columnLineage` covers column flows), no `dbt.syntaxTree`, no
custom `generate_schema_name`-style macro evaluation in `dbt.relation`,
no editor-side tree-sitter grammar builds (highlighting rides the
server's semantic tokens), and no dedicated `cache` workflow beyond
`dbt.status`'s cache counters and `dbt.cache` (stats/clear).

### 5.3b Project DAG view (`:DbtDag`) — **Neovim only**

`lua/cogitator/dag.lua` renders `dbt.projectGraph` as a dbt-docs-style
DAG in a scratch tab: layers left→right, box-drawn nodes with `#tags`,
orthogonal edges with corner glyphs (never overdrawing box content), and
docglow-style column information inside expanded nodes. Nodes are filled
in the dbt-docs resource colours (`src/app/services/graph.service.js`,
groups `DbtDag*` in `cogitator.highlight`, defined with `default = true`);
the selected node and its edges are recoloured in a separate extmark
namespace so moving the selection never rebuilds the grid.
Exactly one node is selected, and movement is node to node (`cogitator.dag_nav`,
pure): `h`/`l` go to a connected parent/child (nearest by row, or back
along the path just taken), `j`/`k` through the same layer, `gg`/`G`
first/last node (or node `[count]`), `{`/`}` first/last layer (or layer
`[count]` from that end), `/` live name search with `n`/`N`,
`<C-o>`/`<C-i>` selection history; counts repeat `hjkl`, `n`/`N` and the
history keys. Native motions and the mouse move the selection through a
`CursorMoved` hit test. Other keys: `<CR>` open, `c` toggle this node's
columns, `C` toggle all, `L` column lineage via `dbt.showLineage`, `R`
refresh, `q` close. The server command is editor-neutral; this UI is
deliberately implemented only in the Neovim plugin.

### 5.3c Project browser (`:DbtBrowse`) — **Neovim only**

`lua/cogitator/browse.lua` is the yazi-style browser over the whole project: the
center float lists every model from `dbt.projectGraph` (`/` filters
live, smart-case substring), the side floats show the selection's
direct upstream/downstream from `dbt.modelInfo` (debounced per cursor
move, stale responses discarded), and the full-width preview below
switches between `[1]` the raw SQL, `[2]` the compiled SQL from
`target/compiled/<project>/…` (a hint when dbt has not compiled the
model — the browser never runs dbt itself) and `[3]` versions + tests.
Keys: `j`/`k` and the other native motions move (counts included, which
is why the preview tabs are not on the digit keys); from the center
`h`/`l` enter the upstream/downstream pane; in a side pane `h` upstream
or `l` downstream re-centers on the entry and stays there, so repeating
it walks the lineage (`[count]` walks that many; steps queue while
`dbt.modelInfo` is in flight), and the opposite key returns to the
center. `p` focuses the preview, `J`/`K` scroll it from any pane, `n`/`N`
step the center list with wraparound, `y` yanks a name, `<CR>` on a side
entry re-selects it in the center, `<CR>`/`o` on the center opens the
file, `<Tab>`/`<S-Tab>` cycle the preview tab and `[count]<Tab>` picks
one (1 raw / 2 compiled / 3 versions+tests / 4 info), `<Esc>` clears the
filter first and closes second, `q` closes. Side-pane entries use the
`DbtDag*Fg` resource colours. The info tab shows description, tags, the
dbt.relation name, documented columns from the project graph, and the
model's tests — `<CR>` on a test line jumps to its YAML entry. The
implementation is the `cogitator.explorer` package (init/render/
filter/keys/preview/info) with `cogitator.browse` as a facade and
`:DbtExplorer` as a command alias; the DAG's canvas + barycenter
layout engine is the pure `cogitator.layout` module. The server
command is editor-neutral; this UI is deliberately implemented only in
the Neovim plugin.

`dbt.modelInfo` takes `{uri?, model?}` (`model` wins) and answers
everything the browser's preview needs in one round trip:

```json
{ "model": "customer_orders", "uri": "file:///…/customer_orders.sql",
  "description": "", "tags": [],
  "upstream":   [{ "name": "orders", "kind": "model", "uri": "file:///…" }],
  "downstream": [],
  "versions": { "latestVersion": 2,
                "versions": [{ "v": 1, "uri": "file:///…_v1.sql" },
                             { "v": 2, "uri": "file:///…customer_orders.sql" }] },
  "tests": [{ "name": "not_null", "column": "order_count", "version": 1,
              "uri": "file:///…/customer_orders.yml", "range": { … } }],
  "compiledUri": null }
```

`upstream`/`downstream` entries are the `dbt.modelGraph` shape, `tests`
the `dbt.modelTests` shape. `versions` is `null` for an unversioned
model; a version's `uri` follows dbt's file convention
(`<model>_v<N>.sql`, the latest version may live in `<model>.sql`).
`compiledUri` is non-null only when
`target/compiled/<project>/<model path>` exists on disk (kept for
display/back-compat). The browser's compiled tab fetches the text
through `dbt.compiledSql` instead of reading that file itself.

When the client advertises `window.workDoneProgress`, context refreshes
and dbt run/test/compile report through `window/workDoneProgress/create`
+ `$/progress` instead of the start/success showMessages (failures keep
their Reason/Solution messages).

`dbt.profiles` answers `{ path, searched, activeProfile, activeTarget,
profiles: [{ name, defaultTarget, allowed, targets: [{ name, type,
auth: { method, issues: [string] } }] }] }` — auth methods and problems,
never credential values; profiles outside the allowlist appear by name
with `allowed: false`. `dbt.setTarget` takes
`{ "target": "prod", "profile"?: "name" }`.

`dbt.modelGraph` also accepts `{ "model": "<name>" }` in place of `uri`
(re-centering) and answers with direct neighbors only:

```json
{ "model": "customers", "uri": "file:///…/customers.sql",
  "upstream":   [ { "name": "stg_customers", "kind": "model",  "uri": "file:///…" } ],
  "downstream": [ { "name": "mart_x",        "kind": "model",  "uri": "file:///…" } ] }
```

`kind` is `model` or `source`; `uri` is `null` for dangling refs and for
sources without a defining YAML.

---

## 2. Plugin layout

```
cogitator.nvim/
├── plugin/cogitator.lua          -- guard + user commands (no setup needed)
├── lua/cogitator/
│   ├── init.lua                  -- setup(opts), vim.lsp.config registration
│   ├── config.lua                -- defaults + validation
│   ├── commands.lua              -- :Dbt* user commands → executeCommand
│   ├── lens.lua                  -- codelens refresh autocmds
│   └── health.lua                -- :checkhealth cogitator
└── doc/cogitator.txt
```

Rules: `require("cogitator")` must be cheap (no I/O at require time); all
`vim.fn`/`vim.loop` probing happens in `setup()` or lazily. Every module
≤ ~200 lines.

---

## 3. Configuration surface

```lua
require("cogitator").setup({
  cmd = { "cogitator" },        -- or full path; extra args appended from opts below
  profiles_dir = nil,           -- string → sent as initializationOptions.profilesDir
  fusion = false,               -- false | true | "/path/to/dbt"
  tree_sitter = true,           -- false → --no-tree-sitter
  debug = false,                -- true → --debug
  filetypes = { "sql", "yaml" },
  root_markers = { "dbt_project.yml" },
  attach_outside_project = false, -- see §4.2
  lens = { enabled = true, events = { "BufEnter", "CursorHold", "InsertLeave" } },
  keymaps = true,               -- buffer-local defaults of §5.3
})
```

Validation with `vim.validate`; unknown keys are an error (fail loudly at
setup, never mid-session). `fusion = true` appends `--fusion`;
`fusion = "<path>"` appends `--fusion <path>`. `profiles_dir` goes through
`init_options`, **not** the CLI, so one Neovim instance can serve several
projects with different launch-time settings later.

---

## 4. Server registration and attach

### 4.1 vim.lsp.config (0.11+)

```lua
vim.lsp.config("cogitator", {
  cmd = opts.cmd_with_flags,
  filetypes = opts.filetypes,
  root_markers = opts.root_markers,
  init_options = opts.profiles_dir and { profilesDir = opts.profiles_dir } or nil,
  capabilities = vim.tbl_deep_extend("force", vim.lsp.protocol.make_client_capabilities(), {
    window = { showDocument = { support = true } },
    workspace = {
      workspaceEdit = {
        documentChanges = true,
        resourceOperations = { "rename" },   -- model rename renames the file
      },
    },
  }),
})
vim.lsp.enable("cogitator")
```

### 4.2 Attach discipline

- Attach **only** when a `dbt_project.yml` is found upward from the buffer
  (`root_markers` does this), unless `attach_outside_project = true`. A dbt
  LSP on arbitrary SQL files is noise.
- YAML buffers: attach under the same root; the server serves schema-YAML
  diagnostics and property completion for them.
- One client per root dir — `vim.lsp.enable` already reuses by
  `(name, root_dir)`; do not spawn manually with `vim.lsp.start` in
  addition.

### 4.3 Handlers

- `window/showDocument`: Neovim 0.11 ships a default handler; keep it.
  Verify in an integration test that the schema file opens and the selection
  is applied.
- `window/showMessage`: route through `vim.notify`, mapping
  Error→`vim.log.levels.ERROR`, Warning→`WARN`, Info→`INFO`. The server
  sends multi-line bodies; pass them through unchanged (notifier plugins
  render them fine; `:messages` keeps them greppable).

---

## 5. User commands and keymaps

### 5.1 Commands (`plugin/cogitator.lua`, buffer-aware)

| command | executeCommand | notes |
|---|---|---|
| `:DbtRun` | `dbt.run` | current buffer's uri |
| `:DbtTest` | `dbt.test` | |
| `:DbtCompile` | `dbt.compile` | compiled SQL opens via showDocument |
| `:DbtParse` | `dbt.parse` | project-wide; refreshes artifacts + server context |
| `:DbtRelation` | `dbt.relation` | yanks the resolved relation into `"` (and `+` when a clipboard exists), `<leader>dR` |
| `:DbtTarget` | `dbt.profiles` + `dbt.setTarget` | picker over allowed profile×target; the choice persists per project in the manager store, `<leader>dT` |
| `:DbtProfiles [dir]` | `dbt.setProfilesDir` | no arg: candidate report; with dir: live switch (persisted per root; restart fallback for servers without the command) |
| `:DbtProjects` | — (manager store) | picker over every project root ever attached; opening its dbt_project.yml attaches the per-root client, `<leader>dP` |
| `:DbtPlan` | `dbt.queryPlan` | each IR level, findings and the rule trace (including skip reasons) in a scratch split |
| `:DbtQuery` | `dbt.relation` | `select * from <relation> limit <query_limit>` through vim-dadbod when configured, else a scratch sql buffer, `<leader>dx` |

The manager store is a JSON file (default
`stdpath("data")/cogitator/manager.json`, override via setup
`manager_path`) holding `{projects = {[root] = {profiles_dir?, profile?,
target?, last_used}}}`. Stored selections are merged into
`initializationOptions` per root through `before_init` at attach, and
applied live through `dbt.setProfilesDir`/`dbt.setTarget` while a client
runs.
| `:DbtSchema` | `dbt.goToSchema` | uri + cursor position; jump to the returned `Location` with `vim.lsp.util.show_document` |
| `:DbtLineage` | `dbt.showLineage` | result arrives as a showMessage |

Implementation: `client:exec_cmd({ command = …, arguments = { { uri = vim.uri_from_bufnr(0), position = <cursor> } } }, { bufnr = 0 }, handler)`.
Only `:DbtSchema` needs a result handler; the rest are fire-and-forget
(`null` results). All five must error cleanly ("cogitator is not attached to
this buffer") when no client is attached.

### 5.2 Code lens

- When `lens.enabled`, create an augroup per attached buffer:
  `vim.lsp.codelens.refresh({ bufnr = bufnr })` on the configured events and
  once on attach.
- `vim.lsp.codelens.run()` executes the lens under the cursor; it sends the
  lens's `command.arguments` verbatim — this matches §1.6, no translation
  layer needed.
- **Macro call-site lenses carry `command = ""`** (informational only). The
  plugin must not offer them for `run` (filter in a small wrapper around
  `vim.lsp.codelens.run`, or accept Neovim's no-op).
- Model lenses in display order: `N downstream`, `N upstream`,
  `Go to schema`, `Column lineage`, `dbt run`, `dbt test`, `dbt compile`.

### 5.3 Model-graph browser (`:DbtGraph`, `lua/cogitator/graph.lua`)

A yazi-style three-pane float over `dbt.modelGraph`: direct upstream on the
left, the current model (name, path, counts) in the center, direct
downstream on the right. Keys: `h`/`l` move focus between panes, `j`/`k`
select in a side pane, `<CR>` re-centers the graph on the selected neighbor
(a `dbt.modelGraph` call by `model` name), `o` opens the selected entry's
file (closing the browser), `q`/`<Esc>` close. Sources are tagged
`[source]` and refuse re-centering with an INFO notice; entries without a
`uri` are tagged `(missing)`. One instance at a time; the fetch runs
through the originating buffer's client.

### 5.4 Default buffer-local keymaps (when `keymaps = true`)

A `<leader>d` prefix, deliberately clear of Neovim 0.11's built-in `gr*`
LSP maps: `<leader>ds` → `:DbtSchema`, `<leader>dl` → `:DbtLineage`,
`<leader>dr` → `:DbtRun`, `<leader>dt` → `:DbtTest`, `<leader>dc` →
`:DbtCompile`, `<leader>dg` → `:DbtGraph`, `<leader>dR` →
`:DbtRelation`, `<leader>dL` → `vim.lsp.codelens.run`. All with `desc`
set, all buffer-local on attach only.

---

## 6. Feature-specific client notes

### 6.1 Rename (`textDocument/rename`)

The server returns one `WorkspaceEdit` with `documentChanges`: text edits in
dependent files, the properties-YAML name edit, and a `RenameFile` **last**.
The plugin must advertise the capabilities in §4.1 or the file rename is
silently dropped by the server-agnostic client code. Caveat to document in
`:h cogitator`: dependents are read from disk, so unsaved changes in *other*
buffers are not seen — save before renaming.

### 6.2 Diagnostics

- Pull: Neovim 0.11 drives `textDocument/diagnostic` automatically from
  `diagnosticProvider`. Nothing to implement; dangling-`ref` and YAML schema
  errors arrive this way.
- Push: with `fusion` enabled the server publishes compile diagnostics and
  suppresses the static ones. Both paths land in `vim.diagnostic`; no merge
  logic in the plugin.

### 6.3 Completion

- The server advertises `triggerCharacters = [".", "'", "\"", " ", "("]`
  so nvim-cmp / blink.cmp / native `vim.lsp.completion` fire in
  member-access (`alias.`), `ref('`, and post-keyword positions with no
  client configuration. The plugin keeps a client-side fallback that sets
  the same characters **only when the server advertised none** (older
  server builds).
- Ranking arrives via `sortText` tiers (`0` < `a` < `b` < `c` < `m` < `z`);
  completion engines must be left on `sortText` ordering (document that
  `cmp.config.compare.sort_text` has to stay in the comparator list).
- `completionItem/resolve` fills documentation — enable
  `resolveSupport` (default in 0.11).
- `insertText` is plain text (no snippet syntax). Do not advertise
  `snippetSupport`-dependent behavior.

### 6.4 Semantic tokens

Automatic in 0.11. Provide optional highlight links for the legend's
non-standard entries, e.g. `@lsp.type.macro.sql → @function.macro`, and a
`semantic_tokens = false` escape hatch
(`vim.lsp.semantic_tokens.enable(false, { bufnr })`).

### 6.5 Signature help

Nothing custom: triggers `(` and `,` come from the server; structured macro
parameters render through the default handler.

---

## 7. Health check (`:checkhealth cogitator`)

Report, in order:

1. `cogitator` binary found on `PATH` (or configured `cmd[1]`) + its
   `--version` output.
2. Neovim version ≥ 0.11 (else point at §10 fallback).
3. For the current buffer/cwd: whether `dbt_project.yml` was found and where.
4. Profiles resolution preview: list the candidate paths in server order
   (§1.3 of the README: override → `$DBT_PROFILES_DIR` → `<root>/profiles.yml`
   → `<root>/.dbt/profiles.yml` → `~/.dbt/profiles.yml`) and mark the first
   that exists. (Duplicated client-side knowingly; keep in sync with
   `util::profiles_candidates`.)
5. If `fusion` is configured: the fusion binary exists and `--version`
   mentions `dbt-fusion` (mirror of the server's own validation).

---

## 8. Testing

- Layout: `tests/run.lua` is a thin orchestrator over `tests/helpers.lua`
  and five ordered step files (`tests/steps/01_unit.lua` … `05_profiles.lua`;
  the state-mutating profiles-dir switch stays last). The Emacs suite
  mirrors it with `tests/helpers.el` under a single `run.el`.
- Determinism rules (binding, mirroring specs/rust-port/05 §7.1):
  - the suites run on a per-run COPY of `testdata/jaffle_shop_duckdb`
    inside a sandboxed `HOME`/`XDG_*` — the shared fixture is never
    opened in place, mutated, or `target/`-wiped;
  - readiness is the `dbt.status` refresh barrier
    (`refresh.count >= n`), never a lens/modelGraph proxy;
  - every wait is labeled and FAILS at its deadline (`H.wait` /
    `cogitator-test-wait`) instead of surfacing later as a stale
    assertion;
  - the quickfix step rides Neovim's built-in pull diagnostics
    (`vim.diagnostic` + `user_data.lsp`): Neovim ≥ 0.12 cancels
    concurrent manual `textDocument/diagnostic` pulls on buffer
    changes, and a cancelled `buf_request_sync` only times out;
  - the server binary resolves via `COGITATOR_BIN`, then
    `CARGO_TARGET_DIR`, with `.exe` on Windows.
- Coverage (superset of the original five): attach + barrier, hover,
  `:DbtSchema`, lenses (`… downstream`, `dbt run`), rename →
  `RenameFile`, commands, code actions + quickfix, formatting, both
  browsers, the DAG, profiles.
- VS Code: `editors/vscode/src/test/` is the extension-host suite on
  the same contract — `runTest.ts` (@vscode/test-electron ≥ 3.1;
  downloads a cached VS Code under `.vscode-test/`) copies the fixture,
  points `cogitator.serverPath` at the cargo-built binary via workspace
  settings, and `suite/index.ts` (no mocha) prints the same `ok`/`FAIL`
  steps: activation, client Running, the dbt.status barrier,
  hover/definition through VS Code's provider pipeline, the dbt.*
  command surface. `activate()` returns `{ client() }` so the suite
  (and other extensions) can reach the live LanguageClient.
- One code-executed entry point: `just system-test` runs
  `crates/cogitator/tests/system_editors.rs` (`#[ignore]`d otherwise),
  which drives all three suites with `COGITATOR_BIN` set to the exact
  cargo-built binary and parses their `ok`/`FAIL` lines; a missing
  editor SKIPs locally and FAILS in CI (`COGITATOR_REQUIRE_EDITORS=1`).
  Dev loops: `just nvim-test` / `just emacs-test` / `just vscode-test`.
- CI: the `editors` job in ci.yml (ubuntu, neovim stable + emacs-nox +
  xvfb for the extension host, path-filtered on `editors/**` and the
  Rust side) runs exactly that under `xvfb-run`.

---

## 9. Non-goals (v1)

- No `$/progress` UI (server does not send it); dbt run feedback stays
  showMessage-based.
- No multi-root workspaces (server reads `workspaceFolders[0]` only).
- No file watcher: the server refreshes on `didSave`/`didOpen`, and
  `workspace/didChangeWatchedFiles` is accepted but never registered for.
- No install/download of the server binary — health check points at the
  release assets (`cogitator-{linux-amd64,darwin-amd64,darwin-arm64,windows-amd64.exe}`)
  and `cargo build`.

## 10. Neovim 0.10 fallback

Ship a one-paragraph recipe in `doc/cogitator.txt` using `nvim-lspconfig`'s
custom-config API with the same `cmd`/`root_dir`/`init_options`/capabilities
tables; the plugin's own modules must not hard-require 0.11 APIs outside the
`vim.lsp.config` call (guard with `vim.fn.has("nvim-0.11")`).
