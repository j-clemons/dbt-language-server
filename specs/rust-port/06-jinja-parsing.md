# 06 — Jinja Parsing: minijinja (now) & tree-sitter via C FFI (planned)

Binding decisions, same status as 05: where this file conflicts with 00–05, this
file wins. Two decisions are covered:

1. **minijinja** parses all dbt macro / Jinja code read from disk during project
   scans — it replaces the regex-based extraction of 03 §5 (macros) and 03 §6.4
   (`{% docs %}` blocks).
2. **tree-sitter**, integrated through its C FFI, is the planned replacement for
   the live-document minijinja+sqlparser pipeline (05 §4). It is specced here so
   phase 1 is built with the right seam, but it is **not required for the parity
   milestone**.

## 1. The two-sided parsing boundary

| Side | Input | Parser | Why |
|---|---|---|---|
| **Project scan** (context refresh, 03 §1.2) | files on disk: macro `.sql`, docs `.md` (this file §2); model `.sql` for graph edges goes through the 05 §4 pipeline | `minijinja` machinery | files are usually well-formed; we want real Jinja structure (signatures, args, block bodies), not regex approximations |
| **Live documents** (open editor buffers) | mid-edit, routinely malformed text | phase 1: `minijinja` tokenize + `sqlparser` pipeline (05 §4) — no hand-rolled lexer; phase 2: tree-sitter (this file §3) | both layers come from the provided crates; must stay fast on every keystroke |

Live-buffer error rule: minijinja's lexer errors on malformed input instead of
recovering, so the live pipeline keeps each document's **last good
`ParsedDocument`** and serves it when tokenization fails mid-edit (05 §4 step 1).
Tree-sitter later removes that staleness with true error tolerance. The scan side
may assume mostly-well-formed files and degrades per §2.4 when not.

## 2. minijinja on the project-scan side

### 2.1 Dependency

```toml
minijinja = { version = "3", features = ["unstable_machinery"] }
```

`unstable_machinery` exposes `minijinja::machinery`: `tokenize` (spanned token
stream) and `parse` (AST). The module is semver-exempt — pin the exact minijinja
version in `Cargo.lock` and wrap all machinery access in one module
(`src/analysis/parser/jinja_scan.rs`) so API churn is contained. Exact function
signatures are pinned at implementation time.

### 2.2 Macro extraction (replaces 03 §5's regex)

Input: raw text of one macro `.sql` file. Output: `Vec<Macro>` — same struct,
same semantics as 03 §5 (name, project_name, description = full signature text,
uri, range of the signature).

Algorithm (lexer-level, the baseline):

1. `machinery::tokenize(text, ...)` → spanned tokens.
2. Scan for the sequence: `BlockStart` (`{%` / `{%-`), `Ident("macro")`,
   `Ident(<name>)`, `ParenOpen`, … balanced … `ParenClose`, `BlockEnd`.
3. `name` = the ident after `macro`. Signature span = from `<name>` through the
   matching `ParenClose` — byte span → `Range` via the spans minijinja reports
   (minijinja spans are line/col-based and 1-based; convert to the 0-based
   positions of 01 §2.2; keep 03 §7.5 `get_line_and_column` for any byte-offset
   conversions).
4. `description` = the source slice of the signature span (identical content to
   what the Go regex captured).

Why lexer-level is the baseline: dbt extends Jinja with tags minijinja's *parser*
does not know (`{% docs %}`, `{% snapshot %}`, `{% materialization %}`,
`{% endmacro %}` bodies containing them) — `machinery::parse` fails on those files,
but `machinery::tokenize` does not care about tag names.

Enrichment (best-effort): when `machinery::parse` succeeds on the file, walk the
AST's macro nodes to capture structured parameters (names + default expressions).
Store them as `Vec<MacroParam { name, default: Option<String> }>` on `Macro`
(new optional field, empty when only lexing succeeded). First consumer: macro
completion `insertText` can become a snippet with argument placeholders — a
post-parity improvement; completion output stays as 03 §9.4 until then.

### 2.3 `{% docs %}` blocks (replaces 03 §6.4's regex)

Same tokenize pass over each docs file: find `BlockStart, Ident("docs"),
Ident(<name>), BlockEnd`, then capture everything up to the matching
`BlockStart, Ident("enddocs"), BlockEnd` as raw source text; `content` =
trimmed slice between the two blocks. Multiple blocks per file now extract
correctly — this fixes **Q2** (broken char class) and the greedy-`(.*)`
cross-block capture quirk of 03 §6.4 by construction. Name matching accepts any
Jinja identifier (supersedes the `[a-zA-z]+` class). The extractor (and the
regex fallback) records the name identifier's 0-based (line, column) on the
block record (`RawDocsBlock.start`) — definition jumps land on the block
header. Adding the field bumped the persisted scan-cache format to 2, so
position-less payloads cold-start instead of serving (0,0).

`replace_description_docs_blocks` (03 §6.4, the `{{ doc("name") }}` substitution
inside YAML descriptions) also moves off its regex: tokenize the description
string, find `VariableStart, Ident("doc"), ParenOpen, Str(<name>), ParenClose,
VariableEnd`, splice `docs_map[name].content` over the construct's span
(right-to-left so earlier spans stay valid — fixes the stale-offset quirk).
If tokenization of a description fails, return it unchanged.

### 2.4 Error handling

- `tokenize` error on a macro/docs file → log (`Error::Yaml`-style context: add an
  `Error::Jinja { path, message }` variant to 05 §2's enum) and fall back to the
  03 §5 / §6.4 **regexes for that file only**. The regexes are thereby demoted to
  documented fallback + behavioral reference; keep their unit tests alive against
  the fallback path.
- A macro header sequence that never closes (`ParenClose`/`BlockEnd` missing before
  EOF) → skip that candidate, continue scanning.
- Everything stays a pure function per 05 §3: `fn extract_macros(text, project,
  path) -> Vec<Macro>`, `fn extract_docs_blocks(text) -> Vec<Docs>` — fallback
  selection inside, I/O outside.

### 2.5 Testing

- Port the existing Go fixtures (`parse_macros_test.go`, `parse_markdown_test.go`)
  as `rstest` cases: identical expected `Macro`/`Docs` values (insta snapshots for
  the full vectors, per 05 §7).
- Add cases the regexes got wrong, locking in the fixes: two `{% docs %}` blocks in
  one file; docs names with digits/underscores; a description with two
  `{{ doc(...) }}` references; whitespace-control variants (`{%- macro ... -%}`);
  a macro signature spanning multiple lines with default args.
- One malformed-file case per extractor proving the regex fallback engages.

## 3. tree-sitter via C FFI (planned phase — live-document side)

Not part of the parity milestone. Phase 1 must only honor §3.5 (the seam).

### 3.1 What it replaces and why

The minijinja+sqlparser pipeline with per-change full retokenize (05 §4, 05 §6.2)
is replaced for open documents by tree-sitter, which provides exactly the two
properties the live side lacks:

- **Error tolerance by design**: broken mid-edit buffers still produce a tree with
  ERROR nodes; features keep working around them — no more serving the last good
  parse while the buffer is broken.
- **Incremental reparsing**: on didChange, apply `tree.edit(InputEdit)` per content
  change and reparse — cost proportional to the edit, not the file. This retires
  the full-document retokenization budget line in 05 §6.1.

### 3.2 FFI & build mechanics

- Crate `tree-sitter` — Rust bindings over the C runtime (`libtree-sitter`),
  already FFI-based; the runtime's C sources build via the crate.
- Grammars are **vendored C sources** in-repo (e.g. `third_party/tree-sitter-jinja2/`):
  `src/parser.c` (+ `scanner.c` where the grammar has one), compiled in `build.rs`
  with the `cc` crate — which, like all C/C++ in this project, runs through the
  Zig toolchain (`CC="zig cc"`, 07 §5) — and linked through the C ABI.
  **Amendment:** a grammar whose generated `parser.c` is disproportionately large
  (tree-sitter-sql's is ~40 MB) comes from crates.io instead of `third_party/`
  (the `tree-sitter-sequel` crate — same C source, built by cargo through the
  same `cc` path); vendoring stays the rule for small grammars. Each vendored
  grammar's `third_party/<name>/README` records provenance AND its known
  limitations (tree-sitter-jinja2 is a deliberate dbt-extractor subset —
  acceptable because the tree only segments; minijinja tokenizes every region).

```rust
unsafe extern "C" { fn tree_sitter_jinja2() -> *const (); }
let language = unsafe { tree_sitter::Language::from_raw(tree_sitter_jinja2() as *const _) };
```

  (Exact constructor per the pinned tree-sitter version — newer releases wrap this
  as `LanguageFn`; the binding site lives in one module,
  `src/analysis/parser/ts/ffi.rs`, and is the only `unsafe` in the project.)
- Grammar choice: `tree-sitter-jinja2` as the host language with SQL as an
  **injected language** (`tree-sitter-sql`) — mirrors how editors highlight dbt
  models. Evaluate a dbt-specific grammar at integration time if one has matured;
  the vendoring/build path is identical.
- Vendored grammar versions are pinned by commit in `third_party/README`.

**Amendment (tree-sitter-dbt):** the dbt-specific grammar this section left
open now exists, written for this project and moved to its own repository,
[sirh3e/tree-sitter-dbt](https://github.com/sirh3e/tree-sitter-dbt), so other
tools can use it without this server. It replaces the vendored
tree-sitter-jinja2: full Jinja (every statement, filters, tests, dotted calls,
whitespace control, `raw`) and dbt's `materialization`, `test`, `snapshot` and
`docs` blocks, with the SQL between tags as `content` nodes. It comes in as a
git dependency pinned by `rev`, built by its own `build.rs` with `cc`, so
`third_party/`, `ffi.rs` and the crate's `build.rs` are gone and the project
has no `unsafe` left for grammars. The workarounds for the old grammar's gaps
(blanking `##}` comments and `{% raw %}` bodies before parsing) went with it,
so every document now keeps its tree for the incremental path. The capture
table below names the old grammar's nodes; `regions.rs` walks the new tree
(each tag's `{%`…`%}` run and each `output` is a region, `content` is SQL).

### 3.3 Mapping trees onto the existing token model

`.scm` query files (compiled once with `tree_sitter::Query`, cached) capture the
constructs the features need; captures are converted into the refined
`TokenKind`s of 05 §4 so **02's behavioral contract, the `TokenIndex`, and every
feature algorithm in 03 §9 stay byte-for-byte unchanged**:

| capture (jinja2 grammar) | refinement |
|---|---|
| call with function ident `ref`, first string arg | arg → `Ref` |
| call `source`, first/second string args | `SourceName` / `SourceTable` |
| call `var`, first string arg | `Var` |
| expression-statement call ident (not ref/source/var/config) | `Macro` (+ preceding `ident . ` → `Package`) |
| SQL injection ranges | collected across the document and fed to `sqlparser` exactly as 05 §4 step 2 (CTE tracking unchanged) |

ERROR-node regions contribute no refined tokens (hover/completion simply find
nothing there — a milder degradation than phase 1's whole-document last-good
fallback).

### 3.4 Incremental pipeline

didChange (dispatcher, 05 §6.2): apply the text splice (03 §2) **and** the
equivalent `InputEdit` (byte offsets + Points derived from the same change
ranges), reparse with the old tree, re-run queries only over changed ranges
(`QueryCursor::set_byte_range` over the edited region ∪ affected injection
ranges), and patch the `TokenIndex` lines in that range. Positions remain bytes
internally; the byte↔UTF-16 conversion single-point (05 §4) applies unchanged.

### 3.5 Phase-1 obligation: the `DocumentParser` seam

So tree-sitter can land without touching features, phase 1 ships document
tokenization behind a trait:

```rust
// src/analysis/parser/mod.rs
pub trait DocumentParser {
    fn parse_full(&mut self, text: &str, dialect: &Dialect) -> ParsedDocument;
    fn parse_incremental(&mut self, prev: ParsedDocument, text: &str,
                         changes: &[ContentChange], dialect: &Dialect) -> ParsedDocument;
}
// ParsedDocument is defined in 08 §1.1: { text: Arc<str>, tokens: TokenIndex,
//                                         def_tokens: HashMap<Box<str>, Token> }
```

Phase 1 implementation: `JinjaSqlParser` (the 05 §4 minijinja+sqlparser pipeline;
`parse_incremental` = full retokenize of the new text, falling back to the
previous `ParsedDocument` on tokenize error). Phase 2 adds `TreeSitterParser`
holding the `tree_sitter::Parser` + previous `Tree` per document. Selection:
tree-sitter becomes the default once it passes the full 02-derived behavior
suite; keep `JinjaSqlParser` compiled as the fallback (and for
`--no-tree-sitter` style debugging) for at least one release.

### 3.6 Later features unlocked (partially delivered)

Semantic tokens, folding ranges, document symbols, and selection ranges all fall
out of the same trees/queries. **Delivered:** the tree-sitter backend parses the
Jinja-masked document once with the SQL grammar (`tree-sitter-sequel`, crate-
sourced per the §3.2 amendment) and attaches plain `SqlStructure` node ranges to
`ParsedDocument`; `folding_ranges` unions them with the token folds and
`selection_ranges` inserts the containment chain between token and line. SQL
tokens stay on sqlparser (05 §4). The phase-1 pipeline attaches nothing and the
token-based fallbacks remain. Semantic tokens and document symbols from trees
stay future additions.

## 4. Layout & manifest deltas (amends 05 §1/§8)

```toml
# [dependencies] additions
minijinja = { version = "3", features = ["unstable_machinery"] }
# phase 2:
# tree-sitter = "…"        # plus cc (build-dependency) and vendored grammars
```

```
src/analysis/parser/jinja_scan.rs   # §2: minijinja-based macro/docs extraction (scan side)
src/analysis/parser/mod.rs          # §3.5: DocumentParser trait + ParsedDocument
src/analysis/parser/ts/             # phase 2: ffi.rs, parser.rs, queries/*.scm
# (third_party/ and build.rs: gone, the grammar is the tree-sitter-dbt crate — §3.2 amendment)
```

## Addendum: §3.4 incremental reparse (resolved)

The `DocumentParser` trait gains a default-implemented
`parse_incremental(uri, text, dialect, edits)` + `forget(uri)` seam;
`Documents::change` folds each ranged didChange into an `AppliedEdit`
(byte offsets + (row, byte-column) points — exactly tree-sitter's
InputEdit shape). The tree-sitter backend keeps the last `Tree` per
open uri and feeds it back through `tree.edit()` for documents past a
32 KiB gate; smaller documents, full-text changes and unresolvable
positions take the full-parse path unchanged. Equivalence is pinned by
a scripted-edit test: the incremental token stream must equal a fresh
full parse of the same final text.
