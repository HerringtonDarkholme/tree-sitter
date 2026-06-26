# Parser Performance Trail

Compact history for raw normal parsing performance work in the Rust runtime.

Target languages: TypeScript, JavaScript, Python, Go, Rust, C++, Java.

## Status

- Universal 20% target: not met.
- Best kept gains: arena-backed reduction parents and parser-owned fresh
  reduction stack-pop builder.
- Current direction: pending-reduction descriptor forest.
- Avoid for now: small local fast paths, refcount-order tweaks, node-pool
  tuning, benchmark-harness edits, and SIMD without a reusable-runtime scan
  loop profile.

## Bottleneck

```text
ts_parser__advance -> ts_parser__reduce
  -> stack pop / child collection
  -> parent allocation and child copy
  -> child summarization
  -> stack push / merge
```

## Itemized Trial Index

### Kept

- Avoid slice creation for subtree child access
- Inline hot array helpers
- Compare lexer modes without `memcmp`
- Delay token reuse mode checks
- Skip progress state updates without callback
- Avoid slice creation for lexer range access
- Fast path single lexer range reset
- Use direct lexer EOF checks internally
- Fast path linear stack pops
- Direct nonterminal next-state lookup in reduce
- Add arena-backed tree storage foundation
- Allocate parser reduction nodes in tree arena
- Parser-owned stack-pop builder for fresh reductions
- Stack-link payload abstraction
- Descriptor-capable stack payload layout
- Pending descriptor metadata dispatch
- Parser-owned pending descriptor storage
- Pending descriptor metadata construction
- Stack push API for pending reduction descriptors
- Descriptor-aware stack-pop collection primitive
- Payload span access/release for reduce wiring
- Pending descriptor payload-child ownership and summary

### Measurement

- Cross-language reduce-construction profiling
- Refreshed C++ `rule.cc` flamegraph
- C++ `marker-index.h` flamegraph
- Fresh-reduce candidate shape counters
- Lexer/runtime boundary counters
- Reduce push/pop shape counters
- Pending materialization pressure counters
- Pending reduction lifetime counters

### Closed: Summarization

- Broad metadata caching in `ts_subtree_summarize_children`
- Single-child summarizer fast path
- Alias-sequence condition reorder
- Specialized no-alias non-error summarizer
- Raw-pointer summarizer loop
- Combine arena copy with summary calculation
- Builder-specific copy plus summary finalization
- Skip summarize for zero-child non-error nodes

### Closed: Stack Pop And Reduce Control

- Smaller stack-pop reserve count
- Specialized graph walk without callback
- Guard no-op subtree-array reversals
- Direct graph builder collection
- Direct linear reduce pop into parser scratch storage
- Stack-pop trailing-extra split before parent construction
- Direct merged-candidate descriptor comparison
- Single-group reduce control-flow split
- Direct arena finalization for linear fresh reductions
- One-pass final-storage linear collection
- Guard halted-version scans in reduce
- Guard zero dynamic-precedence writes
- Hoist reduce nonterminal check

### Closed: Allocation And Storage

- Arena-backed heap leaves during lexing
- 16-bit symbol inline leaf encoding
- Pool-backed zero-child node allocation
- Increase `TS_MAX_TREE_POOL_SIZE`
- Global mutex slab for subtree blocks
- Atomic global slab for subtree blocks
- Parser free lists for 1-4 child blocks
- Use `ts_malloc` instead of `ts_realloc(NULL)`
- Increase tree arena page size
- Adopt stack-pop child arrays into tree arena
- Embedded adopted-block headers

### Closed: Refcount

- Relaxed/release-acquire refcount ordering
- `#[inline]` on `ts_subtree_retain`
- Refcount-one direct release fast path

### Closed: Lexer And Token Path

- Passing `is_leaf` into shift
- Direct `as u8` casts in leaf creation
- ASCII fast path in lexer lookahead
- Direct UTF-8 decode path
- Single-range lexer advance fast path
- No-log lexer advance callback specialization
- Pointer equality for stack merge external tokens
- Same-token external-token set fast path
- Pointer equality in external scanner state equality

### Closed: Parse Table And Stack Helpers

- Terminal-only table-entry helper
- Broad language table-entry inlining
- Caching `language_is_wasm`
- Broad stack getter/push inlining
- Increasing `MAX_NODE_POOL_SIZE`

### Closed: Balancing And Benchmark Scope

- Skip/deferring all balancing
- Propagated contains-repetition balance flag
- Single-pass repeat compression schedule
- Reset benchmark allocator

## Reflections

1. Allocation work helped only when it improved ownership and locality. Pools,
   larger pages, leaf arenas, and refcount tuning did not generalize.
2. Local reduce fast paths are exhausted. Future reduce work must remove a full
   phase, not just make one branch cheaper.
3. Lexer work needs profile proof that reusable runtime code is the bottleneck;
   generated lexers and external scanners often dominate lexer samples.

## Next Direction

Pending-reduction descriptor forest: stack payloads hold materialized `Subtree`
or pending descriptors; hot metadata queries read descriptors; fresh reductions
push descriptors; later reductions consume descriptor children without forced
conversion; materialization happens only at tree output, reparsing, comparison,
mutation, public child iteration, or unsupported metadata boundaries.

Reject this direction if normal reduce pop, stack push, stack merge, or stack
metadata updates force broad materialization.

## Process Rules

- Check this file before every new performance trial.
- Closed trials may be revisited when the hypothesis changes, profiles change,
  or architecture changes make the old result obsolete.
- Do not edit benchmark source code.
- Use `cargo test --all` outside the sandbox for kept production code.
- Commit each kept optimization separately.
- Push after every 10 additional commits unless told otherwise.
- Add one reflection after every 10 unique itemized performance attempts.

## Acceptance Gate

Run `cargo xtask benchmark --kind normal -r 10 --language <lang>` for all target
languages, then `cargo test --all` outside the sandbox.
