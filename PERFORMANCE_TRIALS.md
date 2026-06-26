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

## Kept Trials

Subtree/array helpers: avoid slice creation for subtree child access; inline hot
array helpers.

Lexer/token helpers: compare lexer modes without `memcmp`; delay token reuse
mode checks; skip progress state updates without callback; avoid slice creation
for lexer range access; fast path single lexer range reset; use direct lexer EOF
checks internally.

Reduce/storage: fast path linear stack pops; direct nonterminal next-state lookup
in reduce; add arena-backed tree storage foundation; allocate parser reduction
nodes in tree arena; parser-owned stack-pop builder for fresh reductions.

Pending-reduction foundation: stack-link payload abstraction; descriptor-capable
stack payload layout; pending descriptor metadata dispatch; parser-owned pending
descriptor storage; pending descriptor metadata construction; stack push API for
pending reduction descriptors.

## Measurement Trials

Profiling/counters: cross-language reduce-construction profiling; refreshed C++
`rule.cc` flamegraph; C++ `marker-index.h` flamegraph; fresh-reduce candidate
shape counters; lexer/runtime boundary counters; reduce push/pop shape counters;
pending materialization pressure counters; pending reduction lifetime counters.

## Closed Trials

Summarization: broad metadata caching in `ts_subtree_summarize_children`;
single-child summarizer fast path; alias-sequence condition reorder; specialized
no-alias non-error summarizer; raw-pointer summarizer loop; combine arena copy
with summary calculation; builder-specific copy plus summary finalization; skip
summarize for zero-child non-error nodes.

Stack pop / reduce control: smaller stack-pop reserve count; specialized graph
walk without callback; guard no-op subtree-array reversals; direct graph builder
collection; direct linear reduce pop into parser scratch storage; stack-pop
trailing-extra split before parent construction; direct merged-candidate
descriptor comparison; single-group reduce control-flow split; direct arena
finalization for linear fresh reductions; one-pass final-storage linear
collection; guard halted-version scans in reduce; guard zero dynamic-precedence
writes; hoist reduce nonterminal check.

Allocation/storage: arena-backed heap leaves during lexing; 16-bit symbol inline
leaf encoding; pool-backed zero-child node allocation; increase
`TS_MAX_TREE_POOL_SIZE`; global mutex slab for subtree blocks; atomic global slab
for subtree blocks; parser free lists for 1-4 child blocks; use `ts_malloc`
instead of `ts_realloc(NULL)`; increase tree arena page size; adopt stack-pop
child arrays into tree arena; embedded adopted-block headers.

Refcount: relaxed/release-acquire refcount ordering; `#[inline]` on
`ts_subtree_retain`; refcount-one direct release fast path.

Lexer/token path: passing `is_leaf` into shift; direct `as u8` casts in leaf
creation; ASCII fast path in lexer lookahead; direct UTF-8 decode path;
single-range lexer advance fast path; no-log lexer advance callback
specialization; pointer equality for stack merge external tokens; same-token
external-token set fast path; pointer equality in external scanner state
equality.

Parse table / parser state / stack helpers: terminal-only table-entry helper;
broad language table-entry inlining; caching `language_is_wasm`; broad stack
getter/push inlining; increasing `MAX_NODE_POOL_SIZE`.

Balancing/compression: skip/deferring all balancing; propagated
contains-repetition balance flag; single-pass repeat compression schedule.

Benchmark scope: reset benchmark allocator.

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
- Do not edit benchmark source code.
- Use `cargo test --all` outside the sandbox for kept production code.
- Commit each kept optimization separately.
- Push after every 10 additional commits unless told otherwise.
- Add one reflection after every 10 unique itemized performance attempts.

## Acceptance Gate

Run `cargo xtask benchmark --kind normal -r 10 --language <lang>` for all target
languages, then `cargo test --all` outside the sandbox.
