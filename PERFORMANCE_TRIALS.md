# Parser Performance Trail

Compact history for raw normal parsing performance work in the Rust runtime.

Target languages: TypeScript, JavaScript, Python, Go, Rust, C++, Java.

## Status

- Universal 20% target: not met.
- Best kept gains: arena-backed reduction parents and parser-owned fresh
  reduction stack-pop builder.
- Current direction: complete parse-time representation boundary; no partial
  descriptor stack wiring.
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
unsafe: stack graph traversal can now collect retained `StackLinkPayload`s
without pretending pending descriptors are concrete subtrees.

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

## Next Direction

Do not add more reduce fast paths. The next viable architecture work should be
designed as a complete representation boundary, not a partial stack payload
patch: either a separate parse-time node representation with one final tree
lowering pass, or a fully descriptor-aware stack/recovery/merge/accept layer.
Reject either direction unless it removes a full allocation/copy/summarization
phase on normal parses and passes corpus tests before benchmarking.

The compact stack-node layout trial rules out node-size reduction by itself as
the big architecture path. Next work should focus above the stack storage
level: reducing concrete subtree construction during parsing, or moving normal
parse reductions into a compact parse-time tree that is lowered once at accept.

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
