# Parser Performance Trial Summary

This file tracks raw normal parsing performance work for the Rust runtime.

Target languages:

- TypeScript
- JavaScript
- Python
- Go
- Rust
- C++
- Java

Benchmark source files must not be changed. Profiling helpers may live outside
the repo under `/tmp`.

## Current Status

The 20% universal target is not met.

Current kept architecture change:

- `9e843a09` - allocate parser reduction nodes in a tree arena.

Measured same-session impact of that kept slice:

| Language | Baseline bytes/ms | Arena slice bytes/ms | Delta |
| --- | ---: | ---: | ---: |
| JavaScript | 17119 | 18072 | +5.6% |
| TypeScript | 22095 | 23024 | +4.2% |
| Python | 9031 | 9276 | +2.7% |
| Go | 15102 | 15265 | +1.1% |
| Rust | 13683 | 15139 | +10.6% |
| C++ | 7028 | 7068 | +0.6% |
| Java | 10588 | 11834 | +11.8% |

Mean of language averages:

- Baseline: `94646 bytes/ms`
- Arena slice: `99678 bytes/ms`
- Delta: `+5.3%`

Remaining weak spots: TypeScript, JavaScript, Go, Python, and C++.

## Current Hotspots

Latest useful JavaScript `jquery.js` flamegraph after the kept arena-reduction
slice:

| Frame | Samples | Share |
| --- | ---: | ---: |
| `ts_parser__reduce` | 475 | 27.76% |
| `ts_lex` | 197 | 11.51% |
| `ts_subtree_new_node_in_arena` | 147 | 8.59% |
| `ts_stack_pop_count` | 147 | 8.59% |
| `tree_sitter_javascript_external_scanner_scan` | 137 | 8.01% |
| `ts_parser__balance_subtree` | 112 | 6.55% |
| `ts_subtree_compress` | 92 | 5.38% |
| `ts_subtree_summarize_children` | 91 | 5.32% |

Main shared bottleneck:

```text
ts_parser__advance
  -> ts_parser__reduce
     -> ts_stack_pop_count
     -> subtree node construction
     -> ts_subtree_summarize_children
     -> ts_stack_push / ts_stack_merge
```

SIMD is not the current primary target. The core runtime receives generated
lexer callbacks one codepoint at a time, so there is no obvious long contiguous
scan inside the core library to vectorize without grammar-level changes.

## What Worked

| Area | Kept result |
| --- | --- |
| Tree storage | Arena-backed `TSTree` foundation was added. |
| Reduction allocation | Parser reduction/accept/recovery parent nodes now allocate in the tree arena. |
| Ownership correctness | Heap clones clear `arena_owned`, fixing edit/reparse leaks from cloned arena-backed nodes. |
| Earlier local parser fast paths | Several small wins were kept before the arena work, including linear stack-pop fast paths and direct nonterminal next-state lookup. |

The only recent architecture-level win is arena-backed reduction parent nodes.
It is useful, but it is not large enough by itself.

## Itemized Trial Index

This section keeps one row per unique trial. Grouped summaries later in the file
may refer to these rows, but should not duplicate them as separate attempts.

### Kept

| Trial | Area | Result |
| --- | --- | --- |
| Avoid slice creation for subtree child access | Subtree access | Positive, kept |
| Compare lexer modes without `memcmp` | Token reuse / lexer mode | Positive, kept |
| Delay token reuse mode checks | Token reuse | Positive, kept |
| Inline hot array helpers | Array helpers | Positive, kept |
| Skip progress state updates without callback | Progress checks | Positive, kept |
| Avoid slice creation for lexer range access | Lexer ranges | Positive, kept |
| Fast path single lexer range reset | Lexer ranges | Positive, kept |
| Use direct lexer EOF checks internally | Lexer EOF | Positive, pushed |
| Fast path linear stack pops | Stack pop | Positive, kept |
| Direct nonterminal next-state lookup in reduce | Reduce path | Positive on JS/Go/TS canaries |
| Add arena-backed tree storage foundation | Tree storage | Positive foundation, kept |
| Allocate parser reduction nodes in tree arena | Reduce/node allocation | Positive architecture slice, `+5.3%` mean of seven language averages |

### Rejected Or Closed

| Trial | Area | Result |
| --- | --- | --- |
| Broad metadata caching in `ts_subtree_summarize_children` | Subtree summarize | Regressed JavaScript, Go, and TypeScript |
| Single-child `ts_subtree_summarize_children` fast path | Subtree summarize | Flat or negative on JavaScript, Go, TypeScript |
| Smaller stack pop reserve count | Stack pop allocation | Large regression |
| Specialized `ts_stack_pop_count` graph walk without callback | Stack pop fallback | Mixed; Go improved once, JavaScript regressed |
| ASCII fast path in `ts_lexer__get_lookahead` | Lexer decode | Neutral or negative |
| Direct UTF-8 decode path avoiding decode function pointer | Lexer decode | Mixed or negative |
| Single-range per-character lexer advance fast path | Lexer advance | Negative |
| Alias-sequence condition reorder | Subtree summarize alias handling | Negative |
| Direct `as u8` casts replacing checked conversions in leaf creation | Leaf construction | Negative on JavaScript and Go |
| `#[inline]` on `ts_subtree_retain` | Refcount helper | Negative |
| Relaxed/release-acquire subtree/tree-arena refcount ordering | Refcount/lifetime | Failed twice; clean JavaScript regressed before and after arena work |
| Passing `is_leaf` into `ts_parser__shift` | Shift path | Negative |
| Direct cast for stack reserve count | Stack allocation | Negative |
| Accumulating subtree flags locally in summarizer | Subtree summarize flags | Negative |
| Caching `language_is_wasm` in `TSParser` | Parser state | Negative |
| Increasing `MAX_NODE_POOL_SIZE` from 50 to 128 | Stack node pool | Negative |
| Broad stack getter/push inlining | Stack helpers | Negative |
| Broad `ts_language_table_entry` inlining | Parse table lookup | Negative |
| Broad `ts_parser__check_progress` inlining | Parser progress check | Negative |
| Early no-callback return in `ts_parser__check_progress` | Parser progress check | Clean JavaScript benchmark regressed |
| Guard halted-version scans in `ts_parser__reduce` | Reduce version limiting | Clean JavaScript benchmark regressed |
| Pointer-equality fast path for `ts_stack_can_merge` last external tokens | Stack merge | Retested after reduce lookup win; remained below baseline |
| Guard no-op subtree-array reversals in stack pops | Stack pop | Warm JavaScript remained below baseline |
| Same-token fast path in `ts_stack_set_last_external_token` | External token tracking | Warm JavaScript remained below baseline |
| Skip summarize for zero-child non-error nodes | Subtree construction | Retested after reduce lookup win; remained below baseline |
| Guard zero dynamic-precedence writes in reduce | Reduce path | Retested after reduce lookup win; remained below baseline |
| Pointer-equality fast path in `ts_subtree_external_scanner_state_eq` | External scanner state comparison | Retested after reduce lookup win; remained below baseline |
| Hoist reduce nonterminal check out of pop-slice loop | Reduce path | Retested after reduce lookup win; remained below baseline |
| Specialized no-alias non-error subtree summarizer | Subtree summarize | Retested after reduce lookup win; remained below baseline |
| 16-bit symbol inline leaf encoding | Subtree inline representation | Regressed JavaScript and did not reduce allocation counts |
| Global mutex slab for `SubtreeHeapData + children` blocks | Subtree block allocation | JavaScript benchmark stalled; global lock path not viable |
| Atomic global slab with `SubtreeArray.capacity` slab marker | Subtree block allocation | JavaScript benchmark stalled; ownership marker was too fragile |
| Zero-count fast path in linear stack pops | Stack pop | Warm JavaScript below baseline |
| Refcount-one direct release fast path | Subtree release | Regressed JavaScript |
| Terminal-only table-entry helper in advance loop | Parse table lookup | Warm JavaScript below baseline |
| Increase `TS_MAX_TREE_POOL_SIZE` from 32 to 128 | Childless subtree pool | Allocation counts unchanged; JS got slower/noisier |
| Pool-backed zero-child `ts_subtree_new_node` plus zero-count stack-pop reserve skip | Childless subtree allocation | Allocation counts unchanged; JS/TS/Go regressed |
| Raw-pointer child loop in `ts_subtree_summarize_children` | Subtree summarize | JS/TS/Go/Python regressed |
| Use `ts_malloc` instead of `ts_realloc(NULL, size)` in subtree array allocation | Subtree allocation | JS/TS/Go/Python regressed |
| Parser `SubtreePool` free lists for 1-4 child node blocks | Subtree block allocation | Allocation calls dropped, but harness and JS/TS/Go regressed |
| Arena-backed heap leaves during lexing | Subtree allocation | JS/TS/Python improved, but Go and Rust regressed |
| Increase `TREE_ARENA_PAGE_SIZE` from 16 KiB to 64 KiB | Tree arena page layout | JavaScript regressed to `17256` avg bytes/ms |
| Adopt stack-pop child arrays into `TreeArena` instead of copying into arena pages | Reduce/node construction | JavaScript roughly flat; TypeScript regressed |
| Embedded adopted-block headers in stack-pop arrays | Reduce/node construction | TypeScript improved, but JavaScript slipped; not universal |
| Direct linear reduce pop into parser scratch storage | Reduce/stack pop | Abandoned before benchmarking after history triage; too close to prior linear stack-pop and stack-pop adoption attempts |
| Skip post-parse subtree balancing entirely | Balance/compress upper bound | JavaScript improved, TypeScript regressed badly |
| Reset benchmark allocator for raw parsing | Benchmark harness | Removed because benchmark source changes are out of scope |

## Closed Directions

Do not retry these without new profiler evidence that contradicts the recorded
result.

| Direction | Why closed |
| --- | --- |
| Relaxed/release-acquire subtree or arena refcount ordering | Failed twice. Earlier clean JavaScript benchmark regressed, and post-arena JavaScript canary regressed from `18072` to `17604` avg bytes/ms. Refcount frames are visible but not dominant. |
| Larger tree arena pages | 64 KiB pages regressed JavaScript to `17256` avg bytes/ms. Fewer page allocations did not offset worse locality/cache behavior. |
| Arena-backed lexer leaves | Helped JavaScript/TypeScript/Python but regressed Go to `14165` and Rust to `13219` avg bytes/ms. Not universal. |
| Stack-pop malloc-buffer adoption into `TreeArena` | Both metadata and embedded-header versions were mixed. JavaScript was flat/regressed while TypeScript moved differently. This is not a real builder path. |
| Direct linear reduce-pop scratch buffer | Closed before benchmarking. It is not identical to stack-pop buffer adoption, but it is still an incremental linear stack-pop fast path, not the requested architecture change. |
| Skipping/deferring all balancing | JavaScript improved to `18728`, but TypeScript regressed to `22339` avg and `17610` worst bytes/ms. |
| Subtree allocation pools/slabs | Reduced some allocator counts, but bookkeeping, locking, or locality costs regressed benchmarks. |
| `TS_MAX_TREE_POOL_SIZE` tuning | Allocation counts were unchanged and benchmarks got noisier/slower. |
| Refcount-one release fast path | Regressed JavaScript. |
| Raw pointer summarizer loop | Regressed JS/TS/Go/Python. Existing iterator compiled better. |
| Broad inlining/caching/check-progress fast paths | Repeatedly regressed or stayed below baseline. |
| Lexer ASCII/direct UTF-8 fast paths | Mixed or negative. |

## Reflection 1: Arena/Allocation Batch

Attempts covered:

- Allocation profiling and slab/pool trials.
- Arena-backed tree storage.
- Parser reduction-node arena allocation.
- Arena-backed leaves.
- Arena page-size tuning.
- Stack-pop buffer adoption variants.
- Refcount ordering.
- Skip-balancing upper-bound experiment.

What worked:

- Arena-backed normal tree storage plus reduction parent-node allocation.
- The `arena_owned` clone fix.

What failed:

- Most allocation-count-reduction ideas that did not improve locality.
- Leaf arena allocation because it was not universal.
- Page-size tuning.
- Partial stack-pop array adoption.
- Refcount ordering changes.
- Removing balancing.

Main lesson:

- Allocation count alone is not predictive. Several ideas reduced allocation
  pressure but lost on cache locality, branch layout, or language-specific parse
  shape.
- The next serious work should be a real parser-local reduce builder that writes
  child spans in the desired representation from the start. Do not try to rescue
  already-allocated `SubtreeArray` buffers after the fact.

## Next Directions

### 0. History Triage Gate

Before writing any optimization code:

- Search this file for the target area, using terms from the proposed change
  and nearby implementation names.
- Search the commit history for the same area.
- Write down whether the idea is new, a repeat, or only a variant of a closed
  direction.
- If it is a repeat or near-repeat, stop unless there is new profiler evidence
  that directly contradicts the recorded result.

Minimum searches for reduce/stack/tree-storage work:

```sh
rg -n "stack-pop|stack pop|linear|adopt|builder|SubtreeArray|reduce builder|scratch|child arrays" PERFORMANCE_TRIALS.md
git log --oneline --all --grep='stack' --grep='arena' --grep='builder' --grep='linear' --grep='adopt' --grep='reduce'
```

The `2026-06-26` direct linear reduce-pop scratch-buffer sketch failed this
gate. It was reverted before benchmarking because it was too close to recorded
linear stack-pop and stack-pop adoption attempts.

### 1. Measure Stack-Pop Shape Before Coding

Collected on `2026-06-26` with temporary local instrumentation only:

| Language sample | Linear pop-count calls | Graph fallback calls | Dominant reduce child counts |
| --- | ---: | ---: | --- |
| JavaScript `jquery.js` | `12,582,400` | `180,600` | 1-3 |
| TypeScript `parser.ts` | `11,850,400` | `46,800` | 1-3 |
| Go `proc.go` | `6,079,200` | `500,000` | 1-3 |
| Rust `ast.rs` | `3,542,800` | `0` | 1-3 |

If implementing another builder/storage change, first collect or refresh:

- reduce child-count distribution
- `ts_stack_pop_count` linear vs graph fallback rates
- number of slices returned per reduce
- trailing-extra removal frequency
- bytes requested for child arrays

Minimum target languages for this measurement:

- JavaScript
- TypeScript
- Go
- Rust

Use temporary local instrumentation or profiling harnesses only. Do not commit
benchmark-source changes.

### 2. Real Parser-Local Reduce Builder

If the measurements support it, implement a builder path that replaces this
pipeline:

```text
ts_stack_pop_count
ts_subtree_array_remove_trailing_extras
ts_subtree_new_node_in_arena
ts_subtree_summarize_children
ts_stack_push
```

The builder must write child spans into builder-owned scratch storage from the
start. It should not allocate a normal `SubtreeArray` and then adopt it.

Potential wins:

- fewer child-array allocations
- fewer child-array copies
- better locality during reduction and summarization
- cleaner ownership than adopting malloc buffers

### 3. Summarization During Builder Construction

Only after the builder path exists, test whether summary fields can be computed
while writing child spans, avoiding a second child walk in
`ts_subtree_summarize_children`.

Do not do another standalone summarizer micro-optimization; those have already
failed.

### 4. Lexer Profiling Is Secondary

Lexer/external scanner frames are large for JavaScript, but less obviously
universal. Before touching lexer code, collect flamegraphs for at least:

- JavaScript
- TypeScript
- Go
- Rust

## Acceptance Gate

Before keeping any new library optimization:

- Run same-session `cargo xtask benchmark --kind normal -r 10 --language` for
  all seven target languages.
- Reject a change if any target language has an average regression or meaningful
  worst-file regression, unless the universal explanation is strong and the net
  gain is clearly large.
- Run `cargo test --all` outside the sandbox before committing kept library
  code.
- Record failed trials here immediately with direction and canary numbers.

## Tooling

Primary profiler:

```sh
cargo flamegraph --release -o /tmp/tree-sitter-js-jquery-flamegraph.svg -- \
  /Users/hd/code/test/tree-sitter/test/fixtures/grammars/javascript/src \
  /Users/hd/code/test/tree-sitter/test/fixtures/grammars/javascript/examples/jquery.js \
  1000
```

Run from `/tmp/ts-raw-profile-harness`.

Useful environment:

```sh
TREE_SITTER_HARNESS_SCRATCH=/tmp/ts-raw-profile-harness-cache
CARGO_NET_OFFLINE=true
```

Secondary profiler:

```sh
/usr/bin/sample <pid> <seconds> -file /tmp/sample.txt
```

Validation:

```sh
cargo test --all
```

Validation must run outside the sandbox.

## Process Rules

- Do not edit benchmark source code.
- Do not use `cargo check` as validation.
- Check this trial history and relevant commit history before writing any
  optimization code.
- Do not implement near-duplicate attempts unless new profiler evidence directly
  contradicts the old result.
- Commit each kept optimization separately.
- Push after every 10 additional commits, unless explicitly asked otherwise.
- After every 10 performance attempts, write a reflection before the next code
  experiment.
