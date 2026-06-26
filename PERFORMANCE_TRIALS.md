# Parser Performance Trail

Compact history for raw normal parsing performance work in the Rust runtime.

Target languages: TypeScript, JavaScript, Python, Go, Rust, C++, Java.

## Status

- Universal 20% target: not met.
- Best kept gains: arena-backed reduction parents and parser-owned fresh
  reduction stack-pop builder.
- Current direction: architecture investigation before more code trials. The
  next attempt must remove a hot phase from normal parsing, not add another
  partial fast path.
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

Normal parsing repeatedly crosses four expensive boundaries:

- Persistent stack graph traversal collects children from backward links.
- Concrete child arrays are formed and retained before each parent exists.
- Every reduction eagerly allocates `SubtreeHeapData`, copies children into the
  tree arena, and summarizes child metadata immediately.
- The concrete parent is pushed back into the graph and then participates in
  version merge/recovery/accept logic.

The C++ profile also has a separate generated-lexer cost center. Runtime-only
parser work cannot reclaim that unless it avoids lexer invocations or changes
the generated lexer/runtime contract.

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
- Pending descriptor recursive materialization boundary
- Payload-aware stack graph traversal primitive

### Measurement

- Cross-language reduce-construction profiling
- Refreshed C++ `rule.cc` flamegraph
- C++ `marker-index.h` flamegraph
- Fresh-reduce candidate shape counters
- Lexer/runtime boundary counters
- Reduce push/pop shape counters
- Pending materialization pressure counters
- Pending reduction lifetime counters
- Payload-child foundation `-r 10` checkpoint
- C++ normal `cargo flamegraph` high-sample checkpoint
- Stack-node link-count distribution probe
- Descriptor lazy-candidate pressure counters
- Linear-stack architecture coverage counters

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
- Broad descriptor reduce/accept stack traversal wiring
- Removing unused descriptor-payload layer after failed wiring
- Payload-aware accept/finalization through reduce builder

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
- Compact one-link stack-node layout with overflow links

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
4. Descriptor foundation code is not itself a measured win. A current-vs-origin
   `-r 10` checkpoint was mixed and noisy, with no universal gain. Continue only
   with counters proving that reduce wiring avoids enough materialization.
5. Broad descriptor wiring is incomplete as an incremental patch. Letting
   pending descriptors enter normal stack traversal exposed concrete-subtree
   assumptions in reduce, recovery, accept, merge, and final materialization.
   This failed before benchmarking with allocator traps and subtree metadata
   panics, so it was backed out instead of being tuned.
6. The descriptor-payload layer cannot be treated as dead code by local diff
   inspection. Removing it after the failed wiring made `cargo test --all`
   abort in `test_tree_cursor_child_for_point` with a misaligned subtree
   pointer, so the layer is entangled with the current stack-payload layout.
   Do not prune it without first simplifying the full stack payload model.
7. Representation-boundary work must be validated one ownership boundary at a
   time. Counted payload traversal passed because it preserved existing reduce
   version semantics; accept/finalization through the reduce builder failed
   because pop-all has different stack-version ownership. Future lazy-reduction
   work must first provide explicit, tested models for reduce, merge/recovery,
   and accept/finalization. Do not route one boundary through another boundary's
   helper just because the payload shape is similar.

## Latest Checkpoint

C++ normal flamegraph, `cargo flamegraph --bench benchmark -p tree-sitter-cli`
with bench debuginfo and 2000 repetitions:

| Frame | Samples |
| --- | ---: |
| `ts_parser__reduce` | 24.7% |
| `ts_lex` | 22.2% |
| `ts_subtree_new_node_in_arena` | 12.0% |
| `ts_subtree_summarize_children` | 9.5% |
| `ts_lex_keywords` | 7.9% |
| `ts_parser__balance_subtree` | 4.2% |
| `ts_stack_renumber_version` | 4.0% |
| `ts_stack_pop_count_into` | 3.7% |

Interpretation: C++ is split across generated lexer/keyword code and reduction
construction. A universal 20% library-only gain is unlikely from reducer local
tuning; it requires removing or deferring a full tree-construction phase, or a
separate generated-lexer strategy outside reusable runtime code.

Stack-node link-count probe, normal `-r 1`, showed mostly one-link nodes:

| Language | One-link | Multi-link |
| --- | ---: | ---: |
| TypeScript | 114783 | 269 |
| JavaScript | 179679 | 1005 |
| Python | 102503 | 14 |
| Go | 111874 | 4832 |
| Rust | 35962 | 0 |
| C++ | 7924 | 14 |
| Java | 2062 | 18 |

Follow-up trial replaced the fixed eight-link inline `StackNode` with one
inline link plus lazy overflow links. `cargo test --all` passed outside the
sandbox, but same-session normal `-r 10` benchmarks were mixed:

| Language | Compact | Baseline | Delta |
| --- | ---: | ---: | ---: |
| TypeScript | 25978 | 26008 | -0.1% |
| JavaScript | 20057 | 19557 | +2.6% |
| Python | 10463 | 10373 | +0.9% |
| Go | 16678 | 18286 | -8.8% |
| Rust | 17400 | 17029 | +2.2% |
| C++ | 7843 | 7924 | -1.0% |
| Java | 12121 | 11870 | +2.1% |

Interpretation: node-size reduction alone does not buy a universal win. The
extra indirection/allocation for branchy nodes regressed Go and did not move
C++/TypeScript. Do not retry this as a local layout split unless multi-link
overflow storage is eliminated or the whole stack representation changes.

Payload-aware stack graph traversal primitive was added without enabling lazy
reductions. `cargo test --all` passed outside the sandbox. Normal `-r 10`
checkpoint was mixed/no-win, as expected for infrastructure not yet on the hot
reduce path:

| Language | Speed |
| --- | ---: |
| TypeScript | 26470 |
| JavaScript | 19903 |
| Python | 9982 |
| Go | 17905 |
| Rust | 17236 |
| C++ | 7804 |
| Java | 12603 |

Interpretation: this does not satisfy the performance target by itself. Its
purpose is to close the correctness gap that made broad descriptor wiring
unsafe: counted stack graph traversal can now collect retained
`StackLinkPayload`s without pretending pending descriptors are concrete
subtrees.

Follow-up accept/finalization wiring through the reduce builder was rejected.
`cargo test --all` outside the sandbox aborted in the HTML corpus with an array
bounds assertion in stack version bookkeeping. This shows payload-aware accept
cannot reuse counted-reduce builder slice semantics as-is; it needs a dedicated
pop-all payload result or stack-version removal model. The unsafe pop-all
payload API was removed; keep only counted payload traversal until that model is
designed.

Descriptor lazy-candidate pressure counters, normal `-r 1`, temporary
instrumentation only:

| Language | Spans | Lazy Candidate | Candidate % | Children | Lazy Children % | Main Blockers |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| TypeScript | 65903 | 58094 | 88.1% | 111261 | 87.2% | fragile, multi-version |
| JavaScript | 102012 | 91341 | 89.5% | 177717 | 88.6% | fragile, multi-version |
| Python | 59977 | 55244 | 92.1% | 101788 | 90.6% | fragile |
| Go | 64540 | 25822 | 40.0% | 114091 | 41.1% | multi-version, multi-pop |
| Rust | 21182 | 18782 | 88.7% | 35098 | 85.4% | fragile |
| C++ | 4592 | 3167 | 69.0% | 7881 | 70.5% | multi-version, fragile |
| Java | 1165 | 886 | 76.1% | 2047 | 77.4% | multi-version, fragile |

Accept multi-path count was zero for all seven target-language normal fixtures.
Interpretation: descriptor/lazy reductions remain plausible for TypeScript,
JavaScript, Python, Rust, and Java, and maybe C++. Go is the universal-risk
case: most Go reductions hit multi-version or multi-pop conditions, so a
single-version-only lazy path cannot deliver a universal 20% gain. The next
descriptor trial must either handle Go's branching path or explicitly prove a
different optimization for Go.

Payload-child foundation versus `origin/master`, normal `-r 10` average speed:

| Language | Current | Origin | Delta |
| --- | ---: | ---: | ---: |
| TypeScript | 24955 | 25292 | -1.3% |
| JavaScript | 20486 | 20884 | -1.9% |
| Python | 10760 | 10950 | -1.7% |
| Go | 18301 | 15740 | +16.3% |
| Rust | 17820 | 18338 | -2.8% |
| C++ | 7779 | 8193 | -5.1% |
| Java | 13334 | 12924 | +3.2% |

Interpretation: not a universal win; likely noise plus code-layout effects.
Do not continue descriptor wiring without first proving a complete ownership and
materialization boundary.

Linear-stack architecture coverage counters, temporary library-only
instrumentation, normal `-r 1`:

| Language | Reductions | Multi-Version | Multi-Slice | Pop Fallback | Max Versions | Merge Attempts | Merge Success |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| TypeScript | 65620 | 3325 (5.1%) | 271 (0.4%) | 271 (0.4%) | 4 | 4920 | 288 |
| JavaScript | 101005 | 3272 (3.2%) | 1007 (1.0%) | 1007 (1.0%) | 4 | 5647 | 1005 |
| Python | 59963 | 887 (1.5%) | 14 (0.0%) | 14 (0.0%) | 2 | 1124 | 28 |
| Go | 59524 | 25874 (43.5%) | 4913 (8.3%) | 4913 (8.3%) | 8 | 50181 | 6422 |
| Rust | 21182 | 0 (0.0%) | 0 (0.0%) | 0 (0.0%) | 1 | 0 | 0 |
| C++ | 4581 | 1093 (23.9%) | 16 (0.3%) | 16 (0.3%) | 4 | 1904 | 92 |
| Java | 1147 | 181 (15.8%) | 18 (1.6%) | 18 (1.6%) | 3 | 385 | 21 |

Interpretation: direct child collection is already mostly linear for all target
languages except Go, so replacing only stack-pop graph traversal cannot deliver
a universal 20% gain. A useful linear-stack architecture must instead remove
the whole persistent-node path for straight segments: stack-node allocation,
link payload retain/release, version head updates, and reduce reinsertion. Go
requires first-class branching support because multi-version reductions and
merge attempts are common. Rust, Python, TypeScript, and JavaScript would be
good validation cases for a segmented contiguous stack, but Go is the design
gate for universality.

## Next Direction

### Observations

- `ts_parser__advance` is an action interpreter: get/reuse/lex one lookahead,
  fetch the parse-table entry, run reductions until a shift/accept/recover
  action, then repeat. This creates many small crossings between parser, stack,
  subtree, and language-table code.
- `ts_parser__reduce` eagerly creates a concrete tree node for every reduction,
  even in a fresh normal parse where most nodes will only be consumed by later
  reductions before final tree publication.
- The stack is a persistent graph even when the parse is effectively linear.
  Link-count measurements show most stack nodes have one predecessor, but the
  compact-node trial proved that simply shrinking graph nodes is not enough.
  The bigger question is whether the common path should use a graph at all.
- Pending descriptors already model subtree-like metadata, but partial lazy
  wiring has failed at ownership boundaries. The useful design is not "make
  reduce lazy"; it is "parse into a stack-native forest and materialize once".
- Go is the hard universal case because normal Go fixtures hit more branching
  and multi-pop reductions. Any 20% plan that only optimizes single-version
  reductions is expected to miss Go.
- C++ has enough generated lexer/keyword time that parser-only construction
  wins may be capped. If parser construction drops and C++ still misses, the
  next front is the generated lexer contract, not more subtree micro-tuning.

### Strategy

Do not add more local fast paths. Rank future work by removed phase:

1. **Segmented linear-stack normal parser.** Keep the current persistent graph
   for recovery, reuse, and hard ambiguity, but use contiguous frame segments
   for fresh normal parsing. A frame stores payload, state, position, cost,
   precedence, node count, and external-token metadata. Reductions pop a slice
   of frames directly; forks share immutable segment prefixes and only promote
   to graph nodes when merge/recovery requires full GLR semantics. This attacks
   stack-node allocation, pointer chasing, graph traversal, payload
   retain/release churn, and version-renumber work together. Go's high
   multi-version/merge pressure makes shared segment prefixes mandatory; a
   single-version-only linear path is not a universal plan.
2. **Stack-native parse forest with final materialization.** Push
   `PendingReduction`/payload descriptors through normal parsing and build
   concrete `SubtreeHeapData` only at accept or at forced boundaries
   (recovery, subtree comparison, reusable-node interaction, public tree
   publication). This attacks parent allocation, child copying, and immediate
   summarization. It must handle multi-version and multi-pop paths before it is
   counted as a real candidate.
3. **Action-trace execution.** Cache deterministic state/lookahead action runs
   that contain only normal reductions and one final shift/accept. Execute the
   trace as one parser operation with precomputed reduce metadata and
   nonterminal next states. This attacks repeated table-entry lookup,
   action-loop overhead, and interleaved stack/version bookkeeping.
4. **Generated lexer contract.** If parser construction drops but C++/JS/TS are
   still lexer-bound, prototype a generated-lexer bulk-scan API for common
   ASCII classes and keyword dispatch. SIMD belongs here, not inside random
   parser branches.

### Measurement Plan

- Use `cargo flamegraph --bench benchmark -p tree-sitter-cli` for high-sample
  C++ and TypeScript profiles, plus same-session normal `-r 10` benchmarks for
  all seven target languages.
- Add temporary library-only counters, never benchmark-harness edits, to measure
  phase removal: linear-stack coverage, graph promotions, reductions covered by
  descriptors, forced materializations, action-trace lengths, and lexer calls
  per byte/token.
- For each architecture spike, first prove coverage with counters, then build
  the smallest correctness slice, then benchmark. Do not benchmark code that
  fails `cargo test --all` outside the sandbox.
- Reject a direction when its coverage ceiling cannot plausibly produce a
  universal 20% win, even if one language improves.

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
