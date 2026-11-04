# ADR 011: Content-Addressed Geometry Exchange

- **Status:** Accepted
- **Date:** 2026-11-05
- **Supersedes / Extends:** [ADR 010](010-thingi10k-torture-harness.md)

## Context

No node or MCP tool could read or write a mesh, and only `pcc-torture` used `pcc-io`. An agent could not work on its own geometry or get a result out.

## Decisions

### 1. Scenes refer to content, not paths

A node holding a path would make a scene's result depend on a file that can change, which breaks the content-addressed cache and reproducibility. Instead, `geo_import` parses a file and stores it under its geometry hash, and a new `asset` node names that hash. Deleting or changing the source afterwards has no effect on the scene.

Import does not change the scene. The agent adds the node through the usual revisioned edits, so undo, transactions, and conflict detection apply. The evaluator receives assets through an `AssetResolver`, so it still does no file access.

`geo_export` cooks a node, or the final node, and writes `.obj` (with vertex `uv`), `.stl` (triangulated), or `.ofxg` (lossless). It reports the revision it cooked.

### 2. Paths are hostile input

File tools are off unless the host passes `--workspace DIR`.

- **Lexical checks** run first, on raw text, and treat backslashes as separators on every platform. They reject absolute paths, `.`, `..`, empty segments, NUL, colons (drive letters and NTFS streams), Windows device names, and names ending in a dot or space.
- **Containment checks** then canonicalize the file, or a write target's parent, so that symlinks and junctions cannot leave the workspace.
- **The asset store** (`.pcc-assets/`) is unreachable through paths. A stored file is checked against its hash on load, so an edited file is reported as corrupt rather than used.
- **Writes** go through a temporary file and a rename, so readers never see a partial file and a link at the target is replaced rather than followed. Overwriting requires `overwrite`.
- **Reads** are bounded by the memory budget, and error messages do not include absolute paths.

### 3. A usable OBJ reader

The reader rejected most real files: CRLF line endings, `vn`, `g`, `usemtl`, vertex colours, and negative indices. It now accepts these and reads `vt` into vertex `uv` when every corner has one. Free-form surfaces still fail, so that geometry cannot silently vanish.

## Verification

- `cargo xtask ci`: 163 tests, including an agent importing a mesh over real stdio, deleting the source, remeshing and flattening, and exporting a file with UVs.
- Mutation check: 28 hand-picked mutations by a one-off script, of which 26 are caught. The first run missed 9, partly because the symlink test skipped itself on Windows; it now uses junctions, which need no privileges. The two survivors change nothing observable: `..` also fails the trailing-dot rule, and the bounded read catches what the size pre-check catches.

## Limits

- **The asset store has no eviction.** Assets stay in memory and on disk until removed by hand.
- **Write targets need an existing parent directory.**
- **The checks are not race-free.** A local process that swaps a directory for a link between the check and the open could escape. The threat model is an untrusted agent, not an attacker with local filesystem access.
- **`.obj` loses data.** Only positions, topology, and vertex `uv` survive; `.ofxg` is the lossless format. `.ply`, `.gltf`, and `.bgeo` from M0.3 are still missing.
