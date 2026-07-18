# Subtree Arena and Compacting GC Implementation Plan

## Status

This document records both the architecture decision space and the implementation
contract for replacing independently allocated subtrees in `lib/src_rust` with
arena storage. Subtree value representation is deliberately decided before the
arena and collector implementation so that representation, ownership,
collector safety, public identity, and performance decisions do not drift while
the code is being changed.

The plan is intentionally limited to subtree storage. Stack nodes and generic
runtime arrays remain under their current allocation policies. The deterministic
stack window is compatible with this work but is not part of the arena itself;
its independent invariants are recorded in `DETERMINISTIC_WINDOW.md`.

## Goals

1. Keep the value copied through stack links and child arrays at most eight
   bytes; explicitly compare the current eight-byte inline/reference union with
   a four-byte uniform node index.
2. Store arena-backed node records in contiguous storage rather than in one
   allocator block per node.
3. Treat pointer versus physical index versus logical ID as an explicit
   representation decision rather than an incidental allocator detail.
4. Preserve intrusive reference counts and copy-on-write behavior for heap
   subtrees.
5. Stop calling `malloc` and `free` for individual internal subtree nodes.
6. Reclaim rejected GLR subtrees and other dead parser-private nodes with a
   routine, in-place mark-and-slide collection.
7. Preserve public C ABI layouts and public node/cursor behavior.
8. Compare a pointer-stable paged arena with an index-addressed contiguous arena
   before selecting the final storage policy.

## Non-goals

- This is not an arena conversion for `StackNode`, `StackHead`, or generic
  `Array<T>` buffers.
- This does not remove subtree reference counts.
- This does not change subtree summarization, balancing, recovery, or parse
  selection semantics.
- This does not add incremental reuse to the active Rust parser. The current
  parser does not import `old_tree` subtrees.
- This does not permit unsynchronized collection while callers are reading a
  published `TSTree`.
- This does not introduce a 16-byte `{arena, index}` subtree handle.

## Architecture decision space

The word `Subtree` currently combines two concepts:

1. the small value copied through stack links, child arrays, parser caches, and
   public-tree roots; and
2. for inline leaves, the syntax-node data itself.

Those concepts do not have to remain combined. Even when every syntax node is
stored out of line, the runtime still needs a small copyable reference value,
but that value can be only a pointer or index.

### Independent design axes

| Axis | Principal options |
| --- | --- |
| Subtree value model | inline-or-reference, uniform reference, direct embedded value |
| Reference encoding | raw pointer, physical arena index, stable logical ID, tagged table index |
| Reference width | four-byte index, eight-byte index, native pointer |
| Record uniformity | current full heap record, variable-sized per-kind records, fixed node slab, separate kind tables |
| Node storage | per-node heap, pointer-stable paged arena, contiguous byte arena, fixed-record slab |
| Child storage | coallocated tail, fixed inline capacity, global child range, separate child block, sibling links |
| Leaf ownership | inline/no ownership, refcounted arena record, tracing-only arena record |
| Internal ownership | intrusive refcount, tracing, refcount plus tracing GC |
| Reclamation | bulk-only, free list, mark-sweep, semispace, mark-compact |
| Public identity | address, physical slot index, stable occurrence ID, frozen index after publication |

Pointer versus index, storage shape, reclamation, and child representation are
orthogonal. A pointer normally favors pages because growth must not move existing
addresses. An index normally favors one reallocatable buffer because moving its
base does not invalidate relative indexes. Neither relationship is mandatory:
an index can address pages, and a pointer can address a reserved non-moving
virtual region.

### Subtree value models

| Model | Value copied by the parser | Leaf storage | Internal storage | Main tradeoff |
| --- | --- | --- | --- | --- |
| Hybrid inline/reference | inline leaf bits or pointer/index | qualifying leaves inside the handle | out-of-line record | avoids many leaf allocations but has two access paths |
| Uniform pointer | `*const NodeRecord` | every leaf allocated | every internal allocated | simple and direct, but requires stable addresses |
| Uniform physical index | four- or eight-byte offset | every leaf in arena | every internal in arena | compact and movable, but every access needs arena context |
| Uniform logical ID | ID resolved through a location table | every leaf in storage | every internal in storage | stable movement with a permanent extra lookup |
| Tagged table index | kind plus table index | specialized leaf table | specialized internal table | dense per-kind records but multiple storage domains |
| Direct full value | complete node record | embedded | cannot naturally embed recursive/shared children | unsuitable as the universal tree edge value |

The direct-value model cannot remove references from a shared recursive tree.
Parents, stack links, and GLR alternatives must still refer to independently
owned or shared descendants. The meaningful alternative is therefore not “no
`Subtree` type”; it is “`Subtree` becomes only a `NodeRef`.”

### Handle width

Removing inline payload means the reference no longer intrinsically requires
eight bytes:

| Reference | Capacity | Consequence |
| --- | --- | --- |
| `u32` word index | about 32 GiB at eight-byte granularity | halves child-reference storage and remains ample for one tree arena |
| `u32` byte index | 4 GiB | simpler arithmetic but a lower arena limit |
| `u64` index | effectively unlimited | same width as the current handle |
| native pointer | platform address space | direct lookup and non-moving records |

The performance ledger rejects a 16-byte `Subtree`, which regressed parse time
by 19.74%. It does not reject a four-byte uniform index. The invariant to carry
forward is that the value copied through the parser must remain very small, not
that it must always contain inline syntax data.

### Uniform reference does not require uniform record size

Using the current approximately 88-byte `SubtreeHeapData` literally for every
leaf would be simple but likely wasteful. The allocation audit measured about
59.09% internal nodes, 3.21% heap leaves, and therefore about 37.70% currently
inline leaves. Literal unification would turn that last group from eight-byte
values into full arena records.

Three physical models keep a uniform reference without requiring identical
record sizes:

| Record model | Layout | Benefit | Cost |
| --- | --- | --- | --- |
| Variable-sized arena records | small compact-leaf, full-leaf, and internal block kinds | one reference and one arena while keeping leaves small | block-kind branch and variable-size GC scan |
| Fixed node slab plus child array | `NodeRecord[]` and separate `ChildRef[]` | true fixed-index slab and easy node compaction | separate child lifetime/compaction and additional lookup |
| Separate kind tables | compact-leaf, full-leaf, and internal arrays | densest record for each kind | tagged references and coordinated multi-table collection |

The strongest uniform-reference candidate is a four-byte physical index into a
variable-sized arena:

```text
Subtree = u32 arena word index

compact leaf:  [GC header][packed leaf record]
full leaf:     [GC header][full metadata][scanner bytes]
internal:      [GC header][full metadata][children...]
```

This representation pays an arena lookup for every leaf, but it halves stored
child references, removes the inline/heap union, and makes every syntax node
relocatable.

### Refcount implications of removing inline leaves

| Policy | Internal records | Leaf records | Tradeoff |
| --- | ---: | ---: | --- |
| Refcount every node | yes | yes | uniform ownership but many new leaf atomics |
| Refcount only internals | yes | no | preserves internal DAG/COW ownership; tracing determines leaf liveness |
| Pure tracing | no | no | simplest collector graph but a much larger ownership rewrite |
| Refcount for sharing plus tracing for reclamation | yes | optional | likely final hybrid; policy can differ by record kind |

The requirement that internal nodes remain refcounted does not require every
compact leaf to gain a refcount. A uniform index may address an immutable,
tracing-only compact leaf record while internal nodes retain intrusive counts.

### Children representation

| Model | Node fields/layout | Locality | Reclamation/compaction consequence |
| --- | --- | --- | --- |
| Coallocated tail | `[record][child handles...]` | best parent/child-array locality | variable-sized node blocks move together |
| Fixed inline capacity | fixed child slots plus overflow | excellent for small arities | every node pays capacity; two access paths |
| Global child range | `{child_start, child_count}` into `ChildRef[]` | dense sequential children | nodes and child array compact separately |
| Separate child block | pointer/index to variable block | extra indirection | two block kinds and two ownership paths |
| First-child/next-sibling | two references per record | good sequential walk | indexed child lookup becomes linear |

The existing coallocated child representation is the correct control for the
first pointer and index experiments. Externalizing children at the same time as
changing references would prevent attributing the result to either decision.

### Architecture candidates

| Candidate | Subtree value | Node storage | Children | Reclamation | Question answered |
| --- | --- | --- | --- | --- | --- |
| A | current inline-or-pointer, 8 B | pointer-stable paged arena | coallocated | bulk-only | value of removing per-node allocation alone |
| B | uniform pointer | variable leaf/internal records in pages | coallocated | bulk-only | cost of removing inline leaves while retaining direct access |
| C | current inline-or-index, 8 B | contiguous byte arena | coallocated | bulk-only, then compact | cost of arena-aware indexed heap access |
| D | uniform physical index, 4 B | variable compact-leaf/full-leaf/internal records | coallocated | bulk-only, then compact | benefit of one small reference representation |
| E | uniform physical index, 4 B | fixed `NodeRecord[]` | global `ChildRef[]` | compact both arrays | fully flattened slab architecture |
| F | uniform logical ID | movable arena through location table | either | moving GC | cost/benefit of stable IDs and post-publication movement |

The experiments should proceed A, C, then D. A isolates allocator removal. C
isolates indexed access without allocating current inline leaves. D tests the
more radical uniform model only after the index plumbing exists. B is optional,
and E/F are follow-up architectures rather than first implementations.

## Current contiguous-arena candidate

The rest of this document describes candidate C, the conservative indexed
arena. It remains an implementation candidate rather than a final decision
until the representation gate compares it with A and D.

### One contiguous arena

Each parser result is built in one growable byte buffer. Allocations bump an
arena cursor. Growing the buffer may move its base allocation, but relative
indexes remain valid. There are no pages and no per-node free lists.

The parser transfers the arena to the resulting `TSTree`. Tree copies share the
arena through an arena-level owner count. The final owner releases the entire
buffer at once.

### Candidate C: eight-byte indexed subtree handle

`SubtreeInlineData` remains byte-for-byte unchanged. The other union arm changes
from a heap pointer to an encoded arena word index:

```text
Subtree: 8 bytes

inline value                    arena value
+------------------------+      +------------------------+
| SubtreeInlineData      |      | encoded word index     |
| first-byte bit 0 = 1   |      | first-byte bit 0 = 0   |
+------------------------+      +------------------------+

zero non-inline value = NULL_SUBTREE
```

Indexes count eight-byte words. The encoding must reserve the existing inline
tag and must be defined explicitly for every supported endianness. Compile-time
size/alignment assertions remain mandatory.

An arena index is a physical location, not a stable logical node ID. Collection
moves live blocks and rewrites every registered owning handle to its forwarded
index. This avoids a permanent `NodeId -> offset` lookup on every subtree field
access.

### Arena block layout

The target block layout is:

```text
arena index
    |
    v
+-------------------------------+
| GcHeader                      | 8 bytes
|   size_words: u32             |
|   mark_or_forward: u32        |
+-------------------------------+
| SubtreeHeapData               | current common heap data
+-------------------------------+
| child Subtree handles         | 8 bytes * child_count
+-------------------------------+
| optional scanner-state bytes  | variable tail payload
+-------------------------------+
| alignment padding             |
+-------------------------------+
```

`SubtreeHeapData` precedes the variable child array so resolving common fields
does not require reading duplicated child-count metadata. Children are found at
a constant offset after the heap header.

Large serialized external-scanner state should be stored in the block tail and
addressed relative to the block. Leaving it behind a separately allocated
pointer would preserve a significant class of per-leaf allocation and would
complicate relocation cleanup.

### Refcounts remain authoritative between collections

Heap subtree handles retain and release exactly as they do now:

- `retain` atomically increments the resolved heap header;
- `release` atomically decrements it;
- a transition to zero recursively releases owned heap children; and
- zero-reference blocks become dead arena bytes instead of being individually
  freed.

Keeping the release cascade preserves accurate copy-on-write decisions and
prevents descendants of a dead parent from carrying stale elevated counts.
Collection does not change refcounts because moving a block does not change its
ownership graph.

Debug collection will independently count incoming live references and assert
that they agree with intrusive counts. This is a witness, not the release-build
ownership mechanism.

### In-place mark and slide, not semispace

A semispace collector is deliberately rejected for the first implementation.
It would make evacuation simple, but it makes only about half of a fixed arena
capacity usable and copies the mostly-live successful syntax tree at every
collection.

Stable relative indexes already make arena `realloc` safe. At a collection
safepoint, a four-pass in-place collector is sufficiently direct:

1. Mark all heap blocks reachable from registered roots.
2. Scan blocks and store each live block's compacted destination in its old
   `GcHeader`.
3. Rewrite registered roots and every live child handle through those
   forwarding indexes.
4. Scan from low to high and `memmove` live blocks to their destinations, then
   truncate the arena cursor.

The sweep is folded into the final scan. Dead tail resources are destroyed as
their blocks are skipped. Destinations never exceed sources, so ascending
movement cannot overwrite an unvisited block.

## Collector safety model

### Collection is never implicit in allocation

An arbitrary Rust stack local can contain a copied `Subtree` handle. A moving
collector cannot discover or rewrite such a local. Therefore arena allocation
may grow the buffer but must never initiate collection itself.

Collection occurs only at explicit parser safepoints after operation-local
handles have been transferred into registered runtime state. The initial
safepoint is the outer parser-advance boundary, not `subtree_new_node` or a
general-purpose reserve call.

### Parser-private root set

The root audit must include at least:

- all `StackLink` subtree handles;
- all deterministic-window entries;
- token-cache subtree handles;
- the parser's finished-tree candidate;
- reusable-node state that owns subtrees;
- persistent pop/reduction arrays whose elements own subtrees;
- recovery-owned subtree arrays; and
- every other parser field that can retain a heap subtree across the chosen
  safepoint.

Scratch values borrowed only within an operation must be gone before the
safepoint. A debug root/count witness will fail collection if an unregistered
live owner exists.

### Trigger policy

The first implementation uses a conservative routine trigger:

```text
collect when dead_words >= max(used_words / 4, 1 MiB)
```

Collection also runs immediately before the successful result is published as
a `TSTree`. Thresholds are policy rather than representation and may be tuned
only after correctness and paired perf-gate results exist.

## Public tree, node, and cursor boundary

### `TSTree`

`TSTree` gains an arena owner alongside its root handle. Its public type is
opaque, so this does not change the C layout contract exposed in `api.h`.

```text
TSTree
  root: Subtree
  arena: shared SubtreeArena owner
  language
  included ranges
```

The successful parse performs a final collection and then changes the arena
from `ParserPrivate` to `PublishedFrozen`.

### `TSNode.id`

Today `TSNode.id` points to a `Subtree` slot in the root or a parent's child
array. Arena relocation makes that raw address invalid even if the subtree
handle itself has been rewritten.

The ABI field remains `const void *`, but its opaque value becomes an encoded
arena index for the child-handle slot. The tree pointer supplies the arena
context. A reserved non-null value identifies the root slot stored directly in
`TSTree`.

This preserves occurrence identity: two child slots that happen to contain the
same shared heap subtree still produce distinct public nodes. `ts_node_eq`
continues to compare `(tree, id)`.

`TreeCursorEntry` likewise stores a subtree-slot index rather than
`*const Subtree`. The public `TSTreeCursor` layout remains unchanged.

### Published arenas do not compact

The runtime cannot enumerate `TSNode` values copied into caller memory. The
first implementation therefore never changes physical indexes after a tree is
published. The buffer base may move only during an operation that has exclusive
arena ownership, because even a base-pointer update would race concurrent
readers.

Supporting post-publication compaction would require stable logical occurrence
IDs plus an ID-to-index table, or a read barrier on every node access. That is a
separate design with permanent memory and lookup costs and is not included here.

### Tree copy and edit

- `ts_tree_copy` retains the root and arena owner without copying the buffer.
- deleting a copy releases its root and arena owner;
- deleting the final copy destroys the complete arena;
- editing a uniquely owned tree may append replacement blocks but does not
  compact published indexes; and
- editing a shared tree first detaches by copying the arena at identical
  indexes, then recomputes ownership for the detached root and applies the edit.

Preserving indexes during detach keeps existing `TSNode.id` values associated
with the edited tree meaningful. Whole-arena detach is a real cost compared
with today's path-level copy-on-write and must be measured on edit workloads.

## Scratch-node representation

The current scratch-node path writes a temporary `SubtreeHeapData` after a
generic child-array buffer and returns a normal pointer-backed `Subtree`. An
indexed arena handle must never refer to storage outside the arena.

Scratch summarization will therefore use a distinct borrowed view, for example:

```rust
struct ScratchSubtree<'a> {
    data: &'a SubtreeHeapData,
    children: &'a [Subtree],
}
```

Only the small set of summarization and stack-state helpers that consume the
scratch node should accept this view. Scratch nodes are never retained,
released, placed in the arena, or exposed through public APIs.

## Expected code changes

| Area | Planned change |
| --- | --- |
| `subtree/data.rs` | Add GC header/tail layout support and replace scanner-state pointers with arena-relative storage |
| `subtree/handle.rs` | Replace pointer union arm with encoded index; make heap access arena-aware; retain/release no longer free blocks |
| `subtree/storage.rs` | Replace leaf pool and per-node allocation with bump allocation, block iteration, forwarding, and collection |
| `subtree.rs` | Move child handles into arena blocks; split scratch-node view from owned subtree representation |
| `subtree/edit.rs` | Resolve mutable nodes through an arena and preserve indexes across published edits |
| `stack.rs` and stack modules | Supply arena context for subtree reads and ownership operations; expose stack roots to GC |
| parser modules | Own the private arena, register complete roots, and invoke collection only at parser safepoints |
| `tree.rs` | Transfer/share/detach/destroy arenas with public trees |
| `node.rs` | Resolve opaque slot indexes through the node's tree arena |
| `tree_cursor.rs` | Store and resolve slot indexes instead of subtree pointers |
| changed-range traversal | Carry the arena corresponding to each compared tree |

All `Subtree` methods that currently dereference themselves without context
must become arena-aware. To keep call sites readable, the implementation may
introduce short-lived views such as `SubtreeRef { arena, handle }`, but those
views must not be stored across a collection.

## Implementation stages

### 0. Representation gate

1. Implement or prototype candidate A far enough to measure pointer-stable
   paged allocation with the current eight-byte `Subtree` and coallocated
   children.
2. Record allocation-call elimination, peak RSS, retained dead bytes, and paired
   perf-gate results.
3. Implement candidate C initially without movement and measure the cost of
   adding arena context and index decoding while preserving inline leaves.
4. Once C is correct, prototype candidate D's four-byte uniform index and
   compact per-kind records without changing the collector policy.
5. Select the representation before implementing routine GC. Do not select
   semispace versus mark-compact based on a representation that has already
   failed its non-moving performance gate.

Gate: choose A, C, or D with an explicit ledger entry. Record handle width,
record sizes, child-reference bytes, allocations, throughput, and peak RSS.

### A. Representation and non-moving arena

1. Introduce `ArenaIndex`, `GcHeader`, and `SubtreeArena`.
2. Apply the representation selected by stage 0 and enforce its width/tag with
   compile-time assertions.
3. Convert heap accessors to resolve through an arena.
4. Allocate leaf and internal heap data by bumping the arena.
5. Keep collection disabled; grow the arena as needed.
6. Replace per-node frees with refcount-zero accounting.

Gate: full parity and ABI tests pass before adding movement.

### B. Parser ownership integration

1. Give parser, stack, subtree-array helpers, recovery, and edit paths explicit
   arena context.
2. Replace scratch fake-subtree storage with the borrowed scratch view.
3. Transfer the arena into `TSTree` on acceptance.
4. Implement arena sharing, final destruction, and detach-on-edit.
5. Convert `TSNode.id` and tree-cursor internals to slot indexes.

Gate: full fixtures, node/cursor APIs, tree copy/edit, changed ranges, and ABI
tests are byte-for-byte compatible.

### C. Collector

1. Add block scanning and mark bits.
2. Implement the audited parser root visitor.
3. Compute forwarding indexes.
4. Rewrite roots and child handles.
5. Slide live blocks and clean dead tail payloads.
6. Add debug incoming-reference and post-move field witnesses.
7. Enable final pre-publication collection.
8. Enable the routine dead-byte threshold at parser safepoints.

Gate: forced collection after every eligible parser boundary must pass before
using the routine threshold.

### D. Performance and policy

1. Run paired same-session perf-gate measurements across all seven languages.
2. Measure peak RSS and arena high-water/live/dead bytes.
3. Compare candidate A/C/D representation evidence, then compare normal
   threshold, final-only collection, and collection disabled for the selected
   indexed candidate.
4. Tune only the threshold; do not add pages, size classes, or a second arena
   during this stage.
5. Remove temporary counters after the policy is selected and record results in
   `PERFORMANCE.md`.

## Validation gates

Correctness is required before performance evaluation:

```bash
cargo fmt --check --all
cargo clippy -p tree-sitter --lib --tests -- -D warnings
cargo test -p tree-sitter --test abi_surface
cargo test -p tree-sitter --lib
cargo test --all
```

The repository's corpus/parity and ast-grep gates must also remain green.
Collector-specific tests must cover:

- null handles and, when selected, inline handles across collection;
- four-byte index capacity, alignment, and overflow checks when candidate D is
  selected;
- shared child DAGs and exact refcounts;
- dead parent cascades;
- scanner-state tail relocation;
- copy-on-write before and after movement;
- forced movement with every block changing index;
- stack, token-cache, recovery, and deterministic-window roots;
- public root/child/sibling equality;
- tree cursor traversal after arena base relocation;
- `ts_tree_copy`, edit, changed ranges, and deletion in both orders; and
- 32-bit-compatible handle and `TSNode.id` encodings.

## Performance acceptance

This architecture is justified only if removing allocator traffic outweighs
arena-context access, child copying, GC headers, and collection work.

The initial acceptance rule is:

- no language below -1.0 normalized perf-gate point;
- overall geometric-mean throughput is not worse than noise;
- internal-node `malloc/free` calls are eliminated;
- peak RSS does not regress materially on the largest fixtures; and
- edit/copy benchmarks disclose the detach cost rather than hiding it in the
  parse-only headline.

A neutral throughput result may be retained if allocator-call elimination and
peak-memory behavior are substantial and the implementation remains within the
complexity budget. A clear throughput or RSS regression closes the design
without adding paging or multiple arena variants.

## Known risks

| Risk | Consequence | Mitigation |
| --- | --- | --- |
| Missing parser root | Use-after-move or premature reclamation | Explicit safepoints, forced-GC tests, debug refcount witness |
| Arena context on hot accessors | Parse throughput regression | Keep 8-byte handle, inline fast path, inspect generated code and perf gate |
| Moving atomics | Undefined concurrent access | Move only in parser-private/exclusive phase |
| Child copy into arena | More bytes copied during reductions | Move handles without retain/release and reuse source scratch buffers |
| Eight-byte GC header | Higher live-node size | Keep header minimal and measure RSS |
| Published node identity | Stale caller-held `TSNode` | Freeze physical layout after publication |
| Shared-tree edit detach | O(tree size) copy | Lazy detach only on edit; benchmark explicitly |
| Future old-tree reuse | Cross-arena handles do not fit local index model | Import into the new arena or revisit the one-arena constraint as a separate project |
| Complexity growth | Hard-to-maintain runtime | Stage gates; do not add pages, semispaces, or stable-ID tables in v1 |

## Rollback boundary

Stages A and B must keep arena access behind `subtree/handle.rs` and allocation
behind `subtree/storage.rs`. The collector must remain a policy of
`SubtreeArena`, not leak forwarding logic into parser actions. If the design is
rejected at a gate, these boundaries make it possible to restore pointer-backed
storage without undoing unrelated deterministic-stack work.
