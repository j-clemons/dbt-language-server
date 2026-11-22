#!/usr/bin/env python3
"""Preprocess vendored dbt schemas for cargo-typify 0.8 (08 §2.2, §4).

typify limitations worked around in EVERY mode:
  1. if/then/else is unsupported            -> stripped (validation-only).
  2. enum values differing only in case     -> panic; deduped (keep first).
  3. `+alias` and `alias` style properties sanitize to one Rust ident
     -> duplicate struct fields; the `+`-prefixed spelling is dropped
        (unknown keys are ignored at deserialization, so files using the
        dropped spelling still parse).
  4. oneOf/anyOf of [X, null] names the inner type after the outer def ->
     duplicate type definitions; the null branch is dropped and X merged into
     the parent node (optional fields are Option<T> anyway, so a YAML `null`
     still deserializes to None).
  5. schema `default` values -> typify 0.8 emits broken default-constructor
     code for several; dropped, the server applies its own documented
     defaults (specs/rust-port/03 §3.3).

--mode=artifact adds the LENIENCY rules for the official artifact schemas
(schemas.getdbt.com), which are strict in ways dbt itself never honors
across versions (08 §4):
  a. additionalProperties: false            -> removed (only the literal
     `false`; typed maps like catalog `nodes` keep theirs). Kills every
     deny_unknown_fields so unknown fields from other dbt versions pass.
  b. `required` lists                       -> removed entirely. Every field
     becomes Option + serde(default); the adapter re-imposes the one real
     requirement (unique_id) by DROPPING the row, never the file.
  c. anyOf/oneOf whose branches ALL resolve to string enums (inline, or a
     one-level $ref into $defs/definitions) -> plain {"type": "string"}.
     run-results `status` is three enum sets; a v5 or future status string
     must stay a String, not a rejected union variant.
  d. `format`                               -> removed. typify maps
     date-time/uuid to chrono/uuid foreign types; timestamps stay
     Option<String> in the stable API.
  e. `propertyNames`                        -> removed (env-style maps;
     validation-only, shaky typify support).
  f. a string-typed node carrying `const`   -> plain {"type": "string"}.
     Version pins must trace, never reject (03 §10).
  i. the yaml-mode [X, null] collapse (rule 4) is SKIPPED: typify
     renders kept null branches as Option<X>, and dbt writes literal
     nulls ("errors": null) that a collapsed non-Option field rejects.
  h. additionalProperties: {"type": "string"} -> true. env-style maps
     claim string values, but leniency means an odd value in a map the
     server never reads (env, adapter_response) must not kill the file;
     typed maps of real objects (catalog nodes) keep their schema.
  g. anyOf/oneOf whose branches are ALL objects -> ONE merged open
     object (union of the branches' properties; a key defined by more
     than one branch becomes {"type": "string"} when every definition
     is string/string-enum, else the first definition wins). With
     `required` stripped, typify's untagged unions are order-sensitive
     over open objects — serde would match the first branch for every
     row (sources.json results: RuntimeError | Output). One merged
     object reads every variant's fields leniently instead.
  j. the fields named in RELAX_TO_ANY    -> {} (any JSON value). The
     adapter reads them out of a serde_json::Value and maps an unexpected
     shape to None, so one odd stat cannot reject the whole artifact.

--mode=manifest-projection ignores the input's structure: it ASSERTS every
projected field name still occurs in the pristine 946 KB manifest schema
(an upstream rename fails regeneration loudly instead of drifting
silently), then emits the small projection below — the only part of the
manifest the server reads. typifying the whole schema would cost thousands
of types and minutes of macro expansion for two lookups. `relation_name`
and `config` are emitted as {} so an adapter-specific shape degrades one
field in the adapter rather than failing the manifest.

Usage: preprocess.py [--mode=yaml|artifact|manifest-projection] <in.json> <out.json>
"""

import json
import re
import sys

def ident(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", s.lower()).strip("_")


def is_null_branch(b) -> bool:
    return isinstance(b, dict) and (
        b.get("type") == "null" or ("const" in b and b["const"] is None)
    )


def resolve_ref(node, root):
    """One-level $ref into the same document's $defs/definitions."""
    if not (isinstance(node, dict) and isinstance(node.get("$ref"), str)):
        return node
    m = re.fullmatch(r"#/(\$defs|definitions)/([^/]+)", node["$ref"])
    if not m:
        return node
    return root.get(m.group(1), {}).get(m.group(2), node)


def is_string_enum(node, root) -> bool:
    node = resolve_ref(node, root)
    if not isinstance(node, dict):
        return False
    enum = node.get("enum")
    return isinstance(enum, list) and all(isinstance(x, str) for x in enum)


# rule (j): by object title. A listed field missing from its object fails
# regeneration, the same drift guard the manifest projection has.
RELAX_TO_ANY = {
    "CatalogTable": ["stats"],
    "CatalogMetadata": ["generated_at"],
    "RunResultOutput": ["execution_time"],
}


def patch_node(node: dict, keep_null_branches: bool = False) -> None:
    for k in ("if", "then", "else", "default"):
        node.pop(k, None)
    enum = node.get("enum")
    if isinstance(enum, list) and all(isinstance(x, str) for x in enum):
        seen, out = set(), []
        for v in enum:
            if v.lower() not in seen:
                seen.add(v.lower())
                out.append(v)
        node["enum"] = out
    props = node.get("properties")
    if isinstance(props, dict):
        by_ident = {}
        for k in list(props):
            i = ident(k)
            if i in by_ident:
                drop = k if k.startswith("+") else by_ident[i]
                keep = by_ident[i] if k.startswith("+") else k
                props.pop(drop, None)
                by_ident[i] = keep
            else:
                by_ident[i] = k
    if keep_null_branches:
        return
    for key in ("oneOf", "anyOf"):
        branches = node.get(key)
        if not isinstance(branches, list) or len(branches) != 2:
            continue
        non_null = [b for b in branches if not is_null_branch(b)]
        if len(non_null) == 1 and isinstance(non_null[0], dict):
            node.pop(key)
            for k, v in non_null[0].items():
                node.setdefault(k, v)


def is_object_branch(node, root) -> bool:
    node = resolve_ref(node, root)
    return isinstance(node, dict) and (
        node.get("type") == "object" or "properties" in node
    )


def stringish(schema, root) -> bool:
    schema = resolve_ref(schema, root)
    return isinstance(schema, dict) and (
        schema.get("type") == "string" or is_string_enum(schema, root)
    )


def patch_artifact_node(node: dict, root: dict) -> None:
    # rule (a): only the literal `false` — typed maps keep theirs
    if node.get("additionalProperties") is False:
        node.pop("additionalProperties")
    # rule (h): string-valued open maps relax to any-valued
    if node.get("additionalProperties") == {"type": "string"}:
        node["additionalProperties"] = True
    # rule (b)
    node.pop("required", None)
    # rule (c): all-string-enum unions collapse to a plain string
    for key in ("anyOf", "oneOf"):
        branches = node.get(key)
        if (
            isinstance(branches, list)
            and len(branches) >= 2
            and all(is_string_enum(b, root) for b in branches)
        ):
            node.pop(key)
            node.pop("enum", None)
            node["type"] = "string"
    # rule (g): all-object unions merge into one open object
    for key in ("anyOf", "oneOf"):
        branches = node.get(key)
        if not (
            isinstance(branches, list)
            and len(branches) >= 2
            and all(is_object_branch(b, root) for b in branches)
        ):
            continue
        merged = {}
        for branch in branches:
            branch = resolve_ref(branch, root)
            for name, schema in (branch.get("properties") or {}).items():
                if name not in merged:
                    merged[name] = schema
                elif merged[name] != schema:
                    if stringish(merged[name], root) and stringish(schema, root):
                        merged[name] = {"type": "string"}
                    # else: first definition wins
        node.pop(key)
        node["type"] = "object"
        node["properties"] = merged
    # rules (d) + (e)
    node.pop("format", None)
    node.pop("propertyNames", None)
    # rule (f)
    if isinstance(node.get("const"), str):
        node.pop("const")
        node.setdefault("type", "string")
    # rule (j)
    for name in RELAX_TO_ANY.get(node.get("title"), ()):
        props = node.get("properties") or {}
        assert name in props, f"{node['title']}.{name} is gone upstream"
        props[name] = {}


def walk(node, root, artifact: bool) -> None:
    if isinstance(node, dict):
        if artifact:
            patch_artifact_node(node, root)
        # artifact mode keeps [X, null] branches: typify renders them as
        # proper Option<X>, and dbt WRITES nulls ("errors": null) that a
        # bare #[serde(default)] Vec would reject. The collapse exists
        # for dbt-jsonschema's duplicate-type-name quirk, a YAML-side
        # problem only.
        patch_node(node, keep_null_branches=artifact)
        for v in node.values():
            walk(v, root, artifact)
    elif isinstance(node, list):
        for v in node:
            walk(v, root, artifact)


# the ONLY manifest fields the server reads (plus patch_path, the natural
# next consumer); drives both the drift assertion and the emitted schema
PROJECTION_NODE_FIELDS = ["resource_type", "name", "original_file_path", "attached_node", "patch_path"]
PROJECTION_META_FIELDS = ["dbt_schema_version", "dbt_version", "generated_at"]
# any-valued node fields: the adapter reads them from a serde_json::Value
PROJECTION_NODE_ANY_FIELDS = ["relation_name", "config"]
# config keys the adapter reads that the pristine schema names; partition_by,
# cluster_by and indexes are adapter-specific, so upstream never declares them
PROJECTION_CONFIG_KEYS = ["materialized", "unique_key", "incremental_strategy"]


def manifest_projection(pristine_text: str) -> dict:
    for name in (
        PROJECTION_NODE_FIELDS
        + PROJECTION_NODE_ANY_FIELDS
        + PROJECTION_CONFIG_KEYS
        + PROJECTION_META_FIELDS
        + ["nodes", "metadata"]
    ):
        assert f'"{name}"' in pristine_text, (
            f"projected field {name!r} no longer appears in the pristine "
            "manifest schema — upstream renamed it; update the projection"
        )
    string = {"type": "string"}
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://schemas.getdbt.com/dbt/manifest/v12.json#cogitator-projection",
        "title": "ManifestProjection",
        "type": "object",
        "properties": {
            "metadata": {
                "title": "ManifestProjectionMetadata",
                "type": "object",
                "properties": {name: dict(string) for name in PROJECTION_META_FIELDS},
            },
            "nodes": {
                "type": "object",
                "additionalProperties": {
                    "title": "ManifestProjectionNode",
                    "type": "object",
                    "properties": {
                        **{name: dict(string) for name in PROJECTION_NODE_FIELDS},
                        **{name: {} for name in PROJECTION_NODE_ANY_FIELDS},
                    },
                },
            },
        },
    }


def main() -> None:
    args = sys.argv[1:]
    mode = "yaml"
    if args and args[0].startswith("--mode="):
        mode = args.pop(0)[len("--mode="):]
    assert mode in ("yaml", "artifact", "manifest-projection"), mode
    src_path, out_path = args

    if mode == "manifest-projection":
        out = manifest_projection(open(src_path).read())
    else:
        out = json.load(open(src_path))
        walk(out, out, artifact=(mode == "artifact"))
    json.dump(out, open(out_path, "w"), indent=1)


if __name__ == "__main__":
    main()
