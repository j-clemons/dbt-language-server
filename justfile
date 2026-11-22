# cogitator — development recipes, grouped (`just --list` shows them all)

# default: show available recipes
default:
    @just --list

# ---------------------------------------------------------------- build

# build the whole workspace (debug)
[group('build')]
build:
    cargo build --workspace

# build the release binary (target/release/cogitator)
[group('build')]
release:
    cargo build --release -p cogitator

# cross-compile the release matrix via the Zig toolchain (07 §5)
[group('build')]
zigbuild target:
    cargo zigbuild --release --target {{target}} -p cogitator

# ----------------------------------------------------------------- test

# run the full test suite (the e2e feature builds the fake-dbt test
# bin; a plain `cargo test --workspace` would skip the job e2e tests)
[group('test')]
test:
    cargo test --workspace --features cogitator/e2e

# type-check the feature-gated DuckDB engine (bundled C++ build — slow cold)
[group('test')]
duckdb-check:
    cargo check -p cogitator --features duckdb-parser

# run the differential corpus against DuckDB (bundled C++ build, slow cold)
[group('test')]
oracle-duckdb:
    cargo test -p cogitator-oracle --features duckdb-oracle

# run the differential corpus against DataFusion (isolated workspace)
[group('test')]
oracle-datafusion:
    cargo test --manifest-path oracles/datafusion/Cargo.toml --locked

# run the differential corpus against both engines (specs/ir-pipeline/07)
[group('test')]
oracle: oracle-duckdb oracle-datafusion

# --------------------------------------------------------------- format

# format all crates in place
[group('format')]
fmt:
    cargo fmt --all
    cargo fmt --manifest-path oracles/datafusion/Cargo.toml

# verify formatting without writing
[group('format')]
fmt-check:
    cargo fmt --all --check
    cargo fmt --manifest-path oracles/datafusion/Cargo.toml --check

# Format the Neovim plugin's Lua (stylua from the nix shell)
[group('format')]
lua-fmt:
    stylua editors/nvim

# Check Lua formatting without writing
[group('format')]
lua-fmt-check:
    stylua --check editors/nvim

# ----------------------------------------------------------------- lint

# lint; warnings are errors (the workspace stays at zero)
[group('lint')]
lint:
    cargo clippy --workspace --all-targets --features cogitator/e2e -- -D warnings

# Lint our own shell scripts (the testdata fixture is third-party)
[group('lint')]
lint-shell:
    git ls-files '*.sh' '*.bash' | grep -v '^testdata/' | xargs -r shellcheck

# Lint the Neovim plugin's Lua (luacheck from the nix shell)
[group('lint')]
lua-lint:
    luacheck editors/nvim

# Byte-compile the Emacs integration with warnings as errors
[group('lint')]
elisp-lint:
    emacs -Q --batch --eval "(setq byte-compile-error-on-warn t)" -L editors/emacs -f batch-byte-compile editors/emacs/cogitator.el editors/emacs/cogitator-graph.el
    rm -f editors/emacs/*.elc

# ------------------------------------------------------------------- ci

# everything CI checks: formatting, lints, tests, schema freshness
[group('ci')]
check: fmt-check lint test gen-schemas-check

# -------------------------------------------------------------- schemas

# regenerate the preprocessed dbt schemas (specs/rust-port/08 §2.2)
[group('schemas')]
gen-schemas:
    for f in dbt_project dbt_yml_files packages selectors; do \
        python3 schemas/dbt/preprocess.py "schemas/dbt/$f-latest.json" "schemas/dbt/processed/$f.json"; \
    done
    for f in run-results-v6 sources-v3 catalog-v1; do \
        python3 schemas/dbt/preprocess.py --mode=artifact "schemas/dbt/artifacts/$f.json" "schemas/dbt/processed/artifacts/$f.json"; \
    done
    python3 schemas/dbt/preprocess.py --mode=manifest-projection "schemas/dbt/artifacts/manifest-v12.json" "schemas/dbt/processed/artifacts/manifest-v12-projection.json" 

# fail if the preprocessed schemas are stale (CI job, 09 §5)
[group('schemas')]
gen-schemas-check: gen-schemas
    git diff --exit-code -- schemas/dbt/processed

# regenerate the committed cogitator.yml JSON Schema (03 §10; a sync
# test fails the suite while it is stale)
[group('schemas')]
gen-config-schema:
    cargo run -q -p cogitator --bin cogitator -- schema > schemas/cogitator.schema.json

# ---------------------------------------------------------- perf & fuzz

# run the criterion benches (parser pipeline, context build, lineage)
[group('perf')]
bench:
    cargo bench --workspace

# CI-grade proof that every bench still compiles
[group('perf')]
bench-check:
    cargo bench --workspace --no-run

# run one libFuzzer target (needs nightly + `cargo install cargo-fuzz`)
[group('perf')]
fuzz target='parse_document' time='60':
    cargo +nightly fuzz run {{target}} --fuzz-dir fuzz -- -max_total_time={{time}}

# prove the fuzz package compiles without nightly or cargo-fuzz
[group('perf')]
fuzz-check:
    cargo check --manifest-path fuzz/Cargo.toml

# -------------------------------------------------------------- editors

# The whole system layer: both editor suites driven by the Rust
# orchestrator against the exact binary cargo builds (a missing editor
# prints SKIPPED; CI sets COGITATOR_REQUIRE_EDITORS=1 to forbid that)
[group('test')]
system-test:
    cargo test -p cogitator --features e2e --test system_editors -- --ignored --nocapture

# compile the VS Code extension (needs node/npm; thin stdio client)
[group('editors')]
vscode-build:
    cd editors/vscode && npm install --no-audit --no-fund && npm run compile

# Package the VS Code extension as an installable .vsix (what the
# release workflow uploads; LICENSE is copied in because vsce wants
# one inside the package directory)
[group('editors')]
vsix:
    cd editors/vscode && npm install --no-audit --no-fund && cp ../../LICENSE LICENSE && npx --yes @vscode/vsce package

# Extension-host tests in a downloaded VS Code (cached under
# editors/vscode/.vscode-test; needs node/npm and a debug server build)
[group('editors')]
vscode-test:
    cargo build -p cogitator
    cd editors/vscode && npm install --no-audit --no-fund && npm test

# Headless Neovim plugin tests (needs nvim >= 0.11 and a debug server build)
[group('editors')]
nvim-test:
    cargo build -p cogitator
    nvim --headless -l editors/nvim/tests/run.lua

# Batch Emacs integration tests (needs emacs >= 29 and a debug server build)
[group('editors')]
emacs-test:
    cargo build -p cogitator
    emacs -Q --batch -L editors/emacs -l editors/emacs/tests/run.el

# Compile the Zed extension to WebAssembly the way Zed does
[group('editors')]
zed-build:
    cd editors/zed && cargo build --target wasm32-wasip1

# -------------------------------------------------------------- release

# (range starts at the fork point: upstream Go commits and tags stay out)
# Regenerate CHANGELOG.md from the conventional commit history (git-cliff)
[group('release')]
changelog:
    git cliff 724b0f2..HEAD -o CHANGELOG.md

# Prepare a release on a release/v<version> branch: guards, bump, check,
# changelog, commit. Main is PRs-only, so the commit travels as a pull
# request; after the squash-merge, `just release-tag <version>` tags main.
[group('release')]
release-prep version:
    #!/usr/bin/env bash
    set -euo pipefail
    v="{{version}}"
    [ -z "$(git status --porcelain)" ] || { echo "error: working tree not clean" >&2; exit 1; }
    [ "$(git branch --show-current)" = "main" ] || { echo "error: start from main" >&2; exit 1; }
    git fetch origin main
    [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || { echo "error: main is not up to date with origin/main" >&2; exit 1; }
    if [ "$(printf '%s\n' 0.5.0 "$v" | sort -V | head -1)" != "0.5.0" ]; then
        echo "error: fork releases start at 0.5.0 (upstream owns v0.1.x-v0.4.x)" >&2; exit 1
    fi
    git switch -c "release/v$v"
    perl -pi -e 'if (!$done && s/^version = ".*"/version = "'"$v"'"/) { $done = 1 }' Cargo.toml
    perl -pi -e 'if (!$done && s/^  "version": ".*",/  "version": "'"$v"'",/) { $done = 1 }' editors/vscode/package.json
    cargo update --workspace --quiet
    just check
    git cliff 724b0f2..HEAD --tag "v$v" -o CHANGELOG.md
    git add Cargo.toml Cargo.lock CHANGELOG.md editors/vscode/package.json
    git commit -m "chore(release): v$v"
    echo "release/v$v ready — push it and open a PR titled 'chore(release): v$v';"
    echo "after the squash-merge, on updated main: just release-tag $v"

# Tag the squash-merged release commit on main and hand back the tag push
# (the tag push is what drives release.yml)
[group('release')]
release-tag version:
    #!/usr/bin/env bash
    set -euo pipefail
    v="{{version}}"
    [ -z "$(git status --porcelain)" ] || { echo "error: working tree not clean" >&2; exit 1; }
    [ "$(git branch --show-current)" = "main" ] || { echo "error: not on main" >&2; exit 1; }
    git fetch origin main
    [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || { echo "error: main is not up to date with origin/main" >&2; exit 1; }
    grep -q "^version = \"$v\"" Cargo.toml || { echo "error: Cargo.toml is not at $v — merge the release PR first" >&2; exit 1; }
    git tag -a "v$v" -m "cogitator v$v"
    echo "tagged v$v — review, then publish with: git push origin v$v"

# ---------------------------------------------------------- maintenance

# remove build artifacts
[group('maintenance')]
clean:
    cargo clean

# Install the git hooks via prek (the Rust pre-commit runner)
[group('maintenance')]
hooks:
    prek install
