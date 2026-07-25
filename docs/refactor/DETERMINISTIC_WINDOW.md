# Deterministic Window: The Linear GLR Fast Path

## Status

This document is the architecture contract for the deterministic stack window
implemented in the active Rust parser under `lib/src_rust`. It records why the
window exists, the exact representation and ownership rules, when execution
must return to the graph-structured stack, and what future changes must preserve.

The implementation was introduced in commit `37f91aa1`. It is retained code,
not a speculative design. The term **linear GLR** in this document means the
period where the GLR parser has one active history and therefore behaves like
an ordinary LR parser. It does not mean a separate parser or grammar mode.

## Problem

The ordinary GLR representation pays graph costs even when the history is a
single chain:

```text
base <- StackNode <- StackNode <- StackNode <- head
```

Each push normally creates or reuses a 160-byte `StackNode`. Of those bytes,
128 are eight inline alternate-predecessor slots. A normal deterministic push
uses only slot zero.

The allocation audit found:

| Observation | Result |
| --- | ---: |
| Logical stack nodes created | 1,139,623 |
| Released nodes with one predecessor | 98.898% |
| Mean predecessor-link count | 1.011 |
| Unused link-slot writes across two corpus passes | about 127.4 MB |
| Maximum links required by recovery/ambiguity | all eight |

The eight-link graph representation is required when histories merge, but it
is wasteful while there is only one history. The window makes graph structure
lazy: a linear suffix stays in a compact array until an operation actually
needs graph topology.

## Goals

1. Execute ordinary single-version shifts and reductions without allocating a
   `StackNode` for every LR step.
2. Preserve the existing GLR graph and all merge/pop/recovery behavior after
   materialization.
3. Move subtree ownership through shifts and reductions without compensating
   retain/release pairs.
4. Avoid creating and immediately renumbering a temporary `StackHead` version
   for an eligible deterministic reduction.
5. Make materialization field-for-field equivalent to eager `StackNode`
   construction.
6. Return to deterministic mode promptly after ambiguity or recovery condenses
   to one ordinary active version.
7. Degrade to the existing generalized implementation when the window cannot
   represent an operation.

## Non-goals

- The window does not replace the GSS or change `StackNode` merge semantics.
- It does not support multiple simultaneous windowed versions.
- It does not defer syntax-subtree construction or child summarization.
- It does not alter parse-table actions, conflict resolution, dynamic
  precedence, error costs, or accepted-tree selection.
- It does not optimize recovery; recovery materializes and uses the existing
  graph operations.
- It does not reduce `MAX_LINK_COUNT` or change the eight-link ambiguity bound.
- It does not introduce a heuristic minimum run length. A newly enabled empty
  window is always allowed to accept the next eligible push.

## Representation

`Stack` retains the materialized graph prefix and adds one contiguous suffix:

```text
materialized GSS prefix                    deterministic suffix

base <- node <- node <- window_base        [entry 0][entry 1][entry 2]
                            ^                                      ^
                            | StackHead.node                       | logical top
```

There is no separate `window_base` field in the implementation. While the
window is enabled, `StackHead.node` is the materialized base below the suffix.

```rust
struct WindowEntry {
    state: TSStateId,
    position: Length,
    error_cost: u32,
    node_count: u32,
    dynamic_precedence: i32,
    subtree: Subtree,
}
```

The current 64-bit layout is 40 bytes per entry. Each entry represents exactly
one eager `StackNode` with one predecessor link. The cumulative fields are the
values that eager node would contain after recognizing `subtree`.

`Stack` contains:

```rust
window: Array<WindowEntry>,
window_enabled: bool,
```

`window_enabled` describes execution mode, not whether the array is nonempty.
An enabled empty window means the materialized head is linear and the next
eligible push may append an entry. Materialization clears the array and disables
the mode until condensation explicitly enables it again.

## Core invariant

When the window contains entries `e[0..n]`, eagerly constructing the equivalent
chain must produce:

```text
head.node
   <- node(e[0])
   <- node(e[1])
   ...
   <- node(e[n - 1])
```

For every `i`, all of these fields must match:

| Window field | Equivalent `StackNode` field |
| --- | --- |
| `state` | `state` |
| `position` | `position` |
| `error_cost` | `error_cost` |
| `node_count` | `node_count` |
| `dynamic_precedence` | `dynamic_precedence` |
| `subtree` | `links[0].subtree` |

The predecessor of `node(e[0])` is the materialized base. The predecessor of
every later node is the node for the previous entry. Each materialized node has
exactly one link and the same ownership that the entry previously held.

Debug builds assert cumulative-field equality for every entry during
materialization. This is the primary equivalence witness.

## Logical top cache

`StackHead` caches the logical top's:

- parse state;
- position;
- error cost;
- node count; and
- dynamic precedence.

When the window is disabled, these fields mirror `StackHead.node`. When the
window is enabled, they mirror the final entry, or the materialized base when
the window is empty.

Parser action lookup and lexing use the head accessors and therefore do not
need to materialize merely to ask for the current state or input position.

Any operation that truncates the window must restore the cache from the new
last entry or, when the window becomes empty, from `StackHead.node`.

## Shift

An eligible push requires:

```text
window_enabled
version == 0
head count == 1
subtree != NULL_SUBTREE
```

The push appends one entry and applies the exact arithmetic used by
`stack_node_new`:

```text
entry.position            = old.position + subtree.total_size
entry.error_cost          = old.error_cost + subtree.error_cost
entry.node_count          = old.node_count + subtree public-node contribution
entry.dynamic_precedence  = old.dynamic_precedence + subtree.dynamic_precedence
entry.state               = target state
```

The pushed subtree's ownership moves into the entry. The window does not retain
it. The head cache is updated to the new cumulative values.

A null-subtree push is a recovery discontinuity. It materializes and uses the
ordinary graph path because error-state logic inspects null predecessor links.

## Deterministic reduction

A reduction may use the window path only when:

```text
there is one stack version
version == 0
the window is enabled
the parse-table entry does not invalidate parse state
the reduction is not ending a non-terminal extra via null lookahead
the requested pop fits entirely inside the window
```

The pop scans backward until it has found the requested number of grammar
children. Extra subtrees are included in the physical suffix but do not
decrement the remaining grammar child count. This matches graph-pop semantics.

```text
window before:
  [older][child A][extra][child B]

reduce count = 2

window after:
  [older]

moved children:
  [child A][extra][child B]
```

The selected handles move into a `SubtreeArray` in bottom-to-top/source order.
They are not retained. Shrinking `window.size` relinquishes the entries without
releasing their moved handles.

The existing trailing-extra removal and `subtree_new_node` construction code
then runs unchanged. The parent and removed trailing extras are pushed back
through `stack_push`, normally appending to the same window.

The reduction returns the original version. It does not:

- create a DFS stack iterator;
- retain children and later release the same ownership;
- create a temporary `StackHead` version;
- renumber that version into the original slot; or
- probe for a merge when no other version exists.

If the requested children straddle the materialized base, the window pop returns
`None` without mutation. The parser materializes the complete suffix and reruns
the reduction through the ordinary graph-pop implementation.

## Materialization

Materialization converts entries from oldest to newest:

```text
node = materialized base

for entry in window:
    node = stack_node_new(node, entry.subtree, entry.state)
    assert cumulative fields match entry

head.node = node
window.clear()
window_enabled = false
```

Subtree ownership moves from each entry into link zero of the new node. The
window is cleared without retaining or releasing those subtrees.

Materialization is deferred work, not duplicate work. The graph nodes created
are the nodes eager execution would already have built. If ambiguity appears
frequently, windows remain short and the implementation converges toward the
ordinary GLR path plus a mode check.

## Materialization boundary

The implementation materializes before operations that need graph topology or
more than one version:

| Trigger | Why graph form is required |
| --- | --- |
| Parse-table cell with multiple actions | The parser may fork or invalidate deterministic parse-state assumptions |
| Reduction crossing the materialized base | The pop must traverse predecessor nodes below the suffix |
| Null-subtree push | Recovery logic observes the discontinuity link |
| Accept | `stack_pop_all` enumerates complete graph paths |
| Pause or recovery | Summaries, error pops, earlier-state search, and version forks need graph history |
| Copy/remove/renumber/swap version | Version ownership is expressed through materialized head nodes |
| Merge or merge probe that mutates versions | Alternate predecessor links must be added to a real node |
| Generic pop, pop-all, pop-error, or summary traversal | These operations enumerate predecessor topology |
| Stack dot graph or parser logger | Diagnostics must show the actual graph representation |
| Stack clear/delete | Remaining entry ownership must be released or materialized consistently |

The stack API follows a conservative rule: any operation not explicitly
implemented against the window materializes first. This keeps the generalized
code path unchanged and makes missing window support fail toward correctness.

## Re-entry

`stack_clear` starts parsing with one active base version and enables an empty
window.

After ordinary parsing, ambiguity, or recovery, `parser_condense_stack` may
remove or merge versions. It calls `stack_try_enable_window` when:

```text
there is exactly one head
the head is Active
the head state is not ERROR_STATE
logging and dot-graph output are disabled
```

Re-entry starts with an empty window above the surviving materialized head. It
does not attempt to convert an existing linear graph tail back into entries.
The benefit begins with the next shift or eligible reduction.

## Ownership invariants

1. `StackHead.node` always owns the materialized base/top graph node.
2. Every non-null window entry owns exactly one subtree reference.
3. Appending an entry moves ownership in; it does not retain.
4. A successful window pop moves ownership into the returned child array; it
   does not retain or release.
5. Materialization moves ownership into `StackLink[0]`; it does not retain or
   release.
6. Clearing or deleting an unmaterialized window releases every remaining
   non-null subtree exactly once.
7. The window is never enabled with multiple heads.
8. Generalized stack operations never observe a nonempty window; they
   materialize first.
9. Cached head fields always describe the logical top, regardless of physical
   representation.
10. A failed straddling pop leaves the window and all ownership unchanged.

These invariants are part of subtree-arena GC root enumeration. While the
window is nonempty, each entry is an owning subtree root and must be visited or
rewritten by any moving subtree collector.

## State machine

```text
                         ambiguity / recovery /
                         graph traversal / logging
       +------------------------------------------------+
       |                                                v
+-------------------+   eligible shift/reduce   +-------------------+
| window enabled    | ------------------------> | window enabled    |
| empty suffix      |                           | nonempty suffix   |
+-------------------+                           +-------------------+
       ^                                                |
       | one active non-error version                   | materialize
       | after condensation                             v
       +---------------------------------------- +-------------------+
                                                | ordinary GLR GSS  |
                                                | window disabled   |
                                                +-------------------+
```

The generalized state may itself be a physically linear graph. The runtime
does not retroactively compress it. It only enables a new empty suffix when the
logical version conditions are safe.

## Code map

| Responsibility | Location |
| --- | --- |
| `WindowEntry`, head cache, mode state | `lib/src_rust/stack.rs` |
| Append and cumulative arithmetic | `stack_push` in `stack.rs` |
| Move-semantics suffix pop | `stack_pop_count_from_window` in `stack.rs` |
| Field-equivalent conversion | `Stack::materialize_window` in `stack.rs` |
| Re-entry after condensation | `stack_try_enable_window` and `parser_condense_stack` |
| Deterministic reduction selection | `parser_reduce` in `parser/actions.rs` |
| Multi-action and diagnostic triggers | `parser/advance.rs` |
| Recovery triggers | `parser/recovery.rs` |
| Dot-graph materialization | `stack/debug.rs` |
| Allocation/ownership narrative | `docs/src/5-runtime-memory.md` |

## Measured result

A paired same-session perf gate compared the implemented window with the same
build with window entry disabled. It used the seven default languages, five
repetitions, and a 200 ms minimum fixture sample time.

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

This was an implementation gate rather than a publication-quality benchmark;
some fixture samples exceeded the five-percent coefficient-of-variation
threshold. Every language-level result was positive and the overall effect was
larger than observed pair noise.

## Validation contract

Any change to the window must preserve:

- byte-identical parse trees across the full fixture/parity suite;
- identical state, position, error cost, node count, and dynamic precedence
  before and after forced materialization;
- equivalent extra-child counting and order;
- exact retain/release balance under shift, pop, materialize, clear, recovery,
  and deletion;
- generalized Go and recovery-heavy Ruby behavior;
- parser logging and dot-graph behavior after forced materialization;
- public ABI, because the stack remains private; and
- no material per-language regression under the paired perf gate.

Useful focused tests should force:

1. an empty-window shift followed by materialization;
2. a multi-entry shift chain;
3. unary and multi-child reductions entirely inside the window;
4. reductions containing extras;
5. a reduction straddling the materialized base;
6. a multi-action table cell immediately after a long window;
7. null pushes and recovery;
8. condensation and re-entry;
9. clear/delete with an unmaterialized suffix; and
10. a subtree collector pass while window entries are live.

## Interaction with subtree representation work

The deterministic-window algorithm is independent of whether `Subtree` is an
inline-or-pointer union, an inline-or-index union, or a uniform node index. Its
requirements are:

- `WindowEntry.subtree` remains a small copyable owning handle;
- ownership can move without an implicit retain/release;
- cumulative subtree queries remain available when appending an entry; and
- moving subtree GC registers the complete window as roots.

A four-byte uniform subtree index may shrink `WindowEntry`, subject to normal
alignment. An arena-aware subtree representation may require the stack to carry
arena context for cumulative-field calculation and release, but it must not
change the window state machine or materialization equivalence.

## Change-control rules

The following are separate experiments and must not be folded into routine
window maintenance:

- multi-version windows;
- rematerializing graph tails back into entries;
- compacting `StackNode` alternate-link storage;
- deferring subtree construction;
- changing recovery to operate directly on the window; and
- selecting window entry based on a run-length heuristic.

Each reopens a different correctness or performance dimension. The retained
linear fast path should remain the small, conservative layer described here.
