# ADR 013: Parameter Expressions and Time

- **Status:** Accepted
- **Date:** 2026-11-05
- **Supersedes / Extends:** [ADR 012](012-data-driven-nodes.md)

## Context

Time reached only VEX wrangles, so nothing else could animate. Every node's cache key included the frame, so nothing was reused between frames. `research/08-language-scripting.md` calls for a dedicated stack-bytecode evaluator rather than an embedded general-purpose language, with channel references recorded as dependencies.

## Decisions

### 1. `pcc-expr`

A new tier-0 crate compiles formulas to stack bytecode. An expression has no loops or user functions, so it always terminates. Its stack depth is known at compile time, so evaluation does not allocate, and a formula needing more than 64 slots is refused at compile time. Source length and parser nesting are bounded too.

- **Variables:** `$F` (frame, possibly fractional), `$T` (seconds), `$FPS`, `$PI`, `$E`.
- **Operators:** arithmetic, `^` (right-associative, binding tighter than unary minus), comparisons, and logic, with truth as 1 and 0.
- **Functions:** the usual math, `lerp`, `fit`, `smoothstep`, `clamp`, `if`, and a repeatable `rand`. Trigonometry takes radians, like the VEX-style language.
- **Scope queries:** `ch("name")`, `ch(12, "name")`, `ch(12, "size.x")`, `npoints(0)`, `nprims(0)`, and `bbox(0, "ymin")`.

The research document lists more functions and string expressions, which are not implemented.

### 2. Parameters hold expressions

Numeric and boolean parameters accept `{"expr": "..."}`, and vectors accept a formula per component, as `{"x": "0", "y": "$F * 2", "z": "0"}`. In `.ofxs` a formula is `=<hex>`, which keeps a line's tokens positional and leaves existing scenes byte-identical.

- **Syntax errors** are refused when the edit is made.
- **Ranges and multi-parameter rules** wait until values exist, then run at cook time and report the node and parameter.
- **`ch` references to other nodes are dependencies**, for ordering and for cycle detection.
- **Integer parameters round** a formula's result, and refuse values that do not fit.

### 3. Resolve first, then key the cache

Each node's expressions resolve before its cache key is computed, and the key holds the resolved values. Only an operator that reads time itself, currently `wrangle`, is keyed by the frame. So a node whose values and inputs do not change between frames reuses its result, without any time-dependency tracking.

## Verification

- `cargo xtask ci`, including an agent animating a node over real stdio.
- A fuzz test of 20,000 random inputs, and attacks on each recursion path and on the evaluation stack.
- Mutation check: 38 hand-picked mutations by a one-off script, all caught. The first run left 5 alive, including the stack limit, whose test never reached it because the parser's nesting limit fired first.

## Limits

- **`$FPS` is fixed at 24.** Scenes cannot state their own rate yet.
- **There are no animation curves.** The plan's parameter slot also holds keyframed curves, which do not exist; piecewise animation needs `if`, `lerp`, or `fit`.
- **Expressions compile on every cook.** This costs little next to a cook, but it does not meet the plan's goal of a precompiled, zero-allocation path through the whole evaluator.
- **`ch` reads other nodes' numbers only**, by node id rather than by path. A node's own `ch` reads only its literal parameters.
- **Mesh-level results are not reused by value.** A node recooks if its resolved values or input hashes change, even when its output would turn out identical; downstream nodes then reuse by content.
