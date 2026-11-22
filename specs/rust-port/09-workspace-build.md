# 09 — Cargo Workspace Split, mold & sccache

Binding decisions, same status as 05–08; **this file supersedes the
"single binary crate" decision** in 00 §"Proposed module layout" and reframes
05 §1's manifest. Three decisions:

1. The code is a **Cargo workspace of multiple crates**, cut so compile times
   stay small (parallel crate builds, small rebuild units, heavy leaves cached).
   (00's section is now titled "Proposed module layout" accordingly.)
2. **mold is the linker on Linux** (dev/test builds).
3. **sccache** accelerates builds (Rust via `RUSTC_WRAPPER`, C/C++ via the
   `cc`/zig path from 07 §5).

## 1. Workspace layout

```
dbt-language-server/
├── Cargo.toml                 # [workspace]: members, workspace.{package,dependencies,lints}, profiles
├── .cargo/config.toml         # mold linker config (Linux targets), §3
├── schemas/dbt/               # vendored dbt-jsonschema (08 §2.1)
├── third_party/               # phase 2: vendored tree-sitter grammars (06 §3.2)
├── testdata/                  # shared fixtures (Go repo copy)
└── crates/
    │   (naming — owner's choice: the binary carries the Warhammer 40k brand
    │    `cogitator`; every library crate is `cogitator-<technical role>`)
    ├── cogitator-error/       # Error enum + Result alias (05 §2)
    ├── cogitator-lsp/         # protocol structs + rpc framing (01; Go rpc/ + lsp/)
    ├── cogitator-dialects/    # Dialect enum + function-docs JSON (04 §3; include_str! data)
    ├── cogitator-schemas/     # typify-generated dbt types + saphyr position side-table (08 §2)
    ├── cogitator-parser/      # DocumentParser seam, minijinja+sqlparser pipeline,
    │                          #   refinement, TokenIndex, jinja_scan (05 §4, 06 §2/§3.5)
    ├── cogitator-analysis/    # State, DbtContext, detail-map builders, features,
    │                          #   petgraph ProjectGraph, util (03, 05 §5)
    ├── cogitator-duckdb/      # DuckDB 2.0 parse diagnostics (07) — OWNS the bundled C++ build
    ├── cogitator-treesitter/  # tree-sitter backend (06 §3) — OWNS the grammar C builds
    ├── cogitator-ir/          # IR arenas, HIR/MIR/LIR types, verifiers, printers (specs/ir-pipeline/)
    ├── cogitator-sql-front/   # HIR construction: masking, binder (the only IR crate seeing sqlparser/minijinja::machinery)
    ├── cogitator-lower/       # HIR→MIR, MIR→LIR, dialect backends
    ├── cogitator-rules/       # pass manager, trace, rule registry, rules
    └── cogitator/             # the binary: CLI, tokio task wiring, dispatcher,
                               #   fusion orchestration, observability (01 §3, 04 §1–§2, 05 §6)
```

Dependency edges (a crate may depend only on crates above it in this list):

```
error ◄── lsp ◄── dialects ◄── parser ◄── analysis ◄── dbt-language-server
            ▲                     ▲            ▲              ▲
            └── schemas ──────────┼────────────┘              │
                                  └── dbt-ls-duckdb ──────────┘   (optional)
                                      dbt-ls-treesitter ──────┘   (phase 2)

ir ◄── sql-front (+ parser, dialects, error, sqlparser, minijinja)
ir ◄── lower
ir ◄── rules                       analysis depends on all four
```

`cogitator-ir` depends only on serde and petgraph and must never depend on
`cogitator-error` (which pulls in sqlparser). See `specs/ir-pipeline/00`.

Rationale per cut (the compile-time argument the workspace exists for):

- **`dbt-ls-duckdb`** and **`dbt-ls-treesitter`** isolate the only C/C++ builds
  (DuckDB amalgamation, grammar sources). Touching any Rust code never rebuilds
  them; they rebuild only when their own pins change, and sccache serves them
  from cache across clean checkouts.
- **`dbt-ls-schemas`** isolates the large generated code (08 §2.2) — slow to
  compile, changes only on schema updates.
- **`dbt-ls-dialects`** isolates ~1 MB of static docs data (rarely changes).
- **`dbt-ls-parser` / `dbt-ls-analysis`** split the hot development surface
  from the crates above, so the edit-compile loop recompiles one small crate +
  the binary.
- **`dbt-ls-error` / `dbt-ls-lsp`** are tiny, near-depless leaves everything
  shares without pulling anything heavy.

Rules:

- Versions are declared **once** in `[workspace.dependencies]`; member crates
  use `dep = { workspace = true }`. `[workspace.package]` holds version,
  edition, license; members inherit.
- Cargo **features stay on the crate that owns them** and are forwarded by the
  binary: `dbt-language-server/duckdb-parser → dbt-ls-duckdb` (07 §2),
  `tokio-console` on the binary (05 §6.3). CI builds the workspace both with
  and without `duckdb-parser`.
- Profiles live only in the workspace root: `[profile.release]` keeps 05 §6.4's
  `lto = "thin"`, `codegen-units = 1` (thin LTO crosses crate boundaries, so
  the split costs no release performance).
- Crate-internal module layouts follow the trees in 00/05/06 — those trees are
  the *logical* map; this file says which crate each part lives in.
- Tests live with their crate; the e2e test (05 §7) and parity harness live in
  the binary crate's `tests/`. Fixture paths resolve via a tiny
  `dbt-ls-analysis` test-util that locates the workspace-root `testdata/`
  (e.g. `env!("CARGO_MANIFEST_DIR")/../../testdata`).

## 2. What this changes in earlier files

- 00 §"Proposed module layout": the original "single binary crate" idea is
  void; the tree remains valid as the logical module map, distributed over the
  crates above.
- 05 §1: the manifest's `[dependencies]` block now reads as the
  `[workspace.dependencies]` set; each member declares the subset it uses.
  `[features]` land per §1's rules here.
- 05 §8 / 06 §4 / 08 §3 path deltas: unchanged paths, now rooted inside their
  owning crate (e.g. `src/analysis/parser/jinja_scan.rs` →
  `crates/dbt-ls-parser/src/jinja_scan.rs`).

## 3. mold on Linux

Dev and test builds on Linux link with **mold**. Checked-in
`.cargo/config.toml`:

```toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]

[target.aarch64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

- Linux only: macOS keeps the default linker (mold's mach-o support is not the
  default path; do not add sold), Windows is out of the dev matrix.
- **Release cross-builds are unaffected:** `cargo-zigbuild` (07 §5) drives
  linking through Zig's toolchain; the mold config applies to native
  `cargo build/test/run` on Linux dev machines and CI test jobs.
- mold is a documented dev prerequisite next to Zig and cargo-typify (05 §1);
  CI installs it explicitly. If a contributor lacks mold, the build fails with
  a clear linker error — the README names the escape hatch
  (`--config 'target.x86_64-unknown-linux-gnu.rustflags=[]'`) rather than us
  maintaining a silent fallback.

## 4. sccache

- **Rust:** `RUSTC_WRAPPER=sccache`, set in CI and recommended in the README
  for dev machines — **not** checked into `.cargo/config.toml`, so a machine
  without sccache still builds.
- **C/C++:** sccache also fronts the zig-driven C/C++ compiles (07 §5 already
  relies on this for the DuckDB amalgamation): `CC="sccache zig cc"`,
  `CXX="sccache zig c++"` in CI.
- **Incremental interplay (the one footgun):** sccache cannot cache
  incrementally-compiled crates. Therefore:
  - CI: `CARGO_INCREMENTAL=0` + sccache (shared cache across jobs/runners —
    GHA cache or S3 backend; `sccache --show-stats` printed per job).
  - Local dev: keep incremental compilation **on** (the inner loop relies on
    it plus mold); sccache still pays off for cold dependency builds and
    toolchain bumps. Both modes are correct; they just cache different layers.
- The workspace split is what makes the caches effective: the heavy leaves
  (duckdb, treesitter, schemas, dialects) hit sccache as whole-crate artifacts
  and stay warm for weeks.

## 5. CI build matrix (summary)

| Job | Setup |
|---|---|
| test (Linux) | mold + sccache, `CARGO_INCREMENTAL=0`, workspace `cargo test`, ±`duckdb-parser` |
| test (macOS) | sccache, default linker, workspace `cargo test` |
| lint | `cargo fmt --check`, `clippy --workspace --all-features` |
| schema-gen check | rerun `schemas/dbt/preprocess.py`; fail on `schemas/dbt/processed/` diffs (08 §2.2) |
| release | `cargo-zigbuild` matrix per 07 §5 (zig linker; mold not involved) |

## Addendum: §5 benches and fuzzing

- **Criterion benches** (`just bench`; `just bench-check` compiles
  them): `cogitator-parser/benches/parse.rs` (phase-1 pipeline over a
  realistic model, jinja block scans) and
  `cogitator-analysis/benches/lineage.rs` (context build cold vs
  scan-cache-warm, column lineage). criterion is a workspace dev
  dependency; CI never runs timings.
- **cargo-fuzz** (`just fuzz [target] [seconds]`; `just fuzz-check`
  compiles without nightly): `fuzz/` is its own excluded workspace so
  sanitizer flags never leak. Targets cover the three untrusted-input
  fronts: `parse_document` (every keystroke), `jinja_scan` (every
  macro/docs/snapshot file), `properties_yaml` (every schema file).
  Corpora and artifacts stay untracked.
