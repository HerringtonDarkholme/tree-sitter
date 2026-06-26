# Parser Performance Architecture Plan

Goal: improve universal raw parsing throughput by 20% for normal parses across
TypeScript, JavaScript, Python, Go, Rust, C++, and Java without changing the
benchmark harness.

## Current Evidence

Recent flamegraphs and benchmark trials show that small branch-level fast paths
are not enough. The remaining hot path is structural:

- `ts_parser__reduce` spends a large share of parse time constructing parent
  subtrees and pushing them back onto the stack.
- `ts_stack_pop_count` copies and retains child subtrees while walking stack
  nodes.
- `ts_subtree_new_node` stores children and heap node data in one allocation by
  growing the `SubtreeArray` with `ts_realloc`.
- `ts_subtree_summarize_children` walks the children immediately after each
  reduction.
- `ts_parser__balance_subtree` walks accepted trees again to rebalance repeat
  nodes.
- `ts_subtree_release` and allocator frames remain visible when parse results
  and discarded alternatives are dropped.

The common pattern is repeated allocation, copying, refcount traffic, and cache
unfriendly pointer chasing during tree construction. A 20% target should attack
that data model, not just individual branches.

Allocator instrumentation with a temporary `ts_set_allocator` harness confirmed
that this is a cross-language issue:

| File | Allocation calls / parse | Requested bytes / parse | Dominant sizes |
| --- | ---: | ---: | --- |
| `javascript/examples/jquery.js` | ~68k | ~6.4 MB | 88, 96, 104, 112 byte blocks |
| `go/examples/proc.go` | ~42k | ~4.0 MB | mostly <=128 byte blocks |
| `typescript/examples/parser.ts` | ~62k | ~5.8 MB | mostly <=128 byte blocks |

For `jquery.js`, exact-size sampling showed about 38k 88-byte allocations, 11k
96-byte allocations, 13k 104-byte allocations, and 3.4k 112-byte allocations per
parse. These match `SubtreeHeapData` alone and `SubtreeHeapData` plus small child
arrays. This makes subtree block allocation the first architectural target.

## Primary Revamp: Parser-Owned Tree Builder Arena

Introduce a parser-owned tree builder for normal parses. Reductions should build
nodes in contiguous parser-owned storage, then publish a normal `TSTree` at the
end of parsing.

There are two implementation options:

1. **Build-and-compact**
   - During parsing, allocate nodes and child spans in parser-owned arenas.
   - Avoid per-reduction `ts_realloc` for child arrays.
   - Avoid retain/release for nodes known to be owned only by the active parse.
   - On accept, compact the arena tree once into the current public `Subtree`
     representation.
   - This is the safest first architecture because `TSTree` and public node
     traversal semantics remain unchanged.

2. **Arena-backed `TSTree`**
   - Extend `TSTree` to own arena pages in addition to the root.
   - Let public `TSNode` traversal read arena-backed child spans directly.
   - Update `ts_tree_copy`, `ts_tree_delete`, edit, changed-ranges, and cursor
     code to handle arena-owned storage.
   - This has higher upside but larger compatibility and correctness risk.

Start with build-and-compact. If the final compaction cost consumes most of the
gain, move to arena-backed `TSTree`.

## Proposed Data Model

Add an internal parser-only representation:

```text
ParseNode {
  symbol
  parse_state
  flags
  padding
  size
  lookahead_bytes
  error_cost
  child_count
  child_start
  summary fields
}

ParseChildSpan {
  children: contiguous Subtree-like handles
}

ParseTreeBuilder {
  node_pages
  child_pages
  scratch_child_stack
}
```

The key idea is that a reduction appends child handles into `child_pages` and
places one `ParseNode` in `node_pages`. The parent references a child span
instead of owning a separately reallocated child array.

## Migration Phases

### Phase 1: Instrument the Current Architecture

Add temporary local instrumentation outside benchmark code to count:

- `ts_subtree_new_node` calls
- `ts_realloc` calls from `ts_subtree_new_node`
- bytes requested by `ts_subtree_alloc_size`
- `ts_subtree_array_copy` calls
- `ts_subtree_retain` and `ts_subtree_release` calls during parse
- average and percentile reduction child counts
- final balancing time and node count

Tools:

- `cargo flamegraph` for CPU attribution
- macOS Instruments Allocations for allocator pressure
- `sample` or `samply` if call stacks need lower-overhead confirmation
- same-session `cargo xtask benchmark --kind normal -r 10 --language ...`

No benchmark source changes.

### Phase 2: Builder Prototype Behind Internal Code Path

Add `ParseTreeBuilder` to parser internals, initially disabled by default.

Prototype only normal no-old-tree parses first. Reparse/reuse can continue using
the current `Subtree` path until the normal case proves a measurable win.

The first target is a parser-private replacement for this sequence:

```text
ts_stack_pop_count
ts_subtree_array_remove_trailing_extras
ts_subtree_new_node
ts_subtree_summarize_children
ts_stack_push
```

Expected wins:

- fewer `ts_realloc` calls
- fewer child-array copies
- better spatial locality during summary computation
- less immediate refcount churn for parser-owned nodes

### Phase 3: One-Time Finalization

Convert the accepted builder root into the current `Subtree` representation once
after parsing.

This keeps these APIs unchanged:

- `ts_tree_copy`
- `ts_tree_delete`
- `ts_tree_edit`
- `ts_tree_get_changed_ranges`
- `TSNode` traversal
- tree cursors

The acceptance gate is whether the saved construction cost exceeds the final
compaction cost across the target languages.

### Phase 4: Stack Integration

If Phase 2 is positive but below target, make stack entries carry builder handles
for parser-owned nodes. This avoids converting builder nodes back into normal
`Subtree` handles while they are still transient.

This phase targets:

- `ts_stack_pop_count`
- `stack__iter`
- `ts_stack_push`
- `stack_node_release`
- merge comparisons for parser-owned nodes

### Phase 5: Optional Arena-Backed Trees

Only pursue this if build-and-compact proves the data model but finalization
cost blocks the 20% target.

This phase changes `TSTree` internals to own arena pages and requires a full
audit of tree copy/delete/edit/cursor/changed-ranges behavior.

## Acceptance Gates

A phase is worth keeping only if it improves the weighted normal benchmark
across the target languages, not just one file.

Benchmark set:

```sh
cargo xtask benchmark --kind normal -r 10 --language typescript
cargo xtask benchmark --kind normal -r 10 --language javascript
cargo xtask benchmark --kind normal -r 10 --language python
cargo xtask benchmark --kind normal -r 10 --language go
cargo xtask benchmark --kind normal -r 10 --language rust
cargo xtask benchmark --kind normal -r 10 --language cpp
cargo xtask benchmark --kind normal -r 10 --language java
```

Validation:

```sh
cargo test --all
```

Run validation outside the sandbox.

## Risk Register

- Reparse/reuse depends on stable `Subtree` ownership and parse-state metadata.
- Tree copy/delete depend on atomic refcounts.
- Tree edit and changed-ranges assume current child storage layout.
- `TSNode` points into tree-owned child storage, so any arena-backed public tree
  must preserve stable addresses.
- External scanner state has separate allocation/copy behavior and should not
  be folded into the first arena prototype.

## Decision Rule

Do not continue micro fast paths unless profiling shows a single narrow helper
above 10% after the arena work. The main performance bet is reducing allocation,
copying, refcount traffic, and repeated tree walks in the parser construction
pipeline.
