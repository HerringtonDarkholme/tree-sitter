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

- Last pushed batch: `f17c0325` (`Use direct lexer EOF checks internally`)
- Current local positive commit: `459741ff` (`Fast path linear stack pops`)
- Weighted normal parsing throughput after `459741ff`:
  - baseline at `ff183714`: `18151.8 bytes/ms`
  - current: `19067.5 bytes/ms`
  - delta: `+5.04%`

Per-language weighted deltas after `459741ff`:

| Language | Baseline bytes/ms | Current bytes/ms | Delta |
| --- | ---: | ---: | ---: |
| C++ | 7481.3 | 10363.1 | +38.52% |
| Go | 16483.3 | 16543.6 | +0.37% |
| Java | 7371.4 | 12446.7 | +68.85% |
| JavaScript | 18173.8 | 19110.9 | +5.16% |
| Python | 12615.2 | 13025.8 | +3.26% |
| Rust | 16492.2 | 19762.1 | +19.83% |
| TypeScript | 26661.9 | 27397.0 | +2.76% |

The remaining gap is mostly TypeScript, JavaScript, Go, and Python.

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

## Non-Library Trial Removed

| Trial | Result |
| --- | --- |
| Reset benchmark allocator for raw parsing | Removed from history because benchmark source changes are out of scope |

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
