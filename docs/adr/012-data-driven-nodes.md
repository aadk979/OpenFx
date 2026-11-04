# ADR 012: Data-Driven Nodes

- **Status:** Accepted
- **Date:** 2026-11-05
- **Supersedes / Extends:** [ADR 011](011-geometry-exchange.md)

## Context

`Node` was a closed enum, so adding an operator took about 11 edits across two crates, and `apply_edits` carried a second copy of most validation rules. The enum also could not meet the plan: a field typed `f32` cannot hold the expressions that `research/01-core-architecture.md` requires of a parameter slot, and a closed enum cannot accept the custom nodes that plugins will add at runtime. With 412 `Node::` references and a target of 60 or more operators, the cost of changing this only grows.

## Decisions

A `Node` is now data: `{ id, op, inputs, params }`. A static table of `NodeDef` rows declares each operator's ports, parameter kinds, and constraints, and a `NodeRule` covers constraints spanning several parameters. The catalog, validation, `.ofxs` reading and writing, and cache keys all derive from that table.

- **Adding an operator** takes one table row and one cook arm.
- **Runtime registration.** `register_operator` adds a definition and a cook function for the rest of the process. This is the seam the plan's WASM plugins will use; no plugin host exists yet. Built-ins cannot be replaced, so a scene using only built-ins means the same thing whatever is loaded.
- **`.ofxs` is unchanged.** A golden scene covering every operator except `asset` must round-trip byte for byte, and a new operator cannot skip it.
- **The JSON API changed.** Nodes are now `{"id": 5, "op": "sphere", "params": {"radius": 2}}`. Numbers settle to their declared kind on entry, so equal scenes compare and hash equal. `SetParam` and `SetInput` replace the four operator-specific edits. There were no outside users to break.
- **`is_generator` means no inputs**, and the parser no longer duplicates `validate`'s first-node check.

## Verification

- **Differential test.** A scene of 42 operators was cooked by the old and new builds: identical `.ofxs` bytes and identical geometry hashes at every node. It was a one-off run, because hashes are not yet known to match across platforms.
- **Tests were not weakened.** A script rewrote construction sites mechanically, leaving the assertions alone, so the existing fuzz, malformed-input, and round-trip tests guarded the change.
- **Mutation check.** 33 hand-picked mutations by a one-off script: 31 caught. The other two exposed redundant code, which was removed.
- `cargo xtask ci`: 175 tests.

## Limits

- **All parameters are required.** The plan stores non-default values sparsely against a versioned definition; versions and defaults are not implemented.
- **The registry is process-wide.** Plugins must register before scenes load, and a scene's validity depends on what is loaded.
- **Cook dispatch is still one large match** for built-ins, though it is no longer the source of the edit cost. Moving each arm into its own function can happen as they are next touched.
