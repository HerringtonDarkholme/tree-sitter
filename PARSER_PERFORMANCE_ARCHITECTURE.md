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

A global size-class slab protected by a mutex was prototyped and rejected: it
preserved layout but the JavaScript benchmark did not reach the first file
result in normal time. The next allocator design should avoid per-block global
locking. Prefer parser-local pages with a clear finalization story, or an
arena-backed `TSTree` with page lifetime tied to tree lifetime.

A second atomic global slab was also rejected. It removed the mutex, but still
used a high-bit marker in `SubtreeArray.capacity` to distinguish slab-backed
child storage. That interacted poorly with generic array growth and parser
ownership paths. Future designs should not overload `SubtreeArray` metadata for
allocation ownership; use an explicit tree/builder owner instead.

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

Earlier small allocator experiments changed allocation counts without improving
throughput. The next implementation should stop optimizing allocator call
sites and move directly to an arena-backed tree design. Build-and-compact is
still useful as a fallback, but it likely leaves too much work in the final
copy pass to reach 20%.

## Architecture Decision: Arena-Backed Normal Trees

The primary implementation should make normal no-old-tree parses produce a
`TSTree` whose internal nodes live in tree-owned pages. This is a real storage
model change, not a slab allocator hidden behind `SubtreeArray`.

The current public `Subtree` pointer shape can remain intact:

```text
[child Subtree; child_count][SubtreeHeapData]
                               ^
                               Subtree.ptr
```

For arena nodes, this block lives inside a page owned by the resulting
`TSTree`. Child access still uses the existing pointer arithmetic, so `TSNode`,
tree cursors, and most traversal code do not need a new child representation.

Add a new internal tree storage owner:

```text
TreeArena {
  ref_count
  pages
  current_page
}

ArenaPage {
  next
  size
  capacity
  data[]
}
```

Extend `TSTree`:

```text
TSTree {
  root: Subtree
  language
  included_ranges
  included_range_count
  arena: *mut TreeArena
}
```

Arena ownership is explicit at the tree level. Do not encode ownership in
`SubtreeArray.capacity`, and do not use global pools or global locks.

### Arena Node Marking

Use a dedicated internal heap flag, for example `HEAP_ARENA_OWNED`, to mark
nodes allocated from a `TreeArena`. This is different from the rejected
`SubtreeArray.capacity` marker:

- the flag lives on the node being released
- `child_count` still reconstructs the child block address
- generic array code remains unaware of allocation ownership
- release behavior is localized to subtree deletion paths

### Release Semantics

For arena-owned nodes, `ts_subtree_release` must still walk children so heap
leaves, external scanner state, and reused old-tree nodes are released
correctly. It must not call `ts_free` for the arena-owned node block.

Tree deletion becomes:

```text
ts_tree_delete
  -> release root children and non-arena descendants
  -> decrement TreeArena ref_count
  -> free all arena pages when the last tree copy is gone
```

Tree copy becomes:

```text
ts_tree_copy
  -> retain root so children are released once after the last tree copy
  -> increment TreeArena ref_count for arena trees
```

This keeps copied trees cheap without copying arena pages. The root subtree
refcount and arena refcount are both needed: the subtree refcount controls when
children are released, and the arena refcount controls when pages are freed.

### Parser Fast Path Scope

Only enable the arena path when all of these are true:

- `old_tree` is null
- included ranges are ordinary for the parse
- parsing is not resuming canceled balancing
- language is native, not wasm

All reparse, node reuse, wasm, cancellation-resume, and unusual recovery paths
can keep using the current `Subtree` allocation path until the normal path is
correct and faster.

### Stack Integration Required For 20%

Arena-backed final trees alone are not enough if reductions still allocate a
temporary `SubtreeArray` for every pop. The stack API needs a builder-oriented
pop path:

```text
ts_stack_pop_count_into_builder(stack, version, count, builder)
  -> walks the same stack graph
  -> appends children into builder scratch storage
  -> returns slice/version metadata
```

Then reduction becomes:

```text
children = builder.pop_children(...)
remove trailing extras into parser scratch arrays
parent = arena_new_node(children)
summarize parent from the same child span
push parent
```

The first milestone can keep the current `Subtree` value in `StackLink`. The
node pointer still points at `SubtreeHeapData`; the difference is only where
that block is owned. A later milestone can remove transient retain/release for
arena-owned nodes on the stack.

### Expected Wins

This design attacks the measured costs directly:

- no per-node allocator call for arena-owned internal nodes
- no per-node free for arena-owned internal nodes
- fewer child-array reallocations once stack pop writes into builder storage
- less refcount traffic for parser-owned internal nodes
- better locality for reductions, summarization, and balancing

The target is not a 1-3% branch win. The target is removing the dominant
construction data movement and allocator pressure from normal parsing.

### First Implementation Slice

1. Add `TreeArena` and arena page allocation utilities.
2. Extend `TSTree` with an arena pointer and update copy/delete.
3. Add `HEAP_ARENA_OWNED` and release logic that skips freeing arena blocks
   while still releasing children.
4. Add `ts_subtree_new_node_in_arena` and use it only after accept, guarded by
   a parser flag, to validate tree ownership.
5. Move reduction node creation to arena allocation for normal no-old-tree
   parses.
6. Replace `ts_stack_pop_count` allocation with a builder scratch pop path.
7. Only after correctness and canaries pass, remove retain/release traffic for
   arena-owned stack nodes.

Each step must be independently testable. If any step regresses, revert that
step rather than adding another micro fast path.

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
