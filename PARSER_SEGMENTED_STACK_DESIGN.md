# Segmented Parser Stack Design

Implementation design for the next parser performance architecture trial.

Target: improve raw normal parsing throughput for TypeScript, JavaScript,
Python, Go, Rust, C++, and Java by removing the persistent stack-node path from
straight-line parsing while preserving full GLR behavior for branching,
recovery, and incremental parsing.

This is not a trial result. It is the implementation boundary for the next
large code attempt.

## Why This Direction

The latest counters showed that most child collection is already linear:

- TypeScript, JavaScript, Python, Rust, C++, and Java almost always reduce from
  a one-link path.
- Rust stayed at one active version for the measured normal fixtures.
- Go is the hard case: 43.5% of reductions happened while multiple versions
  existed, 8.3% of pops needed graph fallback, and merge attempts were frequent.

Therefore, replacing only `ts_stack_pop_count_into` is not enough. The win must
come from bypassing the whole persistent graph path for straight segments:

- no `StackNode` allocation per shift/reduce push
- no one-link pointer chasing for normal reductions
- less subtree/payload retain/release churn for transient stack entries
- fewer version head updates for straight-line state changes
- cheaper access to top-of-stack state, position, error cost, dynamic
  precedence, and node count

Go means the design must handle branching as a first-class case. A
single-version-only fast path is not a universal plan.

## Current Stack Model

Current `Stack` state:

```text
Stack
  heads: Array<StackHead>
  slices: Array<StackSlice>
  iterators: Array<StackIterator>
  node_pool: Array<*mut StackNode>
  base_node: *mut StackNode
  subtree_pool: *mut SubtreePool

StackHead
  node: *mut StackNode
  summary: *mut StackSummary
  node_count_at_last_error: u32
  last_external_token: Subtree
  lookahead_when_paused: Subtree
  status: Active | Paused | Halted

StackNode
  state: TSStateId
  position: Length
  links: [StackLink; 8]
  link_count: u16
  ref_count: u32
  error_cost: u32
  node_count: u32
  dynamic_precedence: i32

StackLink
  node: *mut StackNode
  payload: StackLinkPayload
```

Each push creates a new `StackNode` with one link back to the previous node.
Reductions walk backward through links, retain payloads into a builder, create
or reuse a version at the predecessor node, then push the parent as another
node.

## Proposed Model

Add a segment-backed representation beside the current graph:

```text
Stack
  heads: Array<StackHead>
  segments: Array<StackSegment>
  frames: Array<StackFrame>
  graph: existing StackNode graph fields

StackHead
  repr: Graph(node) | Segment(segment_id, top)
  summary
  node_count_at_last_error
  last_external_token
  lookahead_when_paused
  status

StackSegment
  base: StackBase
  start: u32
  len: u32
  ref_count: u32
  sealed: bool

StackFrame
  payload: StackLinkPayload
  state: TSStateId
  position: Length
  error_cost: u32
  node_count: u32
  dynamic_precedence: i32
  flags: u8

StackBase
  Graph(*mut StackNode)
  Segment(segment_id, top)
```

`StackFrame` stores the metadata that is currently cached on `StackNode` after
following a link. This makes top-of-stack queries and linear pops direct array
accesses.

Segments are append-only while exclusively owned. When a version forks, the
segment is sealed and subsequent pushes append to a new segment that references
the sealed prefix as its base. Versions can share sealed prefixes without
copying frames.

## Representation Rules

- Graph representation remains the source of truth for old-tree parsing,
  recovery, and hard ambiguity until each boundary is implemented.
- Segment representation is enabled only for fresh no-old-tree normal parsing.
- Segment frames retain payload ownership exactly like graph links until a
  later explicit ownership trial proves stack-owned internal arena nodes can
  avoid refcounts.
- Segment prefixes are immutable once shared.
- A segment head can be promoted to the graph at any point. Promotion must
  produce a graph path equivalent to the segment path, preserving payload
  ordering, metadata, last external token, summary behavior, and status.

## API Mapping

### Direct Segment Operations

These can operate directly on segment heads:

- `ts_stack_state`: read `frames[top - 1].state`, or base state at bottom.
- `ts_stack_position`: read `frames[top - 1].position`, or base position.
- `ts_stack_error_cost`: read frame/head error metadata plus paused/error
  adjustment.
- `ts_stack_dynamic_precedence`: read frame metadata.
- `ts_stack_node_count_since_error`: read frame `node_count` and head
  `node_count_at_last_error`.
- `ts_stack_last_external_token` and `ts_stack_set_last_external_token`: remain
  `StackHead` fields.
- `ts_stack_is_active`, `ts_stack_is_halted`, `ts_stack_is_paused`: remain
  `StackHead` status checks.
- `ts_stack_push`: append one frame when the head segment is writable.
- `ts_stack_push_pending_reduction`: append one frame carrying a pending
  reduction payload.

### Segment Pop Operations

These should be implemented before changing parser code:

- `ts_stack_pop_count_into`: if the requested count is within the current
  segment chain and no hard graph boundary is crossed, append children directly
  from frames into `StackPopBuilder` and move/create the result version at the
  predecessor segment position.
- `ts_stack_pop_count`: same logic but returning `StackSliceArray`.
- `ts_stack_pop_count_payloads_into`: same logic for payload descriptors.

If a pop crosses a graph base, a multi-link graph node, or a not-yet-supported
boundary, promote and fall back to current graph traversal.

### Promotion-First Operations

These should initially promote segment heads to graph representation before
using the existing implementation:

- `ts_stack_pop_error`
- `ts_stack_pop_pending`
- `ts_stack_pop_all`
- `ts_stack_record_summary`
- `ts_stack_get_summary` after invalidation
- `ts_stack_merge`
- `ts_stack_can_merge`
- `ts_stack_copy_version` when the source segment is writable and would become
  shared
- `ts_stack_remove_version` for shared segment release in the first slice, if
  release logic is not complete
- `ts_stack_renumber_version`
- `ts_stack_swap_versions`
- `ts_stack_pause`
- `ts_stack_resume`
- `ts_stack_print_dot_graph`

This is conservative, but it makes the first implementation slice smaller and
keeps correctness boundaries explicit.

## Promotion Algorithm

Promotion converts a segment chain to graph nodes from oldest base to newest
frame:

```text
promote(head):
  if head is already graph:
    return node

  base_node = promote(head.segment.base)
  for each frame from base to head.top:
    node = stack_node_new_with_payload(base_node, frame.payload, frame.state)
    overwrite node metadata from frame if needed
    base_node = node
  head.repr = Graph(base_node)
  release segment reference
  return base_node
```

Important details:

- Payloads must be retained for graph links when copied from segment frames.
- Segment frame payloads must be released when the segment is freed.
- Metadata should be computed once on frame append and then reused. If graph
  node construction recomputes identical metadata, verify equivalence before
  using direct overwrite.
- `node_count_at_last_error`, `last_external_token`, `lookahead_when_paused`,
  and `status` remain head-level fields and must not change during promotion.

## Segment Push Metadata

Appending a frame mirrors `stack_node_new_with_payload`:

```text
new.position = previous.position
new.error_cost = previous.error_cost
new.dynamic_precedence = previous.dynamic_precedence
new.node_count = previous.node_count
new.state = next_state

if payload is non-null:
  new.error_cost += payload_error_cost
  new.position += payload_total_size
  new.node_count += payload_node_count
  new.dynamic_precedence += payload_dynamic_precedence
```

For null payloads, preserve the existing behavior where pushing a null subtree
updates `node_count_at_last_error`.

## Version/Fork Model

Current code often creates new versions during pop, not explicit copy:

- `stack_pop_builder_add_slice` creates a version at the predecessor node when
  a reduction pops from a path.
- `ts_stack_renumber_version` then often moves the new version into the current
  version slot.
- `ts_stack_merge` tries to fold equivalent versions after pushing a parent.

Segment implementation must preserve this behavior:

- A segment pop returns a `StackSliceSpan` with a version whose head points to
  the predecessor segment position.
- If the predecessor position is within the same unshared segment, do not copy
  frames; create a new head referencing the same segment and top index.
- If the current version later gets removed or renumbered, segment refcounts
  release correctly.
- If two segment versions need merge comparison, first implementation promotes
  both to graph and uses current merge. A later optimization can compare segment
  suffixes directly.

## Correctness Boundaries

Do not cross these boundaries in the first implementation slice:

- Old-tree parsing and reusable-node interaction.
- Error recovery pop/error/pending/all traversal.
- Stack summaries used by recovery.
- Direct segment merge.
- Direct segment DOT output.
- Refcount elision for arena-owned internal subtrees.
- Lazy descriptor materialization.

The first slice should be a representation change for straight normal stack
segments only. It should not also change tree materialization semantics.

## Expected Coverage

Based on normal `-r 1` counters:

- Rust should stay entirely in segments.
- Python should almost entirely stay in segments.
- TypeScript and JavaScript should mostly stay in segments with occasional
  promotion for branch/merge boundaries.
- C++ should benefit for straight segments, but multi-version reductions mean
  promotion frequency must be measured carefully.
- Java should be similar to C++ at smaller scale.
- Go must not promote the whole parse on first fork; it needs shared segment
  prefixes or it will lose the universal target.

## First Implementation Slice

1. Add segment/frame data structures without enabling them by default.
2. Add create/delete/clear/release logic and layout assertions.
3. Add `StackHead` representation tag and helpers, but keep all heads graph by
   default.
4. Add promotion from segment head to graph head and unit-test it through
   existing stack APIs.
5. Enable segment push/pop only behind an internal parser flag for fresh
   no-old-tree parses.
6. Promote on unsupported operations. The first target is correctness, not
   maximal coverage.
7. Run `cargo test --all` outside the sandbox.
8. Run normal `-r 10` benchmarks for the seven target languages.

## Rejection Criteria

Reject or redesign this path if:

- Promotion happens so often on Go that Go regresses or cannot plausibly reach
  the universal target.
- Segment refcount/release code becomes as expensive as graph node release.
- The parser spends most remaining time in tree construction or generated lexer
  after stack node removal, leaving no path to 20% without combining with lazy
  forest or lexer-contract work.
- Correctness requires materializing graph nodes at every reduction boundary.

## Follow-Up Directions If First Slice Works

- Direct segment merge comparison for shared prefixes.
- Direct segment summaries for recovery without graph promotion.
- Combine segment frames with stack-native pending reductions to avoid concrete
  tree construction during reductions.
- Remove transient retain/release for parser-owned arena internal nodes only
  after ownership is proven separately.
