# Parser Performance Trial Log

This log tracks raw parsing optimization attempts for the Rust runtime. The
current target set is normal raw parsing speed for:

- TypeScript
- JavaScript
- Python
- Go
- Rust
- C++
- Java

Benchmark source files are intentionally left unchanged. Profiling helpers may
live outside the repo under `/tmp`.

## Current Baseline

- Last pushed batch: `40bf2b97` (`Record tree arena page size trial`)
- Current local log-only commit: `68512e16` (`Record additional parser architecture trials`)
- Current kept architecture change: `9e843a09` (`Allocate parser reduction nodes in tree arena`)
- Same-session language-average throughput for the kept arena-reduction slice:
  - baseline before arena-reduction slice: `94646 bytes/ms` total across seven language averages
  - after arena-reduction slice: `99678 bytes/ms` total across seven language averages
  - delta: `+5.3%`

Per-language deltas for the kept arena-reduction slice:

| Language | Baseline bytes/ms | Current bytes/ms | Delta |
| --- | ---: | ---: | ---: |
| JavaScript | 17119 | 18072 | +5.6% |
| TypeScript | 22095 | 23024 | +4.2% |
| Python | 9031 | 9276 | +2.7% |
| Go | 15102 | 15265 | +1.1% |
| Rust | 13683 | 15139 | +10.6% |
| C++ | 7028 | 7068 | +0.6% |
| Java | 10588 | 11834 | +11.8% |

The remaining 20% target is not met. The remaining gap is mostly TypeScript,
JavaScript, Go, Python, and C++.

## Profiling Setup

Primary profiling tool:

```sh
cargo flamegraph --release -o /tmp/tree-sitter-js-jquery-flamegraph.svg -- \
  /Users/hd/code/test/tree-sitter/test/fixtures/grammars/javascript/src \
  /Users/hd/code/test/tree-sitter/test/fixtures/grammars/javascript/examples/jquery.js \
  1000
```

The command is run from `/tmp/ts-raw-profile-harness`, with:

```sh
TREE_SITTER_HARNESS_SCRATCH=/tmp/ts-raw-profile-harness-cache
CARGO_NET_OFFLINE=true
```

Secondary profiling tool:

```sh
/usr/bin/sample <pid> <seconds> -file /tmp/sample.txt
```

The temporary raw harness parses from a contiguous source buffer through the
public Rust parser API and does not modify benchmark source code.

## Observed Hot Paths

JavaScript `jquery.js` flamegraph:

- `ts_parser__reduce`: about `27%`
- `ts_stack_pop_count`: about `9%`
- `ts_subtree_summarize_children`: about `6%`
- JavaScript external scanner: about `7%`
- `ts_parser__balance_subtree` / `ts_subtree_compress`: about `5%`
- `ts_subtree_release` and tree drop: visible cleanup cost

Go `proc.go` flamegraph:

- `ts_parser__reduce`: about `33%`
- `ts_stack_pop_count`: about `14%`
- `ts_subtree_summarize_children`: about `7%`
- `ts_stack_merge`: about `6%`
- `ts_parser__balance_subtree` / `ts_subtree_compress`: about `5%`
- `ts_subtree_release` and tree drop: visible cleanup cost

Main conclusion: the shared bottleneck is the reduce path:

```text
ts_parser__advance
  -> ts_parser__reduce
     -> ts_stack_pop_count
     -> ts_subtree_new_node
        -> ts_subtree_summarize_children
     -> ts_stack_push / ts_stack_merge
```

SIMD is not currently the primary target. The runtime receives generated lexer
callbacks one codepoint at a time, so there is no obvious long contiguous scan
inside the core library to vectorize without grammar-level changes.

## Positive Changes Kept

| Commit | Change | Result |
| --- | --- | --- |
| `99bc048b` | Avoid slice creation for subtree child access | Positive, kept |
| `3ddb54cd` | Compare lexer modes without `memcmp` | Positive, kept |
| `5e887437` | Delay token reuse mode checks | Positive, kept |
| `eb634f4c` | Inline hot array helpers | Positive, kept |
| `0d823045` | Skip progress state updates without callback | Positive, kept |
| `7c0c7a1c` | Avoid slice creation for lexer range access | Positive, kept |
| `836c303b` | Fast path single lexer range reset | Positive, kept |
| `f17c0325` | Use direct lexer EOF checks internally | Positive, pushed |
| `459741ff` | Fast path linear stack pops | Positive, local |
| `329f8b08` | Direct nonterminal next-state lookup in reduce | Positive on same-session JS/Go/TS canaries |

Same-session canary result for `329f8b08`:

| Language | Clean baseline bytes/ms | Candidate bytes/ms | Delta |
| --- | ---: | ---: | ---: |
| JavaScript | 17959 | 19140 | +6.58% |
| Go | 15406 | 16929 | +9.89% |
| TypeScript | 22520 | 23785 | +5.62% |

## Negative Trials Reverted

| Trial | Target | Result |
| --- | --- | --- |
| Broad metadata caching in `ts_subtree_summarize_children` | Reduce/subtree metadata | Regressed JavaScript, Go, and TypeScript |
| Single-child `ts_subtree_summarize_children` fast path | Reduce/subtree metadata | Flat or negative on JavaScript, Go, TypeScript |
| Smaller stack pop reserve count | Stack pop allocation | Large regression |
| Specialized `ts_stack_pop_count` graph walk without callback | Stack pop fallback | Mixed; Go improved in one warm run, JavaScript regressed |
| ASCII fast path in `ts_lexer__get_lookahead` | Lexer decode | Neutral or negative |
| Direct UTF-8 decode path avoiding decode function pointer | Lexer decode | Mixed or negative |
| Single-range per-character lexer advance fast path | Lexer advance | Negative |
| Alias-sequence condition reorder | Subtree summarize alias handling | Negative |
| Direct `as u8` casts replacing checked conversions in leaf creation | Leaf construction | Negative on JavaScript and Go |
| `#[inline]` on `ts_subtree_retain` | Refcount helper | Negative |
| Relaxed/release-acquire subtree refcount ordering | Retain/release | Quick harness mixed; clean JavaScript benchmark regressed |
| Passing `is_leaf` into `ts_parser__shift` | Shift path | Negative |
| Direct cast for stack reserve count | Stack allocation | Negative |
| Accumulating subtree flags locally in summarizer | Subtree summarize flags | Negative |
| Caching `language_is_wasm` in `TSParser` | Parser state | Negative |
| Increasing `MAX_NODE_POOL_SIZE` from 50 to 128 | Stack node pool | Negative |
| Broad stack getter/push inlining | Stack helpers | Negative |
| Broad `ts_language_table_entry` inlining | Parse table lookup | Negative |
| Broad `ts_parser__check_progress` inlining | Parser progress check | Negative |
| Early no-callback return in `ts_parser__check_progress` | Parser progress check | Clean JavaScript benchmark regressed |
| Guard halted-version scans in `ts_parser__reduce` | Reduce path version limiting | Clean JavaScript benchmark regressed |
| Pointer-equality fast path for `ts_stack_can_merge` last external tokens | Stack merge | Retested after `329f8b08`; warm JavaScript benchmark remained below current baseline |
| Guard no-op subtree-array reversals in stack pops | Stack pop | Warm JavaScript benchmark remained below current baseline |
| Same-token fast path in `ts_stack_set_last_external_token` | External token tracking | Warm JavaScript benchmark remained below current baseline |
| Skip summarize for zero-child non-error nodes | Subtree construction | Retested after `329f8b08`; warm JavaScript benchmark remained below current baseline |
| Guard zero dynamic-precedence writes in reduce | Reduce path | Retested after `329f8b08`; warm JavaScript benchmark remained below current baseline |
| Pointer-equality fast path in `ts_subtree_external_scanner_state_eq` | External scanner state comparison | Retested after `329f8b08`; warm JavaScript benchmark remained below current baseline |
| Hoist reduce nonterminal check out of pop-slice loop | Reduce path | Retested after `329f8b08`; warm JavaScript benchmark remained below current baseline |
| Specialized no-alias non-error subtree summarizer | Subtree summarize | Retested after `329f8b08`; warm JavaScript benchmark remained below current baseline |
| 16-bit symbol inline leaf encoding | Subtree inline representation | Regressed JavaScript and did not reduce allocation counts in allocator harness |
| Global mutex slab for `SubtreeHeapData + children` blocks | Subtree block allocation | JavaScript benchmark failed to produce first result in normal time; per-block global lock/metadata path is not viable |
| Atomic global slab with `SubtreeArray.capacity` slab marker | Subtree block allocation | JavaScript benchmark still failed to produce first result in normal time; overloading capacity for ownership is too fragile |
| Zero-count fast path in linear stack pops | Stack pop | Warm JavaScript benchmark was below current baseline |
| Refcount-one direct release fast path | Subtree release | Regressed JavaScript canary |
| Terminal-only table-entry helper in advance loop | Parse table lookup | Warm JavaScript benchmark remained below current baseline |
| Increase `TS_MAX_TREE_POOL_SIZE` from 32 to 128 | Childless subtree pool | Allocator counts were unchanged; JS harness got slower despite noisy benchmark canaries |
| Pool-backed zero-child `ts_subtree_new_node` plus zero-count stack-pop reserve skip | Childless subtree allocation | Allocator counts were unchanged and JS/TS/Go canaries regressed, so zero-child reductions are not the dominant 80-byte allocation source |
| Raw-pointer child loop in `ts_subtree_summarize_children` | Subtree summarize | JS/TS/Go/Python canaries regressed; the existing slice iterator appears to optimize better |
| Use `ts_malloc` instead of `ts_realloc(NULL, size)` in subtree array allocation | Subtree allocation | JS/TS/Go/Python canaries regressed; allocator-call simplification did not overcome codegen/layout effects |
| Parser `SubtreePool` free lists for 1-4 child node blocks | Subtree block allocation | Allocation calls dropped by ~1.8k/parse on JS, but harness and JS/TS/Go canaries regressed; per-release pool bookkeeping outweighed reuse |
| Arena-backed heap leaves during lexing | Subtree allocation | JavaScript/TypeScript/Python improved, but Go regressed to 14165 avg bytes/ms and Rust regressed to 13219 avg bytes/ms; not viable as a universal normal-parse optimization |
| Increase `TREE_ARENA_PAGE_SIZE` from 16 KiB to 64 KiB | Tree arena page layout | JavaScript canary regressed to 17256 avg bytes/ms from 18072, so fewer page allocations did not offset worse locality/cache behavior |
| Adopt stack-pop child arrays into `TreeArena` instead of copying into arena pages | Reduce/node construction | JavaScript was roughly flat at 18123 avg bytes/ms, but TypeScript regressed to 22639 avg bytes/ms from 23024; consuming malloc blocks also complicates arena release order |
| Embedded adopted-block headers in stack-pop arrays | Reduce/node construction | Removed metadata allocation from the adopted-block idea, but JavaScript still slipped to 18051 avg / 16438 worst bytes/ms while TypeScript improved; not universal enough to keep |
| Relax subtree/tree-arena refcount ordering from `SeqCst` to relaxed/release-acquire | Refcount/lifetime | JavaScript canary regressed to 17604 avg bytes/ms from 18072; weaker ordering did not improve the hot parse path on this target |
| Skip post-parse subtree balancing entirely | Balance/compress upper bound | JavaScript improved to 18728 avg bytes/ms, but TypeScript regressed to 22339 avg / 17610 worst; balancing is not a standalone 20% universal opportunity |

## Non-Library Trial Removed

| Trial | Result |
| --- | --- |
| Reset benchmark allocator for raw parsing | Removed from history because benchmark source changes are out of scope |

## Reflection Cadence

After every ten performance attempts, stop and write a reflection before making
the next code experiment. A reflection must answer:

- Which attempts were positive, negative, or inconclusive?
- Which directions are now closed?
- Which profiler evidence still supports the next direction?
- What acceptance gate must the next experiment pass?

This is meant to prevent repeating already-failed ideas like refcount ordering,
page-size tuning, or partial arena adoption without new evidence.

### Reflection 1: Arena/Allocation Architecture Batch

Attempts covered:

1. Record subtree allocation profiling results.
2. Record subtree slab allocation trials.
3. Add subtree ownership regression test.
4. Record additional subtree allocation misses.
5. Record subtree allocation malloc trial.
6. Record subtree small-block pool trial.
7. Add arena-backed tree storage foundation.
8. Allocate parser reduction nodes in tree arena.
9. Record arena leaf allocation trial.
10. Record tree arena page size trial.
11. Record additional parser architecture trials.

What worked:

- Arena-backed normal tree storage plus arena allocation for parser reduction
  parent nodes is the only kept architecture win in this batch.
- It gave a same-session average gain of `+5.3%` across the seven target
  language averages, with stronger Rust and Java wins.
- The `arena_owned` clone bug was real and fixed by clearing the ownership bit
  on heap clones.

What failed:

- Pooling/slab variants reduced some allocator activity but added enough
  bookkeeping or locality cost to regress benchmarks.
- Arena-backed heap leaves were not universal: Go and Rust regressed.
- Larger arena pages regressed JavaScript.
- Stack-pop array adoption variants were mixed and should not be mistaken for a
  real builder path.
- Refcount ordering changes failed twice and are closed.
- Skipping balancing showed no universal upside.

Main lesson:

- Allocation count alone is not predictive. Several changes reduced allocation
  pressure or looked cheaper locally but lost on cache locality, branch layout,
  or language-specific parse shape.
- The next serious work should build a real parser-local reduce builder that
  writes child spans in the desired representation from the start. Do not try to
  rescue already-allocated `SubtreeArray` buffers after the fact.

Next acceptance gate:

- Before coding the builder, collect reduce child-count distribution and
  `ts_stack_pop_count` linear-vs-graph fallback rates across at least
  JavaScript, TypeScript, Go, and Rust.
- Any kept optimization must pass same-session normal benchmarks for all seven
  target languages and `cargo test --all` outside the sandbox.

## Direction Triage Before Further Experiments

### Closed Directions

Do not retry these without new profile evidence that contradicts the recorded
results:

| Direction | Why closed |
| --- | --- |
| Relaxed/release-acquire subtree or arena refcount ordering | Failed twice: the earlier clean JavaScript benchmark regressed, and the post-arena JavaScript canary regressed from 18072 to 17604 avg bytes/ms. Current flamegraph shows retain/release frames, but not as a dominant standalone bottleneck. |
| Larger tree arena pages | 64 KiB pages reduced page churn in theory but regressed JavaScript to 17256 avg bytes/ms, likely from worse locality/cache behavior. |
| Arena-backed lexer leaves | Helped some languages but regressed Go and Rust, so it is not universal. |
| Adopting stack-pop malloc buffers into `TreeArena` | Both external metadata and embedded-header variants were mixed: JavaScript was flat/regressed while TypeScript moved in opposite directions. This is not the same as a real builder path and should not be repeated. |
| Skipping or deferring all balancing | JavaScript improved, but TypeScript regressed badly. Balancing cannot be removed as a universal normal-parse win. |

### Current Hotspot Evidence

Latest JavaScript `jquery.js` flamegraph after the kept arena-reduction slice:

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

The evidence says the next experiment should target the reduce/stack-pop/node
construction pipeline as a whole. It should not target isolated refcount memory
ordering unless a new flamegraph shows refcount operations rising above the
reduce/stack-pop/summarize frames.

### Candidate Directions

| Priority | Direction | Reason | Pre-code gate |
| --- | --- | --- | --- |
| 1 | Real parser-local builder pop path | Directly targets the shared `ts_parser__reduce` + `ts_stack_pop_count` + node construction pipeline. Unlike adopted-block trials, it should write child spans into builder scratch storage from the start, not allocate a normal `SubtreeArray` and try to rescue it later. | Before coding, collect per-language child-count and stack-pop fallback rates using profiling or temporary local instrumentation outside committed benchmark code. |
| 2 | Reduce summarization work during builder construction | `ts_subtree_summarize_children` remains visible and is called immediately for every reduction. A builder could accumulate summary fields while writing children, avoiding a second child walk. | Only proceed if builder-pop profiling shows child-span construction still dominates after stack-pop copying is removed. |
| 3 | Lexer/external scanner profiling by language | JavaScript spends substantial time in `ts_lex`, external scanner scan, and keyword lexing. This may help JS/TS but is less likely to be universal across Go/Rust/Python/C++/Java. | Require flamegraphs for at least JS, TS, Go, Rust before implementing lexer changes. |

### Experiment Acceptance Gate

Before keeping any new library optimization:

- Run same-session `cargo xtask benchmark --kind normal -r 10 --language` for
  TypeScript, JavaScript, Python, Go, Rust, C++, and Java.
- Treat a target-language average regression or meaningful worst-file regression
  as a reject unless another target-language gain is large enough and there is a
  clear universal explanation.
- Run `cargo test --all` outside the sandbox before committing kept library code.
- Record failed trials here immediately, including the direction and the
  canary numbers that rejected it.

## Measurement Rules

- Do not edit benchmark source code.
- Use `/tmp/ts-raw-profile-harness` for flamegraph/sample profiling.
- Use `cargo test --all` for repo-level validation. Do not treat
  `cargo check` as a test result.
- Use the existing benchmark runner for acceptance:

```sh
cargo xtask benchmark --kind normal -r 10 --language javascript
cargo xtask benchmark --kind normal -r 10 --language go
cargo xtask benchmark --kind normal -r 10 --language typescript
```

- Commit only positive library changes.
- Keep each optimization in its own commit.
- Push only after 10 additional optimization commits, unless explicitly asked.
- After every 10 perf attempts, add a reflection before the next experiment.

## Validation Notes

### 2026-06-26 parser tree arena reduction slice

Change:

- Added parser-owned `TreeArena` allocation for normal parse runs.
- Moved reduction/accept/recovery parent node allocation through
  `ts_subtree_new_node_in_arena`.
- Transferred the arena to the returned `TSTree` on successful parse.

Initial failure:

- `cargo test --all` failed in corpus allocation checks.
- Narrow repro: `cargo test -p tree-sitter-cli test_corpus_for_javascript_language -- --nocapture`.
- The first JavaScript corpus case passed parsing but reported leaked allocation indices during
  edit/reparse trials.

Root cause:

- `ts_subtree_clone` copied the `arena_owned` flag from an arena-backed source subtree into a
  fresh heap allocation.
- Edit/reparse can clone arena-owned subtrees from an old tree. Those clones must be normal
  heap-owned nodes; otherwise release skips freeing them.

Fix:

- Clear `arena_owned` on cloned heap subtrees.

Validation:

- `cargo test -p tree-sitter-cli test_corpus_for_javascript_language -- --nocapture` passed
  outside the sandbox.

Full validation:

- First `cargo test --all` run passed corpus allocation checks but aborted later with a transient
  misaligned-pointer panic in `test_tree_cursor_child_for_point`.
- The same test passed in isolation.
- A second `cargo test --all` run passed outside the sandbox.

Benchmark result, compared against the same worktree with only this library-code patch reversed:

| Language | Baseline avg bytes/ms | Arena slice avg bytes/ms | Avg delta | Baseline worst | Arena slice worst | Worst delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| JavaScript | 17119 | 18072 | +5.6% | 16463 | 16770 | +1.9% |
| TypeScript | 22095 | 23024 | +4.2% | 19086 | 19649 | +2.9% |
| Python | 9031 | 9276 | +2.7% | 441 | 395 | -10.4% |
| Go | 15102 | 15265 | +1.1% | 12399 | 13884 | +12.0% |
| Rust | 13683 | 15139 | +10.6% | 10388 | 11696 | +12.6% |
| C++ | 7028 | 7068 | +0.6% | 5697 | 6104 | +7.1% |
| Java | 10588 | 11834 | +11.8% | 8827 | 9721 | +10.1% |

Mean of language-average throughput values:

- Baseline: 94646 bytes/ms total across the seven language averages.
- Arena slice: 99678 bytes/ms total across the seven language averages.
- Delta: +5.3%.

Conclusion:

- This is a real architecture-level allocation/layout improvement, but it is not the requested
  20% gain by itself.
- It should remain a foundation for later work only if additional arena/tree-builder changes can
  build on it without regressing C++/Go/Python.

Flamegraph/profiling:

- Command:

```sh
cargo flamegraph --release -o /tmp/tree-sitter-js-jquery-arena-slice-flamegraph.svg -- \
  /Users/hd/code/test/tree-sitter/test/fixtures/grammars/javascript/src \
  /Users/hd/code/test/tree-sitter/test/fixtures/grammars/javascript/examples/jquery.js \
  2000
```

- Output: `/tmp/tree-sitter-js-jquery-arena-slice-flamegraph.svg`.
- Harness result on JavaScript `jquery.js`: 18750 bytes/ms, 13.19 ms/parse.
- Allocator result: about 68038 allocation calls/parse and 12.4 MB requested/parse.
- Dominant exact allocation sizes remained 88, 96, and 104 bytes. This means parent-node arena
  allocation is not enough; major heap traffic still comes from other subtree allocation paths.

### 2026-06-26 sandboxed run

Command:

```sh
cargo test --all
```

Result:

- Failed in `tree-sitter-cli` detect-language tests.
- Parser/runtime tests completed before the failure.
- Failing tests:
  - `tests::detect_language::detect_language_by_double_barrel_file_extension`
  - `tests::detect_language::detect_language_by_first_line_regex`
  - `tests::detect_language::detect_language_without_file_extension`
  - `tests::detect_language::detect_language_without_filename`

The failure happened while only this trial-log markdown file was changed.

### 2026-06-26 non-sandbox run

Command:

```sh
cargo test --all
```

Result:

- Passed when run outside the sandbox.
- Summary:
  - `tree_sitter`: 8 passed
  - `tree_sitter_cli`: 269 passed
  - `tree_sitter_generate`: 59 passed
  - `tree_sitter_tags`: 2 passed
  - doctests passed, with one ignored doc test
