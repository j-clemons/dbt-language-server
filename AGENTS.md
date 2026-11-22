# Agent Guidelines for cogitator (dbt language server)

Rust implementation of the dbt language server — a complete rewrite of
j-clemons/dbt-language-server (the Go original lives in git history up to
commit `724b0f2` and is fully specified in `specs/rust-port/`). First-party
editor integrations live under `editors/`.

## Build/Test Commands
- **justfile**: `just --list` — `build` / `test` / `fmt` / `fmt-check` /
  `lint` / `lint-shell` (shellcheck; testdata fixtures excluded) /
  `check` (the CI set) / `system-test` / `release` / `duckdb-check` /
  `gen-schemas(-check)` / `zigbuild` / `nvim-test` / `emacs-test` /
  `zed-build` / `changelog` / `hooks` / `oracle` (= `oracle-duckdb` +
  `oracle-datafusion`, the differential corpus; never in the default build)
- **Build**: `cargo build --workspace`
- **Test all**: `just test` = `cargo test --workspace --features
  cogitator/e2e` (the `e2e` feature builds the fake-dbt test bin; a bare
  `cargo test --workspace` skips the job e2e tests)
- **Test one crate**: `cargo test -p cogitator-parser`
- **Test one case**: `cargo test -p cogitator-analysis ref_source_var_refined`
- **Lint/format**: `cargo clippy --workspace --all-targets --features
  cogitator/e2e` (zero warnings enforced), `cargo fmt --all`
- **e2e harness** (crates/cogitator/tests/harness/, binding rules in
  specs/rust-port/05 §7.1): deadline-bounded waits only
  (`COGITATOR_E2E_DEADLINE_MS` override), per-test fixture copies via
  `fixture::jaffle()`, `wait_refresh(n)` (dbt.status refresh.count) as
  the refresh barrier, fake dbt via `harness::fake::install`
- **Editor suites** (self-sandboxing, per-run fixture COPIES):
  `just system-test` (the Rust orchestrator driving nvim + emacs +
  the VS Code extension host; what CI runs), `just nvim-test`
  (Neovim ≥ 0.11 headless), `just emacs-test` (Emacs ≥ 29 batch/eglot),
  `just vscode-test` (extension host via @vscode/test-electron, VS Code
  cached under editors/vscode/.vscode-test), `just zed-build`
  (wasm32-wasip1 compile check)
- **DuckDB feature** (bundled C++ build, slow): `cargo check -p cogitator --features duckdb-parser`
- **Release cross-builds**: cargo-zigbuild via the Zig toolchain
  (specs/rust-port/07 §5; .github/workflows/release.yml — linux-amd64,
  darwin-amd64/arm64, windows-amd64.exe); tests CI in ci.yml runs on
  Linux, Windows and macOS

## Tooling and Conventions
- **Nix dev environment**: the toolchain is declared once in
  `nix/tooling.nix` and consumed by both `shell.nix` (nix-shell/direnv)
  and `devenv.nix` (devenv; Rust via its languages.rust module). New
  tools go into `nix/tooling.nix`, nowhere else; CI's `nix-shell` job
  proves the shell assembles.
- **Workflow**: trunk-based with typed branches (`feat/…`, `fix/…`),
  squash-merged PRs, defined in CONTRIBUTING.md. Main is protected on
  GitHub (PRs-only, squash merge, no direct pushes); releases via
  `just release-prep <version>` (guards, version bump, changelog
  section, release commit on a `release/v…` branch → PR) and, after
  the squash-merge, `just release-tag <version>` on main — the tag
  push drives release.yml.
- **Commits are Conventional Commits** (`feat(scope): …`, `fix: …`,
  `docs(specs): …`); CHANGELOG.md is generated with git-cliff
  (`just changelog` — ranged from the fork point `724b0f2`; the upstream
  v0.1.x–v0.4.x tags are ignored via cliff.toml, and the first release of
  this fork must be tagged **v0.5.0 or later**).
- **Git hooks** run through prek (`prek install`; config in
  `.pre-commit-config.yaml`: fmt check, clippy -D warnings, dbt-schema
  freshness, shellcheck).
- **CI builds are path-gated**: the three-OS cargo matrix and the lint job
  run only when the Rust/C side changed (`crates/**`, Cargo files,
  schemas, testdata, .cargo, justfile, ci.yml — via dorny/paths-filter);
  shell changes run a shellcheck job instead.
- **Installers**: `install.sh` (Linux/macOS) and `install.ps1` (Windows),
  both pointing at THIS repository's releases with
  `COGITATOR_REPO/VERSION/BIN_DIR` overrides — never at the upstream Go
  project.
- **GitHub issue forms** live in `.github/ISSUE_TEMPLATE/`.
- **Dependabot** (`.github/dependabot.yml`) opens weekly PRs for crate
  updates (workspace + editors/zed, minor/patch grouped) and GitHub
  Actions, with conventional `chore(deps)` commit prefixes.

## Code Style (binding rules in specs/rust-port/05 §2–§3, 08 §1)
- **Functional core, imperative shell**: extraction/feature logic is pure
  functions over values; filesystem, process and LSP I/O stay in thin shell
  modules (`cache::FileCache` at the scan boundary, the `server/` tasks).
- **Errors**: one `Error` enum (`cogitator-error`) with `thiserror`; `anyhow`
  only in `main()`/task top-levels. User-facing failures carry a Reason and
  a Solution. Degradation is explicit at the feature layer, never silent.
- **Zero-copy**: tokens are byte spans into `Arc<str>` document text; inbound
  params borrow via `Cow`; a `clone()`/`to_string()` on a hot path must name
  the owner boundary it feeds (08 §1.5).
- **No hand-written lexers**: minijinja machinery + sqlparser only (05 §4).
- **Files stay small**: ≤ ~300 lines per module; split by concern.
- **Logging**: `tracing` spans/events only; nothing but LSP frames on stdout.
- **Windows rules**: never hand-strip `file://` — URIs go through
  `cogitator_lsp::uri_to_fs_path` / `fs_path_to_uri` (drive-letter aware);
  home lookup via `cogitator_analysis::util::home_dir()`
  (`$HOME` → `%USERPROFILE%`); compare paths as `Path` components, not
  strings.

## Server surface beyond stock LSP
- Custom commands (advertised in `cogitator-lsp/src/types/init.rs`,
  documented in `specs/neovim-plugin/00-plugin.md` §1.6): dbt.goToSchema,
  dbt.columnLineage, dbt.openSchema, dbt.showLineage, dbt.run/test/compile,
  dbt.modelGraph, dbt.profiles, dbt.setTarget, dbt.extractModelYaml,
  dbt.modelTests, dbt.goToTests, dbt.status, dbt.projectGraph,
  dbt.modelInfo (neighbors + versions + tests + compiledUri for the
  Neovim-only :DbtBrowse), dbt.relation (database.schema.identifier of
  the source/ref/model under the cursor, dbt defaults only),
  dbt.compiledSql (compiled SQL content, 2 MiB cap), dbt.parse
  (project-wide, refreshes the context on success), dbt.openModel
  (showDocument navigation for the YAML-side code actions),
  dbt.setProfilesDir, dbt.cache, dbt.reloadConfig, dbt.sourceFreshness,
  dbt.cancel + dbt.runSelection (job tokens), dbt.deadModels,
  dbt.impact, dbt.expandJinja, dbt.doctor, dbt.queryPlan (IR levels,
  findings and rule trace for one model; `features/plan/`). Every
  command takes one argument object (`{uri, position?, …}`).
- `textDocument/codeAction` serves the executable lens set as
  command-only actions (shared truth: `lenses::model_commands`); the
  completion capability advertises `triggerCharacters`.
- `workspace/symbol` understands `tag:x` and `selector:x` query prefixes.
- `$/progress` reports context refreshes and dbt runs when the client
  advertises window.workDoneProgress.
- **Profiles auth manager**: `profiles::inventory` (cogitator-analysis)
  classifies/validates Snowflake auth per target, redacted; read scope =
  project profile + `--profiles-allow`; active target via `--target` /
  `dbt.setTarget` flows into dialect resolution and dbt `--target`.

## Environment Variables / Flags
- **DBT_PROFILES_DIR**: profiles.yml directory. Full resolution order
  (`util::profiles_candidates`): `--profiles-dir` /
  `initializationOptions.profilesDir` override → `$DBT_PROFILES_DIR` →
  `<root>/profiles.yml` → `<root>/.dbt/profiles.yml` →
  `~/.dbt/profiles.yml`; the first file whose profile resolves a target wins.
- CLI: `--profiles-dir`, `--target`, `--profiles-allow`, `--fusion [path]`,
  `--no-tree-sitter`, `--debug`, `--otlp [endpoint]`, `--config [path]`.
- **Hot config file**: `<root>/cogitator.yml` (or `.cogitator.yml`) —
  `profiles.{dir,profile,target,allow}`, `limits.compiled_sql_mib`,
  `refresh.debounce_ms`, `perf.{profile,catalog_stats}` and `perf_rules`
  (rule id → severity or off; profile default `off`, specs/ir-pipeline/08)
  hot-reload (2 s stamp poll + didSave fast-path +
  `dbt.reloadConfig`); changed-keys-only, removed keys revert to launch
  values; `startup.{debug,otlp,tree_sitter,fusion}` only via `--config`
  at launch. Precedence: defaults < file < CLI < initializationOptions.
  Core in `server/config_file.rs` (pure), watcher in
  `server/config_watcher.rs`; `dbt.status` reports path/loaded/error.

## Project Structure (specs/rust-port/09 §1)
- `crates/cogitator-error` — error type
- `crates/cogitator-lsp` — protocol types + Content-Length framing + URI helpers
- `crates/cogitator-dialects` — dialect + bundled SQL function docs
- `crates/cogitator-schemas` — typify-generated dbt schema types (`schemas/dbt/`) + saphyr position side-tables
- `crates/cogitator-parser` — minijinja+sqlparser pipeline, refinement, TokenIndex, jinja_scan
- `crates/cogitator-analysis` — context builders, feature functions, petgraph project graph, profiles auth manager, lineage
- `crates/cogitator-duckdb` — Jinja masking + feature-gated DuckDB 2.0 parse diagnostics
- `crates/cogitator-treesitter` — tree-sitter document backend (vendored grammar in `third_party/`)
- `crates/cogitator-ir` — IR arenas, HIR/MIR/LIR types, verifiers, printers, `testkit` feature (serde and petgraph only)
- `crates/cogitator-sql-front` — HIR construction: masking, binder (the only IR crate seeing sqlparser/`minijinja::machinery`)
- `crates/cogitator-lower` — HIR→MIR, MIR→LIR, dialect backends
- `crates/cogitator-rules` — pass manager, trace, rule registry, rules
- `crates/cogitator-oracle` — differential tests against DuckDB (`just oracle`; feature-gated, never in the default build); `oracles/datafusion` is its separate DataFusion workspace
- `crates/cogitator` — the binary: CLI, tokio tasks, dispatcher, fusion, observability
- `editors/nvim` — first-party Neovim plugin (commands, graph browser, the
  Neovim-only :DbtDag project view, headless test suite)
- `editors/emacs` — eglot + lsp-mode integration (batch test suite)
- `editors/zed` — Zed extension (Rust→WASM)
- `schemas/dbt/` — vendored dbt-jsonschema (pristine + `processed/` for typify; regenerate with `preprocess.py`)
- `specs/rust-port/` — the binding implementation specs (00–09);
  `specs/neovim-plugin/` — the editor-integration spec;
  `specs/ir-pipeline/` — the HIR/MIR/LIR pipeline, rules and oracles (00–09)
- `testdata/` — shared fixtures (jaffle_shop_duckdb)
