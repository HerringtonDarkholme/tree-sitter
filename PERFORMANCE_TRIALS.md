# Parser Performance Trail

This file tracks raw normal parsing performance work for the Rust runtime.

Target languages: TypeScript, JavaScript, Python, Go, Rust, C++, Java.

Benchmark source files must not be changed. Profiling helpers may live outside
the repo under `/tmp`.

## Current Status

The 20% universal target is not met.

Kept architectural gains so far:

| Change | Net result |
| --- | --- |
| Arena-backed reduction parent allocation | About `+5.3%` mean across the seven target languages |
| Parser-owned stack-pop builder for fresh reductions | About `+7.3%` mean on top of the arena slice |
| Stack-link payload abstraction | Behavior-preserving foundation for pending reductions |
| Descriptor-capable stack payload layout | Future pending descriptor pointer fits without growing `StackLink` or `StackNode` |

Current primary bottleneck:

```text
ts_parser__advance
  -> ts_parser__reduce
     -> stack pop / child collection
     -> parent allocation and child copy
     -> child summarization
     -> stack push / merge
```

Current direction: pending-reduction descriptor forest. The parser repeatedly
materializes internal parent nodes that are soon consumed by later reductions.
The next real optimization should avoid hot-loop parent materialization and
materialize only at explicit tree-output or unsupported-operation boundaries.

SIMD is not the current primary target. The reusable runtime does not own a
large contiguous scan loop in the hot path; generated lexers and external
scanners dominate most lexer-side samples.

## Key Lessons

- Allocation count alone is not predictive. Several allocation-reduction trials
  regressed due to locality, locking, metadata, or branch costs.
- Reduce work must remove a whole phase, not add another local fast path.
- Merged-candidate selection is too rare in normal fresh parsing to be the main
  target.
- Trailing extras are too rare for batching them to be the main win.
- External scanner metadata is the hard part for pending descriptors.
- C++ and JavaScript still have large lexer shares, so reduce-only work may not
  be sufficient for the full universal 20%.
- Repeated small fast paths in closed areas have mostly regressed.

## Itemized Trial Index

Keep one row per unique trial. Do not duplicate reruns.

### Kept

| Trial | Area | Summary |
| --- | --- | --- |
| Avoid slice creation for subtree child access | Subtree access | Kept |
| Compare lexer modes without `memcmp` | Token reuse | Kept |
| Delay token reuse mode checks | Token reuse | Kept |
| Inline hot array helpers | Array helpers | Kept |
| Skip progress state updates without callback | Progress checks | Kept |
| Avoid slice creation for lexer range access | Lexer ranges | Kept |
| Fast path single lexer range reset | Lexer ranges | Kept |
| Use direct lexer EOF checks internally | Lexer EOF | Kept |
| Fast path linear stack pops | Stack pop | Kept |
| Direct nonterminal next-state lookup in reduce | Reduce lookup | Kept on JS/Go/TS canaries |
| Add arena-backed tree storage foundation | Tree storage | Kept foundation |
| Allocate parser reduction nodes in tree arena | Reduce allocation | Kept, broad positive |
| Parser-owned stack-pop builder for fresh reductions | Reduce stack pop | Kept, broad positive; reparses use old slice path |
| Stack-link payload abstraction | Stack foundation | Kept pending-reduction foundation |
| Descriptor-capable stack payload layout | Stack foundation | Kept; layout unchanged for hot stack nodes |
| Pending descriptor metadata dispatch | Stack foundation | Kept; stack metadata helpers can read pending descriptor fields without materialization |

### Measurement And Design

| Trial | Area | Summary |
| --- | --- | --- |
| Cross-language reduce-construction profiling | Profiling | Reduce remained the largest shared parser-owned target |
| Refreshed C++ `rule.cc` flamegraph | Profiling | C++ has co-dominant reduce and lexer costs |
| C++ `marker-index.h` flamegraph | Profiling | Confirmed reduce plus lexer split for C++ |
| Fresh-reduce candidate shape counters | Reduce shape | Normal fresh parsing is overwhelmingly single-candidate |
| Lexer/runtime boundary counters | Lexer boundary | Included ranges/chunking are not useful normal-case targets |
| Reduce push/pop shape counters | Reduce shape | Internal subtree churn is high; trailing extras are not |
| Pending materialization pressure counters | Pending reduce | Full tree comparison is rare; external metadata matters |
| Pending reduction lifetime counters | Pending reduce | Internal parents are almost immediately consumed by later reductions |

### Rejected Or Closed

| Trial | Area | Summary |
| --- | --- | --- |
| Broad metadata caching in `ts_subtree_summarize_children` | Summarize | Regressed |
| Single-child summarizer fast path | Summarize | Flat/negative |
| Alias-sequence condition reorder | Summarize | Regressed |
| Specialized no-alias non-error summarizer | Summarize | Below baseline |
| Raw-pointer summarizer loop | Summarize | Regressed JS/TS/Go/Python |
| Combine arena copy with summary calculation | Reduce/summarize | Regressed JavaScript |
| Builder-specific copy plus summary finalization | Reduce/summarize | Mixed; Python regressed |
| Smaller stack-pop reserve count | Stack pop | Large regression |
| Specialized graph walk without callback | Stack pop | Mixed; JS regressed |
| Guard no-op subtree-array reversals | Stack pop | Below baseline |
| Direct graph builder collection | Stack pop | Go regressed |
| Direct linear reduce pop into parser scratch storage | Stack pop | Abandoned as near-repeat |
| Stack-pop trailing-extra split before parent construction | Stack pop | Abandoned as low leverage |
| Direct merged-candidate descriptor comparison | Candidate selection | Mixed; TypeScript aggregate regressed |
| Single-group reduce control-flow split | Reduce finalization | JavaScript regressed |
| Direct arena finalization for linear fresh reductions | Reduce construction | TypeScript improved, JavaScript regressed |
| One-pass final-storage linear collection | Reduce construction | Mixed; Go/Rust/Java regressed |
| Guard halted-version scans in reduce | Reduce control | JavaScript regressed |
| Guard zero dynamic-precedence writes | Reduce control | Below baseline |
| Hoist reduce nonterminal check | Reduce control | Below baseline |
| Passing `is_leaf` into shift | Shift | Regressed |
| Direct `as u8` casts in leaf creation | Leaf construction | Regressed JS/Go |
| Arena-backed heap leaves during lexing | Leaf allocation | Helped some, regressed Go/Rust |
| 16-bit symbol inline leaf encoding | Leaf representation | Regressed |
| Pool-backed zero-child node allocation | Node allocation | Allocation counts unchanged; benchmarks regressed |
| Increase `TS_MAX_TREE_POOL_SIZE` | Node pool | Allocation counts unchanged; slower/noisier |
| Global mutex slab for subtree blocks | Allocation | Stalled/regressed |
| Atomic global slab for subtree blocks | Allocation | Stalled/fragile |
| Parser free lists for 1-4 child blocks | Allocation | Fewer allocations, slower benchmarks |
| Use `ts_malloc` instead of `ts_realloc(NULL)` | Allocation | Regressed |
| Increase tree arena page size | Arena layout | JavaScript regressed |
| Adopt stack-pop child arrays into tree arena | Arena ownership | JS flat, TypeScript regressed |
| Embedded adopted-block headers | Arena ownership | Mixed; not universal |
| Relaxed/release-acquire refcount ordering | Refcount | Failed twice |
| `#[inline]` on `ts_subtree_retain` | Refcount | Regressed |
| Refcount-one direct release fast path | Refcount | Regressed |
| ASCII fast path in lexer lookahead | Lexer decode | Neutral/negative |
| Direct UTF-8 decode path | Lexer decode | Mixed/negative |
| Single-range lexer advance fast path | Lexer advance | Regressed |
| No-log lexer advance callback specialization | Lexer callback | Mixed; worst-file regression |
| Terminal-only table-entry helper | Parse table | Below baseline |
| Broad language table-entry inlining | Parse table | Regressed |
| Caching `language_is_wasm` | Parser state | Regressed |
| Broad stack getter/push inlining | Stack helpers | Regressed |
| Increasing `MAX_NODE_POOL_SIZE` | Stack node pool | Regressed |
| Pointer equality for stack merge external tokens | Stack merge | Below baseline |
| Same-token external-token set fast path | External token | Below baseline |
| Pointer equality in external scanner state equality | External state | Below baseline |
| Skip summarize for zero-child non-error nodes | Node construction | Below baseline |
| Skip/deferring all balancing | Balance/compress | JS improved, TypeScript regressed badly |
| Propagated contains-repetition balance flag | Balance/compress | Rust regressed |
| Single-pass repeat compression schedule | Balance/compress | Did not improve Rust |
| Reset benchmark allocator | Benchmark harness | Removed; benchmark source changes are out of scope |

## Reflections

### Reflection 1: Arena And Allocation

Allocation reductions helped only when they improved ownership and locality.
Arena-backed reduction parents worked; global pools, larger pages, leaf arena
allocation, and refcount tweaks did not produce universal wins.

### Reflection 2: Reduce Protocol

The parser-owned stack-pop builder was the last broad reduce win. Later local
reduce changes failed because they removed only one symptom while leaving stack
collection, parent creation, child copying, summarization, and stack push as
separate phases. Future reduce work must remove a full phase.

## Next Direction

Highest priority: pending-reduction descriptor forest.

Required shape:

- Stack payloads can hold materialized `Subtree` or pending reduction descriptor.
- Hot stack metadata queries must read descriptor metadata without materializing.
- Fresh reduce can push descriptors for selected parents.
- Later reduce can consume pending children without converting them first.
- Materialization happens at final tree output, public child iteration, mutable
  subtree access, tree comparison, reparsing, or unsupported metadata queries.

Reject this direction if ordinary reduce pop, stack push, stack merge, or stack
metadata updates force materialization.

Secondary directions:

- Lexer/runtime boundary work only if profiles isolate reusable runtime cost,
  not generated `ts_lex` or external scanner bodies.
- Balance/compress redesign only with tree-shape evidence; do not tune schedules
  or add branch-pruning metadata again.
- Summary computation only after the reduce ownership protocol changes.

## Process Rules

- Always check this history before starting a new performance trial.
- Do not edit benchmark source code.
- Do not use `cargo check` as validation.
- Use `cargo test --all` for kept production code, outside the sandbox.
- Commit each kept optimization separately.
- Push after every 10 additional commits unless explicitly asked otherwise.
- Write one reflection after every 10 unique itemized performance attempts.

Counting reflection attempts:

- Count kept, rejected, abandoned-before-benchmark, and measurement/design
  trials if they add a new itemized row.
- Do not count benchmark reruns, baseline reruns, formatting fixes, or commits
  that only support the same trial.
- If a trial is split across multiple commits, count it once unless a later
  commit tests a materially different hypothesis.

## Acceptance Gate

For the current universal target, a kept performance optimization needs:

```sh
cargo xtask benchmark --kind normal -r 10 --language typescript
cargo xtask benchmark --kind normal -r 10 --language javascript
cargo xtask benchmark --kind normal -r 10 --language python
cargo xtask benchmark --kind normal -r 10 --language go
cargo xtask benchmark --kind normal -r 10 --language rust
cargo xtask benchmark --kind normal -r 10 --language cpp
cargo xtask benchmark --kind normal -r 10 --language java
cargo test --all
```

Validation must run outside the sandbox for kept production code.
