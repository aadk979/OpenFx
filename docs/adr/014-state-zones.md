# ADR 014: State Zones

- **Status:** Accepted
- **Date:** 2026-11-05
- **Supersedes / Extends:** [ADR 013](013-parameter-expressions.md)

## Context

The audit's first mandatory prototype is "state-zone checkpoint replay", ahead of broad node work and of every simulation. `research/01-core-architecture.md` specifies it: the graph is a DAG except inside a typed `StateZone`, with an initial-state input, a monotonic integer tick, and a checkpoint key. A zone may only read committed tick `n-1`, never a live mutable value, and seeking restores the latest checkpoint at or before `n`, then replays fixed ticks.

## Decisions

### 1. Zones are ordinary nodes in the flat graph

```
box ──────────────────┐
                      ├─ solver(initial, body_output, ticks)
state_prev → xform ───┘
```

`state_prev` names its `solver` by id, rather than through an input, so the graph stays acyclic and `.ofxs` stays flat. `foreach` and `foreach_piece` have the same shape. The body is everything between the entry and the owner's second input.

### 2. Sealing makes the read rule structural

Validation refuses any node outside a body that reads a value inside it, by input or by `ch`. The owner's committed result is the only way out, so a live value cannot be read at all. Also refused: an entry naming a missing owner or the wrong kind, a body that ignores its state, an entry that feeds nothing, and a source that does not precede the body. Nested zones are refused for now.

### 3. Solver evaluation

- **Bodies run only from their owner.** The main pass skips them, so a body formula cannot fail at a scene frame no tick reaches.
- **Inside a body, `$F` and `@Frame` are the tick.** Use the expression `$F` for `ticks` to advance with the timeline.
- **A zone's identity** hashes the initial state, each body node as written, and everything the body reads from outside: both geometry and resolved parameter values. Equal identity means equal state at every tick, so checkpoints cannot go stale.
- **Checkpoints** land on multiples of 8 and on each run's final tick. Seeking replays from the latest one at or before the target.
- **Budget.** `CookBudget` gained `max_state_ticks`. Exceeding it is an error before any work begins, and cancellation is checked per body node.
- **Inspection.** Cooking a body node directly shows one evaluation from the source geometry, at the scene frame.

### 4. Foreach

Primitives with equal values of an integer primitive attribute form a piece, and pieces run in ascending order of that value, with `$F` as the ordinal. Results merge in that order, with a budget check on the running total.

### 5. `merged` no longer loses data

`GeoDetail::merged` dropped every attribute and group, so every `merge` node silently lost data, and no test had noticed. It now keeps point, vertex, and primitive attributes from both sides, filling a side that lacks one with zeros. Detail values come from the first side that has them, and groups union. One name with two types is now an error.

## Verification

- **The central oracle:** a seek must equal a fresh linear run at every frame, and must compute exactly the expected number of ticks, which shows that it restores rather than recomputes. A sequence covering forward, backward, exact-checkpoint, and before-first-checkpoint seeks passes.
- **Analytic results:** a constant step gives `n`, and a tick-valued step gives `n(n+1)/2`.
- **Mutation check:** 34 hand-picked mutations by a one-off script, all caught. The script now refuses to run on a failing baseline, after one run reported every mutant "caught" only because a test was already red. Real gaps found: exact-checkpoint restore was never reached because the whole-scene cache answered first, and each part of a zone's identity hid the other.
- `cargo xtask ci`, including an agent scrubbing a solver's timeline over real stdio.

## Limits

- **No nesting.** A zone inside a zone is refused.
- **Checkpoints live in memory** with no eviction, like the rest of `CookCache`, and do not survive a restart. The plan's `MemoryBudgetManager` and on-disk checkpoints do not exist.
- **The checkpoint interval is fixed** at 8 ticks.
- **Feedback is whole-state only.** A solver holds one geometry, and nothing yet expresses substeps, multiple state streams, or `sim_checkpoint` manifests from the fluids addendum.
- **Foreach pieces need an integer primitive attribute**, with no per-point or per-connectivity modes, and merging is quadratic in piece count.
- **Compile blocks**, from the same section of the plan, are not implemented.
