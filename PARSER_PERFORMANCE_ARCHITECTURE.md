# Parser Performance Architecture

This document records the current parser performance direction. The detailed
benchmark and trial history lives in `PERFORMANCE.md`.

## Goal

Improve raw normal parsing throughput across TypeScript, JavaScript, Python,
Go, Rust, C++, and Java without changing benchmark semantics or public runtime
behavior.

## Current Evidence

The repeated hot pattern is:

```text
ts_parser__advance
  -> reduce
  -> stack pop / child collection
  -> parent node allocation
  -> child summarization
  -> stack push / merge
```

Kept improvements:

- arena-backed internal reduction nodes for fresh normal parses
- parser-owned stack-pop builder for fresh reductions
- focused readability and dead-code cleanup around parser/stack runtime

Rejected or removed directions:

- global slab allocators
- local refcount ordering tweaks
- small branch-only fast paths
- pending-reduction descriptor wiring
- payload-based stack traversal for lazy reductions
- dormant segmented-stack storage

## Current Architecture

Fresh normal parses allocate internal nodes in a `TreeArena` owned by the
returned `TSTree`. This reduces per-node allocator/free traffic while preserving
the existing `Subtree` pointer shape:

```text
[child Subtree; child_count][SubtreeHeapData]
                               ^
                               Subtree.ptr
```

`subtree_release` still walks children for arena-owned nodes. It only skips
freeing the node block itself, because page memory is released with the
`TreeArena`.

The arena path is disabled when parsing with an old tree, because reused nodes
may point into the old tree's arena and a returned tree currently owns only one
arena pointer.

## Remaining Bottleneck

Arena-backed nodes improve allocation ownership, but reductions still eagerly
materialize concrete tree nodes and summarize child metadata at every reduce.
The remaining universal-performance problem is structural, not a missing helper
inline.

Any next performance attempt should target one of these boundaries:

- defer concrete tree construction across reduction chains
- reduce child summarization work with a proven metadata model
- reduce stack graph traversal in straight-line parses without regressing GLR
  branching and recovery
- improve generated lexer/runtime contract when profiles show lexer dominance

## Rules For Future Trials

- Start with counters or flamegraphs that identify a hot phase.
- Keep correctness boundaries explicit: reduce, merge/recovery, accept, old-tree
  reuse, external scanner state, and tree publication are separate.
- Do not add dormant storage foundations without a benchmarked activation plan.
- Remove failed trial scaffolding instead of keeping it under `dead_code`.
- Validate with `cargo test --all`.
- Benchmark across the target language set, not a single file.

## Validation

For code changes:

```bash
cargo test --all
```

For performance work, record commands and results in `PERFORMANCE.md`.
