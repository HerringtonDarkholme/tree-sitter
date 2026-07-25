# Runtime Memory, Ownership, and Reuse

This chapter explains where the active Rust runtime allocates memory, what
each allocation physically contains, who owns it, how it is released, and
which storage is reused. It focuses on `lib/src_rust`, especially parser data,
the graph-structured parse stack, syntax subtrees, public trees, nodes, and
tree cursors.

Read [Runtime Implementation Deep Dive](./5-implementation-deep-dive.md) first
for the parsing algorithm. This chapter follows memory rather than control
flow.

## Scope and terminology

The active runtime is `lib/src_rust`. `lib/src/lib.c` is its C build entry
point. The generated language is still C-compatible immutable data, and the
public API remains a C ABI.

`lib/src_rust/query.rs` is legacy/inactive code and is intentionally excluded.
Allocations made by a generated external scanner are also opaque to the
runtime; the scanner's `create` and `destroy` callbacks own them.

Four similar words mean different things here:

- a **subtree** is the internal syntax value stored on stack links and in a
  completed tree;
- a **tree** is the public `TSTree` owner wrapped around one root subtree;
- a **node** is a public `TSNode`, which only borrows a subtree slot; and
- a **stack node** is one vertex of the persistent GLR history graph, unrelated
  to a public syntax node.

Unless stated otherwise, “allocation” means one call through Tree-sitter's
configurable `malloc`, `calloc`, or `realloc` hooks. Rust-layout types below do
not promise stable byte offsets. The diagrams show ownership and allocation
boundaries; only layouts explicitly marked `repr(C)` or asserted in source are
ABI layouts.

## The complete ownership graph

At the middle of a parse, the important allocations look like this:

```text
generated TSLanguageFull and tables                         caller input chunks
  static/borrowed; never freed by TSParser                    borrowed briefly
             ^                                                        ^
             | language pointer                                       | TSInput
             |                                                        |
+------------------------------- TSParser allocation ---------------------------+
| Lexer fields and 1 separately allocated included-ranges buffer                |
| TokenCache -------- owns token + last-external-token subtree references       |
| SubtreePool ------- owns free leaf blocks + iterative-work-stack buffer        |
| scratch Arrays ---- own capacity buffers; some elements own subtrees           |
| external scanner payload ---- owned through scanner create/destroy callbacks   |
| stack pointer                                                               |  |
+----------------------------------------------------------------------------|--+
                                                                             |
                                                                             v
                      +---------------- Stack allocation ----------------+
                      | Array buffers: heads, slices, iterators, node pool|
                      | base_node pointer                                 |
                      +-------------------------|--------------------------+
                                                |
                               owns fixed-size StackNode allocations
                                                |
                      head --> current node --> predecessor --> ...
                                  | link owns             |
                                  v                       v
                              Subtree handles ------> subtree allocations
                                                       | children own
                                                       v
                                                 descendant subtrees

After acceptance, ownership of the selected root moves out of TSParser:

             TSTree allocation
             + root Subtree handle ------> same subtree hierarchy
             + language pointer
             + included-ranges Array ----> copied TSRange buffer
                    ^
                    |
             TSNode / TreeCursor borrow; they do not retain the tree
```

There is no arena containing all nodes of one parse. Stack nodes are separate
fixed-size allocations. Heap leaves are separate header allocations. Each
internal syntax node is one combined child-array-and-header allocation. Sharing
is therefore possible at both the stack-graph and syntax-subtree levels.

## Quick comparison

### Storage and allocation types

| Concept | Heap allocation? | Physical representation | Owner | Released when | Reused? |
| --- | ---: | --- | --- | --- | --- |
| Generated language tables | No runtime allocation | Static parse tables, symbols, fields, and lexer data | Generated parser/library | Usually process or module unload | Shared read-only |
| Inline leaf subtree | No | Entire leaf packed into an eight-byte `Subtree` handle | Wherever the handle is stored | Nothing to free | Copied by value |
| Heap leaf subtree | One fixed-size block | `SubtreeHeapData` header | Intrusive subtree reference count | Last reference is released | Up to 32 blocks in parser leaf pool |
| External-scanner bytes | Sometimes | Up to 24 bytes inline in a heap leaf; otherwise a separate byte block | Heap leaf | Leaf is destroyed | Out-of-line byte blocks are not pooled |
| Internal syntax subtree | One variable-size block per distinct internal node | `[child handles][SubtreeHeapData header][possible spare capacity]` | Intrusive subtree reference count | Last reference is released | Not pooled |
| `Subtree` handle | No independent allocation | Eight-byte inline value or heap pointer | Depends on containing object | Depends on referenced subtree | Freely copied, but heap owners require `retain` |
| Scratch internal subtree | No independent block | Temporary header appended inside parser scratch-array capacity | Parser scratch operation | Invalidated when scratch buffer changes | Buffer is reused |
| `TSTree` | One small outer block | Root handle, language pointer, ranges-array descriptor | API caller | `ts_tree_delete` | Not pooled |
| Tree included ranges | One contiguous buffer | `TSRange[count]` | `TSTree` | Tree deletion | No |
| Public `TSNode` | No | Coordinates plus pointers to tree and subtree slot | Borrowed from `TSTree` | Nothing to free | Copied by value |
| Tree-cursor path | When first needed | Growable `TreeCursorEntry[]` | Cursor value | `ts_tree_cursor_delete` | Capacity survives cursor reset |
| `TSParser` | One outer block | Lexer, pools, caches, arrays, and stack pointer | API caller | `ts_parser_delete` | Entire parser survives multiple parses |
| `Stack` | One outer block | Array descriptors, base-node pointer, and pool | `TSParser` | Parser deletion | Survives parser reset |
| Deterministic stack window | One growable buffer, not one allocation per entry | 40-byte entries containing one subtree handle plus state and cumulative counters | `Stack` | Moved into syntax children, materialized, reset, or parser deletion | Capacity survives reductions and reset |
| `StackNode` | One fixed-size block per materialized graph configuration | State, position, eight predecessor links, and counters | Stack-node reference count | No head or successor link references it | Up to 50 blocks in stack pool |
| `StackLink` | No separate allocation | Embedded inside `StackNode` | Containing stack node | Stack node is released | Eight slots reused with node |
| `StackSummary` | Descriptor block plus entry buffer | `Array<StackSummaryEntry>` | One `StackHead` | Head replacement or deletion | Not pooled |
| Generic `Array<T>` buffer | Usually | Contiguous `T[capacity]` | Object containing the array | `delete()` | `clear()` preserves capacity |
| External-scanner payload | Scanner-defined | Opaque pointer returned by scanner `create` | Generated scanner callbacks | Scanner `destroy` | Scanner-defined |

### Observed normal-parse distribution

The perf-gate audit described in
[Measured allocation distribution](#measured-allocation-distribution) observed
the following totals for mechanisms exercised by its 40 normal parse fixtures.
This audit predates the deterministic stack window described below, so its
`StackNode` row is the eager-allocation baseline retained for comparison.
“Allocation calls” is `malloc + calloc + realloc`.

| Mechanism from the table above | Allocation calls | Share of calls | Explicit `free` | Requested bytes | Share of requested bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Internal syntax subtree | 327,678 | 94.76% | 324,599 | 33,390,560 | 95.31% |
| Heap leaf subtree | 17,268 | 4.99% | 17,268 | 1,519,584 | 4.34% |
| `StackNode` | 651 | 0.19% | 651 | 104,160 | 0.30% |
| Generic temporary arrays not transferred into nodes | 127 | 0.04% | 69 | 18,280 | 0.05% |
| `TSTree` | 40 | 0.01% | 40 | 1,280 | less than 0.01% |
| Tree included ranges | 40 | 0.01% | 40 | 960 | less than 0.01% |
| **Total** | **345,804** | **100.00%** | **342,667** | **35,034,824** | **100.00%** |

Inline leaves, `Subtree` handles, public `TSNode` values, and embedded
`StackLink`s have no independent allocation and therefore contribute no calls.
Parser/stack construction was outside the warmed audit window. Tree cursors,
queries, edits, and diagnostic-result APIs were not exercised, so the absence
of their rows is not evidence that they never allocate.

### Terms that are easy to confuse

| Term | Meaning |
| --- | --- |
| Syntax subtree | Actual token or grammar structure; reference-counted when heap-backed |
| Internal subtree | Syntax subtree with children; one combined variable-sized allocation |
| Stack node | Parser-history configuration; not a syntax node |
| Public `TSNode` | Allocation-free borrowed view of a syntax subtree slot |
| Pooling | Reusing a dead allocation by overwriting it |
| Sharing | Multiple owners referencing the same live allocation |
| Copy-on-write | Sharing until mutation requires a private allocation |
| Capacity reuse | Keeping an array buffer while setting its logical size to zero |

## The allocator boundary

`alloc.rs` exports four mutable function pointers:

```text
ts_current_malloc
ts_current_calloc
ts_current_realloc
ts_current_free
```

The default functions call libc. A nonzero allocation failure aborts rather
than returning a recoverable error. `ts_set_allocator` replaces any or all of
the hooks globally; a null callback restores that operation's default.

Most runtime heap storage goes through these hooks, including `TSParser`,
`Stack`, `StackNode`, `TSTree`, subtree allocations, `Array<T>` buffers,
serialized scanner bytes, and returned diagnostic buffers.

Allocator replacement is temporal, not stored per object. Deallocation calls
whatever `ts_current_free` is current at release time. Consequently, changing
allocators while live objects exist is safe only when the new allocator can
free the old allocator's blocks. The public API explicitly requires either
that compatibility or deletion of all existing objects first.

One current exception is the explicit traversal stack in `subtree/edit.rs`:
it is a Rust `Vec<EditEntry>`, so it uses Rust's global allocator rather than
Tree-sitter's hook. It is temporary and is dropped before `ts_tree_edit`
returns. The persistent runtime collections use `utils::Array<T>`.

## Borrowed static and caller-owned data

Not every pointer in the ownership graph identifies runtime-owned heap memory.
The generated `TSLanguageFull` and the tables it references are normally static
data emitted in `parser.c`: parse tables, action entries, lexer modes, symbol
names and metadata, aliases, fields, and external-token maps. `TSParser` and
`TSTree` borrow the language pointer. Neither deletes these tables.

The lexer also borrows input chunks returned by `TSInput::read`. A chunk pointer
is cached only as the current input window and is never freed or resized by the
runtime. The caller remains responsible for the backing source bytes. The
lexer's included-ranges array is different: it is an owned copy allocated and
resized by the runtime.

The external-scanner payload is a third ownership domain. Its pointer is
created, interpreted, and destroyed only through the generated scanner's
callbacks. Tree-sitter stores the pointer but does not know its layout or free
it directly. Generated scanners that allocate payload data are expected to
pair their own `create` and `destroy` behavior correctly.

## Caller-owned result allocations

Some APIs return allocations rather than opaque owning objects:

- `ts_tree_included_ranges` returns a new `TSRange` copy;
- `ts_tree_get_changed_ranges` transfers its result array buffer; and
- `ts_node_string` returns a new null-terminated diagnostic string.

These go through the runtime allocator, and the caller is responsible for
freeing them as documented by the public API. They do not retain a `TSTree` or
subtree after the call. By contrast, `ts_parser_included_ranges` returns the
parser's borrowed internal buffer; the caller must neither modify nor free it.

## `Array<T>`: capacity ownership without element ownership

Many heap diagrams in this chapter contain an `Array<T>` value:

```text
Array<T> stored in an owning object          separate allocation
+------------------+                        +-------------------------+
| contents --------|----------------------->| T[capacity]             |
| size             |                        | [0..size) initialized   |
| capacity         |                        | [size..capacity) spare  |
+------------------+                        +-------------------------+
```

`Array<T>` is an internal Rust-layout triple. Its behavior is deliberately
manual:

- `reserve(n)` allocates or reallocates only when `n > capacity`;
- ordinary growth chooses `max(capacity * 2, 8, required_size)`;
- `clear()` sets `size` to zero and keeps the allocation;
- `erase()` and `splice()` move bytes inside the existing allocation when
  capacity is sufficient; and
- `delete()` frees the buffer and zeros the triple.

The array owns its buffer, but the generic type does not automatically retain,
release, or drop its elements. That distinction is central to the runtime.
For `SubtreeArray`, specialized helpers supply element ownership:

- `subtree_array_copy` allocates a destination buffer, copies handles, and
  retains each heap subtree;
- `subtree_array_clear` releases every element but retains the buffer; and
- `subtree_array_delete` releases every element and frees the buffer.

A plain `Array::assign`, by contrast, is only a bytewise handle copy. It is
appropriate for borrowed scratch comparisons such as `scratch_trees`, not for
creating a new long-lived subtree owner.

## `TSParser`: one long-lived reuse boundary

`ts_parser_new` makes one `malloc(size_of::<TSParser>())` allocation and writes
the Rust-layout object into it. Most fields are embedded values, not separate
objects:

```text
TSParser allocation
+-------------------------------------------------------------------+
| Lexer                                                             |
| *mut Stack                                                        |
| SubtreePool { free_trees: Array, tree_stack: Array }               |
| language pointer                                                  |
| reduce_actions: Array                                             |
| finished_tree: Subtree                                            |
| trailing_extras, trailing_extras2, scratch_trees: Array           |
| TokenCache { token, last_external_token, byte_index }              |
| external scanner payload pointer                                  |
| callbacks, counters, and parse status                             |
+-------------------------------------------------------------------+
```

Construction also creates or reserves:

- a lexer included-range allocation containing the default range;
- a `SubtreePool::free_trees` buffer with capacity 32;
- a separate `Stack` and its initial buffers and base node; and
- a `reduce_actions` buffer with capacity 4.

### Reset is reclamation, not destruction

`ts_parser_reset` deliberately leaves the parser's capacity in place. It:

1. calls the external scanner's `destroy` callback;
2. resets lexer position but keeps the included-range allocation;
3. clears the stack back to one head at its base node;
4. releases both retained token-cache subtrees;
5. releases an unreturned `finished_tree`; and
6. resets counters, cancellation state, and callbacks.

The stack object, its array capacities, pooled stack nodes, pooled leaf
subtrees, parser scratch arrays, and configured included ranges survive. A
subsequent parse can therefore perform substantially fewer allocator calls
even though it builds a fresh syntax tree.

### Parser deletion order

`ts_parser_delete` first resets, then destroys owned storage in dependency
order:

```text
reset parse and scanner state
  -> delete Stack (releasing link subtrees through parser's SubtreePool)
  -> delete reduce-action buffer
  -> free lexer included ranges
  -> clear token cache defensively
  -> delete pooled subtree blocks and release-stack capacity
  -> free remaining parser scratch buffers
  -> free TSParser itself
```

The `SubtreePool` must outlive stack deletion because stack links release their
subtrees into that pool.

## Subtree handles: inline value or heap pointer

`Subtree` and `MutableSubtree` are `repr(C)` unions whose size is asserted to
be exactly eight bytes on every supported target:

```text
union Subtree (8 bytes)
  SubtreeInlineData data
  *const SubtreeHeapData ptr
```

Bit zero of the first byte is the tag. Valid heap pointers have that bit clear
because of allocator alignment.

```text
bit 0 = 1    inline leaf; all syntax data is in the handle
bit 0 = 0    heap-data pointer, or all-zero NULL_SUBTREE
```

This means a `Subtree` slot is not always a pointer. Public `TSNode::id` points
to the slot so that the same addressing rule works for both inline and heap
values.

### Inline leaf layout

Small ordinary leaves require no allocation and no reference count:

```text
byte     0        1          2..3             4
      +-------+--------+----------------+-----------------+
      | flags | symbol | parse_state    | padding_columns |
      +-------+--------+----------------+-----------------+

byte     5                               6              7
      +-----------------------------+---------------+------------+
      | lookahead:4 | padding_rows:4 | padding_bytes | size_bytes |
      +-----------------------------+---------------+------------+
```

The constructor uses this arm only for a leaf without external tokens when
the symbol and measurements fit the bit fields. Copying the handle copies the
entire leaf. `retain` and `release` are no-ops.

### Heap leaf layout

A larger leaf or any leaf carrying external-scanner state uses one header
allocation:

```text
Subtree handle                         leaf allocation
+------------------+                 +--------------------------------+
| heap pointer -----|---------------->| SubtreeHeapData                |
+------------------+                 | ref_count: AtomicU32           |
                                     | padding, size, lookahead       |
                                     | error cost, symbol, state      |
                                     | child_count = 0, flags         |
                                     | payload: scanner state or char |
                                     +--------------------------------+
```

The physical field order of `SubtreeHeapData` is Rust-internal. Allocation
size is exactly `size_of::<SubtreeHeapData>()`; no child storage precedes a
leaf header.

External scanner state up to 24 bytes is stored inside the header payload.
Longer state uses a second exact-length byte allocation:

```text
heap leaf header -> ExternalScannerState::Heap -> serialized bytes
```

The mutable scanner payload and these bytes are different objects. The payload
belongs to the scanner callback lifecycle. Serialized bytes are immutable
checkpoints owned by token leaves and are freed when the leaf's last reference
is released.

### Internal syntax-node layout

Every internal subtree uses one combined allocation. Its occupied prefix and
minimum required byte count are:

```text
child_count * size_of::<Subtree>() + size_of::<SubtreeHeapData>()
```

Since a handle is eight bytes, the allocation is physically:

```text
allocation base                                             possible allocation end
|                                                                            |
v                                                                            v
+----------+----------+-----+----------+------------------------+-------------+
| child[0] | child[1] | ... | child[n] | SubtreeHeapData header | spare bytes |
+----------+----------+-----+----------+------------------------+-------------+
^                                      ^
|                                      |
children() returns this base            the parent handle points here
```

This reverse-addressed layout is intentional. Given the header pointer and
`child_count`, `children_ptr` subtracts `child_count * 8` bytes to recover the
allocation base. Final deallocation must therefore free `children().as_ptr()`,
not the header pointer. If the original child array already had excess
capacity, unused bytes can remain after the header; the runtime does not shrink
the allocation merely to remove that capacity.

The allocation originates as a `SubtreeArray` built while popping a
reduction. `subtree_take_children` ensures enough trailing capacity for the
header, possibly reallocating the array, and transfers the whole allocation
to the new parent. The array value must not subsequently be cleared or
deleted: its child references now belong to the parent.

The header payload stores `SubtreeChildrenData`, including visible and named
child counts, visible descendants, dynamic precedence, repeat depth, and
production id. Other header fields cache aggregate size, lookahead, errors,
and scanner flags. These summaries trade a larger header for non-recursive
navigation and fast parse-path comparison.

### Scratch internal nodes borrow instead of own

`subtree_new_scratch_node` uses the same apparent
`[children][header]` layout, but `subtree_reuse_children` appends the header to
the parser's reusable `scratch_trees` buffer without transferring it.

```text
parser.scratch_trees allocation
+----------------------+-------------------------+
| borrowed child copy  | temporary heap header   |
+----------------------+-------------------------+
```

It exists only long enough to compare a candidate child list with an existing
parent. It must never be retained or released. The next resize, assignment, or
deletion of `scratch_trees` invalidates the header. This is storage reuse, not
a real owned subtree allocation.

## Intrusive subtree ownership

Heap subtrees use an intrusive atomic `ref_count`. The reference belongs to an
owner outside the handle; copying eight handle bytes alone creates no owner.

Typical owners are:

- a `TSTree` root field;
- an internal parent's child slot;
- a `StackLink`;
- retained subtree arrays returned by stack popping;
- `TSParser::finished_tree` and both fields of `TokenCache`; and
- a stack head's last external token or paused lookahead.

Inline and null handles can appear in the same locations without count work.

### Ownership transfer during parsing

The hot paths avoid redundant count changes by moving ownership:

```text
lexed token owner
   --shift--> StackLink owner
   --pop retains--> reduction child-array owner
   --subtree_new_node consumes array--> internal-parent owner
   --push parent--> new StackLink owner
   --accept--> TSParser.finished_tree owner
   --TSTree::new--> TSTree.root owner
```

Branching is where retains appear. Copying a stack version retains its head
node and last external token. Adding a second graph link retains both its
predecessor and subtree. Copying a pop iterator's child list retains every heap
subtree. Selecting one path releases the rejected alternatives.

`TSTree::new` is a transfer: after acceptance the parser puts its
`finished_tree` handle directly into the tree and replaces the parser field
with `NULL_SUBTREE`. It does not increment the root count merely to decrement
it again.

### Copy-on-write

`Subtree::make_mut` implements the mutation boundary:

```text
inline or null                 -> mutate/copy value directly
heap ref_count == 1            -> reuse the allocation in place
heap ref_count > 1             -> clone allocation, then release old reference
```

Cloning an internal node allocates another combined block, copies its child
handles, and retains every heap child. Cloning an external-token leaf also
copies any out-of-line serialized scanner bytes.

This makes `ts_tree_copy` cheap: it allocates a new `TSTree` and included-range
buffer but only retains the shared root hierarchy. Editing either tree clones
only shared nodes on affected paths. Unaffected descendants remain shared.

### Final release is iterative

`Subtree::release` decrements the root. If it reaches zero, release uses
`SubtreePool::tree_stack` rather than recursive Rust calls:

```text
decrement requested subtree
  if still referenced: stop
  if zero: push on tree_stack

while tree_stack not empty:
  pop dead subtree
  if internal:
    decrement every heap child
    push children that become zero
    free combined [children][header] allocation immediately
  if leaf:
    free out-of-line scanner bytes, if any
    return header to leaf pool or free it
```

The explicit stack prevents call-stack overflow on a deeply nested syntax
tree. Child decrements happen before their parent's combined allocation is
freed, while the child handles are still readable.

The reference count uses sequentially consistent atomic increments and
decrements. Shared subtrees are immutable; the atomic is the only field
mutated through shared references.

## The subtree pool

`SubtreePool` has two arrays with different purposes:

```text
free_trees: Array<MutableSubtree>   owns dead heap-leaf blocks
tree_stack: Array<MutableSubtree>   temporary work list; does not own live refs
```

Only standalone heap-leaf headers fit the free list. Internal allocations are
variable-sized because their children precede the header, so they are freed
immediately and never put in this pool. Out-of-line scanner bytes are also
freed rather than cached.

The parser constructs its pool with capacity 32. A released leaf is cached
only when the free-list buffer has nonzero capacity and fewer than
`TS_MAX_TREE_POOL_SIZE` (32) entries. `subtree_pool_allocate` pops a block and
completely overwrites its header; otherwise it calls `malloc`.

Short-lived pools used by `TSTree::delete` and `ts_tree_edit` start with
capacity zero. They still reuse the `tree_stack` capacity during one iterative
operation, but they do not retain dead leaf blocks after it. Leaves released
outside a parser are therefore freed directly.

## The stack is a lazy deterministic suffix over a GLR graph

`Stack` is one separate fixed-size allocation, but its histories are not stored
inside it. It embeds array descriptors and pointers:

```text
Stack allocation
+----------------------------------------------------------------+
| heads: Array<StackHead> ------> contiguous version heads        |
| slices: Array<StackSlice> ----> reusable pop-result descriptors |
| iterators: Array<StackIterator> -> reusable DFS work entries    |
| node_pool: Array<NonNull<StackNode>> -> cached block pointers   |
| window: Array<WindowEntry> ---> unmaterialized linear suffix    |
| base_node, halted count, SubtreePool pointer                    |
+----------------------------------------------------------------+

separate allocations:
  StackNode A <---- StackNode B <---- StackNode C
       ^                   ^                 ^
       | predecessor links may converge     | StackHead
       +-------------------------------------+
```

While there is one active deterministic version, ordinary shifts and eligible
reductions do not build this graph. They operate on `window`, a contiguous
array of 40-byte entries. Each entry owns the pushed subtree handle and stores
the state, position, error cost, node count, and dynamic precedence that the
equivalent eager `StackNode` would contain. The head caches the same logical
top fields, so lexing and action lookup do not require graph materialization.

An eligible reduction scans backward for the suffix containing its requested
number of non-extra children. Extras are moved too, but do not decrement the
grammar child count. The suffix's handles move directly into the existing
child array in bottom-to-top order; no subtree retain/release pair and no
temporary `StackHead` version is created. The parent and any trailing extras
are then appended to the same window.

The runtime materializes the suffix before ambiguity, a reduction that
straddles the materialized base, recovery, accept, null-subtree pushes, graph
logging, or any operation that copies/merges/traverses versions. Materializing
walks entries bottom-up and creates the exact `StackNode` chain that eager
execution would have created. Subtree ownership moves into link zero without
retain or release. Debug builds assert that every cumulative field matches the
entry. Once condensation leaves one normal active version, deterministic mode
can begin again with an empty suffix.

A paired perf-gate run on the seven default languages compared this path with
the same build compiled with window entry disabled. It used five repetitions
and a 200 ms minimum sample time per fixture; the numbers below are geometric
means of the fixture median-throughput ratios within each language.

| Language | Throughput change |
| --- | ---: |
| C++ | +2.04% |
| Go | +2.54% |
| Java | +4.22% |
| JavaScript | +13.17% |
| Python | +11.99% |
| Rust | +14.38% |
| TypeScript | +15.72% |
| **Seven-language geometric mean** | **+9.01%** |

This was a short implementation gate rather than a publication-quality
benchmark: isolated fixture samples exceeded the five-percent coefficient-of-
variation threshold. Every language-level result was positive, however, and
the cross-language effect was much larger than the observed noise.

Each materialized `StackNode` is one
`malloc(size_of::<StackNode>())` block. It includes an inline fixed array of
eight `StackLink` slots; adding a GLR alternative does not allocate a link
object.

```text
StackNode, internal Rust layout
+--------------------------------------------------------------+
| state | position | links[8] | link_count | ref_count          |
| error_cost | node_count | dynamic_precedence                  |
+--------------------------------------------------------------+

StackLink
+---------------------------+
| predecessor StackNode ptr |
| Subtree handle            |
+---------------------------+
```

The node's `position`, cost, node count, and dynamic precedence are cached
path summaries. A link is stored backward from current configuration to its
predecessor; it owns one reference to that predecessor and one to its non-null
subtree.

### Stack-node references

`StackNode::ref_count` is non-atomic because the parser confines stack mutation
to one thread. References come from:

- one `StackHead` for each version currently ending at the node;
- successor `StackLink`s pointing back to it; and
- the `Stack::base_node` field, which keeps the reset target alive.

Outside deterministic mode, pushing allocates or reuses a node and gives it one
reference for the head. The new node takes the old head's existing node
reference and subtree value as its first link; the head is then moved to the
new node. Copying a version must explicitly retain the shared head node. In
deterministic mode, the head continues to own the materialized base node and
each window entry owns its subtree until the entry is popped or materialized.

Merging compatible versions does not copy the common history. It adds the
second current node's predecessor links to the first current node, retaining
the referenced predecessor nodes and subtrees, then removes the redundant
head. Thus multiple futures and pasts share stable node addresses.

### Stack-node release and pooling

When a node count reaches zero, `stack_node_release`:

1. releases the subtrees on all of its live links;
2. releases alternate predecessor nodes recursively;
3. continues iteratively through link zero to avoid recursion on the common
   linear path; and
4. caches or frees the dead node block.

Up to 50 blocks are stored in `Stack::node_pool`. Reuse pops one pointer and
overwrites the complete `StackNode`, including all eight link slots. The pool's
pointer buffer is itself reserved to 50 entries when the stack is created.

`stack_clear` preserves this pool. It first retains the base node, deletes
every existing head and its reachable history, clears the heads array without
freeing capacity, and pushes one fresh active head referencing the base node.
This ordering keeps the base alive even when deleting the old histories
cascades back to it.

`stack_delete` finally releases the base and heads, frees every block still in
the node pool, deletes all array buffers, and frees the `Stack` allocation.

### Pop traversal allocations and ownership

A graph pop is a DFS because one current node can have several predecessors.
The stack keeps reusable `iterators` and `slices` descriptor arrays. Each
iterator may also own a `SubtreeArray` containing the handles collected along
that path.

For the common first link, traversal mutates the existing iterator and avoids
copying its child buffer. For alternate links, it clones the iterator's
subtree array and retains the elements. The hard limit of 64 iterators bounds
this ambiguity work.

Completed pop slices retain their child subtrees. Reduction either:

- transfers the child buffer into a new internal subtree;
- releases and deletes an inferior buffer; or
- temporarily splits trailing extras into parser-owned scratch arrays, later
  pushing those owned handles back onto the stack.

The `slices` descriptor buffer is cleared and reused on the next pop; ownership
of each completed slice's subtree buffer has already been moved to parser
logic.

### Recovery summaries

A `StackHead` may point to a separately allocated `StackSummary` object. That
object is an `Array<StackSummaryEntry>`, so recording a summary can allocate
both the summary descriptor block and its entry buffer. Replacing or deleting
the head deletes the entry buffer and then frees the descriptor. Summaries are
not pooled.

## From accepted subtree to `TSTree`

`TSTree::new` allocates:

```text
TSTree block                              included-range block
+--------------------------+             +----------------------+
| root: Subtree            |             | TSRange[count]       |
| language: borrowed ptr   |             +----------------------+
| included_ranges: Array --|------------>
+--------------------------+
```

The root owns the syntax hierarchy transitively. The language remains borrowed
immutable generated data. Included ranges are copied because their lifetime
must match the tree, not the parser.

`ts_tree_copy` creates a new outer block and a new ranges buffer, then retains
the same root. It does not duplicate subtree allocations. `ts_tree_delete`
releases the root with a non-caching temporary subtree pool, deletes the range
buffer, then frees the outer block.

Deleting one copied tree commonly decrements only the shared root and stops.
Deleting the last copy initiates the iterative cascade through every no-longer-
shared descendant.

## `TSNode`: a borrowed coordinate with no allocation

A `TSNode` is returned by value and contains:

```text
context[0] = absolute start byte
context[1] = absolute start row
context[2] = absolute start column
context[3] = alias symbol
id         = pointer to one Subtree handle slot
tree       = pointer to the owning TSTree
```

For the root, `id` points to `TSTree::root`. For a child, it points into the
eight-byte child-slot prefix of an internal subtree allocation. It does not
point directly to `SubtreeHeapData`; an inline child has no such object.

Creating, copying, navigating from, or deleting a `TSNode` performs no heap
operation and no retain/release. Its validity is entirely derived from its
owning `TSTree`; deleting the tree invalidates the borrowed slot. If a caller
keeps a node across `ts_tree_edit`, `ts_node_edit` adjusts only the coordinate
fields in that value. It still performs no allocation and does not turn the
node into an owner.

Subtrees intentionally have no parent pointers. A node carries its absolute
start coordinate, while child navigation scans forward through sibling sizes
and hidden grammar structure.

## `TreeCursor`: borrowed tree, owned path capacity

The public `TSTreeCursor` value contains a `repr(C)` `TreeCursor` adapter. It
borrows the tree and owns one separately allocated path buffer:

```text
TSTreeCursor value
+------------------------------------------+
| tree pointer (borrowed; not retained)    |
| path { contents, size, capacity } -------|----> TreeCursorEntry[capacity]
| root alias                               |      [0..size) root-to-current path
+------------------------------------------+
```

Each entry contains a borrowed subtree-slot pointer, absolute position, raw
child index, structural child index, and visible descendant index.

The first cursor initialization pushes one entry, so generic array growth
normally allocates capacity 8. Navigation reuses it until a deeper path
requires growth. `ts_tree_cursor_reset` clears only the size and preserves
capacity. Cursor copy creates an independent buffer with copied borrowed
entries. `ts_tree_cursor_delete` frees only the path buffer; it never releases
the tree or a subtree.

## What “reuse” means in this runtime

There are several independent mechanisms:

| Mechanism | Reuses live data or dead storage? | Scope | What it saves | Important limitation |
| --- | --- | --- | --- | --- |
| Inline leaves | Avoids allocation entirely | Every small token | Heap calls and reference counting | Only small leaves without external state fit |
| Subtree reference counting | Shares live syntax data | Stack paths and copied trees | Duplicating complete subtrees | Shared nodes must remain immutable |
| Copy-on-write | Reuses live allocations when unique | Tree editing and mutation | Cloning unaffected nodes | Shared nodes on changed paths must be cloned |
| Heap-leaf pool | Reuses dead storage | Parser operations and parses | Leaf `malloc` and `free` calls | Maximum 32; internal nodes are excluded |
| Stack-node pool | Reuses dead storage | Stack operations and parser resets | Stack-node `malloc` and `free` calls | Maximum 50 |
| Array capacity retention | Reuses dead buffer capacity | Parser, stack, cursor, and scratch arrays | Buffer reallocations | Elements still need correct release handling |
| Graph-structured stack | Shares live parse history | GLR branches | Copying common stack prefixes | Needs reference counts and multi-link traversal |
| Token cache | Shares one live lexed token | GLR versions at the same position | Repeated lexing and token allocation | Lexer mode and scanner state must be compatible |
| Scratch subtree | Reuses temporary array memory | Candidate comparison | Temporary parent allocation | Must never be retained or released |
| `ts_tree_copy` | Shares the live root hierarchy | Multiple public trees | Deep tree copying | Still allocates an outer `TSTree` and ranges buffer |
| Cursor reset | Reuses dead path capacity | Repeated traversal | Cursor buffer allocations | Cursor still borrows the tree |
| Old-tree parsing | Currently none | `ts_parser_parse(old_tree, ...)` | Nothing currently | Active Rust parser ignores `old_tree` |

### Token reuse is not allocation pooling

`TokenCache` owns one token and one last-external-token reference plus a byte
offset. Another GLR stack version may reuse the token only when its position,
lexer mode, generated `reusable` flag, keyword constraints, and serialized
external-scanner state are compatible. Reuse retains the same subtree handle;
replacing or clearing the cache releases the old references.

This prevents duplicate lexing and may share the same heap token allocation
among stack paths. It is logically different from taking a dead leaf block
from `SubtreePool` and overwriting it for a new token.

### Old-tree incremental reuse is currently absent

The public parse functions still accept `old_tree`, but the active Rust
`ts_parser_parse` explicitly ignores it. Every call builds a fresh parse from
the input. The edit machinery, change flags, cached parse states, and changed-
range comparison exist, but the parser does not currently splice unchanged
subtrees from an old tree into the new parse.

Therefore, keeping a parser alive reduces allocator traffic through pools and
capacity retention, but passing an old tree does not currently reduce parsing
work or make the new and old trees share syntax allocations. Sharing between
trees occurs through `ts_tree_copy`, not through `ts_parser_parse(old_tree, ...)`.

## Measured allocation distribution

The qualitative rules above can hide the scale difference between mechanisms.
The following audit counted calls through Tree-sitter's allocator adapters on
the repository's normal perf-gate corpus.

### Method

The measurement used:

- commit `2006de8a` on Darwin arm64;
- the release/benchmark build with Rust 1.96.0;
- the perf gate's seven default languages: C++, Go, Java, JavaScript, Python,
  Rust, and TypeScript;
- all 40 repository-owned normal fixtures, totaling 1,289,348 source bytes;
- the same one-parser-per-language and ordered-file reuse as `perf-gate`; and
- one validation/warmup parse followed by one audited parse of each fixture.

The returned tree was deleted inside each audit window, so its final frees are
included. Parser construction, grammar loading, and persistent buffers already
allocated by the warmup are outside the window.

Instrumentation followed block identity. A buffer first allocated as a
reduction `SubtreeArray` was reclassified as an internal subtree when
`subtree_take_children` transferred it into `[children][header]`. This avoids
mislabeling internal-node storage as a generic temporary array merely because
the allocation happened before `subtree_new_node` wrote its header.

“Requested bytes” is the sum of the size arguments passed to allocation calls,
including replacement sizes passed to `realloc`. It is allocator traffic, not
peak memory or the number of simultaneously live bytes. Audit bookkeeping used
Rust's global allocator and is excluded.

### Aggregate calls

| Final mechanism | `malloc` | `calloc` | `realloc` | Allocation calls | Explicit `free` | Requested bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Internal subtree block | 318,108 | 6,491 | 3,079 | 327,678 | 324,599 | 33,390,560 |
| Heap leaf subtree | 17,268 | 0 | 0 | 17,268 | 17,268 | 1,519,584 |
| `StackNode` | 651 | 0 | 0 | 651 | 651 | 104,160 |
| Other/temporary arrays | 43 | 26 | 58 | 127 | 69 | 18,280 |
| `TSTree` | 40 | 0 | 0 | 40 | 40 | 1,280 |
| Tree included ranges | 40 | 0 | 0 | 40 | 40 | 960 |
| **Total** | **336,150** | **6,517** | **3,137** | **345,804** | **342,667** | **35,034,824** |

Internal subtree storage dominates every measure in this workload:

- 94.76% of all allocation calls;
- 94.63% of `malloc` calls;
- 94.73% of explicit `free` calls; and
- 95.31% of requested bytes.

The 3,137-call difference between allocation calls and explicit frees is the
number of `realloc` calls. A reallocation is counted as an allocation operation,
but the final block still produces only one call through Tree-sitter's `free`
hook; any release of the old address is internal to `realloc`.

On this target, the fixed requested sizes are visible in the totals:

| Mechanism | Requested size per direct call |
| --- | ---: |
| Heap leaf header | 88 bytes |
| `StackNode` | 160 bytes |
| `TSTree` | 32 bytes |
| One default tree range | 24 bytes |

These byte sizes describe this compiler and target, not a stable Rust ABI.
Internal subtree blocks are variable-sized. Their mean request in this audit
was about 102 bytes per allocation event, with replacement requests counted
again when a block was reallocated.

### Distribution by perf-gate language

“Calls/kB” uses decimal source kilobytes and includes internal-subtree
`malloc`, `calloc`, and `realloc` calls.

| Language | Files | Source bytes | Internal-block calls | All allocation calls | Internal share | Internal calls/kB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C++ | 4 | 19,026 | 6,735 | 6,807 | 98.94% | 353.99 |
| Go | 5 | 208,143 | 66,395 | 70,303 | 94.44% | 318.99 |
| Java | 4 | 8,521 | 2,807 | 2,824 | 99.40% | 329.42 |
| JavaScript | 2 | 398,882 | 103,081 | 106,830 | 96.49% | 258.42 |
| Python | 12 | 177,365 | 60,631 | 69,105 | 87.74% | 341.84 |
| Rust | 2 | 72,717 | 21,444 | 22,016 | 97.40% | 294.90 |
| TypeScript | 11 | 404,694 | 66,585 | 67,919 | 98.04% | 164.53 |

The largest individual contributors were:

| Fixture | Source bytes | Internal-block calls | Calls/kB |
| --- | ---: | ---: | ---: |
| JavaScript `jquery.js` | 247,351 | 65,722 | 265.70 |
| TypeScript `parser.ts` | 376,385 | 60,308 | 160.23 |
| JavaScript `text-editor-component.js` | 151,531 | 37,359 | 246.54 |
| Go `proc.go` | 118,832 | 35,950 | 302.53 |
| Go `value.go` | 73,937 | 24,928 | 337.15 |
| Rust `ast.rs` | 66,281 | 17,971 | 271.13 |
| Python `python3.8_grammar.py` | 50,403 | 16,214 | 321.69 |

### Interpretation

The hypothesis that internal nodes cause most allocator traffic is correct for
this corpus. The precise call site is subtler than “`subtree_new_node` calls
`malloc` once.” Parser reductions first allocate child-handle arrays. When the
reduction wins, `subtree_take_children` transfers those same blocks into
internal nodes and may append the header with no extra allocator call. If the
buffer lacks header capacity, it uses `realloc`. Alternative GLR paths can also
create internal blocks that are later rejected, so the counts are allocation
traffic rather than merely the number of nodes in the returned trees.

Heap leaves are a distant second because small leaves are inline and invisible
to the allocator audit. The leaf and stack-node pools also suppress repeated
allocation calls for dead fixed-size blocks. Internal blocks cannot use the
leaf pool because their sizes depend on child count; once their last reference
is released, they are freed immediately.

The audit did not exercise queries, public tree cursors, edits, changed-range
results, or caller-requested diagnostic strings. It also does not include Rust
global-allocator traffic or allocations performed directly inside external
scanner implementations. The table therefore characterizes normal parsing and
tree destruction, not every API workload in the runtime.

## Lifecycle trace for one parse

The following trace ties the separate rules together:

```text
ts_parser_new
  allocate TSParser, lexer range, Stack, base StackNode, initial Array buffers

ts_parser_parse
  create external-scanner payload
  lex leaf
    inline: no allocation
    heap: pop dead leaf block or allocate header (+ optional scanner bytes)
  shift
    deterministic: append one WindowEntry and move token ownership into it
    generalized: pop/allocate StackNode; move token ownership into first link
  branch
    materialize window bottom-up as an equivalent StackNode chain
    retain shared StackNode/Subtree references
  reduce
    deterministic: move suffix handles into the child array; append parent
    generalized: DFS pop retains child handles in arrays
    consume winning child array as [children][parent header]
    release losing arrays and histories
  accept
    move best root into parser.finished_tree; release alternatives
  clear stack
    release history graph; fill stack-node and leaf pools up to their limits
  balance unique repeat nodes in place
  TSTree::new
    allocate outer tree and copied ranges; transfer finished root
  parser reset
    destroy scanner payload; preserve pools and buffer capacities

ts_tree_delete
  release root
    if last owner, iteratively free internal blocks and leaf blocks
  free copied ranges
  free TSTree

ts_parser_delete
  final reset
  release Stack and every cached StackNode
  free pooled leaf blocks and all retained buffers
  free TSParser
```

## Ownership invariants for audits

When changing allocation code, verify these invariants before local control
flow details:

1. Every live `StackHead` owns one reference to its materialized top/base
   `StackNode`.
2. Every deterministic-window entry owns its non-null subtree until that
   ownership moves into a child array or materialized link.
3. Every live `StackLink` owns its predecessor node and its non-null subtree.
4. A copied heap `Subtree` handle is not a new owner until `retain` runs.
5. A shared heap subtree is immutable except for its atomic reference count.
6. An internal subtree owns the child handles immediately before its header.
7. The base pointer, not the header pointer, frees an internal allocation.
8. A scratch subtree borrows parser array storage and is never retained or
   released.
9. Only childless header blocks enter `SubtreePool::free_trees`.
10. Stack-node pool entries and subtree free-list entries are dead blocks with
   no live outgoing ownership.
10. Pop-result subtree arrays are either transferred exactly once or released
    exactly once before their descriptors are reused.
11. `TSTree` owns its root; `TSNode` and `TreeCursor` only borrow it.
12. The parser's subtree pool outlives every stack link that can release into
    it.
13. Allocator hooks remain mutually compatible for the entire lifetime of all
    outstanding runtime and returned allocations.

## Source-reading order

For allocation-first investigation, read:

1. `lib/src_rust/alloc.rs` and `utils.rs` for allocator and array mechanics;
2. `subtree/data.rs`, `handle.rs`, and `storage.rs` for physical tree storage;
3. `stack/stack_node.rs`, `stack/pop.rs`, and `stack.rs` for graph ownership;
4. `parser.rs`, `parser/lexing.rs`, and `parser/actions.rs` for transfers among
   lexer, cache, stack, reductions, and accepted root;
5. `tree.rs` and `subtree/edit.rs` for tree copy, edit, and final release; and
6. `node.rs` and `tree_cursor.rs` for borrowed public views.
