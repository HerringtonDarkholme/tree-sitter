# Rust Core Performance Log

This log records Rust-core performance against the pre-rewrite C core. Every
parser performance change should add a new entry with the command, workload,
Rust throughput, C throughput, overall delta, and delta from the prior relevant
baseline.

Use report-only mode while optimizing:

```sh
cargo xtask perf-gate --language typescript --repetitions 10 --error-limit 8 --report-only --offline
```

Use strict mode before release:

```sh
cargo xtask perf-gate --offline
```

## Trial History Summary

The former `PERFORMANCE_TRIALS.md` file has been merged here as the compact
history for raw normal parsing performance work in the Rust runtime.

Target languages: TypeScript, JavaScript, Python, Go, Rust, C++, Java.

Current status:

- Universal 20% normal-parse target: not met.
- Kept gains: arena-backed reduction parents for fresh normal parses and the
  parser-owned fresh reduction stack-pop builder.
- Current direction: architecture investigation before more code trials. The
  next attempt must remove a hot phase from normal parsing, not add another
  partial fast path.
- Avoid for now: small local fast paths, refcount-order tweaks, node-pool
  tuning, benchmark-harness edits, dormant storage foundations, and SIMD
  without a reusable-runtime scan-loop profile.

The hot loop remains:

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
- Every reduction eagerly allocates parent storage and summarizes child
  metadata immediately.
- The concrete parent is pushed back into the graph and then participates in
  version merge, recovery, and accept logic.

Closed or removed directions:

- broad metadata caching in `ts_subtree_summarize_children`
- single-child, no-alias, raw-pointer, and zero-child summarizer fast paths
- direct graph builder collection and linear reduce-pop variants
- stack-pop reserve, reversal, trailing-extra, and control-flow tweaks
- broad descriptor/lazy-reduction wiring
- payload-aware accept/finalization through the reduce builder
- arena-backed heap leaves, inline leaf symbol changes, and zero-child pools
- global slab allocators, parser-local small child-block free lists, and larger
  tree-arena pages
- compact stack-node and page-backed extra-link storage
- refcount ordering, retain inline, and direct-release fast paths
- lexer/token micro-optimizations without generated-lexer profile proof
- broad parse-table and stack helper inlining
- balancing deferral and benchmark allocator resets
- dormant `StackSegment` / `StackFrame` storage foundations

Useful measurements from the trial log:

- C++ normal flamegraph samples were split between reduction construction and
  generated lexer work: `ts_parser__reduce` 24.7%, `ts_lex` 22.2%,
  `ts_subtree_new_node_in_arena` 12.0%,
  `ts_subtree_summarize_children` 9.5%, `ts_lex_keywords` 7.9%,
  `ts_parser__balance_subtree` 4.2%, `ts_stack_renumber_version` 4.0%, and
  `ts_stack_pop_count_into` 3.7%.
- Stack-node link-count probes showed mostly one-link nodes, but compacting
  graph-node layout alone did not produce a universal win and regressed Go.
- Descriptor lazy-candidate counters looked promising for TypeScript,
  JavaScript, Python, Rust, and Java, but Go hit much more multi-version and
  multi-pop pressure. A single-version-only lazy path cannot satisfy a
  universal target.
- Linear-stack coverage counters showed direct child collection is already
  mostly linear outside Go. Future stack work must remove the persistent-node
  path for straight segments, not just replace stack-pop graph traversal.

Reflections from the trial sequence:

1. Allocation work helped only when it improved ownership and locality. Pools,
   larger pages, leaf arenas, and refcount tuning did not generalize.
2. Local reduce fast paths are exhausted. Future reduce work must remove a full
   phase, not just make one branch cheaper.
3. Lexer work needs profile proof that reusable runtime code is the bottleneck;
   generated lexers and external scanners often dominate lexer samples.
4. Descriptor foundation code was not itself a measured win. Partial lazy wiring
   exposed concrete-subtree assumptions in reduce, recovery, accept, merge, and
   final materialization, so it was backed out instead of tuned.
5. Representation-boundary work must be validated one ownership boundary at a
   time. Reduce, merge/recovery, and accept/finalization need explicit tested
   models before any future lazy-reduction attempt.
6. Do not keep dormant foundations. The segmented stack and descriptor payload
   scaffolding were removed after failing to produce a measured performance
   improvement.

Future architecture candidates should be ranked by removed phase:

1. A linear/common-path stack representation that avoids persistent graph-node
   allocation for straight segments while preserving first-class branching,
   merge, recovery, old-tree reuse, and GLR semantics.
2. A stack-native parse forest that materializes concrete `SubtreeHeapData` only
   at accept or forced boundaries.
3. Action-trace execution for deterministic state/lookahead runs that contain
   normal reductions followed by one shift or accept.
4. Generated-lexer contract work if parser construction drops but C++, JS, or
   TS remain lexer-bound.

Process rules:

- Check this file before every new performance trial.
- Closed trials may be revisited only when the hypothesis, profile, or
  architecture changes.
- Do not edit benchmark source code.
- Use `cargo test --all` for kept production code.
- Add benchmark commands and results to this log.

## Baseline

### 2026-06-23 17:07 EDT

- Repo base: `8d700257`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Command:

```sh
CARGO_HOME=/tmp/tree-sitter-cargo-home cargo xtask perf-gate --language typescript --repetitions 3 --error-limit 2 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 19064.9 | 19750.3 | -3.47% |
| TypeScript error parses | 17 | 3304.0 | 3162.0 | +4.49% |
| TypeScript overall parser throughput | 28 | 4230.7 | 4067.1 | +4.02% |

Regressions above the 5% per-case threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `types.ts` | 12238.0 | 17039.6 | 28.18% |
| `utilities.ts` | 12264.1 | 15470.8 | 20.73% |
| `transform.ts` | 15806.4 | 19099.1 | 17.24% |
| `performanceCore.ts` | 18205.6 | 20930.6 | 13.02% |

This is a smoke baseline, not a final release benchmark. Use 10+ repetitions
for optimization decisions and record the gain against this table until a
broader baseline replaces it.

## Checkpoints

### 2026-06-28 EDT - rejected linear-tail and progress-callback trials

- Repo head: `f087bc4f`
- Trial status: not kept. Source experiments were reverted after measurement.

Linear-tail stack experiment:

- Hypothesis: keep straight-line stack pushes in per-version tail storage and
  materialize persistent graph nodes only when branching, recovery, debugging,
  or fallback stack iteration forces it. This directly tested the current
  architecture direction of avoiding persistent graph-node allocation for
  straight segments while preserving GLR branching.
- Command:

```sh
TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=20 cargo bench benchmark -p tree-sitter-cli --offline
```

- The all-language direct benchmark later failed on an unrelated PHP grammar
  dynamic-library lock permission error, but it had already measured several
  normal workloads.

| Workload | Linear-tail Rust speed | Prior instrumented Rust speed | Movement |
| --- | ---: | ---: | ---: |
| JavaScript normal | 11032 bytes/ms | 19222 bytes/ms | -42.61% |
| Go normal | 11929 bytes/ms | 17769 bytes/ms | -32.87% |
| C++ normal | 7174 bytes/ms | 9338 bytes/ms | -23.17% |
| Java normal | 8589 bytes/ms | 12917 bytes/ms | -33.51% |

Interpretation:

- This version of a linear-tail stack is decisively worse. It avoids some
  immediate graph-node allocation, but per-head tail arrays, tail-prefix
  cloning, and forced materialization on merge/fallback add more overhead than
  they remove.
- Do not retry this shape as a per-version dynamic tail array. A future
  straight-segment design would need fixed inline storage or a representation
  that avoids both persistent node allocation and per-version tail cloning.

Progress-callback hot-loop branch experiment:

- Hypothesis: `parser_check_progress` should return before incrementing and
  wrapping `operation_count` when no progress callback is installed. Normal
  benchmarks have no callback, so the existing arithmetic is pure hot-loop work.
- Patch shape:

```rust
if self_.parse_options.progress_callback.is_none() {
    return true;
}
```

- Trial command with the patch:

```sh
TMPDIR=/private/tmp/tree-sitter-perf-gate-fresh cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

- Baseline rerun command after reverting the patch:

```sh
TMPDIR=/private/tmp/tree-sitter-perf-gate-baseline cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Patched Rust | Patched C | Patched delta | Baseline Rust | Baseline C | Baseline delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| TypeScript normal | 28641.7 | 22254.3 | +28.70% | 27842.6 | 24249.4 | +14.82% |
| JavaScript normal | 19169.6 | 15939.7 | +20.26% | 19323.9 | 15770.1 | +22.54% |
| Python normal | 12248.5 | 11200.2 | +9.36% | 12725.4 | 10555.1 | +20.56% |
| Go normal | 16841.6 | 13171.7 | +27.86% | 16721.7 | 13456.1 | +24.27% |
| Rust normal | 19756.6 | 15945.7 | +23.90% | 20209.7 | 16659.7 | +21.31% |
| C++ normal | 7610.9 | 9955.5 | -23.55% | 7959.9 | 10574.3 | -24.72% |
| Java normal | 10157.6 | 11134.9 | -8.78% | 10250.6 | 11290.0 | -9.21% |
| Overall normal | 18936.9 | 15781.8 | +19.99% | 19043.6 | 15952.8 | +19.37% |

Interpretation:

- The patched run's overall Rust-vs-C delta is slightly higher, but absolute
  Rust throughput is lower overall and lower in JavaScript, Python, Rust, C++,
  and Java. The apparent delta gain mostly comes from C-side movement between
  runs.
- This is not a reliable kept optimization. The patch was reverted.
- The main remaining target-language misses are still C++ and Java normal
  parsing, while the full seven-language normal aggregate is already near the
  20% target because TypeScript, JavaScript, Go, and Rust are strong.

C++ normal sample profile:

- Command:

```sh
TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=cpp TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=10000 target/release/deps/benchmark-cbf7a217e4c2dbe8 >/private/tmp/tree-sitter-cpp-profile2-bench.out 2>/private/tmp/tree-sitter-cpp-profile2-bench.err & pid=$!; sleep 0.1; sample $pid 5 -file /private/tmp/tree-sitter-cpp-profile2.sample >/private/tmp/tree-sitter-cpp-sample2.out 2>/private/tmp/tree-sitter-cpp-sample2.err; wait $pid
```

- Benchmark result during sampling:

| Workload | Speed |
| --- | ---: |
| C++ `marker-index.h` | 13301 bytes/ms |
| C++ `rule.cc` | 12387 bytes/ms |
| C++ normal average | 12844 bytes/ms |

- Sample: 3850 main-thread samples from `/usr/bin/sample`.

| Area | Samples | Approx share |
| --- | ---: | ---: |
| Generated `ts_lex` in `cpp.dylib` | 1557 | 40.44% |
| `ts_lex_keywords` in `cpp.dylib` | 123 | 3.19% |
| `parser_reduce` region | 894 | 23.22% |
| `subtree_new_node_in_arena` under reduce | 341 | 8.86% |
| `subtree_summarize_children` under arena node creation | 272 | 7.06% |
| `stack_pop_count_into` under reduce | 140 | 3.64% |
| `stack_node_new` under reduce | 87 | 2.26% |
| `stack_renumber_version` region | 137 | 3.56% |
| `parser_balance_subtree` region | 97 | 2.52% |
| `parser_shift` region | 81 | 2.10% |

Interpretation:

- C++ is lexer-heavy enough that a parser-only reduction or stack experiment
  cannot plausibly close the C++ gap by itself. This supports the existing
  future-candidate note about generated-lexer contract work once parser
  construction work is not the dominant remaining issue.
- The parser-side C++ cost still matches the old profile: reduction
  construction, arena node creation, child summarization, stack pop, and stack
  node creation. The rejected linear-tail stack trial made this worse, so the
  next parser-side attempt should remove materialization/summarization work
  rather than wrap stack pushes in another side structure.

Java normal sample profile and UTF-8 direct-decode trial:

- Java sample command:

```sh
TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=java TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=10000 target/release/deps/benchmark-cbf7a217e4c2dbe8 >/private/tmp/tree-sitter-java-profile-bench.out 2>/private/tmp/tree-sitter-java-profile-bench.err & pid=$!; sleep 0.1; sample $pid 5 -file /private/tmp/tree-sitter-java-profile.sample >/private/tmp/tree-sitter-java-sample.out 2>/private/tmp/tree-sitter-java-sample.err; wait $pid
```

- Benchmark result during sampling:

| Workload | Speed |
| --- | ---: |
| Java `LargeService.java` | 14309 bytes/ms |
| Java `Service.java` | 15363 bytes/ms |

- Sample: 1490 main-thread samples from `/usr/bin/sample`.

| Area | Samples | Approx share |
| --- | ---: | ---: |
| Generated `ts_lex` in `java.dylib` | 472 | 31.68% |
| `ts_lex_keywords` in `java.dylib` | 59 | 3.96% |
| `lexer_do_advance` under generated lexers | 140 | 9.40% |
| `parser_reduce` region | 377 | 25.30% |
| `subtree_new_node_in_arena` under reduce | 161 | 10.81% |
| `subtree_summarize_children` under arena node creation | 129 | 8.66% |
| `stack_pop_count_into` under reduce | 56 | 3.76% |
| `stack_node_new` under reduce | 48 | 3.22% |
| `stack_renumber_version` region | 56 | 3.76% |
| `parser_balance_subtree` region | 29 | 1.95% |

- Trial: specialize `lexer_get_lookahead` for UTF-8, handling ASCII directly
  and calling `utf8_next` without the C-compatible decoder function pointer.
- Command:

```sh
TMPDIR=/private/tmp/tree-sitter-utf8-fastpath cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta | Baseline Rust | Baseline C | Baseline delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| TypeScript normal | 26930.6 | 23079.6 | +16.69% | 27842.6 | 24249.4 | +14.82% |
| JavaScript normal | 17672.6 | 16356.6 | +8.05% | 19323.9 | 15770.1 | +22.54% |
| Python normal | 13355.7 | 11072.9 | +20.62% | 12725.4 | 10555.1 | +20.56% |
| Go normal | 16912.2 | 13451.1 | +25.73% | 16721.7 | 13456.1 | +24.27% |
| Rust normal | 19614.4 | 15822.2 | +23.97% | 20209.7 | 16659.7 | +21.31% |
| C++ normal | 7088.2 | 10473.5 | -32.32% | 7959.9 | 10574.3 | -24.72% |
| Java normal | 11015.7 | 11236.2 | -1.96% | 10250.6 | 11290.0 | -9.21% |
| Overall normal | 18513.7 | 16072.7 | +15.19% | 19043.6 | 15952.8 | +19.37% |

Interpretation:

- Java confirms that generated lexing and lexer callbacks are meaningful for
  weak languages, but parser reduction/materialization is still a similarly
  large bucket.
- The direct UTF-8 lookahead trial is not keepable. It improves Java and
  Python in this run, but regresses JavaScript and C++ enough to lower the
  seven-language aggregate. The likely issue is that moving UTF-8 dispatch into
  the hot lookahead function worsens code layout or branch prediction more than
  the ASCII direct path saves.
- Do not retry this exact shape. A lexer-side attempt needs stronger evidence
  from generated-lexer call patterns or a contract-level change that reduces
  callback frequency, not just a different internal UTF-8 decoder branch.

Stack-node pool reset trial:

- Hypothesis: `stack_node_new_with_payload` fully initializes the eight inline
  `StackLink` slots on every push. Nodes reused from `node_pool` already contain
  valid old `StackLink` values that are unreachable behind the reset
  `link_count`, so the pooled-node path can reset only live scalar fields and
  avoid rewriting the whole links array.
- Command:

```sh
TMPDIR=/private/tmp/tree-sitter-stack-node-reuse cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta | Baseline Rust | Baseline C | Baseline delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| TypeScript normal | 28884.0 | 22573.7 | +27.95% | 27842.6 | 24249.4 | +14.82% |
| JavaScript normal | 19527.4 | 16305.7 | +19.76% | 19323.9 | 15770.1 | +22.54% |
| Python normal | 13324.6 | 10439.5 | +27.64% | 12725.4 | 10555.1 | +20.56% |
| Go normal | 16600.8 | 13537.3 | +22.63% | 16721.7 | 13456.1 | +24.27% |
| Rust normal | 20569.3 | 16722.4 | +23.00% | 20209.7 | 16659.7 | +21.31% |
| C++ normal | 7570.9 | 10381.7 | -27.07% | 7959.9 | 10574.3 | -24.72% |
| Java normal | 9757.2 | 11264.3 | -13.38% | 10250.6 | 11290.0 | -9.21% |
| Overall normal | 19402.1 | 15851.7 | +22.40% | 19043.6 | 15952.8 | +19.37% |

Interpretation:

- The aggregate looks better, but the two target misses both regress in
  absolute Rust throughput: C++ drops from 7959.9 to 7570.9 bytes/ms, and Java
  drops from 10250.6 to 9757.2 bytes/ms.
- This is not a keepable universal optimization. It was reverted. The result is
  consistent with earlier node-pool tuning notes: changing node allocation/reset
  behavior can move aggregate noise or help easy workloads without closing the
  weak-language gap.

Arena-source child summarization trial:

- Hypothesis: fresh reductions build arena nodes by copying child subtrees into
  the arena and then immediately reading those arena children back during
  `subtree_summarize_children`. The reduction-builder child span is already hot,
  so summarizing from that source span could reduce cache traffic while keeping
  the same arena layout.
- Patch shape: factor the summarization body to accept an explicit
  `&[Subtree]`; have `subtree_new_node_in_arena` summarize from the incoming
  child span after initializing the heap data, while still copying the children
  into the arena allocation for storage.
- Command:

```sh
TMPDIR=/private/tmp/tree-sitter-arena-source-summary cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta | Baseline Rust | Baseline C | Baseline delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| TypeScript normal | 28164.8 | 22503.3 | +25.16% | 27842.6 | 24249.4 | +14.82% |
| JavaScript normal | 18497.8 | 15382.8 | +20.25% | 19323.9 | 15770.1 | +22.54% |
| Python normal | 12820.5 | 10630.4 | +20.60% | 12725.4 | 10555.1 | +20.56% |
| Go normal | 16148.4 | 13273.3 | +21.66% | 16721.7 | 13456.1 | +24.27% |
| Rust normal | 19806.2 | 15634.6 | +26.68% | 20209.7 | 16659.7 | +21.31% |
| C++ normal | 7767.8 | 9744.9 | -20.29% | 7959.9 | 10574.3 | -24.72% |
| Java normal | 9199.1 | 11139.0 | -17.42% | 10250.6 | 11290.0 | -9.21% |
| Overall normal | 18697.7 | 15484.2 | +20.75% | 19043.6 | 15952.8 | +19.37% |

Interpretation:

- The change does not close the weak-language gap and lowers the absolute
  seven-language Rust average. It was reverted.
- The likely cost is worse code shape around the already-large summarizer, not
  cold arena reads. Future summarizer work should remove fields or defer
  materialization rather than redirecting the same summarization work to a
  different child slice.

Fresh C++ sample and in-place reduction trial:

- C++ sample command:

```sh
TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=cpp TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=20000 target/release/deps/benchmark-cbf7a217e4c2dbe8 >/private/tmp/tree-sitter-cpp-profile3-bench.out 2>/private/tmp/tree-sitter-cpp-profile3-bench.err & pid=$!; sleep 0.1; sample $pid 5 -file /private/tmp/tree-sitter-cpp-profile3.sample >/private/tmp/tree-sitter-cpp-sample3.out 2>/private/tmp/tree-sitter-cpp-sample3.err; wait $pid
```

- Benchmark result during sampling:

| Workload | Speed |
| --- | ---: |
| C++ `marker-index.h` | 13522 bytes/ms |
| C++ `rule.cc` | 12594 bytes/ms |

- `sample` again showed generated `ts_lex`, reduction construction,
  `subtree_summarize_children`, `lexer_do_advance`, `stack_node_new`,
  `stack_pop_count_into`, `ts_lex_keywords`, and `stack_renumber_version`.
  Parse-table lookup/action dispatch did not appear as a named hot symbol.
- Trial: for fresh parses with one active version and non-fragile reduce
  actions, pop a linear stack chain in place instead of creating a temporary
  version and immediately renumbering it back over the original version.
- Full seven-language command for the broad variant:

```sh
TMPDIR=/private/tmp/tree-sitter-reduce-in-place cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Broad trial Rust | Broad trial C | Broad trial delta | Baseline Rust | Baseline C | Baseline delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| TypeScript normal | 28809.3 | 23176.7 | +24.30% | 27842.6 | 24249.4 | +14.82% |
| JavaScript normal | 19509.1 | 16366.1 | +19.20% | 19323.9 | 15770.1 | +22.54% |
| Python normal | 13414.3 | 11273.5 | +18.99% | 12725.4 | 10555.1 | +20.56% |
| Go normal | 16712.0 | 13347.7 | +25.21% | 16721.7 | 13456.1 | +24.27% |
| Rust normal | 21178.3 | 16179.2 | +30.90% | 20209.7 | 16659.7 | +21.31% |
| C++ normal | 8025.0 | 10241.9 | -21.65% | 7959.9 | 10574.3 | -24.72% |
| Java normal | 9376.7 | 11256.3 | -16.70% | 10250.6 | 11290.0 | -9.21% |
| Overall normal | 19492.0 | 16139.3 | +20.77% | 19043.6 | 15952.8 | +19.37% |

- Narrowed trial: same idea, but only for reductions with `count > 1`.
- Targeted weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-reduce-in-place-count2 cargo xtask perf-gate --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Narrow trial Rust | Narrow trial C | Narrow trial delta |
| --- | ---: | ---: | ---: |
| C++ normal | 7084.8 | 10278.2 | -31.07% |
| Java normal | 9220.3 | 11469.1 | -19.61% |
| C++ + Java normal | 7422.6 | 10492.3 | -29.26% |

Interpretation:

- The broad in-place reduction variant improved aggregate throughput and
  slightly improved C++, but it regressed Java too much to keep.
- Restricting the change to reductions with more than one child made both C++
  and Java worse, so the problem is not simply over-applying it to cheap
  one-child reductions.
- Do not retry active-head in-place reduction with immediate release of the old
  stack path. The useful future version of this idea would need delayed old-head
  release or a different stack representation that avoids both temporary
  version creation and eager release churn.

- Delayed-release variant: add a stack-owned retired-node list, rewrite the
  active head in place, and release retired heads only at `stack_clear`.
- Targeted weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-reduce-in-place-deferred-cpp-java cargo xtask perf-gate --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Delayed trial Rust | Delayed trial C | Delayed trial delta |
| --- | ---: | ---: | ---: |
| C++ normal | 6666.6 | 10382.8 | -35.79% |
| Java normal | 7117.0 | 11024.9 | -35.45% |
| C++ + Java normal | 6750.5 | 10503.0 | -35.73% |

Interpretation:

- Delaying release made the weak languages much worse, probably by increasing
  live stack/subtree pressure until reset. This closes the active-head in-place
  reduction family for now.
- Future stack work should avoid creating the old head in the first place,
  rather than creating it and deciding whether to release it immediately or
  later.

Lexer no-log advance callback split:

- Hypothesis: generated lexers call `TSLexer::advance` frequently, and normal
  benchmarks have no logger. Split `ts_lexer__advance` into no-log and logging
  callbacks, install the no-log callback by default, and switch callback
  pointers in `ts_parser_set_logger`.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Targeted lexer-heavy command:

```sh
TMPDIR=/private/tmp/tree-sitter-lexer-advance-nolog-cpp-java-js cargo xtask perf-gate --language cpp --language java --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| C++ normal | 6439.8 | 10299.9 | -37.48% |
| Java normal | 9231.2 | 10813.9 | -14.64% |
| JavaScript normal | 19215.4 | 15656.1 | +22.73% |
| C++ + Java + JavaScript normal | 17926.8 | 15347.2 | +16.81% |

Interpretation:

- Splitting the callback makes C++ substantially worse and does not recover
  Java. It was reverted.
- The likely cost is worse generated-lexer call target/code layout rather than
  the removed logger branch. Future lexer work should target callback
  frequency or generated lexer structure, not another internal advance callback
  split.

Lexer single-range `mark_end` fast path:

- Hypothesis: generated lexers call `TSLexer::mark_end` frequently, and normal
  benchmarks use one included range. In that case the included-range-boundary
  check cannot select a previous range, so `mark_end` can directly assign
  `token_end_position = current_position`.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Targeted lexer-heavy command:

```sh
TMPDIR=/private/tmp/tree-sitter-mark-end-single-range-cpp-java-js cargo xtask perf-gate --language cpp --language java --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| C++ normal | 6623.0 | 9726.8 | -31.91% |
| Java normal | 8881.8 | 11195.8 | -20.67% |
| JavaScript normal | 19033.8 | 16108.4 | +18.16% |
| C++ + Java + JavaScript normal | 17807.9 | 15724.6 | +13.25% |

Interpretation:

- The one-range `mark_end` fast path regressed the targeted workloads and was
  reverted.
- The generated-lexer callback hot spot is not helped by small branch removal
  in individual callbacks. Treat lexer callback micro-fast-paths as closed
  unless a future profile shows a different callback shape.

Lexer cold logging helper:

- Hypothesis: keep the same `TSLexer::advance` callback target, but move the
  logging-only formatting block into a `#[cold]` helper so the no-logger hot
  function is smaller without changing generated-lexer call targets.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Targeted lexer-heavy command:

```sh
TMPDIR=/private/tmp/tree-sitter-lexer-cold-log-cpp-java-js cargo xtask perf-gate --language cpp --language java --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| C++ normal | 6951.5 | 6445.1 | +7.86% |
| Java normal | 9130.2 | 11144.3 | -18.07% |
| JavaScript normal | 19280.8 | 15825.7 | +21.83% |
| C++ + Java + JavaScript normal | 18097.4 | 15074.7 | +20.05% |

Interpretation:

- The positive C++ delta is not useful because the C-side comparison was
  anomalously slow. Absolute Rust throughput regressed for C++ and Java versus
  the current baseline, so the patch was reverted.
- Moving the logging block out of line does not solve the generated-lexer
  callback cost. Do not pursue another callback-local logging/code-layout
  variant without a new profile.

Fresh current-source baseline and inline first arena page trial:

- Current-source baseline command:

```sh
TMPDIR=/private/tmp/tree-sitter-current-baseline-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Current Rust | Current C | Current delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 27035.9 | 23104.0 | +17.02% |
| JavaScript normal | 18443.7 | 15472.0 | +19.21% |
| Python normal | 12746.9 | 10793.5 | +18.10% |
| Go normal | 16900.8 | 13936.0 | +21.27% |
| Rust normal | 20789.3 | 17013.5 | +22.19% |
| C++ normal | 7800.2 | 10493.3 | -25.67% |
| Java normal | 10960.9 | 11472.3 | -4.46% |
| Overall normal | 18714.4 | 15886.8 | +17.80% |

- Trial: embed the first `TreeArenaPage` and a 16KB first-page buffer inside
  `TreeArena`, replacing the arena-struct + first-page-header +
  first-page-buffer allocation sequence with one arena allocation for common
  one-page parses.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Targeted fixed-overhead/weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-inline-arena-page-python-cpp-java cargo xtask perf-gate --language python --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12566.9 | 10506.1 | +19.62% |
| C++ normal | 6857.4 | 10646.2 | -35.59% |
| Java normal | 8810.0 | 10646.0 | -17.25% |
| Python + C++ + Java normal | 11809.1 | 10517.9 | +12.28% |

Interpretation:

- The inline first arena page did not fix Python's tiny-fixture overhead and
  substantially regressed C++ and Java. It was reverted.
- Avoid embedding the first arena page. The returned tree holding a larger
  arena object appears worse than paying the separate first-page allocation.
  Future fixed-overhead work should target parse/tree lifecycle or benchmarked
  tiny-file-specific operations, not arena object size.

Python normal sample profile:

- Command:

```sh
TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=python TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=30000 target/release/deps/benchmark-cbf7a217e4c2dbe8 >/private/tmp/tree-sitter-python-profile-bench.out 2>/private/tmp/tree-sitter-python-profile-bench.err & pid=$!; sleep 0.1; sample $pid 5 -file /private/tmp/tree-sitter-python-profile.sample >/private/tmp/tree-sitter-python-sample.out 2>/private/tmp/tree-sitter-python-sample.err; wait $pid
```

- The run was interrupted after the `sample` report was written because the
  full 12-case, 30000-repetition benchmark would have taken longer than needed
  for the profile. The partial benchmark output covered seven Python normal
  fixtures before interruption.
- Sample: 3833 main-thread samples from `/usr/bin/sample`.

| Area | Samples | Approx share |
| --- | ---: | ---: |
| `ts_parser_parse` top-of-stack frames | 867 | 22.62% |
| `lexer_do_advance` | 452 | 11.79% |
| `parser_reduce` | 377 | 9.84% |
| `subtree_summarize_children` | 303 | 7.90% |
| Generated `ts_lex` in `python.dylib` | 263 | 6.86% |
| Python external scanner `scan` | 178 | 4.64% |
| `subtree_release` | 173 | 4.51% |
| `stack_node_new` | 168 | 4.38% |
| `stack_pop_count_into` | 129 | 3.37% |
| Python external scanner `deserialize` | 65 | 1.70% |
| `subtree_new_node_in_arena` | 54 | 1.41% |
| `parser_balance_subtree` | 51 | 1.33% |
| `stack_node_release` | 49 | 1.28% |
| `subtree_new_leaf` | 49 | 1.28% |
| `stack_renumber_version` | 35 | 0.91% |
| `parser_shift` | 31 | 0.81% |

Interpretation:

- Python remains split across runtime lexer callbacks, generated lexer/external
  scanner work, reduction, child summarization, and stack mutation. This
  matches the broader seven-language profile instead of revealing a new
  Python-only tree-lifecycle bottleneck.
- `subtree_release` and `ts_tree_delete` are visible because the tiny fixtures
  amplify parse/tree lifecycle overhead, but release is not dominant enough to
  justify another refcount-ordering or arena-release micro-optimization. The
  earlier closed guidance on refcount and node-pool tuning still applies.
- The next Python-relevant work should still remove a parser phase or reduce
  lexer callback frequency. A specialized tree-delete path for arena-owned
  roots is unlikely to close the remaining gap on its own.

Stack-node live-field initialization trial:

- Trial: change `stack_node_new_with_payload` so each new node initializes only
  live scalar fields and the first link slot instead of writing all eight
  inline `StackLink` slots. This is broader than the earlier pooled-node reset
  trial because it also avoids dead-link writes for freshly allocated nodes.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Initial seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-stack-node-live-init cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 30290.5 | 24666.9 | +22.80% |
| JavaScript normal | 18909.9 | 15370.3 | +23.03% |
| Python normal | 13539.1 | 11243.7 | +20.41% |
| Go normal | 17423.0 | 13764.1 | +26.58% |
| Rust normal | 21098.9 | 17072.3 | +23.59% |
| C++ normal | 8112.2 | 9924.6 | -18.26% |
| Java normal | 9594.2 | 11723.6 | -18.16% |
| Overall normal | 19695.3 | 16160.8 | +21.87% |

- A safety issue was found in the initial patch: zero-link nodes could leave
  `links[0]` uninitialized even though `stack_error_cost` can inspect that slot
  for zero-link error-state nodes. The patch was tightened to initialize
  `links[0]` and skip only slots 1 through 7.
- Focused rerun command:

```sh
TMPDIR=/private/tmp/tree-sitter-stack-node-live-init-pcj cargo xtask perf-gate --language python --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Tightened trial Rust | Tightened trial C | Tightened trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 13785.9 | 10766.6 | +28.04% |
| C++ normal | 7063.5 | 10366.2 | -31.86% |
| Java normal | 10023.1 | 11364.5 | -11.80% |
| Python + C++ + Java normal | 12866.3 | 10747.7 | +19.71% |

Interpretation:

- The idea is not keepable despite improving Python. C++ and Java are worse
  than the current-source baseline, and the focused rerun stays below the
  target aggregate for the weak-language set.
- Avoid another stack-node initialization or node-pool reset variant unless a
  new profile shows a different bottleneck. The stack-node write cost is real,
  but reducing dead-link initialization does not preserve broad throughput.

Arena child copy and summary fusion trial:

- Trial: in `subtree_new_node_in_arena`, copy child pointers into the arena
  allocation while computing the parent summary, replacing the separate
  `ptr::copy_nonoverlapping` plus `subtree_summarize_children` pass with one
  source-slice walk.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Focused weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-arena-copy-summary-pcj cargo xtask perf-gate --language python --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 13033.2 | 10487.0 | +24.28% |
| C++ normal | 7803.3 | 10554.5 | -26.07% |
| Java normal | 10229.4 | 11435.6 | -10.55% |
| Python + C++ + Java normal | 12406.8 | 10506.2 | +18.09% |

Interpretation:

- The fused loop does not beat the existing memcpy plus summary pass on the
  languages that need help. Python is slightly above the current baseline and
  C++ is effectively neutral, but Java regresses and the combined weak-language
  set stays below target.
- Keep the existing bulk child copy. Future reduction-construction work must
  remove materialization or summarization, not just fuse pointer copy with the
  summary loop.

Deterministic in-place reduction trial:

- Trial: for fresh parses with exactly one stack version and exactly one reduce
  action, pop a straight-line stack chain directly into the current version,
  build the parent, and continue with the same version. This avoids creating a
  separate reduction version only to immediately `stack_renumber_version` it
  back over the original version. Branched stack pops fall back to the existing
  GLR reduction path.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Initial seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-deterministic-in-place-reduce cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 29381.6 | 25037.1 | +17.35% |
| JavaScript normal | 21340.3 | 16263.7 | +31.21% |
| Python normal | 13197.5 | 10483.2 | +25.89% |
| Go normal | 16220.6 | 12953.1 | +25.23% |
| Rust normal | 20875.9 | 16895.3 | +23.56% |
| C++ normal | 7984.5 | 10693.2 | -25.33% |
| Java normal | 9615.8 | 11599.5 | -17.10% |
| Overall normal | 19914.6 | 16087.0 | +23.79% |

- Follow-up variant: only use the in-place path for reductions with more than
  one child. This tested whether unary reductions lacked enough avoided work to
  pay for immediate stack mutation.
- Focused weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-reduce-count2-pcj cargo xtask perf-gate --language python --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Multi-child trial Rust | Multi-child trial C | Multi-child trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12800.8 | 11020.5 | +16.15% |
| C++ normal | 7891.3 | 10424.4 | -24.30% |
| Java normal | 11246.1 | 11427.3 | -1.59% |
| Python + C++ + Java normal | 12250.5 | 10984.1 | +11.53% |

Interpretation:

- The architecture direction has real signal: the broad deterministic in-place
  path materially improves JavaScript and Python and clears the seven-language
  aggregate target in that run.
- It is still not keepable. The broad guard regresses Java and Go absolute Rust
  throughput versus the current-source baseline; the multi-child guard recovers
  Java but loses the Python and aggregate gain.
- A future version needs a better safety/profitability predicate than
  "single action" or child count. Useful candidates are reduction-chain shape,
  whether the old version would merge with the reduction version, and whether
  the language/state tends to produce branchy reductions. Do not reapply this
  exact guard.

Deterministic in-place reduction warm-up trial:

- Instrumentation: count deterministic fresh reductions by child-count bucket
  and whether the stack could pop the reduction through a linear chain.
- Commands:

```sh
TREE_SITTER_REDUCE_STATS=1 TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=python TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=5 cargo bench benchmark -p tree-sitter-cli --offline
TREE_SITTER_REDUCE_STATS=1 TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=java TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=5 target/release/deps/benchmark-cbf7a217e4c2dbe8
TREE_SITTER_REDUCE_STATS=1 TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=cpp TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=5 target/release/deps/benchmark-cbf7a217e4c2dbe8
TREE_SITTER_REDUCE_STATS=1 TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=go TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=5 target/release/deps/benchmark-cbf7a217e4c2dbe8
```

| Workload | Deterministic buckets 0,1,2,3,4,5,6,7+ | Linear buckets 0,1,2,3,4,5,6,7+ |
| --- | --- | --- |
| Python normal | `[0, 164670, 64655, 32310, 10500, 3220, 2075, 305]` | `[0, 164625, 64655, 32285, 10500, 3220, 2075, 305]` |
| Java normal | `[0, 2580, 730, 840, 290, 10, 0, 30]` | `[0, 2545, 730, 835, 280, 10, 0, 30]` |
| C++ normal | `[0, 8000, 4500, 2725, 640, 55, 0, 0]` | `[0, 7995, 4485, 2715, 635, 55, 0, 0]` |
| Go normal | `[0, 72830, 35400, 26930, 4520, 1525, 265, 40]` | `[0, 66700, 34060, 24890, 4455, 1460, 255, 40]` |

- Interpretation: Python and Go have very high deterministic-linear volume,
  while Java has little. A warm-up threshold can avoid applying the in-place
  path to small/Java-like parses while still enabling large deterministic
  workloads.

- Trial: reintroduce deterministic in-place reduction only after
  `10_000` deterministic single-version reductions in the current parse.
- Focused weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 10k Rust | Warm-up 10k C | Warm-up 10k delta |
| --- | ---: | ---: | ---: |
| Python normal | 12934.1 | 10494.8 | +23.24% |
| Go normal | 17077.9 | 13929.9 | +22.60% |
| C++ normal | 7491.3 | 10514.2 | -28.75% |
| Java normal | 10741.2 | 11580.7 | -7.25% |
| Python + Go + C++ + Java normal | 14355.6 | 12031.2 | +19.32% |

- Seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 10k Rust | Warm-up 10k C | Warm-up 10k delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 28330.9 | 24961.6 | +13.50% |
| JavaScript normal | 20191.7 | 15196.9 | +32.87% |
| Python normal | 12990.0 | 10578.9 | +22.79% |
| Go normal | 16690.3 | 13522.2 | +23.43% |
| Rust normal | 20457.0 | 16378.1 | +24.90% |
| C++ normal | 7711.8 | 10507.2 | -26.60% |
| Java normal | 10399.3 | 11237.7 | -7.46% |
| Overall normal | 19448.0 | 15863.6 | +22.60% |

- Follow-up threshold trials:

| Threshold | Workload | Trial Rust | Trial C | Trial delta |
| ---: | --- | ---: | ---: | ---: |
| 50,000 | Overall seven-language normal | 19371.6 | 16269.0 | +19.07% |
| 25,000 | Python + Go + C++ + Java normal | 13524.6 | 12129.9 | +11.50% |

Interpretation:

- The `10_000` warm-up is the best measured in-place reduction variant so far.
  It preserves the aggregate seven-language target and avoids the catastrophic
  Java collapse from the broad in-place trial.
- It is still not perfect: C++ remains weak and Java's absolute Rust throughput
  is below the current-source baseline. Treat this as a candidate performance
  win, not a completed architecture solution.
- Higher thresholds skip too much of the profitable path and fall below target.
  The warm-up predicate is crude but better than action-count or child-count
  alone.

Validation status for the kept candidate source:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test --all
cargo test -p tree-sitter-cli --lib tests::detect_language::detect_language_by_double_barrel_file_extension -- --nocapture
cargo test -p tree-sitter-cli --lib tests::detect_language::detect_language_by_double_barrel_file_extension -- --exact # clean f087bc4f worktree
cargo test -p tree-sitter --lib
```

- `cargo fmt --check --all`: passed.
- `cargo check -p tree-sitter --lib --offline`: passed.
- `cargo test -p tree-sitter --lib`: passed.
- `cargo test --all`: failed in the CLI language-detection tests only:
  `detect_language_by_double_barrel_file_extension`,
  `detect_language_by_first_line_regex`,
  `detect_language_without_file_extension`, and
  `detect_language_without_filename`.
- The focused double-barrel detect-language test also fails by itself with
  `left: None` and `right: Some("source.blade")`.
- The same focused test was reproduced in a clean detached worktree at
  `f087bc4f`, so the CLI detect-language failure is baseline behavior for this
  checkout and not caused by the current parser/stack diff.

Action-trace instrumentation and deterministic-chain in-place trial:

- Temporary instrumentation: `TREE_SITTER_ACTION_TRACE_STATS=1`, counting
  consecutive deterministic single-version reductions before the next terminal
  action. The probe was removed after measurement.
- Command template:

```sh
TREE_SITTER_ACTION_TRACE_STATS=1 TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=<language> TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=5 target/release/deps/benchmark-cbf7a217e4c2dbe8
```

| Workload | Reduce-chain buckets 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15+ | Max chain |
| --- | --- | ---: |
| TypeScript normal | `[0, 26255, 31610, 18700, 17600, 5105, 5240, 1615, 1315, 65, 55, 15, 10, 0, 0, 0]` | 12 |
| JavaScript normal | `[0, 53335, 43110, 33985, 24345, 11040, 7795, 2455, 1120, 245, 105, 25, 10, 0, 5, 5]` | 16 |
| Python normal | `[0, 45355, 23210, 13205, 10150, 12260, 4385, 1910, 315, 155, 25, 25, 0, 25, 0, 0]` | 13 |
| Go normal | `[0, 16710, 12805, 10175, 5340, 2165, 2315, 2125, 890, 55, 10, 0, 0, 0, 0, 0]` | 10 |
| Rust normal | `[0, 10200, 7285, 9520, 7240, 1430, 500, 240, 40, 15, 10, 10, 0, 0, 0, 10]` | 48 |
| C++ normal | `[0, 3125, 1915, 1260, 720, 310, 120, 5, 0, 0, 0, 0, 0, 0, 0, 0]` | 7 |
| Java normal | `[0, 335, 415, 210, 220, 185, 75, 50, 10, 0, 0, 0, 0, 0, 0, 0]` | 8 |

- Trial: replace the whole-parse `10_000` warm-up predicate with a local
  deterministic-chain predicate, enabling in-place reduction only from the
  third reduction in a deterministic chain onward.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-chain3-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Chain>=3 trial Rust | Chain>=3 trial C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 5713.9 | 6911.3 | -17.33% |
| Go normal | 9435.7 | 8407.7 | +12.23% |
| C++ normal | 5106.3 | 6140.4 | -16.84% |
| Java normal | 6369.1 | 6207.3 | +2.61% |
| Python + Go + C++ + Java normal | 7140.8 | 7565.5 | -5.61% |

Interpretation:

- Do not pursue the local chain-threshold predicate. It avoids many isolated
  reductions, but Python collapses and the weak-language aggregate falls below
  C.
- The chain histogram weakens the case for an action-trace cache as a universal
  fix. C++ and Java have short chains and low total deterministic-chain volume,
  while the large-chain languages are already the ones helped by the broader
  `10_000` warm-up predicate.
- Future action-trace work would need to remove a much larger phase than
  action dispatch, such as combining goto lookup, stack mutation, and parent
  construction for a whole precomputed trace. A cache that only skips action
  table dispatch is unlikely to move the target languages.

Single-allocation tree-arena page trial:

- Trial: allocate each `TreeArenaPage` header and its bump buffer in one
  allocation instead of allocating the page header and contents separately. This
  targets allocation count in fresh arena-backed reduction construction without
  embedding a larger first page in `TreeArena`.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-arena-singlealloc-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Single-allocation page Rust | Single-allocation page C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12482.0 | 10400.0 | +20.02% |
| Go normal | 16657.5 | 13607.7 | +22.41% |
| C++ normal | 7614.8 | 10526.7 | -27.66% |
| Java normal | 5007.2 | 10905.3 | -54.08% |
| Python + Go + C++ + Java normal | 13801.1 | 11845.9 | +16.50% |

Interpretation:

- This is not keepable. It preserves Python and Go but collapses Java and keeps
  C++ weak.
- Along with the rejected inline-first-page trial, this closes arena page-shape
  tuning for now. The remaining reduction-construction cost is not solved by
  reducing arena page allocation count.

Subtree and tree-arena refcount ordering trial:

- Trial: replace `SeqCst` refcount operations with the standard intrusive
  refcount pattern: relaxed increments, release decrements, and an acquire
  fence before freeing on the final decrement. This targeted retain/release
  traffic in reduction and stack cleanup without changing ownership semantics.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-refcount-order-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Refcount-order trial Rust | Refcount-order trial C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12218.3 | 10986.2 | +11.21% |
| Go normal | 16824.7 | 13330.3 | +26.21% |
| C++ normal | 7581.0 | 10480.8 | -27.67% |
| Java normal | 10637.3 | 10526.6 | +1.05% |
| Python + Go + C++ + Java normal | 13873.7 | 12053.1 | +15.10% |

Interpretation:

- This is not keepable. Go and Java move positively in this run, but Python
  regresses enough that the focused aggregate misses the target.
- Do not weaken refcount ordering as a standalone optimization. If refcount
  traffic is revisited, the stronger candidate is reducing retains/releases or
  avoiding concrete subtree materialization, not changing atomic ordering.

Byte-position in-place reduction gate trial:

- Trial: replace the `10_000` deterministic-reduction warm-up with a byte
  progress predicate, enabling in-place reductions only after the stack position
  reaches 16 KiB. This was meant to skip the small C++/Java benchmark files
  entirely while enabling large TypeScript/JavaScript/Python/Go/Rust files.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-byte16k-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Byte>=16KiB trial Rust | Byte>=16KiB trial C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12967.1 | 10900.5 | +18.96% |
| Go normal | 16801.6 | 14006.9 | +19.95% |
| C++ normal | 7695.0 | 10199.9 | -24.56% |
| Java normal | 9761.5 | 10472.7 | -6.79% |
| Python + Go + C++ + Java normal | 14280.5 | 12268.3 | +16.40% |

Interpretation:

- This is not keepable. It protects small C++/Java parses from the broad
  in-place path, but it skips too much profitable work in Python and Go and
  falls below the current `10_000` reduction warm-up result.
- Do not replace the reduction-count warm-up with a simple byte-position gate.
  Any future gate needs to combine parse size with reduction density or
  language/state shape, not just input progress.

Warm-up dispatch rewrite trial:

- Trial: keep the `10_000` deterministic-reduction warm-up but avoid calling
  the in-place helper before the threshold is reached. The dispatch computed
  the deterministic fresh single-version predicate once, incremented the
  counter, and called the in-place helper only after the counter crossed the
  warm-up threshold.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup-dispatch-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Dispatch rewrite Rust | Dispatch rewrite C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 13317.9 | 10650.6 | +25.04% |
| Go normal | 14825.7 | 13183.5 | +12.46% |
| C++ normal | 7787.0 | 10610.4 | -26.61% |
| Java normal | 9509.8 | 11359.5 | -16.28% |
| Python + Go + C++ + Java normal | 13665.7 | 11823.9 | +15.58% |

Interpretation:

- This is not keepable. The refactor helps Python in this run but loses too
  much Go and Java throughput.
- Keep the original helper-gated `10_000` warm-up shape. Its duplicated checks
  appear cheaper than the altered hot-loop code shape for the broader focused
  set.

Warm-up counter plain-increment micro-trial:

- Trial idea: replace `deterministic_reduction_count.saturating_add(1)` with a
  plain increment in the warm-up counter.
- Status: abandoned before measurement. The benchmark run was interrupted, and
  the code was restored to `saturating_add` for defensive correctness.
- Do not retry this as a performance optimization unless the counter is
  redesigned to make overflow behavior explicit.

Warm-up threshold `5_000` follow-up:

- Trial: keep the in-place deterministic reduction path and lower the
  deterministic-reduction warm-up from `10_000` to `5_000`. The counter remains
  a `saturating_add` counter; overflow behavior is correctness-sensitive and is
  not part of the optimization.
- Validation after keeping the candidate:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
git diff --check
cargo test --all
```

- `cargo fmt --check --all`, `cargo check -p tree-sitter --lib --offline`, and
  `git diff --check` pass.
- `cargo test --all` still fails in the four known baseline CLI
  `detect_language` tests:
  `detect_language_by_double_barrel_file_extension`,
  `detect_language_by_first_line_regex`,
  `detect_language_without_file_extension`, and
  `detect_language_without_filename`.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup5k-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 5k Rust | Warm-up 5k C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12915.6 | 10554.3 | +22.37% |
| Go normal | 17183.5 | 13952.7 | +23.16% |
| C++ normal | 7893.7 | 10669.4 | -26.02% |
| Java normal | 10552.9 | 11266.9 | -6.34% |
| Python + Go + C++ + Java normal | 14427.9 | 12078.5 | +19.45% |

- Seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup5k-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 5k Rust | Warm-up 5k C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 29343.7 | 23814.9 | +23.22% |
| JavaScript normal | 20985.3 | 16459.3 | +27.50% |
| Python normal | 13458.5 | 11261.9 | +19.50% |
| Go normal | 16665.9 | 13067.3 | +27.54% |
| Rust normal | 19788.7 | 15831.6 | +24.99% |
| C++ normal | 11990.9 | 9949.9 | +20.51% |
| Java normal | 13473.3 | 11650.1 | +15.65% |
| Overall normal | 20143.0 | 16166.4 | +24.60% |

Interpretation:

- Keep the `5_000` threshold for now. It is slightly better than the `10_000`
  threshold on the focused weak-language set and materially better on the
  seven-language gate in this run.
- The focused run still shows C++ and Java behind C, so this should not be
  treated as a complete fix for those languages. It is the strongest broad
  candidate so far and should be validated again after any nearby stack or
  reduction changes.

Unary in-place pop fast-path trial:

- Trial: inside the kept warmed in-place reduction path, specialize one-child
  pops where the top stack link is already the single non-extra child. This
  bypasses reserve-size math and subtree reversal for the common unary
  reduction case, while falling back to the existing loop for extras or
  branched stack nodes.
- Fresh focused baseline command on the kept `5_000` candidate:

```sh
TMPDIR=/private/tmp/tree-sitter-current-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Current Rust | Current C | Current delta |
| --- | ---: | ---: | ---: |
| Python normal | 13096.4 | 11246.5 | +16.45% |
| Go normal | 16226.7 | 13400.4 | +21.09% |
| C++ normal | 7253.3 | 10418.6 | -30.38% |
| Java normal | 9425.6 | 8664.2 | +8.79% |
| Python + Go + C++ + Java normal | 14071.6 | 12193.1 | +15.41% |

- Focused trial command:

```sh
TMPDIR=/private/tmp/tree-sitter-unary-in-place-fast-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Unary fast Rust | Unary fast C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 13597.8 | 10940.3 | +24.29% |
| Go normal | 15722.5 | 13628.0 | +15.37% |
| C++ normal | 7584.6 | 10318.5 | -26.50% |
| Java normal | 10659.8 | 11738.2 | -9.19% |
| Python + Go + C++ + Java normal | 14178.0 | 12156.2 | +16.63% |

- Seven-language trial command:

```sh
TMPDIR=/private/tmp/tree-sitter-unary-in-place-fast-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Unary fast Rust | Unary fast C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 27570.5 | 23000.6 | +19.87% |
| JavaScript normal | 20876.5 | 16587.3 | +25.86% |
| Python normal | 13623.8 | 11132.0 | +22.38% |
| Go normal | 16751.3 | 13504.7 | +24.04% |
| Rust normal | 19967.4 | 16575.9 | +20.46% |
| C++ normal | 7522.6 | 10346.0 | -27.29% |
| Java normal | 8990.7 | 11847.7 | -24.11% |
| Overall normal | 19674.4 | 16201.9 | +21.43% |

Interpretation:

- This is not keepable. The focused set improved slightly against a fresh
  baseline, but the broader gate is worse than the kept `5_000` threshold run
  and Java regresses substantially.
- Do not add a top-link unary fast path inside the warmed in-place pop helper.
  The likely cost is worse branch/code layout in the already-sensitive stack
  pop path, not reserve/reversal overhead.

In-place pop no-pre-reserve trial:

- Trial: remove the explicit `array_reserve` call from
  `stack_pop_count_linear_in_place`, relying on `array_push` to grow the
  parser-owned scratch builder as needed. This tested whether the per-reduction
  `subtree_alloc_size` and reserve-capacity check cost more than the extra
  push-time growth checks.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-no-reserve-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | No-reserve Rust | No-reserve C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12858.5 | 10495.0 | +22.52% |
| Go normal | 15342.9 | 13988.8 | +9.68% |
| C++ normal | 8211.6 | 10519.1 | -21.94% |
| Java normal | 11179.8 | 11466.3 | -2.50% |
| Python + Go + C++ + Java normal | 13727.1 | 12053.1 | +13.89% |

Interpretation:

- This is not keepable. It helps C++ and Java in this focused run, but loses
  too much Python and Go throughput and lowers the focused aggregate.
- Keep the explicit reserve in the in-place stack pop helper. The scratch
  builder's pre-reserve is still beneficial for the broad language set.

Warm-up threshold sweep follow-up:

- Trial: sweep lower warm-up thresholds after keeping `5_000`, while preserving
  the `saturating_add` counter and the same in-place stack/subtree ownership
  behavior.
- `2_500` focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup2500-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 2.5k Rust | Warm-up 2.5k C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 13268.5 | 10609.2 | +25.07% |
| Go normal | 16960.4 | 13749.1 | +23.36% |
| C++ normal | 6215.2 | 10303.5 | -39.68% |
| Java normal | 10926.5 | 11822.9 | -7.58% |
| Python + Go + C++ + Java normal | 14306.0 | 12019.7 | +19.02% |

- `2_500` seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup2500-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 2.5k Rust | Warm-up 2.5k C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 28744.0 | 22694.7 | +26.66% |
| JavaScript normal | 20620.2 | 16120.9 | +27.91% |
| Python normal | 13626.7 | 10894.5 | +25.08% |
| Go normal | 16856.4 | 13447.1 | +25.35% |
| Rust normal | 21363.4 | 16380.6 | +30.42% |
| C++ normal | 7746.0 | 10318.6 | -24.93% |
| Java normal | 11133.5 | 11729.4 | -5.08% |
| Overall normal | 19920.0 | 15918.1 | +25.14% |

- Immediate `5_000` A/B seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup5k-7lang-ab cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 5k Rust | Warm-up 5k C | A/B delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 28342.0 | 23291.0 | +21.69% |
| JavaScript normal | 20399.7 | 15988.6 | +27.59% |
| Python normal | 13336.5 | 11206.6 | +19.01% |
| Go normal | 16677.5 | 13591.2 | +22.71% |
| Rust normal | 21187.0 | 16507.8 | +28.35% |
| C++ normal | 8029.1 | 10320.0 | -22.20% |
| Java normal | 11403.2 | 11168.7 | +2.10% |
| Overall normal | 19679.1 | 16096.4 | +22.26% |

- `3_500` focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup3500-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 3.5k Rust | Warm-up 3.5k C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12920.6 | 10426.0 | +23.93% |
| Go normal | 17082.2 | 13778.9 | +23.97% |
| C++ normal | 7763.6 | 10362.9 | -25.08% |
| Java normal | 11171.0 | 11388.0 | -1.91% |
| Python + Go + C++ + Java normal | 14388.0 | 11924.7 | +20.66% |

- `3_500` seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup3500-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 3.5k Rust | Warm-up 3.5k C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 29514.3 | 23688.0 | +24.60% |
| JavaScript normal | 20762.1 | 16277.8 | +27.55% |
| Python normal | 13488.2 | 11171.1 | +20.74% |
| Go normal | 16940.2 | 13755.3 | +23.15% |
| Rust normal | 21055.2 | 16612.3 | +26.74% |
| C++ normal | 7512.6 | 10685.4 | -29.69% |
| Java normal | 10938.1 | 11366.6 | -3.77% |
| Overall normal | 20019.4 | 16289.9 | +22.89% |

- `4_500` focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup4500-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 4.5k Rust | Warm-up 4.5k C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 13181.7 | 10632.2 | +23.98% |
| Go normal | 14481.1 | 13090.3 | +10.62% |
| C++ normal | 5931.1 | 10242.6 | -42.09% |
| Java normal | 9528.4 | 11359.3 | -16.12% |
| Python + Go + C++ + Java normal | 13213.4 | 11759.6 | +12.36% |

Interpretation:

- Keep `5_000`. The lower thresholds can raise the broad aggregate in some
  runs, but they do so while making C++ and Java worse. The immediate `5_000`
  A/B run is the only threshold in this sweep with Java ahead of C and the
  least-bad C++ result.
- Do not lower the threshold solely to chase overall aggregate delta. The goal
  is broader language coverage, and lower thresholds shift work toward the
  already-strong TypeScript/JavaScript/Python/Go/Rust group.

In-place trailing-extra fast-path trial:

- Trial: in `parser_reduce_in_place_after_warmup`, check the last child first
  and call `subtree_array_remove_trailing_extras` only when that child is
  actually extra. This targeted the common no-trailing-extra case in the warmed
  deterministic path, while leaving the general reduction path unchanged.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-trailing-fast-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trailing-fast Rust | Trailing-fast C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12640.8 | 10451.3 | +20.95% |
| Go normal | 14212.5 | 13710.3 | +3.66% |
| C++ normal | 7595.5 | 10445.5 | -27.28% |
| Java normal | 10266.6 | 11661.1 | -11.96% |
| Python + Go + C++ + Java normal | 13071.2 | 11918.8 | +9.67% |

Interpretation:

- This is not keepable. Avoiding the generic remover in the no-extra case
  lowers the focused aggregate and hurts Go and Java substantially.
- Keep the existing `subtree_array_remove_trailing_extras` call in the in-place
  reduce helper. The extra branch and changed code layout cost more than the
  skipped empty-removal work.

In-place reduction next-state shortcut trial:

- Trial: in `parser_reduce_in_place_after_warmup`, keep the builtin error
  guard but otherwise assume reduce-action symbols are nonterminals and call
  `language_lookup` directly. This avoids loading `token_count` and calling
  `ts_language_next_state` for normal warmed reductions.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-nextstate-direct-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Direct next-state Rust | Direct next-state C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 11854.8 | 10979.7 | +7.97% |
| Go normal | 16682.6 | 13561.3 | +23.02% |
| C++ normal | 7317.6 | 10402.1 | -29.65% |
| Java normal | 9704.6 | 11328.1 | -14.33% |
| Python + Go + C++ + Java normal | 13571.2 | 12150.4 | +11.69% |

Interpretation:

- This is not keepable. The extra condition in the current next-state path is
  cheaper than the altered code shape, and Python/Java regress substantially.
- Keep the existing `language_full(...).token_count` guard and
  `ts_language_next_state` fallback in warmed in-place reductions.

Warm-up threshold `7_500` follow-up:

- Trial: raise the kept warmed in-place threshold from `5_000` to `7_500` to
  see whether delaying activation protects C++ and Java while preserving most
  gains in larger TypeScript/JavaScript/Python/Go/Rust files. The older
  `10_000` threshold had already been measured; this tested a midpoint above
  the kept value.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup7500-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 7.5k Rust | Warm-up 7.5k C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12681.9 | 10045.7 | +26.24% |
| Go normal | 16356.6 | 13752.5 | +18.94% |
| C++ normal | 7798.1 | 10203.0 | -23.57% |
| Java normal | 11237.0 | 11132.4 | +0.94% |
| Python + Go + C++ + Java normal | 13993.4 | 11680.9 | +19.80% |

- Seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-warmup7500-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Warm-up 7.5k Rust | Warm-up 7.5k C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 28900.5 | 23007.9 | +25.61% |
| JavaScript normal | 20759.6 | 16438.7 | +26.29% |
| Python normal | 13297.0 | 10817.3 | +22.92% |
| Go normal | 13733.3 | 10953.7 | +25.38% |
| Rust normal | 17238.5 | 13223.0 | +30.37% |
| C++ normal | 6747.9 | 8252.7 | -18.23% |
| Java normal | 8002.3 | 8671.4 | -7.72% |
| Overall normal | 18729.4 | 15097.9 | +24.05% |

Interpretation:

- This is not keepable. The focused set is close, but the full gate loses too
  much absolute Rust throughput and still regresses Java.
- Keep `5_000` as the best measured balance between broad aggregate throughput
  and weak-language coverage.

Unique-path ownership-transfer pop trial:

- Trial: add a warmed in-place pop variant that activates only when every
  popped stack node is uniquely owned (`ref_count == 1`) and linear
  (`link_count == 1`). In that case, collect child payloads without retaining
  them, move the stack head to the predecessor, and free the popped stack nodes
  without releasing their payloads. Shared GLR paths fall back to the existing
  retain/release helper.
- Safety validation before benchmarking:

```sh
cargo test -p tree-sitter --lib --offline
```

- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-transfer-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Transfer Rust | Transfer C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 13198.5 | 10605.4 | +24.45% |
| Go normal | 16444.8 | 13545.2 | +21.41% |
| C++ normal | 7778.5 | 10245.3 | -24.08% |
| Java normal | 10606.3 | 11228.6 | -5.54% |
| Python + Go + C++ + Java normal | 14290.4 | 11928.9 | +19.80% |

- Seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-transfer-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Transfer Rust | Transfer C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 28895.4 | 23394.0 | +23.52% |
| JavaScript normal | 20985.8 | 15924.6 | +31.78% |
| Python normal | 13474.6 | 11199.3 | +20.32% |
| Go normal | 16581.7 | 13379.6 | +23.93% |
| Rust normal | 18949.6 | 15962.0 | +18.72% |
| C++ normal | 6348.0 | 10131.2 | -37.34% |
| Java normal | 9873.1 | 11767.4 | -16.10% |
| Overall normal | 19678.3 | 16008.9 | +22.92% |

Interpretation:

- This is not keepable. It removes retain/release churn on uniquely-owned
  straight stack paths, but C++ and Java regress substantially in the full gate,
  and Rust also loses throughput.
- Do not retry this exact ownership-transfer shape. A future transfer design
  would need stronger language/state predicates or a stack representation that
  avoids changing the hot helper's code shape for weak languages.

Adaptive in-place failure fuse trial:

- Trial: add a parser-local counter for failed warmed in-place pop probes. Once
  a parse exceeds the failure limit, skip the in-place helper and use the
  baseline reduction path for the rest of the parse. This tested whether branchy
  languages like C++ and Java were paying repeated failed-probe costs after the
  warm-up threshold.
- `128`-failure focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-failure-fuse128-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Fuse 128 Rust | Fuse 128 C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12919.6 | 10796.2 | +19.67% |
| Go normal | 15978.5 | 13954.7 | +14.50% |
| C++ normal | 7822.3 | 10603.6 | -26.23% |
| Java normal | 10690.3 | 10182.8 | +4.98% |
| Python + Go + C++ + Java normal | 13969.2 | 12204.1 | +14.46% |

- `16`-failure focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-failure-fuse16-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Fuse 16 Rust | Fuse 16 C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12922.5 | 9857.1 | +31.10% |
| Go normal | 16193.2 | 13639.2 | +18.73% |
| C++ normal | 7478.6 | 10699.9 | -30.11% |
| Java normal | 11173.4 | 11418.4 | -2.15% |
| Python + Go + C++ + Java normal | 14022.5 | 11547.7 | +21.43% |

- `16`-failure seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-failure-fuse16-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Fuse 16 Rust | Fuse 16 C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 28175.4 | 23614.9 | +19.31% |
| JavaScript normal | 19446.5 | 16317.5 | +19.18% |
| Python normal | 13536.7 | 11157.7 | +21.32% |
| Go normal | 16859.3 | 13558.5 | +24.34% |
| Rust normal | 20237.0 | 16007.7 | +26.42% |
| C++ normal | 5539.7 | 10538.0 | -47.43% |
| Java normal | 8706.1 | 11438.0 | -23.88% |
| Overall normal | 19174.5 | 16205.0 | +18.32% |

Interpretation:

- This is not keepable. The adaptive fuse can make a focused run look better
  through C-side movement, but the broad gate collapses C++ and Java and lowers
  the aggregate below the target.
- Failed in-place probes are not the main weak-language cost, or the extra
  parser state/branch overwhelms any skipped probes. Do not retry a simple
  failure-count fuse without first measuring failed-probe rates by language.

In-place pop cold-cleanup trial:

- Trial: move the rare partial-retain cleanup path in
  `stack_pop_count_linear_in_place` into a `#[cold]` helper. This keeps the
  success loop smaller without changing reserve sizing, child collection,
  retain/release behavior, or stack mutation order.
- Focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-cold-cleanup-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cold-cleanup Rust | Cold-cleanup C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12328.1 | 9556.0 | +29.01% |
| Go normal | 16325.6 | 13592.0 | +20.11% |
| C++ normal | 8019.0 | 10510.0 | -23.70% |
| Java normal | 11122.7 | 11220.4 | -0.87% |
| Python + Go + C++ + Java normal | 13808.9 | 11335.7 | +21.82% |

- Seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-in-place-cold-cleanup-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cold-cleanup Rust | Cold-cleanup C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 30679.1 | 23539.3 | +30.33% |
| JavaScript normal | 20899.2 | 15431.8 | +35.43% |
| Python normal | 12763.0 | 10462.7 | +21.99% |
| Go normal | 16205.0 | 13061.8 | +24.06% |
| Rust normal | 21044.1 | 16914.0 | +24.42% |
| C++ normal | 7975.6 | 10417.6 | -23.44% |
| Java normal | 10253.5 | 11358.0 | -9.72% |
| Overall normal | 19841.9 | 15636.3 | +26.90% |

Interpretation:

- This is not keepable. The aggregate delta is high, but Java coverage gets
  worse and the absolute Rust average does not beat the strongest kept `5_000`
  warmed in-place run.
- Keep the cleanup inline in `stack_pop_count_linear_in_place`; splitting the
  failure path is another code-layout tradeoff that does not preserve broad
  language coverage.

Single-slice reduction fast path trial:

- Trial: preserve the existing "pop into a new reduction version, then
  renumber it over the original version" semantics, but specialize the
  single-version, straight-line, single-slice case inside the parser. This
  avoids the full `parser_reduce` loop over pop slices, same-version
  child-selection, max-version handling, and merge scan while leaving stack
  mutation order closer to the baseline than the in-place trial.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Focused weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-single-slice-reduce-pcj cargo xtask perf-gate --language python --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Single-slice trial Rust | Single-slice trial C | Single-slice trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12971.9 | 10589.5 | +22.50% |
| C++ normal | 7844.5 | 9881.4 | -20.61% |
| Java normal | 9857.9 | 11141.9 | -11.52% |
| Python + C++ + Java normal | 12353.5 | 10546.5 | +17.13% |

- Follow-up variant: only use the single-slice fast path for unary reductions,
  so a failed linear probe only checks the top stack link rather than walking a
  deeper reduction chain before falling back.
- Focused weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-single-slice-unary-pcj cargo xtask perf-gate --language python --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Unary-only trial Rust | Unary-only trial C | Unary-only trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12781.8 | 10435.5 | +22.48% |
| C++ normal | 7787.1 | 10536.5 | -26.09% |
| Java normal | 10557.9 | 11323.7 | -6.76% |
| Python + C++ + Java normal | 12202.7 | 10456.1 | +16.70% |

Interpretation:

- Keeping baseline stack mutation order avoids the most severe Java regression
  from the broad in-place trial, but it also loses the large JavaScript/Python
  gains that made the in-place direction interesting.
- Failed linear probes are not the whole problem. Restricting to unary
  reductions improves Java relative to the broad single-slice variant but still
  misses the weak-language target and does not improve C++.
- Do not keep this single-slice wrapper. A successful reduction-control change
  likely needs to skip more of the outer action-loop machinery or use
  profile-guided state/reduction-chain predicates, not just replace the inner
  `parser_reduce` loop for straight pops.

Multi-entry token-cache trial:

- Trial: expand the parser's retained token cache from one entry to a small
  round-robin cache, keeping the existing byte-position, external-scanner-state,
  and `parser_can_reuse_first_leaf` predicates. This tests whether GLR versions
  in branchy languages revisit several nearby lexed positions and can avoid
  generated lexer or external scanner calls.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Four-entry focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-token-cache4-pcjg cargo xtask perf-gate --language python --language cpp --language java --language go --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | 4-entry Rust | 4-entry C | 4-entry delta |
| --- | ---: | ---: | ---: |
| Python normal | 12306.1 | 10818.2 | +13.75% |
| C++ normal | 7961.9 | 10323.4 | -22.87% |
| Java normal | 10927.8 | 11368.6 | -3.88% |
| Go normal | 16664.1 | 13973.1 | +19.26% |
| Python + C++ + Java + Go normal | 13911.3 | 12223.5 | +13.81% |

- Two-entry focused command:

```sh
TMPDIR=/private/tmp/tree-sitter-token-cache2-pcjg cargo xtask perf-gate --language python --language cpp --language java --language go --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | 2-entry Rust | 2-entry C | 2-entry delta |
| --- | ---: | ---: | ---: |
| Python normal | 13061.1 | 11285.7 | +15.73% |
| C++ normal | 7691.2 | 10605.4 | -27.48% |
| Java normal | 10845.8 | 11399.9 | -4.86% |
| Go normal | 17156.3 | 13091.5 | +31.05% |
| Python + C++ + Java + Go normal | 14479.2 | 12121.1 | +19.45% |

Interpretation:

- The cache-size signal is mixed. Remembering more tokens can help Go and Java,
  but the lookup/retention overhead and changed replacement behavior hurt
  Python and C++ enough that neither capacity is keepable.
- A broader token cache is not a universal fix for repeated lexing. Future
  lexing work should measure cache hit rates by language/state before changing
  cache shape again, or target callback frequency directly.

External scanner state-cache trial:

- Trial: cache the serialized state currently loaded in the external scanner
  payload. Before `deserialize`, compare the requested `last_external_token`
  state to the cached live scanner state and skip `deserialize` when they match.
  Invalidate the cache after failed or ignored external scans, and refresh it
  after successful `serialize`.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Focused external-scanner/weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-scanner-state-cache-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 4676.9 | 10812.9 | -56.75% |
| Go normal | 15653.6 | 13157.2 | +18.97% |
| C++ normal | 7981.5 | 10703.5 | -25.43% |
| Java normal | 10786.7 | 11377.2 | -5.19% |
| Python + Go + C++ + Java normal | 7533.2 | 11905.0 | -36.72% |

Interpretation:

- This is decisively worse. Python collapses, so maintaining and comparing the
  cached serialized scanner state costs more than any skipped deserializes, or
  it disrupts scanner-state locality enough to increase total scan work.
- Do not keep a live external-scanner state cache. If external scanner work is
  revisited, measure deserialize/serialize hit opportunities first and prefer
  scanner-specific or grammar-level evidence over a generic payload-state cache.

Lexer single-included-range advance trial:

- Trial: specialize `lexer_do_advance` for the common case where
  `included_range_count == 1`, bypassing the general `lexer_seek_visible_range`
  loop while preserving the existing chunk loading and lookahead decoding path.
  This targets the profiled `lexer_do_advance` cost without changing generated
  lexer callbacks or UTF-8 decoding.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Focused lexer/weak-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-single-range-advance-pgcj cargo xtask perf-gate --language python --language go --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| Python normal | 12658.7 | 11086.4 | +14.18% |
| Go normal | 16986.5 | 12984.6 | +30.82% |
| C++ normal | 7530.7 | 10112.4 | -25.53% |
| Java normal | 9415.9 | 11624.8 | -19.00% |
| Python + Go + C++ + Java normal | 14154.0 | 11951.6 | +18.43% |

Interpretation:

- Bypassing the general included-range loop helps Go in this run, but Python
  and Java regress and the focused weak-language set misses the target.
- Do not keep this single-range `advance` specialization. Along with the
  rejected `mark_end`, logging-layout, and UTF-8 callback trials, this reinforces
  that individual lexer callback micro-fast-paths are not stable enough; future
  lexer work needs to reduce callback frequency or change generated lexer
  structure with profile proof.

Reduction goto-cache trial:

- Trial: add a parser-owned cache for the most recent non-terminal
  `(state, symbol) -> next_state` lookup used after reductions. This targets
  `language_lookup` calls in the reduction path without changing language table
  layout or generated parsers.
- Validation before benchmarking:

```sh
cargo fmt --check --all
cargo check -p tree-sitter --lib --offline
cargo test -p tree-sitter --lib --offline
```

- Seven-language command:

```sh
TMPDIR=/private/tmp/tree-sitter-goto-cache-7lang cargo xtask perf-gate --language typescript --language javascript --language python --language go --language rust --language cpp --language java --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Trial Rust | Trial C | Trial delta |
| --- | ---: | ---: | ---: |
| TypeScript normal | 29913.5 | 24856.2 | +20.35% |
| JavaScript normal | 20096.5 | 15630.7 | +28.57% |
| Python normal | 12957.3 | 10779.7 | +20.20% |
| Go normal | 15587.2 | 13803.4 | +12.92% |
| Rust normal | 14553.7 | 17023.3 | -14.51% |
| C++ normal | 2509.9 | 10635.8 | -76.40% |
| Java normal | 4779.6 | 11460.9 | -58.30% |
| Overall normal | 17912.8 | 16159.1 | +10.85% |

Interpretation:

- This is not keepable. C++ and Java collapse, and Rust regresses hard. The
  extra parser state and cache branch are far more expensive than any repeated
  goto lookup reuse in these workloads.
- Do not pursue a one-entry reduction goto cache. Future parse-table work needs
  measured hit rates or a layout/algorithm change that removes table scanning,
  not an ad hoc cache on the parser hot path.

### 2026-06-28 EDT - parse instrumentation probe

- Repo head: `f087bc4f`
- Instrumentation: `TREE_SITTER_PARSE_STATS=1`, aggregate report emitted when
  the parser is deleted. This does not edit benchmark source code.
- Whole-language normal parse command template:

```sh
TREE_SITTER_PARSE_STATS=1 TREE_SITTER_CORE_IMPL=rust TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=<language> TREE_SITTER_BENCHMARK_KIND_FILTER=normal TREE_SITTER_BENCHMARK_REPETITION_COUNT=20 cargo bench benchmark -p tree-sitter-cli --offline
```

- Languages: TypeScript, JavaScript, Python, Go, Rust, C++, Java.

| Workload | Cases | Avg speed | Single-version samples | Single-version advances | Linear reduce candidates | Multi-slice reduces | Avg reduction-chain length | Max reduction-chain length |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| TypeScript normal | 11 | 25597 bytes/ms | 94.96% | 94.11% | 94.57% | 0.41% | 2.77 | 12 |
| JavaScript normal | 2 | 19222 bytes/ms | 96.93% | 96.44% | 95.78% | 1.00% | 2.72 | 16 |
| Python normal | 12 | 10447 bytes/ms | 99.00% | 98.77% | 98.50% | 0.02% | 2.63 | 13 |
| Go normal | 4 | 17769 bytes/ms | 58.51% | 54.57% | 52.59% | 8.25% | 3.04 | 11 |
| Rust normal | 2 | 17609 bytes/ms | 100.00% | 100.00% | 100.00% | 0.00% | 2.90 | 48 |
| C++ normal | 2 | 9338 bytes/ms | 82.39% | 78.31% | 75.92% | 0.35% | 2.32 | 7 |
| Java normal | 2 | 12917 bytes/ms | 74.55% | 68.66% | 83.17% | 1.57% | 2.82 | 8 |

Materialization counts over 20 repetitions:

| Workload | Materialized nodes | Arena nodes | Heap nodes | 1 child | 2 children | 3 children | 4 children | 5+ children |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| TypeScript normal | 1318280 | 1318280 | 0 | 776760 | 267740 | 211480 | 45820 | 16480 |
| JavaScript normal | 2040280 | 2040280 | 0 | 1172720 | 351700 | 411440 | 88900 | 15520 |
| Python normal | 1199780 | 1199780 | 0 | 669500 | 331020 | 133140 | 43060 | 23060 |
| Go normal | 1290880 | 1290880 | 0 | 710560 | 261200 | 253660 | 53400 | 12060 |
| Rust normal | 423680 | 423680 | 0 | 264180 | 81200 | 51880 | 18300 | 8120 |
| C++ normal | 91880 | 91880 | 0 | 47000 | 27160 | 14680 | 2820 | 220 |
| Java normal | 23340 | 23340 | 0 | 13560 | 3780 | 4620 | 1220 | 160 |

Interpretation:

- The straight-line/common-path stack opportunity is real for TypeScript,
  JavaScript, Python, and Rust normal parses: at least 94% of stack samples and
  reduce pops are single-version candidates in those workloads.
- Go is the important counterexample. It spends 45.43% of advances in
  multi-version states and 8.25% of reduce-pop calls are multi-slice. A
  single-version-only fast path is not enough for the universal target.
- C++ and Java sit between those groups. They still have a majority linear
  path, but stack branching is common enough that a replacement stack model
  must preserve cheap branching and merge behavior rather than treating it as a
  rare fallback.
- The next stack experiment should remove persistent graph-node allocation for
  straight segments while keeping branching first-class. Prior stack-pop
  micro-optimizations do not address this.
- Reduction chains are short but frequent. This supports investigating
  action-trace execution for deterministic reduce chains followed by a terminal
  action.
- Every internal node in these fresh parses is arena-backed after `f087bc4f`, so
  additional allocator tuning is unlikely to explain the remaining gap.

### 2026-06-25 16:17 EDT

- Repo head: `3d976f95`
- Batch base: `5951068a`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core raw-pointer containment cleanups:
  `Avoid transmute for range array init` through
  `Use slice for subtree external token scan`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

Primary run:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25238.1 | 24058.5 | +4.90% |
| TypeScript error parses | 32 | 1653.6 | 1605.7 | +2.98% |
| JavaScript normal parses | 2 | 17241.2 | 16042.2 | +7.47% |
| JavaScript error parses | 37 | 2024.3 | 1933.9 | +4.67% |
| Overall parser throughput | 82 | 2283.3 | 2201.9 | +3.70% |

Per-case regressions over 5% in the primary run:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error crlf-line-endings.py` | 1012.7 | 1160.0 | 12.70% |

Rerun to check whether the per-case regression was stable:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24971.7 | 24000.1 | +4.05% |
| TypeScript error parses | 32 | 1667.9 | 1582.1 | +5.42% |
| JavaScript normal parses | 2 | 17158.7 | 15867.8 | +8.14% |
| JavaScript error parses | 37 | 2042.5 | 1971.1 | +3.62% |
| Overall parser throughput | 82 | 2302.4 | 2197.2 | +4.79% |

Per-case regressions over 5% in the rerun:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2934.4 | 3179.9 | 7.72% |

Prior checkpoint at `5951068a` recorded Rust overall throughput of 2316.9
bytes/ms and a Rust-vs-C delta of +4.10% in its primary rerun. This checkpoint
measured 2283.3 bytes/ms in the first run and 2302.4 bytes/ms in the rerun, so
absolute Rust throughput moved by about -1.45% and -0.63%, respectively. The
Rust-vs-C delta remained positive at +3.70% and +4.79%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, and no parsing table or parser action semantic
  changes.
- The first commit replaces a `transmute` between same-layout range arrays with
  explicit field movement.
- The tree/node/cursor/changed-range/parser/subtree commits move direct
  `ts_subtree_children(...).add(...)` and included-range indexing behind local
  slice helpers. Bounds are still established by the existing surrounding
  control flow; the changes reduce scattered raw pointer dereferences.
- `subtree_children` explicitly returns `&[]` for zero-child subtrees to avoid
  constructing a slice from the null pointer returned by `ts_subtree_children`
  for inline/leaf subtrees.
- The only per-case regression above 5% changed between the two runs
  (`typescript error crlf-line-endings.py` in the first run,
  `javascript error compound-statement-without-trailing-newline.py` in the
  rerun). The touched code is general child-array access rather than
  language-specific or file-specific logic, and both runs remain positive
  overall, so the per-case outliers are treated as measurement noise for this
  checkpoint.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 18:45 EDT

- Repo head: `9a0904a5`
- Batch base: `e5e46384`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small parser pointer/language helper cleanups:
  `Use language helper in token lexing` through
  `Use parser pointer helper in const getters`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24854.8 | 24064.5 | +3.28% |
| TypeScript error parses | 32 | 1643.2 | 1591.4 | +3.26% |
| JavaScript normal parses | 2 | 16830.1 | 15969.8 | +5.39% |
| JavaScript error parses | 37 | 2011.2 | 1927.0 | +4.37% |
| Overall parser throughput | 82 | 2268.0 | 2187.1 | +3.70% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error crlf-line-endings.py` | 975.6 | 1294.2 | 24.61% |
| `javascript error update-authors.sh` | 575.6 | 659.1 | 12.67% |
| `typescript error weird-exprs.rs` | 954.1 | 1068.8 | 10.74% |
| `javascript error python3-grammar.py` | 963.0 | 1038.6 | 7.28% |
| `typescript error release.sh` | 590.7 | 637.1 | 7.27% |

Prior checkpoint at `e5e46384` measured Rust overall throughput of 2307.0
bytes/ms and a Rust-vs-C delta of +4.64%. This checkpoint measured 2268.0
bytes/ms, so absolute Rust throughput moved by about -1.69%. C throughput moved
from 2204.6 to 2187.1 bytes/ms, and the Rust-vs-C delta moved to +3.70%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header changes, and no parser table or parser
  action semantic changes.
- The first four commits replace repeated raw `TSLanguageFull` casts in parser
  hot paths with the existing typed language helper. The helper already performs
  the same cast and unchecked reference formation, so this should not alter
  generated code materially beyond inlining decisions.
- The remaining parser commits centralize nullable tree access and parser
  pointer borrowing at API bodies using `as_ref`/`as_mut` helper functions. The
  affected code preserves the same null checks and control flow.
- The largest slowdowns are individual error fixtures, while every aggregate
  bucket remains faster than C. Because the changed code is mechanical pointer
  reference formation with unchanged parser decisions, these outliers do not
  currently identify a source-level regression in this batch. If they reproduce
  in a later checkpoint, inspect compiler output around helper inlining before
  considering rollback.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 19:16 EDT

- Repo head: `43436438`
- Batch base: `e85423b2`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core visibility cleanups:
  `Keep Rust-only language helpers internal` through
  `Restrict lexer API visibility`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25391.3 | 24946.8 | +1.78% |
| TypeScript error parses | 32 | 1712.8 | 1642.5 | +4.28% |
| JavaScript normal parses | 2 | 17606.7 | 16032.8 | +9.82% |
| JavaScript error parses | 37 | 2092.7 | 1882.5 | +11.17% |
| Overall parser throughput | 82 | 2362.1 | 2210.0 | +6.88% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal transform.ts` | 18526.8 | 24329.4 | 23.85% |
| `typescript normal builderStatePublic.ts` | 17798.9 | 20607.4 | 13.63% |
| `typescript error doc-build.sh` | 580.9 | 661.2 | 12.15% |
| `typescript error clean-old.sh` | 427.8 | 461.6 | 7.34% |

Prior checkpoint at `e85423b2` measured Rust overall throughput of 2268.0
bytes/ms and a Rust-vs-C delta of +3.70%. This checkpoint measured 2362.1
bytes/ms, so absolute Rust throughput moved by about +4.15%. C throughput moved
from 2187.1 to 2210.0 bytes/ms, and the Rust-vs-C delta moved to +6.88%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header changes, and no parser table or parser
  action semantic changes.
- The first five commits remove `#[no_mangle] extern "C"` from Rust-only helper
  functions after checking that their call sites are internal. C-visible API and
  binding symbols were left exported.
- The remaining five commits restrict Rust-only helper visibility to
  `pub(crate)` in `subtree.rs`, `stack.rs`, and `lexer.rs`. This does not change
  function signatures, call sites, control flow, or data representation.
- The two TypeScript normal-case slowdowns are worth watching because normal
  TypeScript throughput had previously been a concern. In this batch, however,
  the edited code is visibility metadata and stale-comment cleanup, so there is
  no source-level parser behavior change that explains those per-case outliers.
  If the same cases reproduce in the next checkpoint, inspect inlining/codegen
  around the affected helpers before considering rollback.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 19:40 EDT

- Repo head: `9cec2e70`
- Batch base: `e36d8f13`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core helper/reference cleanups:
  `Restrict language helper visibility` through
  `Use stack node reference for link precedence`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25159.5 | 23793.8 | +5.74% |
| TypeScript error parses | 32 | 1649.8 | 1547.5 | +6.61% |
| JavaScript normal parses | 2 | 17228.1 | 16057.6 | +7.29% |
| JavaScript error parses | 37 | 2034.2 | 1966.7 | +3.44% |
| Overall parser throughput | 82 | 2284.2 | 2166.3 | +5.44% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2756.2 | 3029.1 | 9.01% |

Prior checkpoint at `e36d8f13` measured Rust overall throughput of 2362.1
bytes/ms and a Rust-vs-C delta of +6.88%. This checkpoint measured 2284.2
bytes/ms, so absolute Rust throughput moved by about -3.30%. C throughput moved
from 2210.0 to 2166.3 bytes/ms, and the Rust-vs-C delta moved to +5.44%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header changes, and no parser table or parser
  action semantic changes.
- The language and lexer commits only restrict Rust visibility or replace a
  callback-local raw reference with the existing typed lexer helper.
- The stack commits replace raw `StackIterator`, callback payload, and
  `StackNode` dereferences with existing typed reference helpers in callbacks,
  accessors, merge paths, and link handling. They preserve the same calls,
  refcount operations, comparison order, and dynamic-precedence arithmetic.
- The previous checkpoint's TypeScript normal-case regressions did not
  reproduce. This run has one JavaScript error fixture above the 5% threshold,
  while every aggregate bucket remains faster than C. Because C throughput also
  dropped materially versus the prior run and the changed code is mechanical
  reference formation, this does not currently prove a source-level regression.
  If this fixture repeats in the next checkpoint, inspect codegen around
  `stack_node_ref` in `stack_node_add_link` before considering rollback.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 18:20 EDT

- Repo head: `0786a35a`
- Batch base: `adbc3547`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small parser pointer-access cleanup commits:
  `Use parser stack accessors in parser API paths` through
  `Use language helper in external scanner`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25273.0 | 23228.4 | +8.80% |
| TypeScript error parses | 32 | 1667.4 | 1612.5 | +3.40% |
| JavaScript normal parses | 2 | 17168.0 | 15597.0 | +10.07% |
| JavaScript error parses | 37 | 2053.8 | 1931.4 | +6.33% |
| Overall parser throughput | 82 | 2307.0 | 2204.6 | +4.64% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error compound-statement-without-trailing-newline.py` | 938.1 | 998.0 | 6.01% |

Prior checkpoint at `8f3b555e` measured Rust overall throughput of 2347.1
bytes/ms and a Rust-vs-C delta of +4.72%. This checkpoint measured 2307.0
bytes/ms, so absolute Rust throughput moved by about -1.71%. C throughput moved
from 2241.3 to 2204.6 bytes/ms, and the Rust-vs-C delta moved slightly to
+4.64%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parser action or
  parsing table semantic changes.
- The parser stack/API commits finish replacing direct parser stack reference
  formation in parser API paths and replace a few redundant `&mut *self_`
  reborrows with the existing parser reference.
- The typed-array commits route parser-local generic array operations through
  existing or matching local helpers for `SubtreeArray`, `MutableSubtreeArray`,
  and `TSRangeArray`. These are reference-formation cleanups around unchanged
  array operations and do not alter allocation, capacity, or element movement.
- The external scanner commits use typed pointer access for the embedded scanner
  state and centralize `TSLanguageFull` access behind a helper in lexing and
  scanner callbacks. Nullable scanner create/destroy paths still perform the
  null checks before using the helper.
- The single per-case slowdown is the same narrow TypeScript error fixture that
  appeared in prior checkpoints. All aggregate buckets remain faster than C,
  and both Rust and C absolute throughput moved down compared with the prior
  run, so this does not currently indicate a source-level regression in this
  batch.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 20:35 EDT

- Repo head: `a4730269`
- Batch base: `65dfb73b`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 Rust-core reference/raw-pointer cleanups:
  `Tie parse action entry lifetime to language` through
  `Take array reserve by mutable reference`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24887.6 | 24370.2 | +2.12% |
| TypeScript error parses | 32 | 1678.0 | 1593.0 | +5.34% |
| JavaScript normal parses | 2 | 16981.4 | 15223.4 | +11.55% |
| JavaScript error parses | 37 | 2009.1 | 1917.6 | +4.78% |
| Overall parser throughput | 82 | 2296.3 | 2183.0 | +5.19% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2782.3 | 3212.4 | 13.39% |

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parser action or
  parsing table semantic changes.
- The tree-cursor commits remove unused Rust-only raw-pointer adapters. The
  C-facing `ts_tree_cursor_goto_first_child_internal` and
  `ts_tree_cursor_goto_next_sibling_internal` remain exported because C query
  code still uses the header declarations.
- The stack and language commits narrow Rust-internal signatures from raw
  pointers to references: changed-range access, `ts_stack_new`'s subtree pool
  parameter, language table out-parameters, and generic `array_clear` /
  `array_reserve`.
- The repeated JavaScript error-case regression is not explained by a direct
  parser algorithm change in this batch. The only code in the parser hot path is
  reference-shape cleanup around existing stack/language/array operations. Since
  aggregate Rust throughput remains faster than C in every bucket and overall,
  no rollback was performed. This fixture should remain the first targeted rerun
  if a future source change touches stack iteration, parser error recovery, or
  array growth behavior.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 20:06 EDT

- Repo head: `52235569`
- Batch base: `e14aaa98`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small raw-pointer/reference cleanups:
  `Use subtree array helper in stack callback` through
  `Use language references for parse actions`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25797.2 | 24129.8 | +6.91% |
| TypeScript error parses | 32 | 1679.3 | 1574.8 | +6.64% |
| JavaScript normal parses | 2 | 17670.8 | 16428.5 | +7.56% |
| JavaScript error parses | 37 | 2120.1 | 2000.2 | +6.00% |
| Overall parser throughput | 82 | 2345.6 | 2204.0 | +6.43% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2784.9 | 3186.6 | 12.61% |

Prior checkpoint at `e14aaa98` measured Rust overall throughput of 2284.2
bytes/ms and a Rust-vs-C delta of +5.44%. This checkpoint measured 2345.6
bytes/ms, so absolute Rust throughput moved by about +2.69%. C throughput moved
from 2166.3 to 2204.0 bytes/ms, and the Rust-vs-C delta moved to +6.43%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parser action or
  parsing table semantic changes.
- The stack commit replaces one direct `SubtreeArray.contents.add(0)` read in
  an internal stack callback with a typed local accessor. This preserves the
  same indexed read and does not alter stack traversal control flow.
- The subtree commits convert internal Rust-only helpers from raw pointer
  parameters to references where all callers already had references, then remove
  now-unused pointer adapters. The affected areas are trailing extras removal,
  subtree compression, subtree comparison, subtree edit, subtree constructors,
  and symbol update helpers.
- The language commit changes parse-action helper inputs from
  `*const TSLanguageFull` to `&TSLanguageFull` at two internal call sites. It
  does not change parse table representation, action pointer arithmetic, or
  exported `TSLanguage` APIs.
- The JavaScript error outlier is the same
  `compound-statement-without-trailing-newline.py` fixture that appeared in the
  previous checkpoint, but the slowdown is larger in this run. The source
  changes in this batch do not special-case JavaScript or error recovery, and
  all aggregate buckets are faster than C with absolute Rust throughput higher
  than the previous checkpoint. No rollback was performed, but this fixture
  should remain the first targeted rerun if the next batch reports another
  >5% per-case regression.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 16:40 EDT

- Repo head: `4811d155`
- Batch base: `f3fda4ab`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core aliasing cleanups:
  `Use slice for subtree string children` through
  `Use pointer accessors in language helpers`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

Primary run:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24580.3 | 22164.6 | +10.90% |
| TypeScript error parses | 32 | 1652.1 | 1571.0 | +5.16% |
| JavaScript normal parses | 2 | 16731.8 | 15806.5 | +5.85% |
| JavaScript error parses | 37 | 2029.5 | 1921.3 | +5.63% |
| Overall parser throughput | 82 | 2282.5 | 2165.4 | +5.41% |

Per-case regressions over 5% in the primary run:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error malloc.c` | 1111.3 | 1249.2 | 11.04% |
| `javascript error corePublic.ts` | 2385.7 | 2556.6 | 6.68% |

JavaScript-only rerun to check whether those per-case regressions were stable:

```sh
cargo xtask perf-gate --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| JavaScript normal parses | 2 | 16991.9 | 15378.8 | +10.49% |
| JavaScript error parses | 37 | 1957.2 | 1922.7 | +1.79% |
| JavaScript overall | 39 | 2549.7 | 2496.5 | +2.13% |

Per-case regressions over 5% in the JavaScript rerun:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error letter_test.go` | 1590.1 | 1715.2 | 7.29% |
| `javascript error marker-index.h` | 1155.4 | 1230.0 | 6.06% |
| `javascript error cluster.c` | 2158.5 | 2288.2 | 5.67% |

Prior checkpoint at `3d976f95` measured Rust overall throughput of 2283.3
bytes/ms in the primary run and 2302.4 bytes/ms in its rerun. This checkpoint
measured 2282.5 bytes/ms in the primary run, so absolute Rust throughput is
effectively flat against the prior primary run (-0.04%) and down about 0.86%
against the prior rerun. The Rust-vs-C delta remains positive at +5.41%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parsing table or
  parser action semantic changes.
- The subtree commits move repeated child-pointer arithmetic behind local
  slice helpers and one mutable child accessor. They preserve existing loop
  bounds and explicitly keep the zero-child null-pointer case out of slice
  construction.
- The point, changed-ranges, tree, tree-cursor, and language commits replace
  direct `&*ptr` / `&mut *ptr` conversions with localized pointer accessors at
  existing unsafe boundaries. These changes alter how references are formed,
  but not the underlying control flow, allocation behavior, or parser state
  transitions.
- The primary JavaScript per-case regressions did not reproduce as the same
  files in the JavaScript-only rerun; the rerun reported different outliers
  while JavaScript normal throughput improved to +10.49% vs C. Given the
  changing outlier set, flat absolute Rust throughput against the prior primary
  checkpoint, and positive overall Rust-vs-C result, no rollback was performed.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 17:30 EDT

- Repo head: `a964b3ca`
- Batch base: `289df69a`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core pointer-accessor cleanups:
  `Use pointer accessors for stack nodes` through
  `Use parser stack accessors in reduce setup`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25658.3 | 23488.0 | +9.24% |
| TypeScript error parses | 32 | 1665.8 | 1646.0 | +1.21% |
| JavaScript normal parses | 2 | 16745.5 | 16447.5 | +1.81% |
| JavaScript error parses | 37 | 2077.1 | 1974.6 | +5.19% |
| Overall parser throughput | 82 | 2314.7 | 2252.6 | +2.76% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error compound-statement-without-trailing-newline.py` | 899.6 | 994.3 | 9.53% |
| `typescript error crlf-line-endings.py` | 1298.7 | 1369.9 | 5.20% |

Prior checkpoint at `289df69a` measured Rust overall throughput of 2270.1
bytes/ms and a Rust-vs-C delta of +2.37%. This checkpoint measured 2314.7
bytes/ms, so absolute Rust throughput moved by about +1.96%. C throughput moved
from 2217.6 to 2252.6 bytes/ms, and the Rust-vs-C delta moved to +2.76%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parsing table or
  parser action semantic changes.
- The stack commits centralize raw stack node, subtree pool, graph-output, and
  lifecycle pointer conversions behind local accessors. They do not alter stack
  versioning, push/pop semantics, merge behavior, summary recording, or
  retain/release ownership.
- The parser commits add local stack pointer accessors and apply them to
  breakdown, version-status comparison, lexing, shift, and reduce setup call
  sites. These are reference-formation cleanups only; the same stack operations
  are called with the same version/state/subtree values.
- The two per-case TypeScript error slowdowns are isolated to error-fixture
  parses. The same checkpoint shows TypeScript normal at +9.24% vs C,
  JavaScript error at +5.19% vs C, and aggregate Rust throughput higher than the
  previous checkpoint. The outliers do not indicate a stable source-level
  parser regression in this batch, so no rollback was performed.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 17:04 EDT

- Repo head: `8085dfda`
- Batch base: `2f1dc597`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core pointer-accessor cleanups:
  `Use pointer accessors in lexer callbacks` through
  `Use pointer accessors in stack callbacks`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

Primary run:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 21109.1 | 24003.2 | -12.06% |
| TypeScript error parses | 32 | 1621.6 | 1596.1 | +1.60% |
| JavaScript normal parses | 2 | 17688.1 | 16376.3 | +8.01% |
| JavaScript error parses | 37 | 2067.9 | 1989.1 | +3.96% |
| Overall parser throughput | 82 | 2270.1 | 2217.6 | +2.37% |

Per-case regressions over 5% in the primary run:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error no_newline_at_eof.go` | 770.2 | 1199.7 | 35.80% |
| `typescript error jquery.js` | 11455.3 | 15479.7 | 26.00% |
| `typescript normal codeFixProvider.ts` | 14841.4 | 17261.0 | 14.02% |
| `typescript normal parser.ts` | 21281.6 | 24497.9 | 13.13% |
| `typescript error value.go` | 971.5 | 1102.0 | 11.84% |
| `typescript error letter_test.go` | 1530.7 | 1715.4 | 10.77% |
| `typescript error rule.cc` | 562.7 | 615.3 | 8.55% |
| `typescript error parser.c` | 1060.3 | 1125.7 | 5.81% |

TypeScript-only rerun to check whether the normal-case regression was stable:

```sh
cargo xtask perf-gate --language typescript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25943.4 | 24645.0 | +5.27% |
| TypeScript error parses | 32 | 1683.1 | 1605.6 | +4.83% |
| TypeScript overall | 43 | 2100.7 | 2003.8 | +4.83% |

Per-case regressions over 5% in the TypeScript rerun:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error python2-grammar-crlf.py` | 953.1 | 1114.8 | 14.51% |
| `typescript error python3-grammar.py` | 2178.5 | 2306.7 | 5.56% |
| `typescript error python2-grammar.py` | 1030.0 | 1089.3 | 5.44% |

Prior checkpoint at `4811d155` measured Rust overall throughput of 2282.5
bytes/ms in the primary run. This checkpoint measured 2270.1 bytes/ms, so
absolute Rust throughput moved by about -0.54%. The Rust-vs-C delta remained
positive at +2.37%, while C throughput moved from 2165.4 to 2217.6 bytes/ms.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parsing table or
  parser action semantic changes.
- The lexer and node commits only replace existing FFI-boundary raw reference
  creation with local pointer accessor helpers.
- The subtree commits centralize array, pool, mutable-subtree, heap-data, and
  edit pointer conversions. They preserve the existing subtree allocation,
  reference counting, child iteration, and external scanner state ownership
  logic.
- The stack commits touch generic array accessor wrappers and two callback
  payload conversions. They do not alter stack versioning, push/pop semantics,
  merge behavior, or parser action control flow.
- The primary TypeScript normal regression did not reproduce in the
  TypeScript-only rerun. The rerun measured TypeScript normal at +5.27% vs C
  and reported a different set of per-case outliers, all in error cases. Given
  the non-reproducing normal-case result, the small absolute overall Rust
  throughput movement (-0.54%), and the lack of source changes to parsing
  decisions, this checkpoint treats the primary TypeScript normal drop as
  benchmark noise and performs no rollback.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-24 13:33 EDT

- Repo head: `51ab1851`
- Batch base: `a2956575`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core cleanups through `Use reference for breakdown lookahead`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25827.5 | 23528.3 | +9.77% |
| TypeScript error parses | 32 | 1663.3 | 1567.1 | +6.14% |
| JavaScript normal parses | 2 | 17037.7 | 15002.8 | +13.56% |
| JavaScript error parses | 37 | 2036.1 | 1966.3 | +3.55% |
| Overall parser throughput | 82 | 2296.5 | 2180.1 | +5.34% |

Prior checkpoint at `a2956575` reported overall +4.77% on the same
TypeScript/JavaScript gate, so this batch is +0.57 percentage points overall.

Regressions above the 5% per-case threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `builderStatePublic.ts` | 18691.6 | 20427.8 | 8.50% |

### 2026-06-24 13:50 EDT

- Repo head: `331d0c5d`
- Batch base: `6cb0e70f`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core reference cleanups through `Use reference for stack node pool`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26064.2 | 25146.6 | +3.65% |
| TypeScript error parses | 32 | 1700.4 | 1655.4 | +2.72% |
| JavaScript normal parses | 2 | 17355.2 | 16457.1 | +5.46% |
| JavaScript error parses | 37 | 2093.0 | 2003.1 | +4.49% |
| Overall parser throughput | 82 | 2352.1 | 2274.2 | +3.42% |

Prior checkpoint at `6cb0e70f` reported Rust overall throughput of 2296.5
bytes/ms on the same TypeScript/JavaScript gate, so this batch improved
absolute Rust throughput by 2.42%. The Rust-vs-C delta fell from +5.34% to
+3.42% because this C run was also faster.

Regressions above the 5% per-case threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `compound-statement-without-trailing-newline.py` | 3020.9 | 3223.9 | 6.30% |

### 2026-06-24 14:11 EDT

- Repo head: `555f5c3b`
- Batch base: `05532c15`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core raw-pointer/reference cleanups through `Use stack slice mutable cleanup`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26568.9 | 25293.2 | +5.04% |
| TypeScript error parses | 32 | 1730.1 | 1668.5 | +3.69% |
| JavaScript normal parses | 2 | 17740.5 | 16046.7 | +10.56% |
| JavaScript error parses | 37 | 2116.3 | 2013.5 | +5.11% |
| Overall parser throughput | 82 | 2387.9 | 2288.6 | +4.34% |

Prior checkpoint at `331d0c5d` reported Rust overall throughput of 2352.1
bytes/ms on the same TypeScript/JavaScript gate, so this batch improved
absolute Rust throughput by 1.52%. The Rust-vs-C delta rose from +3.42% to
+4.34%.

Regressions above the 5% per-case threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2849.2 | 3278.4 | 13.09% |
| `typescript error compound-statement-without-trailing-newline.py` | 987.1 | 1061.7 | 7.02% |
| `typescript error crlf-line-endings.py` | 1366.1 | 1443.9 | 5.39% |

### 2026-06-24 14:27 EDT

- Repo head: `91e06d77`
- Batch base: `1d3eb55b`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core stack reference/raw-pointer cleanups through `Use stack iterator move helper in graph`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26901.5 | 24996.5 | +7.62% |
| TypeScript error parses | 32 | 1714.9 | 1658.4 | +3.41% |
| JavaScript normal parses | 2 | 17988.3 | 16295.7 | +10.39% |
| JavaScript error parses | 37 | 2099.5 | 2007.2 | +4.60% |
| Overall parser throughput | 82 | 2369.0 | 2277.9 | +4.00% |

Prior checkpoint at `555f5c3b` reported Rust overall throughput of 2387.9
bytes/ms on the same TypeScript/JavaScript gate, so this batch regressed
absolute Rust throughput by 0.79%. The Rust-vs-C delta fell from +4.34% to
+4.00%.

No per-case regressions above the 5% threshold.

### 2026-06-24 14:45 EDT

- Repo head: `17161bf3`
- Batch base: `5a6e3223`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core stack reference/raw-pointer cleanups through `Use stack slice mutable accessor`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24965.0 | 22972.6 | +8.67% |
| TypeScript error parses | 32 | 1657.5 | 1606.3 | +3.19% |
| JavaScript normal parses | 2 | 17530.6 | 16342.7 | +7.27% |
| JavaScript error parses | 37 | 2007.2 | 1918.3 | +4.63% |
| Overall parser throughput | 82 | 2279.8 | 2195.3 | +3.85% |

Prior checkpoint at `91e06d77` reported Rust overall throughput of 2369.0
bytes/ms on the same TypeScript/JavaScript gate, so this batch regressed
absolute Rust throughput by 3.77%. The Rust-vs-C delta fell from +4.00% to
+3.85%.

No per-case regressions above the 5% threshold.

### 2026-06-24 15:03 EDT

- Repo head: `58d69eb2`
- Batch base: `a4dbb358`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core stack accessor/raw-pointer cleanups through `Use stack head getter helpers`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25201.5 | 24049.8 | +4.79% |
| TypeScript error parses | 32 | 1717.0 | 1602.6 | +7.14% |
| JavaScript normal parses | 2 | 17825.7 | 15768.5 | +13.05% |
| JavaScript error parses | 37 | 2105.5 | 1977.9 | +6.45% |
| Overall parser throughput | 82 | 2371.2 | 2217.1 | +6.95% |

Prior checkpoint at `a4dbb358` recorded Rust overall throughput of 2279.8
bytes/ms on the same TypeScript/JavaScript gate, so this batch improved
absolute Rust throughput by 4.01%. The Rust-vs-C delta rose from +3.85% to
+6.95%.

No per-case regressions above the 5% threshold.

### 2026-06-24 15:28 EDT

- Repo head: `a59e0857`
- Batch base: `a013b88c`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small parser, changed-range, and tree-cursor accessor/raw-pointer cleanups through `Use tree cursor entry accessor`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24823.8 | 24413.7 | +1.68% |
| TypeScript error parses | 32 | 1701.0 | 1629.0 | +4.42% |
| JavaScript normal parses | 2 | 17564.7 | 16233.2 | +8.20% |
| JavaScript error parses | 37 | 2036.1 | 1970.8 | +3.31% |
| Overall parser throughput | 82 | 2327.8 | 2237.6 | +4.03% |

Prior checkpoint at `a013b88c` recorded Rust overall throughput of 2371.2
bytes/ms on the same TypeScript/JavaScript gate, so this batch regressed
absolute Rust throughput by 1.83%. The Rust-vs-C delta fell from +6.95% to
+4.03%.

Per-case regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2765.4 | 3143.9 | 12.04% |

Investigation:

- A repeat current-head run before this checkpoint reported different per-case regressions: `javascript error compound-statement-without-trailing-newline.py` at 9.25% and `typescript error compound-statement-without-trailing-newline.py` at 5.44%, while overall Rust throughput was 2340.6 bytes/ms.
- A separate comparison run at prior checkpoint `a013b88c`, using the same ignored grammar fixtures via a symlinked comparison worktree, also reported a compound-statement per-case regression: `typescript error compound-statement-without-trailing-newline.py` at 7.64%.
- Because the affected case changed across runs and an equivalent compound-statement regression appears at the prior checkpoint, this was treated as benchmark instability or an existing noisy case, not a proven culprit in this batch. No rollback was performed.

Source-code analysis:

- The parser-throughput benchmark does not use tree cursor traversal APIs or changed-range diffing in its hot loop, so `Use tree cursor back accessor`, `Use tree cursor entry accessor`, and `Use changed ranges cursor move helper` are unlikely explanations for a parser-only regression.
- `Use parser logger move helper`, `Use reusable node stack helper`, and `Use parser range array accessor` affect logger access, reusable-node bookkeeping, or included-range bookkeeping. These are outside the repeated parse/reduce hot path for the reported error case and are also unlikely primary causes.
- The plausible hot-path commits are `Use parser stack slice move helper`, `Use parser slice subtree move helper`, `Use parser stack summary accessor`, and `Use parser mutable subtree stack helper`. They wrap raw `array_get`/`ptr::read` operations in private helper functions used by `ts_parser__reduce`, `ts_parser__recover_to_state`, `ts_parser__accept`, and subtree balancing. If a regression later proves reproducible, these parser helpers should be checked first for missed inlining or optimizer differences.
- The source changes are semantic no-ops: they preserve the same pointer arithmetic, struct moves, and field reads, with no FFI signature or `#[repr(C)]` layout changes. Given private helper functions in optimized builds and the noisy per-case behavior above, no specific source culprit was identified in this batch.

### 2026-06-24 15:52 EDT

- Repo head: `5a0a2fcf`
- Batch base: `e97f7b3b`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small raw-pointer/reference cleanups through `Use references for mutable array cleanup`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24747.2 | 23999.3 | +3.12% |
| TypeScript error parses | 32 | 1698.0 | 1597.5 | +6.29% |
| JavaScript normal parses | 2 | 16869.9 | 15408.5 | +9.48% |
| JavaScript error parses | 37 | 2041.2 | 1963.1 | +3.98% |
| Overall parser throughput | 82 | 2325.9 | 2205.9 | +5.44% |

Prior checkpoint at `e97f7b3b` recorded Rust overall throughput of 2327.8
bytes/ms on the same TypeScript/JavaScript gate, so this batch was effectively
flat at -0.08% absolute Rust throughput. The Rust-vs-C delta rose from +4.03%
to +5.44%, mostly because the measured C baseline was lower in this run.

No per-case regressions above the 5% threshold.

Source-code analysis:

- The batch contains several changes outside parser throughput hot loops:
  tree cursor entry slot helpers, changed-range stack helpers, lexer callback
  reference binding, and tree C API reference binding. These should not affect
  the steady-state TypeScript/JavaScript parse loop except through measurement
  noise or code layout.
- `Use parser subtree array accessor` is the main parser hot-path-adjacent
  change. It routes existing `SubtreeArray` element reads in reduce, accept,
  breakdown, and recovery paths through a private helper while preserving the
  same `array_get` pointer arithmetic and value loads.
- The subtree changes reduce raw-pointer signatures or repeated dereferences in
  `ExternalScannerState`, `SubtreeArray`, and `MutableSubtreeArray` helpers.
  They preserve the same allocations, `#[repr(C)]` data layout, stack element
  moves, and release/retain behavior. The mutable subtree stack helpers are
  used during subtree compression/balancing, but the measured overall Rust
  throughput is flat versus the prior checkpoint.
- Because the perf gate reported no per-case regressions above threshold and
  overall Rust throughput changed by less than 0.1%, there is no reproducible
  performance culprit to investigate in this batch. No rollback was performed.

### 2026-06-24 16:11 EDT

- Repo head: `954e5f7c`
- Batch base: `d85451be`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small internal raw-pointer/reference cleanups through `Use reference for stack condensation helper`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24978.2 | 24849.0 | +0.52% |
| TypeScript error parses | 32 | 1659.5 | 1621.4 | +2.35% |
| JavaScript normal parses | 2 | 17804.2 | 15255.7 | +16.71% |
| JavaScript error parses | 37 | 2067.0 | 1906.5 | +8.42% |
| Overall parser throughput | 82 | 2306.8 | 2201.9 | +4.76% |

Prior checkpoint at `5a0a2fcf` recorded Rust overall throughput of 2325.9
bytes/ms on the same TypeScript/JavaScript gate, so this batch measured -0.82%
absolute Rust throughput. That is below the 5% investigation threshold and in
line with prior run-to-run noise. The Rust-vs-C delta fell from +5.44% to
+4.76%, mostly because TypeScript error throughput measured lower in this run.

Per-case Rust-vs-C regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal builderStatePublic.ts` | 17955.3 | 20836.7 | 13.83% |
| `typescript error release.sh` | 591.4 | 657.8 | 10.09% |

Source-code analysis:

- This batch intentionally focused on replacing private helper signatures and
  repeated `&(*ptr)` / `&mut (*ptr)` style dereferences with Rust references.
  It did not change any FFI-facing signature, `#[repr(C)]` type, allocation
  strategy, parse table access, stack ordering rule, or subtree ownership rule.
- The scanner lifecycle, serialization, deserialization, and scan helpers still
  call the same external scanner ABI and wasm store functions with the same
  payload pointers. The Rust reference is only used inside the parser-owned
  helper before crossing those ABI boundaries.
- `Use reference for parser logging helper` affects debug logging and dot graph
  output. The parser throughput benchmark runs without parser logging, so this
  is not a plausible source-code explanation for the reported TypeScript cases.
- `Use reference for stack condensation helper` is the only hot-path parser
  commit in this batch. It preserves the same calls to `ts_stack_version_count`,
  `ts_stack_is_halted`, `ts_stack_merge`, `ts_stack_swap_versions`, pause/resume
  handling, and error-cost comparisons; the change removes repeated raw pointer
  field dereferences on `TSParser`.
- Because overall Rust throughput changed by less than 1% versus the previous
  checkpoint, and because the per-case slowdowns are Rust-vs-C comparisons
  rather than regressions against the previous Rust checkpoint, no specific
  culprit was identified and no rollback was performed.

### 2026-06-24 16:29 EDT

- Repo head: `4f8c04bc`
- Batch base: `6e7d886b`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small internal raw-pointer/reference cleanups through `Use reference for stack version helper`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25167.0 | 24165.9 | +4.14% |
| TypeScript error parses | 32 | 1650.3 | 1606.4 | +2.73% |
| JavaScript normal parses | 2 | 16877.8 | 15845.4 | +6.52% |
| JavaScript error parses | 37 | 2029.6 | 1905.9 | +6.49% |
| Overall parser throughput | 82 | 2281.9 | 2190.2 | +4.19% |

Prior checkpoint at `954e5f7c` recorded Rust overall throughput of 2306.8
bytes/ms on the same TypeScript/JavaScript gate, so this batch measured -1.08%
absolute Rust throughput. That is below the 5% investigation threshold. The
Rust-vs-C delta fell from +4.76% to +4.19%, with both Rust and C baselines
moving between runs.

Per-case Rust-vs-C regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal builderStatePublic.ts` | 16621.0 | 21149.4 | 21.41% |
| `javascript error performanceCore.ts` | 3941.8 | 4345.0 | 9.28% |
| `typescript error mixed-spaces-tabs.py` | 289.5 | 313.5 | 7.66% |

Source-code analysis:

- This batch continued replacing internal raw pointer receivers with Rust
  references. It did not alter FFI-visible signatures, `#[repr(C)]` layouts,
  allocation sizes, parse table data, or stack/subtree ownership rules.
- The parser-facing changes were limited to subtree balancing and stack/subtree
  cleanup helpers. `Use reference for subtree balancing helper` preserves the
  same tree stack operations, compression calls, progress checks, and child
  traversal.
- The subtree cleanup changes (`pool_free`, `make_mut`, array clear/delete, and
  release) keep the same retain/release decisions and free-list behavior. They
  remove raw `SubtreePool` parameters where callers already had a mutable pool
  reference.
- The stack changes (`stack_node_retain`, `stack_head_delete`,
  `stack_node_release` pool parameters, `stack_node_add_link` pool parameter,
  and `ts_stack__add_version`) preserve the same stack node reference counts,
  link merging, version insertion, and subtree release calls.
- `typescript normal builderStatePublic.ts` also appeared as a Rust-vs-C
  slowdown in the previous checkpoint, so it is not new evidence for this
  batch. The overall Rust checkpoint delta is about -1%, and no >5% Rust
  checkpoint regression was observed, so no specific source culprit was
  identified and no rollback was performed.

### 2026-06-24 16:49 EDT

- Repo head: `b9ae4832`
- Batch base: `aa46cf23`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small internal stack raw-pointer/reference cleanups through `Use reference for stack halted check`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25460.3 | 24537.9 | +3.76% |
| TypeScript error parses | 32 | 1677.2 | 1659.1 | +1.09% |
| JavaScript normal parses | 2 | 17722.6 | 16614.8 | +6.67% |
| JavaScript error parses | 37 | 2084.5 | 1939.9 | +7.45% |
| Overall parser throughput | 82 | 2329.2 | 2249.8 | +3.53% |

Prior checkpoint at `4f8c04bc` recorded Rust overall throughput of 2281.9
bytes/ms on the same TypeScript/JavaScript gate, so this batch measured +2.07%
absolute Rust throughput. That is below the 5% regression investigation
threshold and in the favorable direction. The Rust-vs-C delta fell from +4.19%
to +3.53%, with both Rust and C baselines moving upward between runs.

Per-case Rust-vs-C regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal transform.ts` | 20274.0 | 24041.5 | 15.67% |
| `javascript error compound-statement-without-trailing-newline.py` | 3039.0 | 3251.5 | 6.53% |
| `typescript normal refactorProvider.ts` | 21703.7 | 23202.4 | 6.46% |

Source-code analysis:

- This batch only changed internal stack helper receivers from raw pointers to
  Rust references where callers already had parser-owned stack access. It did
  not change FFI-facing signatures, `#[repr(C)]` layouts, stack allocation,
  parse table access, link ordering, node ownership, or external scanner
  behavior.
- The changed helpers are narrow accessors and status queries:
  `ts_stack_halted_version_count`, `ts_stack_last_external_token`,
  `ts_stack_set_last_external_token`, `ts_stack_node_count_since_error`,
  `ts_stack_has_advanced_since_error`, `ts_stack_error_cost`,
  `ts_stack_dynamic_precedence`, `ts_stack_is_paused`, `ts_stack_is_active`,
  and `ts_stack_is_halted`.
- The status and metric helpers preserve the same `stack_head` lookups and
  `StackStatus` comparisons. `ts_stack_node_count_since_error` and
  `ts_stack_set_last_external_token` still perform the same mutations and
  retain/release operations; the only change is moving the raw pointer
  dereference to the caller boundary.
- The per-case slowdowns are Rust-vs-C comparisons in the current noisy gate,
  not regressions against the previous Rust checkpoint. Since overall Rust
  throughput improved by about 2%, no source-level culprit was identified and
  no rollback was performed.

### 2026-06-24 17:23 EDT

- Repo head: `138d96b9`
- Batch base: `f6b8c2b0`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small internal raw-pointer/reference cleanups plus one
  comment-only boundary clarification, through `Use reference for child
  selection`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24506.6 | 23772.8 | +3.09% |
| TypeScript error parses | 32 | 1632.4 | 1574.2 | +3.70% |
| JavaScript normal parses | 2 | 16720.5 | 15903.5 | +5.14% |
| JavaScript error parses | 37 | 2034.1 | 1927.8 | +5.52% |
| Overall parser throughput | 82 | 2267.8 | 2172.7 | +4.38% |

Prior checkpoint at `b9ae4832` recorded Rust overall throughput of 2329.2
bytes/ms on the same TypeScript/JavaScript gate, so this batch measured -2.64%
absolute Rust throughput. That is below the 5% regression investigation
threshold. The Rust-vs-C delta rose from +3.53% to +4.38%, with both Rust and
C baselines moving downward between runs.

Per-case Rust-vs-C regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error ast.rs` | 1100.7 | 1537.7 | 28.42% |
| `typescript normal transform.ts` | 20251.8 | 23206.0 | 12.73% |
| `typescript normal packageJsonCache.ts` | 16669.9 | 18332.6 | 9.07% |
| `javascript error performanceCore.ts` | 5123.3 | 5480.4 | 6.52% |
| `typescript normal performanceCore.ts` | 22375.0 | 23746.1 | 5.77% |
| `typescript error rule.cc` | 574.9 | 605.9 | 5.11% |

Source-code analysis:

- This batch continued replacing Rust-core raw pointer receivers with
  references where the caller already had a valid stack, subtree, or parser
  reference. It did not change `#[repr(C)]` layouts, allocation sizes, parse
  table data, generated parser templates, or public `#[no_mangle] extern "C"`
  APIs.
- The stack changes (`halt`, `clear`, `remove_version`, `version_count`,
  `state`, and `position`) preserve the same `StackHead` lookups, state
  comparisons, version removal order, node release calls, and base-node retain
  behavior. The main difference is that callers perform the raw pointer
  boundary conversion before entering these helpers.
- The generic array transfer helpers now take Rust references. `array_swap`
  still swaps the same three `Array<T>` fields via `std::mem::swap`;
  `array_assign` still reserves capacity, sets size, and copies the same byte
  count from the source contents.
- The subtree array copy/reverse helpers preserve the same allocation, retain,
  and swap behavior. Their names still appear in the old private
  `lib/src/subtree.h`, but the Rust versions are not C-exported symbols and are
  not referenced by `alloc.h`, `array.h`, or `parser.h.inc`; this is private
  C-header drift rather than public ABI drift.
- The parser child-selection helper now takes `&mut TSParser`. The tree
  selection behavior remains unchanged: scratch trees are assigned, a scratch
  node is created with the same language pointer, and the existing
  `ts_parser__select_tree` path is called.
- Several current per-case Rust-vs-C slowdowns also appeared in nearby
  checkpoints (`transform.ts` and `performanceCore.ts` in particular), and the
  overall Rust checkpoint delta is -2.64%, below the agreed 5% investigation
  threshold. No specific source-level culprit was identified and no rollback
  was performed.

### 2026-06-24 17:42 EDT

- Repo head: `737fab65`
- Batch base: `6236fb6d`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-only parser helper raw-pointer/reference
  cleanups through `Use reference for parser state recovery`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24974.0 | 25770.9 | -3.09% |
| TypeScript error parses | 32 | 1640.9 | 1604.9 | +2.24% |
| JavaScript normal parses | 2 | 17572.5 | 16538.7 | +6.25% |
| JavaScript error parses | 37 | 2069.9 | 1991.0 | +3.96% |
| Overall parser throughput | 82 | 2291.6 | 2227.8 | +2.87% |

Prior checkpoint at `138d96b9` recorded Rust overall throughput of 2267.8
bytes/ms on the same TypeScript/JavaScript gate, so this batch measured +1.05%
absolute Rust throughput. That is below the 5% regression investigation
threshold and in the favorable direction. The Rust-vs-C delta fell from +4.38%
to +2.87%, with the C comparison baseline moving upward in this run.

Per-case Rust-vs-C regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal builderStatePublic.ts` | 18802.9 | 20588.6 | 8.67% |
| `typescript error compound-statement-without-trailing-newline.py` | 860.2 | 917.3 | 6.23% |
| `typescript error weird-exprs.rs` | 1104.6 | 1176.7 | 6.13% |
| `typescript error python3-grammar-crlf.py` | 2309.2 | 2441.1 | 5.40% |
| `typescript error jquery.js` | 14855.7 | 15637.8 | 5.00% |

Source-code analysis:

- This batch changed only Rust-internal parser helper receivers and call sites.
  It did not change public `#[no_mangle] extern "C"` APIs, generated parser
  templates, `#[repr(C)]` layouts, parse table data, allocation sizes, or FFI
  struct field order.
- The version comparison/status helpers preserve the same error-cost,
  dynamic-precedence, node-count, stack-position, and merge checks. One helper
  (`ts_parser__compare_versions`) dropped an unused parser pointer entirely.
- The token reuse/cache helpers preserve the same token cache lookup and update
  behavior, including retained returned tokens and releases of replaced cached
  token/external-token subtrees.
- The shift, accept, and recover-to-state helpers preserve the same stack
  pushes/pops, subtree retain/release calls, finished-tree selection, error
  node construction, trailing-extra handling, and version halt/remove behavior.
  The edits move raw parser pointer dereferences to the call boundary and use
  parser references inside the helper body.
- Current per-case slowdowns are Rust-vs-C comparisons in this run. Since
  overall Rust throughput improved by about 1% versus the previous Rust
  checkpoint, no >5% Rust checkpoint regression was observed and no rollback
  was performed.

### 2026-06-24 18:07 EDT

- Repo head: `254f42a3`
- Batch base: `5ac7f910`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-only parser raw-pointer/reference cleanups
  through `Use reference for parser recovery`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25638.5 | 24619.3 | +4.14% |
| TypeScript error parses | 32 | 1680.3 | 1664.6 | +0.95% |
| JavaScript normal parses | 2 | 17558.2 | 16576.5 | +5.92% |
| JavaScript error parses | 37 | 2064.3 | 1981.8 | +4.16% |
| Overall parser throughput | 82 | 2323.4 | 2272.4 | +2.24% |

Prior checkpoint at `737fab65` recorded Rust overall throughput of 2291.6
bytes/ms on the same TypeScript/JavaScript gate, so this batch measured +1.39%
absolute Rust throughput. That is below the 5% regression investigation
threshold and in the favorable direction.

Per-case Rust-vs-C regressions above the 5% threshold: none reported by the
perf gate.

Source-code analysis:

- This batch changed only Rust-internal parser helper receivers and call sites.
  It did not change public `#[no_mangle] extern "C"` APIs, generated parser
  templates, `#[repr(C)]` layouts, parse table data, allocation sizes, or FFI
  struct field order.
- The generated header templates are live compatibility surface:
  `ALLOC_HEADER`, `ARRAY_HEADER`, and `PARSER_HEADER` are written by
  `crates/generate/src/generate.rs` and used by CLI test fixtures. This batch
  did not edit `templates/alloc.h`, `templates/array.h`, or `parser.h.inc`.
- The old private C headers still declare internal functions such as stack and
  subtree helpers, but these Rust helpers are not exported C ABI symbols. The
  signature cleanups remain private Rust call-graph changes unless a symbol is
  explicitly exposed through `#[no_mangle] extern "C"`.
- Parser stack breakdown, lexing, reusable-node lookup, tree selection,
  progress checks, lookahead breakdown, potential reductions, error handling,
  reductions, and recovery preserve the same stack operations, subtree
  retain/release behavior, lexer calls, token-cache behavior, logging events,
  error-cost checks, and recovery state transitions. The edits move raw parser
  pointer dereferences to internal call boundaries and use Rust references
  within the helper bodies.
- No >5% Rust checkpoint regression was observed and no rollback was performed.

### 2026-06-24 18:31 EDT

- Repo head: `cc8f8619`
- Batch base: `0c0b47af`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-only raw-pointer/reference cleanups through
  `Use reference for lexer reset`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

Repeat run used for checkpoint recording:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25402.2 | 24268.2 | +4.67% |
| TypeScript error parses | 32 | 1690.5 | 1641.7 | +2.97% |
| JavaScript normal parses | 2 | 17379.8 | 16703.1 | +4.05% |
| JavaScript error parses | 37 | 2021.6 | 2006.3 | +0.76% |
| Overall parser throughput | 82 | 2313.2 | 2263.9 | +2.18% |

The first run of the same command produced overall Rust throughput of 2260.2
bytes/ms and C throughput of 2245.3 bytes/ms. Because the repeat run improved
Rust overall throughput by about 2.35% without source changes, the first run
was treated as noisy for checkpoint comparison.

Prior checkpoint at `254f42a3` recorded Rust overall throughput of 2323.4
bytes/ms on the same TypeScript/JavaScript gate. The repeat run measured
-0.44% Rust throughput versus that checkpoint, which is below the 5% regression
investigation threshold.

Per-case Rust-vs-C regressions above the 5% threshold on the repeat run:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2516.6 | 3184.4 | 20.97% |
| `typescript normal refactorProvider.ts` | 21901.3 | 23436.7 | 6.55% |
| `javascript error crlf-line-endings.py` | 1354.3 | 1445.1 | 6.29% |

Source-code analysis:

- This batch changed only Rust-internal function receivers and call sites in
  `subtree.rs`, `parser.rs`, and `lexer.rs`. It did not change public
  `#[no_mangle] extern "C"` APIs, generated parser templates, `#[repr(C)]`
  layouts, parse tables, allocation sizes, or FFI struct field order.
- The live generated header inputs `templates/alloc.h`, `templates/array.h`,
  and `parser.h.inc` were not edited.
- Parser changes in this batch preserve the same parse-advance control flow:
  lexing setup, cached token reuse, progress checks, parse action dispatch,
  reductions, lookahead breakdown, stack halt/pause, keyword fallback, and
  error detection all call the same operations in the same order. The edits
  replace raw parser-pointer dereferences with the existing `&mut TSParser`
  receiver.
- Lexer changes only convert private Rust receivers for `ts_lexer_mark_end`,
  `ts_lexer_set_input`, and `ts_lexer_reset` from raw pointers to references.
  `ts_lexer_reset` required saving `token_start_position` before borrowing the
  lexer mutably; this preserves the same value and call order.
- The persistent `javascript error compound-statement-without-trailing-newline.py`
  Rust-vs-C slowdown is a C comparison in this run, not a >5% Rust checkpoint
  regression. The changed code does not contain case-specific logic for this
  fixture, and the aggregate Rust result remained within normal run-to-run
  variation versus the prior checkpoint. No rollback was performed.

### 2026-06-24 18:50 EDT

- Repo head: `00178fdb`
- Batch base: `973693df`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small raw-pointer/reference cleanups through
  `Use references in parser range accessors`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25449.3 | 23787.9 | +6.98% |
| TypeScript error parses | 32 | 1684.7 | 1641.6 | +2.62% |
| JavaScript normal parses | 2 | 17729.1 | 16135.4 | +9.88% |
| JavaScript error parses | 37 | 2102.9 | 1961.5 | +7.21% |
| Overall parser throughput | 82 | 2343.0 | 2243.1 | +4.45% |

Prior checkpoint at `cc8f8619` recorded Rust overall throughput of 2313.2
bytes/ms on the same TypeScript/JavaScript gate. This run measured +1.29%
Rust throughput versus that checkpoint, so no >5% Rust checkpoint regression
was observed.

Per-case Rust-vs-C regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2859.9 | 3250.4 | 12.01% |
| `typescript error ast.rs` | 1438.3 | 1617.1 | 11.06% |
| `javascript error no_newline_at_eof.go` | 1444.4 | 1592.0 | 9.27% |
| `typescript error parser.ts` | 23162.0 | 25167.6 | 7.97% |
| `typescript error compound-statement-without-trailing-newline.py` | 904.0 | 964.8 | 6.30% |
| `typescript error python2-grammar-crlf.py` | 1095.7 | 1154.2 | 5.08% |

Source-code analysis:

- This batch changed Rust-internal receiver/body style in `lexer.rs`,
  `stack.rs`, and `parser.rs`. It did not change any exported function
  signatures, `#[no_mangle] extern "C"` ABI, `#[repr(C)]` layouts, allocation
  sizes, parse tables, generated parser templates, or FFI struct field order.
- The live generated header inputs `templates/alloc.h`, `templates/array.h`,
  and `parser.h.inc` were not edited.
- Lexer lifecycle helpers now take references internally after the FFI/parser
  boundary has already produced a valid lexer object. The TSLexer vtable
  callback signatures remain unchanged; `ts_lexer__eof` only binds a shared
  reference inside the existing callback body.
- `ts_stack_delete` now takes `&mut Stack` and splits disjoint fields in the
  deletion loop so the borrow checker can see the same ownership that the
  previous raw-pointer body used. It preserves the same slice/iterator/head
  deletion order, node release calls, node-pool frees, and final `ts_free` of
  the stack allocation.
- Parser accessor and included-range entrypoints keep their public C ABI
  signatures. The edits bind `&TSParser` or `&mut TSParser` once inside the
  body before accessing fields, preserving the same calls to logger and lexer
  included-range helpers.
- The reported per-case slowdowns are Rust-vs-C comparisons in this run, not
  Rust-vs-previous-checkpoint regressions. Overall Rust throughput improved
  versus the previous checkpoint, so no rollback was performed.

### 2026-06-24 19:10 EDT

- Repo head: `b2935073`
- Batch base: `4598be2c`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small raw-pointer/reference cleanups through
  `Use cursor reference in init and delete`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26369.4 | 24617.3 | +7.12% |
| TypeScript error parses | 32 | 1735.4 | 1651.3 | +5.09% |
| JavaScript normal parses | 2 | 17210.3 | 16195.2 | +6.27% |
| JavaScript error parses | 37 | 2127.3 | 2000.2 | +6.35% |
| Overall parser throughput | 82 | 2395.5 | 2268.5 | +5.60% |

Prior checkpoint at `00178fdb` recorded Rust overall throughput of 2343.0
bytes/ms on the same TypeScript/JavaScript gate. This run measured +2.24%
Rust throughput versus that checkpoint, so no >5% Rust checkpoint regression
was observed.

Per-case Rust-vs-C regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal builderStatePublic.ts` | 18611.4 | 20062.0 | 7.23% |

Source-code analysis:

- This batch changed Rust-internal receiver/body style in `parser.rs`,
  `point.rs`, `tree.rs`, and `tree_cursor.rs`. It did not change any exported
  function signatures, `#[no_mangle] extern "C"` ABI, `#[repr(C)]` layouts,
  allocation sizes, parse tables, generated parser templates, or FFI struct
  field order.
- The live generated header inputs `templates/alloc.h`, `templates/array.h`,
  and `parser.h.inc` were not edited.
- Parser lifecycle/configuration changes bind `&TSParser` or `&mut TSParser`
  after the existing raw C entrypoint has been entered. The same reset,
  language, logger, DOT graph, parse option, allocation, deletion, and WASM
  store calls happen in the same order.
- The `point.rs`, `tree.rs`, and `tree_cursor.rs` changes reduce local
  raw-pointer ceremony or clarify local reference names after the ABI boundary;
  they do not alter cursor stack operations, tree copying/editing, point edit
  math, memory ownership, or frees.
- The reported `builderStatePublic.ts` slowdown is a Rust-vs-C comparison in
  this run, not a Rust-vs-previous-checkpoint regression. Overall Rust
  throughput improved versus the previous checkpoint, so no rollback was
  performed.

### 2026-06-24 19:29 EDT

- Repo head: `fb368cb5`
- Batch base: `c9350f4f`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small tree-cursor reference cleanups through
  `Use cursor reference for current field`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26307.2 | 25819.4 | +1.89% |
| TypeScript error parses | 32 | 1756.8 | 1698.6 | +3.43% |
| JavaScript normal parses | 2 | 18115.9 | 17122.2 | +5.80% |
| JavaScript error parses | 37 | 2164.3 | 1673.4 | +29.33% |
| Overall parser throughput | 82 | 2430.6 | 2151.6 | +12.97% |

Prior checkpoint at `b2935073` recorded Rust overall throughput of 2395.5
bytes/ms on the same TypeScript/JavaScript gate. This run measured +1.47%
Rust throughput versus that checkpoint, so no >5% Rust checkpoint regression
was observed.

Per-case Rust-vs-C regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal corePublic.ts` | 24483.6 | 26706.9 | 8.32% |
| `typescript normal types.ts` | 18282.1 | 19655.4 | 6.99% |
| `typescript normal builderStatePublic.ts` | 19598.8 | 20836.7 | 5.94% |
| `typescript normal packageJsonCache.ts` | 17940.1 | 19020.2 | 5.68% |
| `typescript normal transform.ts` | 23371.5 | 24672.5 | 5.27% |

Source-code analysis:

- This batch changed Rust-internal receiver/body style only in
  `tree_cursor.rs`. It did not change any exported function signatures,
  `#[no_mangle] extern "C"` ABI, `#[repr(C)]` layouts, allocation sizes,
  parse tables, generated parser templates, or FFI struct field order.
- The live generated header inputs `templates/alloc.h`, `templates/array.h`,
  and `parser.h.inc` were not edited.
- Internal cursor helpers for child and sibling navigation now accept
  `&mut TreeCursor` after the public wrappers convert from `TSTreeCursor`
  pointers at the ABI boundary. The same stack push/pop, descendant index,
  field lookup, alias lookup, and node construction operations happen in the
  same order.
- The reported TypeScript normal-case slowdowns are Rust-vs-C comparisons in
  this run, not Rust-vs-previous-checkpoint regressions. Overall Rust
  throughput improved versus the previous checkpoint, while C throughput also
  moved materially between checkpoints, so these are not treated as rollback
  evidence.

### 2026-06-24 19:54 EDT

- Repo head: `4cc7fc9a`
- Batch base: `c9f64570`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small cursor/range reference cleanups through
  `Use references when editing ranges`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26984.8 | 25242.6 | +6.90% |
| TypeScript error parses | 32 | 1772.9 | 1698.6 | +4.37% |
| JavaScript normal parses | 2 | 18218.0 | 17099.7 | +6.54% |
| JavaScript error parses | 37 | 2153.3 | 2061.8 | +4.44% |
| Overall parser throughput | 82 | 2440.3 | 2336.1 | +4.46% |

Prior checkpoint at `fb368cb5` recorded Rust overall throughput of 2430.6
bytes/ms on the same TypeScript/JavaScript gate. This run measured +0.40%
Rust throughput versus that checkpoint, so no >5% Rust checkpoint regression
was observed.

No per-case Rust-vs-C regressions above the 5% threshold were reported.

Source-code analysis:

- This batch changed Rust-internal receiver/body style in `tree_cursor.rs` and
  `get_changed_ranges.rs`. It did not change any exported function signatures,
  `#[no_mangle] extern "C"` ABI, `#[repr(C)]` layouts, allocation sizes,
  parse tables, generated parser templates, or FFI struct field order.
- The live generated header inputs `templates/alloc.h`, `templates/array.h`,
  and `parser.h.inc` were not edited.
- Cursor changes bind `&TreeCursor` or `&mut TreeCursor` after entering the
  existing raw-pointer ABI boundary. The same stack indexing, descendant
  navigation, field lookup, field-name lookup, and node/cursor operations occur
  in the same order.
- Changed-range changes bind `&TSRangeArray`, `&mut TSRangeArray`, `&TSRange`,
  `&mut TSRange`, and `&TSInputEdit` after the exported C entrypoints are
  entered. The same range comparison, included-range scanning, edit arithmetic,
  and result array appends occur in the same order.
- Overall Rust throughput improved slightly versus the previous checkpoint and
  the gate reported no per-case Rust-vs-C slowdowns above 5%, so no regression
  investigation or rollback was needed for this batch.

### 2026-06-24 20:42 EDT

- Repo head: `52b81b76`
- Batch base: `d90d18ba`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 9 small Rust-core parser, lexer, and tree-cursor cleanups
  through `Use cursor refs for sibling navigation`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

Repeat run used for checkpoint recording:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25669.6 | 24511.9 | +4.72% |
| TypeScript error parses | 32 | 1670.1 | 1600.3 | +4.36% |
| JavaScript normal parses | 2 | 16972.2 | 16312.5 | +4.04% |
| JavaScript error parses | 37 | 2043.0 | 1917.6 | +6.54% |
| Overall parser throughput | 82 | 2304.8 | 2191.6 | +5.16% |

The first current-head run reported similar aggregate Rust throughput:
2300.2 bytes/ms Rust, 2191.5 bytes/ms C, +4.96% overall.

The previous logged checkpoint at `d90d18ba` recorded Rust overall throughput
of 2440.3 bytes/ms on the same TypeScript/JavaScript gate, which would imply a
5.55% regression versus this repeat run. Because that crossed the investigation
threshold, the prior checkpoint was rerun in a separate comparison worktree at
`d90d18ba`, using the current fetched grammar fixtures and explicit TypeScript
repository path. That fresh prior-checkpoint comparison measured 2273.3
bytes/ms Rust and 2197.5 bytes/ms C overall. Current HEAD is therefore +1.39%
versus the fresh prior-checkpoint Rust result, so the old 2440.3 bytes/ms log
entry was not treated as a reproducible baseline for rollback.

Per-case Rust-vs-C regressions above the 5% threshold on the repeat run:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error malloc.c` | 686.3 | 757.6 | 9.41% |
| `javascript error update-authors.sh` | 591.9 | 633.8 | 6.61% |

Investigation:

- The first current-head run reported a different per-case slowdown
  (`javascript error atom.sh` at 26.77%) while the repeat run reported
  `malloc.c` and `update-authors.sh`. The changing affected cases point to
  per-case benchmark noise rather than one stable source-code culprit.
- The fresh `d90d18ba` comparison worktree reported different Rust-vs-C
  slowdowns again (`corePublic.ts`, shell-script error fixtures, `cluster.c`,
  `no_newline_at_eof.go`, `utilities.ts`, and `crlf-line-endings.py`). That
  confirms the per-case Rust-vs-C list is unstable across runs/checkouts.

Source-code analysis:

- This batch changed Rust-internal bodies in `parser.rs`, `lexer.rs`, and
  `tree_cursor.rs`. It did not change any exported function signatures,
  `#[no_mangle] extern "C"` ABI, `#[repr(C)]` layouts, allocation sizes, parse
  tables, generated parser templates, or FFI struct field order.
- The live generated header inputs `templates/alloc.h`, `templates/array.h`,
  and `parser.h.inc` were not edited.
- The parser change only binds the `TSStringInput` payload as `&TSStringInput`
  inside the existing callback and preserves the same byte bounds check,
  `length` update, null return, and `string.add(byte)` return.
- Lexer changes replace repeated included-range pointer arithmetic with a
  private indexed helper, an index/boolean loop state, and slice iteration for
  validation. They preserve the same included-range ordering checks,
  range-index advancement, EOF transition, chunk refresh decision, token-end
  boundary behavior, and TSLexer callback signatures. These are hot-path-adjacent
  changes, but the fresh prior-checkpoint comparison did not reproduce a Rust
  throughput regression.
- Tree-cursor changes move child and sibling navigation bodies behind private
  `&mut TreeCursor` helpers while preserving the exported wrappers used by the
  C header/query code. Parser throughput benchmarks do not exercise tree cursor
  traversal as their parse hot loop, so these are not plausible explanations
  for an aggregate parser regression.
- No reproducible >5% Rust checkpoint regression was observed after rerunning
  the prior checkpoint in the same environment. No rollback was performed.

### 2026-06-24 22:46 EDT

- Repo head: `98118c0e`
- Batch base: `5bda68da`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core reference cleanups from
  `Use reference helper for point edits` through
  `Use refs for scanner state comparison`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24727.0 | 25274.5 | -2.17% |
| TypeScript error parses | 32 | 1685.8 | 1678.1 | +0.46% |
| JavaScript normal parses | 2 | 17745.1 | 14152.7 | +25.38% |
| JavaScript error parses | 37 | 2085.4 | 1998.5 | +4.35% |
| Overall parser throughput | 82 | 2336.1 | 2285.0 | +2.24% |

The previous logged checkpoint at `52b81b76` recorded repeat-run Rust overall
throughput of 2304.8 bytes/ms on the same TypeScript/JavaScript gate. This
checkpoint is +1.36% versus that Rust throughput, so no aggregate parser
regression was observed.

Per-case Rust-vs-C regressions above the 5% threshold:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal builderStatePublic.ts` | 13335.7 | 19192.1 | 30.51% |
| `typescript normal corePublic.ts` | 23566.5 | 27499.0 | 14.30% |
| `typescript error letter_test.go` | 1563.5 | 1780.3 | 12.18% |
| `javascript error crlf-line-endings.py` | 1313.3 | 1446.0 | 9.18% |
| `typescript normal performance.ts` | 18935.3 | 20649.9 | 8.30% |
| `javascript error builderStatePublic.ts` | 2430.2 | 2593.1 | 6.28% |
| `javascript error compound-statement-without-trailing-newline.py` | 3080.2 | 3284.4 | 6.22% |
| `typescript error crlf-line-endings.py` | 1267.7 | 1335.5 | 5.08% |

Source-code analysis:

- This batch changed only Rust-internal code in `point.rs`, `tree.rs`,
  `get_changed_ranges.rs`, `subtree.rs`, `parser.rs`, and `stack.rs`, replacing
  some by-value `#[repr(C)]` copies and raw-pointer-adjacent access with
  reference-based helper signatures and call sites.
- No exported `#[no_mangle] extern "C"` signatures, public FFI struct layouts,
  allocation headers, generated parser templates, or included C header text were
  edited.
- The latest cleanup changes `ts_subtree_external_scanner_state_eq` from taking
  two `Subtree` values to borrowing them internally. All call sites pass
  references to existing values, preserving the same scanner-state lookup and
  byte comparison logic without changing tree ownership or allocation behavior.
- The per-case Rust-vs-C outliers are not treated as a source-change regression
  because the aggregate Rust throughput improved versus the previous checkpoint,
  and prior checkpoint investigation showed the named outlier cases move between
  runs. No rollback was performed.

### 2026-06-24 23:19 EDT

- Repo head: `7f782d91`
- Batch base: `9ff73abd`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core raw-pointer/reference cleanups from
  `Use raw field addresses in subtree release` through
  `Use stack ref when printing dot graph`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25987.8 | 24731.8 | +5.08% |
| TypeScript error parses | 32 | 1713.0 | 1682.7 | +1.80% |
| JavaScript normal parses | 2 | 17319.4 | 16077.9 | +7.72% |
| JavaScript error parses | 37 | 2097.9 | 1990.4 | +5.40% |
| Overall parser throughput | 82 | 2364.5 | 2289.8 | +3.26% |

The previous checkpoint at `98118c0e` recorded Rust overall throughput of
2336.1 bytes/ms on the same TypeScript/JavaScript gate. This checkpoint is
+1.22% versus that Rust throughput, so no aggregate parser regression was
observed.

This run reported no per-case Rust-vs-C regressions above the 5% threshold.

TypeScript-normal note:

- The previous checkpoint showed TypeScript normal parses at -2.17% Rust vs C,
  including large per-file outliers. This run did not reproduce that slowdown:
  TypeScript normal parses measured +5.08% Rust vs C, and no per-case outlier
  exceeded the 5% threshold.
- Because the sign changed between adjacent 10-repetition checkpoints, the
  TypeScript-normal gap should be treated as a noisy area to keep watching, not
  as a confirmed source-change regression from this batch.

Source-code analysis:

- This batch changed Rust-internal bodies in `subtree.rs`, `language.rs`,
  `parser.rs`, and `stack.rs`, replacing temporary references from raw pointer
  fields and repeated `(*self_)` field access with local `&Stack`, `&mut Stack`,
  `&mut TSParser`, and raw field-address operations.
- No exported `#[no_mangle] extern "C"` signatures, C header declarations,
  public FFI struct layouts, allocation headers, generated parser templates, or
  included C header text were edited.
- Parser hot-path changes were limited to binding the existing parser pointer
  as a local `&mut TSParser` inside `ts_parser_parse`; all parse-loop calls,
  stack operations, tree creation, scanner-state behavior, and reset paths are
  preserved.
- Most stack changes affect helper bodies and debug/DOT graph code, not parser
  table lookup or lexing. The only stack parse-path effects are local reference
  bindings around existing operations, with unchanged public function
  signatures and unchanged stack-head/node/subtree ownership behavior.
- No reproducible >5% regression was observed in this checkpoint. No rollback
  was performed.

### 2026-06-24 23:34 EDT

Focused TypeScript-normal investigation after a report that TypeScript normal
parses appeared consistently slower than C.

- Repo head: `bcd5a99c`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Aggregate command:

```sh
cargo xtask perf-gate --language typescript --repetitions 100 --error-limit 0 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26063.0 | 24820.3 | +5.01% |

This higher-repetition TypeScript-normal run reported no per-case Rust-vs-C
regressions above the 5% threshold, so it does not confirm a broad
TypeScript-normal parser regression.

Focused single-file check:

```sh
TREE_SITTER_BENCHMARK_EXAMPLE_FILTER=builderStatePublic.ts \
TREE_SITTER_BENCHMARK_REPETITION_COUNT=1000 \
cargo bench benchmark -p tree-sitter-cli --offline
```

| Case | Bytes | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| `builderStatePublic.ts` | 382 | 16262 | 20528 | -20.78% |

Source-code analysis:

- `builderStatePublic.ts` is only 382 bytes in the local TypeScript checkout,
  so fixed parser and benchmark-loop overhead is a large part of its measured
  time. Earlier direct 100-repetition focused runs flipped the sign, measuring
  Rust faster on the same file, which confirms high noise sensitivity for this
  tiny case.
- The 100-repetition aggregate TypeScript-normal run is a better signal for
  parser throughput because it includes the larger TypeScript samples and did
  not show any >5% per-case regression.
- No source change was made and no rollback was performed. Keep the
  TypeScript-normal aggregate and tiny-file outliers on the watchlist for the
  next 10-change checkpoint.

### 2026-06-25 00:00 EDT

- Repo head: `4f4904c9`
- Batch base: `1cd98830`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Rust-core raw-pointer/reference cleanups from
  `Use stack fields when pushing` through
  `Use heap ref for external scanner state`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 30 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26712.8 | 25832.5 | +3.41% |
| TypeScript error parses | 32 | 1760.0 | 1688.3 | +4.25% |
| JavaScript normal parses | 2 | 17328.9 | 17035.0 | +1.73% |
| JavaScript error parses | 37 | 2055.6 | 2047.7 | +0.38% |
| Overall parser throughput | 82 | 2385.5 | 2322.1 | +2.73% |

The previous TypeScript/JavaScript checkpoint at `1cd98830` recorded Rust
overall throughput of 2364.5 bytes/ms. This checkpoint is +0.89% versus that
Rust throughput, so no aggregate parser regression was observed.

Per-case regression investigation:

- The first 10-repetition run reported JavaScript error outliers in
  `compound-statement-without-trailing-newline.py` and `crlf-line-endings.py`.
- A second 10-repetition rerun did not reproduce `crlf-line-endings.py`; it
  instead reported `typescript normal corePublic.ts` and a much smaller
  `compound-statement-without-trailing-newline.py` outlier.
- The 30-repetition checkpoint only kept
  `javascript error compound-statement-without-trailing-newline.py` above 5%:
  Rust 3120.6 bytes/ms, C 3369.1 bytes/ms, slowdown 7.37%.
- Because the outlier set changed between repeated runs, and the remaining
  outlier is a mismatched-language error-corpus parse while JavaScript error
  aggregate throughput stayed slightly positive, this is tracked as noisy
  per-case variance rather than a confirmed source-change regression.

Source-code analysis:

- This batch changed Rust-internal bodies in `stack.rs` and `subtree.rs`,
  replacing repeated raw-pointer dereferences with local references or copied
  `Subtree` values.
- No exported `#[no_mangle] extern "C"` signatures, C header declarations,
  public FFI struct layouts, allocation headers, generated parser templates, or
  included C header text were edited.
- Stack changes were limited to local reference bindings in stack push,
  summary, release, and link append paths. The link merge recursion, stack-head
  ownership, subtree retain/release behavior, and public stack function
  signatures are unchanged.
- Subtree changes were limited to local references in compare, set-symbol,
  edit, clone, external-scanner-state, and DOT graph helper bodies. Allocation
  sizes, heap layout, scanner-state copying, tree edit traversal, and subtree
  ownership behavior are unchanged.
- No rollback was performed.

### 2026-06-25 08:09 EDT

- Repo head: `f9ed5938`
- Batch base: `76c4401a`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Clippy/raw-pointer cleanup commits from
  `Use pointer cast in stack array delete` through
  `Use usize loop in stack node release`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25809.6 | 24561.8 | +5.08% |
| TypeScript error parses | 24 | 1665.3 | 1639.5 | +1.57% |
| TSX normal parses | 1 | 5420.3 | 5319.3 | +1.90% |
| TSX error parses | 27 | 1681.8 | 1624.3 | +3.54% |
| Overall parser throughput | 63 | 2043.4 | 1992.6 | +2.55% |

Prior checkpoint at `f5a30dbf` recorded Rust overall throughput of 2134.9
bytes/ms on the same TypeScript/TSX gate, so this repeat run measured -4.29%
absolute Rust throughput. The Rust-vs-C delta moved from +1.26% to +2.55%.

Per-case regression investigation:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error no_newline_at_eof.go` | 1072.1 | 1156.1 | 7.26% |

The first run at this same head reported lower aggregate throughput and a
different set of per-case outliers:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24628.0 | 24934.3 | -1.23% |
| TypeScript error parses | 24 | 1653.3 | 1649.9 | +0.20% |
| TSX normal parses | 1 | 5399.4 | 5589.5 | -3.40% |
| TSX error parses | 27 | 1662.8 | 1623.7 | +2.41% |
| Overall parser throughput | 63 | 2024.3 | 2002.3 | +1.10% |

First-run outliers were `typescript error
compound-statement-without-trailing-newline.py` at 11.37%,
`typescript normal builderStatePublic.ts` at 7.63%, and `typescript error
crlf-line-endings.py` at 6.89%. The repeat run removed those and reported only
`no_newline_at_eof.go`, so the per-case result is not stable.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation sizes, parse-table data, generated parser templates, C headers,
  parser control flow, stack ownership, subtree ownership, or generated parser
  ABI.
- Seven commits are direct pointer-cast spelling cleanups in internal stack
  helpers and allocation/free paths: `array_delete`, `array_reserve`,
  `array_assign`, stack node allocation/free, stack head summaries, stack
  allocation/destruction, and stack summary allocation/free. They keep the same
  allocator calls, element sizes, copy sizes, and pointer addresses.
- Two commits replace reference-to-raw-pointer constructions with
  `ptr::from_mut` or `ptr::addr_of!`/`ptr::addr_of_mut!` for internal callback
  payloads. The payload lifetime and callback type are unchanged, and the
  callbacks still cast the `void *` payload back to the same Rust type.
- The final commit rewrites a signed reverse loop over stack node links as a
  `usize` reverse range over `1..link_count`, preserving the separate handling
  of link zero and the release order for nonzero links.
- Given the repeat-run flip from TypeScript normal -1.23% to +5.08% Rust-vs-C
  and the changed per-case outliers, the >5% cases are most consistent with
  benchmark variance rather than a source-code regression in this batch.
- No rollback was performed.

### 2026-06-25 09:26 EDT

- Repo head: `469a0a98`
- Batch base: `91c0ecb8`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Clippy/stack cleanup commits from
  `Use u32 stack node pool size` through
  `Flip stack iterator subtree branch`
- Doc-only follow-up commit: `e0c1907d` clarifies that fixing Clippy warnings
  comes before broader raw pointer cleanup in `ROADMAP.md`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25273.7 | 23576.8 | +7.20% |
| TypeScript error parses | 24 | 1650.3 | 1588.1 | +3.92% |
| TSX normal parses | 1 | 5030.9 | 5403.1 | -6.89% |
| TSX error parses | 27 | 1628.9 | 1612.2 | +1.04% |
| Overall parser throughput | 63 | 1997.4 | 1956.1 | +2.11% |

Prior checkpoint at `91c0ecb8` recorded Rust overall throughput of 2043.4
bytes/ms on the same TypeScript/TSX gate, so this repeat run measured -2.25%
absolute Rust throughput. The Rust-vs-C delta moved from +2.55% to +2.11%.

Per-case regression investigation:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal performanceCore.ts` | 15099.2 | 21297.8 | 29.10% |
| `tsx error install.sh` | 316.7 | 389.8 | 18.76% |
| `typescript error deeply-nested-custom.html` | 6235.8 | 7227.1 | 13.72% |
| `tsx error cluster.c` | 2069.2 | 2306.9 | 10.30% |
| `tsx normal parser.ts` | 5030.9 | 5403.1 | 6.89% |
| `tsx error malloc.c` | 1004.3 | 1075.5 | 6.62% |
| `typescript normal builderStatePublic.ts` | 17634.6 | 18653.3 | 5.46% |

The first run at this same head reported a better aggregate result and a
different single per-case outlier:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24602.3 | 22862.6 | +7.61% |
| TypeScript error parses | 24 | 1670.9 | 1593.0 | +4.89% |
| TSX normal parses | 1 | 5477.2 | 5476.0 | +0.02% |
| TSX error parses | 27 | 1677.4 | 1628.2 | +3.02% |
| Overall parser throughput | 63 | 2044.1 | 1968.6 | +3.83% |

First-run outlier was `typescript error
compound-statement-without-trailing-newline.py` at 28.01%. The repeat run
removed that outlier but introduced a different set, including a TSX normal
aggregate regression that was not present in the first run.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation sizes, parse-table data, generated parser templates, C headers,
  parser control flow, stack ownership, subtree ownership, or generated parser
  ABI.
- The runtime edits are limited to internal `stack.rs` Clippy cleanups: using
  `u32` constants to match existing stack-array sizes, rewriting two reverse
  loops from signed-index `while` loops to `u32` reverse ranges, replacing
  reference-to-raw-pointer expressions with `ptr::addr_of!`, `ptr::from_ref`,
  or `.cast::<T>()`, and marking the stack version count accessor `const`.
- One internal stack iterator sentinel changed from a signed `i32` where `-1`
  meant "do not collect subtrees" to `Option<u32>`. Call sites map previous
  nonnegative values to `Some(...)` and the previous `-1` case to `None`;
  subtree collection, reserve sizing, and callback behavior are otherwise
  unchanged.
- One iterator branch was inverted to handle the null-subtree case first. The
  two branch bodies are unchanged, and retain/release behavior for non-null
  subtrees is preserved.
- Given the different outlier sets between the first and repeat runs, the
  first-run TSX normal result of +0.02% versus the repeat result of -6.89%,
  and the absence of parser-control-flow or ownership changes, the >5% cases
  are most consistent with benchmark variance rather than a confirmed
  source-code regression in this batch.
- No rollback was performed.

### 2026-06-25 01:24 EDT

- Repo head: `dbf0cdb2`
- Batch base: `3d53c44b`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small header/import/clippy cleanups from
  `Use Rust imports for parser core functions` through `Clean unicode helper
  docs`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 3 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24977.9 | 24292.7 | +2.82% |
| TypeScript error parses | 24 | 1735.4 | 1722.1 | +0.77% |
| TSX normal parses | 1 | 5639.5 | 5584.3 | +0.99% |
| TSX error parses | 27 | 1734.6 | 1726.5 | +0.47% |
| Overall parser throughput | 63 | 2117.4 | 2103.6 | +0.66% |

No per-case regressions above the 5% threshold.

Notes:

- This checkpoint used 3 repetitions and a smaller TypeScript/TSX-only gate to
  keep the ten-commit checkpoint fast after non-performance-focused cleanup.
  Treat it as a smoke perf record, not a release-quality benchmark.
- The batch did not touch exported FFI signatures, `#[repr(C)]` layouts,
  allocation behavior, parse-table data, or parser ownership rules.
- The parser-facing code changes are limited to replacing Rust declarations of
  Rust-implemented symbols with normal imports and removing redundant control
  flow in error comparison. The remaining changes are header trimming for stale
  transitional prototypes, internal helper mutability clarity, `const fn`
  helper annotations, and documentation/literal cleanup.
- Since Rust throughput stayed positive versus C in every measured aggregate
  and the gate reported no >5% per-case regressions, no source-level
  performance culprit was identified and no rollback was performed.

### 2026-06-25 01:55 EDT

- Repo head: `a82fee10`
- Batch base: `df732265`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small clippy/idiomatic cleanup commits from
  `Clean subtree flag imports` through `Use expression for reusable node end
  offset`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26452.9 | 24465.3 | +8.12% |
| TypeScript error parses | 24 | 1786.1 | 1690.6 | +5.65% |
| TSX normal parses | 1 | 5628.4 | 5741.9 | -1.98% |
| TSX error parses | 27 | 1777.6 | 1716.8 | +3.54% |
| Overall parser throughput | 63 | 2172.7 | 2082.1 | +4.35% |

Per-case regression investigation:

- The initial 3-repetition checkpoint reported `tsx error
  builderStatePublic.ts` as an 11.93% Rust slowdown versus C, while overall
  Rust throughput was +5.15%.
- A 10-repetition rerun did not reproduce `builderStatePublic.ts`; it instead
  reported `tsx error no_newline_at_eof.go` at 13.04% and `typescript error
  compound-statement-without-trailing-newline.py` at 6.71%.
- The final 10-repetition checkpoint reported a third set of outliers:
  `typescript error multiple-newlines.py` at 7.09% and `tsx error malloc.c` at
  5.80%.
- Because the >5% per-case outliers moved between unrelated mismatched-language
  error fixtures while aggregate TypeScript, TSX error, and overall throughput
  stayed positive, this checkpoint treats the outliers as benchmark noise rather
  than a confirmed source regression.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation behavior, parse-table data, generated parser templates, or C
  headers.
- Most commits converted local mutable temporaries into expression
  initializers, simplified one subtree-pool bound, or reused an existing
  iterator value. These are source-level cleanup changes rather than algorithm
  changes.
- Parser-facing changes were limited to local setup in scanner deserialize and
  reusable-node end-offset computation. The scanner state, EOF handling, tree
  reuse descent, and parse action flow remain semantically unchanged.
- No rollback was performed.

### 2026-06-25 02:21 EDT

- Repo head: `8a5d1c2d`
- Batch base: `177dd6b0`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small clippy/idiomatic cleanup commits from
  `Use expression for shifted subtree` through `Use From for subtree lengths`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26700.9 | 25630.6 | +4.18% |
| TypeScript error parses | 24 | 1737.8 | 1725.6 | +0.71% |
| TSX normal parses | 1 | 5743.4 | 5532.9 | +3.80% |
| TSX error parses | 27 | 1759.4 | 1709.0 | +2.95% |
| Overall parser throughput | 63 | 2136.0 | 2095.7 | +1.93% |

Per-case regression investigation:

- The first 10-repetition run reported two TSX mismatched-language error
  outliers above 5%: `corePublic.ts` at 8.04% and `multiple-newlines.py` at
  7.33%, while aggregate Rust throughput stayed positive.
- A second 10-repetition run did not reproduce either outlier and reported no
  per-case regressions above 5%.
- Because the outliers disappeared on immediate rerun and the aggregate
  TypeScript, TSX, and overall results stayed positive, this checkpoint treats
  the first-run outliers as benchmark noise rather than a confirmed source
  regression.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation behavior, parse-table data, generated parser templates, or C
  headers.
- Parser-facing changes were limited to local expression initializers in
  reduction and shift helpers. The shift/reduce control flow and stack/tree
  ownership behavior are unchanged.
- The remaining changes are clippy cleanups for documentation, local binding
  names, and lossless integer/boolean widening with `From`.
- No rollback was performed.

### 2026-06-25 02:53 EDT

- Repo head: `eb5b6bae`
- Batch base: `316f5ff2`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small clippy cleanup commits from
  `Use From for inline length limit` through `Use From for node alias context`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26554.0 | 25561.3 | +3.88% |
| TypeScript error parses | 24 | 1749.8 | 1715.2 | +2.02% |
| TSX normal parses | 1 | 5697.7 | 5736.9 | -0.68% |
| TSX error parses | 27 | 1750.1 | 1712.5 | +2.20% |
| Overall parser throughput | 63 | 2136.9 | 2094.7 | +2.01% |

Per-case regression investigation:

- The 10-repetition checkpoint reported no per-case regressions above the 5%
  threshold, so no rerun or rollback was needed.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation behavior, parse-table data, generated parser templates, or C
  headers.
- The changes are clippy-oriented idiomatic cleanups: explicit auto-deref
  removal, documentation formatting, checked inline leaf byte narrowing, and
  lossless integer widening via `From`.
- Parser-facing changes are limited to lossless widening of existing
  production id values when building subtrees; parse control flow, stack
  ownership, and tree layout are unchanged.
- No rollback was performed.

### 2026-06-25 03:25 EDT

- Repo head: `97e2a0a2`
- Batch base: `68a7fcc9`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small clippy cleanup commits from
  `Use From for language table indexes` through
  `Use From for visible child index`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26952.3 | 25813.3 | +4.41% |
| TypeScript error parses | 24 | 1740.3 | 1720.5 | +1.15% |
| TSX normal parses | 1 | 5880.0 | 5747.8 | +2.30% |
| TSX error parses | 27 | 1753.2 | 1724.3 | +1.67% |
| Overall parser throughput | 63 | 2136.1 | 2105.1 | +1.47% |

Per-case regression investigation:

- The first 10-repetition run reported two TSX error outliers above 5%:
  `builderStatePublic.ts` at 6.39% and `corePublic.ts` at 5.57%, while
  aggregate Rust throughput stayed positive at +1.30% overall.
- An immediate second 10-repetition run reported no per-case regressions above
  5%, with aggregate TypeScript, TSX, and overall Rust throughput still
  positive.
- Because the outliers disappeared on rerun and no source changes in this
  batch affect TSX-specific parsing logic, this checkpoint treats the first-run
  outliers as benchmark noise rather than a confirmed source regression.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation behavior, parse-table data, generated parser templates, or C
  headers.
- The changes are clippy-oriented lossless integer widening cleanups using
  `From` in `language.rs`, `subtree.rs`, `tree_cursor.rs`, `stack.rs`, and
  `parser.rs`.
- Parser-facing changes are limited to replacing existing casts with
  equivalent lossless conversions for external lexer state, reduce child count,
  recovery/log state values, and debug/DOT output fields. Parse control flow,
  stack ownership, and subtree layout are unchanged.
- No rollback was performed.

### 2026-06-25 03:58 EDT

- Repo head: `77f89d0c`
- Batch base: `edd7fdbc`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small clippy cleanup commits from
  `Use From for repeat depth delta` through `Use From for parser tree index`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26534.3 | 25266.0 | +5.02% |
| TypeScript error parses | 24 | 1751.0 | 1718.2 | +1.90% |
| TSX normal parses | 1 | 5755.0 | 5770.1 | -0.26% |
| TSX error parses | 27 | 1745.5 | 1717.7 | +1.62% |
| Overall parser throughput | 63 | 2135.7 | 2099.8 | +1.71% |

Per-case regression investigation:

- The 10-repetition checkpoint reported no per-case regressions above the 5%
  threshold, so no rerun or rollback was needed.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation behavior, parse-table data, generated parser templates, or C
  headers.
- The changes are clippy-oriented lossless integer widening cleanups using
  `From` in `subtree.rs`, `language.rs`, `parser.rs`, and `stack.rs`.
- Parser-facing changes are limited to equivalent widening conversions in
  debug/DOT output, logging state values, reduction dynamic precedence, and a
  reverse-loop index seed. Parse control flow, stack ownership, and subtree
  layout are unchanged.
- No rollback was performed.

### 2026-06-25 04:36 EDT

- Repo head: `7f716db7`
- Batch base: `3985e779`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small clippy cleanup commits from
  `Use From for language action count` through
  `Make subtree leaf parse state accessor const`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 27080.8 | 25392.8 | +6.65% |
| TypeScript error parses | 24 | 1749.1 | 1709.4 | +2.32% |
| TSX normal parses | 1 | 5852.9 | 5675.7 | +3.12% |
| TSX error parses | 27 | 1750.9 | 1710.5 | +2.36% |
| Overall parser throughput | 63 | 2139.6 | 2089.2 | +2.41% |

Per-case regression investigation:

- The 10-repetition checkpoint reported no per-case regressions above the 5%
  threshold, so no rerun or rollback was needed.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation behavior, parse-table data, generated parser templates, or C
  headers.
- The changes are clippy-oriented idiomatic cleanups: one lossless `From`
  conversion, a `map_or_else` simplification, and `const fn` annotations for
  pure subtree flag/metadata helpers.
- Parser-facing changes are limited to compile-time-callable annotations and
  equivalent helper construction/conversion logic. Runtime parse control flow,
  stack ownership, allocation, subtree layout, and raw pointer operations are
  unchanged.
- No rollback was performed.

### 2026-06-25 05:16 EDT

- Repo head: `0f412658`
- Batch base: `01bf0f01`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small clippy cleanup commits from
  `Make subtree conversion helpers const` through
  `Make parser read helpers const`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26646.0 | 24791.0 | +7.48% |
| TypeScript error parses | 24 | 1750.2 | 1705.2 | +2.64% |
| TSX normal parses | 1 | 5838.9 | 5674.4 | +2.90% |
| TSX error parses | 27 | 1741.9 | 1708.5 | +1.96% |
| Overall parser throughput | 63 | 2134.5 | 2085.1 | +2.37% |

Per-case regression investigation:

- The 10-repetition checkpoint reported no per-case regressions above the 5%
  threshold, so no rerun or rollback was needed.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation behavior, parse-table data, generated parser templates, or C
  headers.
- The changes are clippy-oriented `const fn` annotations for private or
  non-exported helpers in `subtree.rs`, `language.rs`, `stack.rs`,
  `parser.rs`, `get_changed_ranges.rs`, and `node.rs`.
- Parser-facing changes are limited to making existing pure construction,
  predicate, and read-copy helpers callable in const contexts. Runtime parse
  control flow, stack ownership, allocation, subtree layout, and raw pointer
  operations are unchanged.
- No rollback was performed.

### 2026-06-25 05:48 EDT

- Repo head: `07716c87`
- Batch base: `d25d89b0`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small clippy cleanup commits from
  `Make language alias helper const` through `Backtick language docs`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26817.1 | 25851.8 | +3.73% |
| TypeScript error parses | 24 | 1752.0 | 1717.5 | +2.01% |
| TSX normal parses | 1 | 5791.7 | 5653.3 | +2.45% |
| TSX error parses | 27 | 1752.0 | 1730.2 | +1.26% |
| Overall parser throughput | 63 | 2140.8 | 2105.3 | +1.69% |

Per-case regression investigation:

- The 10-repetition checkpoint reported no per-case regressions above the 5%
  threshold, so no rerun or rollback was needed.

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation behavior, parse-table data, generated parser templates, or C
  headers.
- The runtime changes are clippy-oriented `const fn` annotations for private
  helpers in `subtree.rs`, `language.rs`, `tree.rs`, `tree_cursor.rs`,
  `parser.rs`, and `node.rs`.
- The remaining changes are documentation-only clippy cleanups in `lexer.rs`
  and `language.rs`.
- Parser-facing runtime changes are limited to making existing pure
  construction, pointer-cast, read-copy, length arithmetic, and comparison
  helpers callable in const contexts. Runtime parse control flow, stack
  ownership, allocation, subtree layout, and raw pointer operations are
  unchanged.
- No rollback was performed.

### 2026-06-25 06:22 EDT

- Repo head: `b255b076`
- Batch base: `b393fc6d`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small raw-pointer cast cleanup commits from
  `Use pointer casts in scanner state helpers` through
  `Use pointer casts in subtree string allocation`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26932.4 | 25883.9 | +4.05% |
| TypeScript error parses | 24 | 1752.3 | 1724.7 | +1.60% |
| TSX normal parses | 1 | 5867.1 | 5716.1 | +2.64% |
| TSX error parses | 27 | 1749.1 | 1728.9 | +1.17% |
| Overall parser throughput | 63 | 2140.5 | 2109.8 | +1.46% |

Prior checkpoint at `07716c87` recorded Rust overall throughput of 2140.8
bytes/ms on the same TypeScript/TSX gate, so this batch was effectively flat
at -0.01% absolute Rust throughput. The Rust-vs-C delta fell from +1.69% to +1.46%,
mainly because this C run was slightly faster.

Per-case regression investigation:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `tsx error crlf-line-endings.py` | 1228.9 | 1316.0 | 6.62% |
| `typescript error compound-statement-without-trailing-newline.py` | 902.3 | 963.0 | 6.30% |

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation sizes, parse-table data, generated parser templates, C headers,
  parser control flow, stack ownership, or subtree ownership.
- The runtime edits are pointer-cast spelling cleanups in `subtree.rs`:
  replacing equivalent raw pointer `as *const/*mut T` casts with `.cast()`
  or `.cast_mut()`, including scanner-state, subtree-array, pool,
  construction, cloning, child access, and string-formatting paths.
- The only cast cleanup touching an internal allocation path keeps the same
  allocation size and offset, but computes the clone footer pointer by
  advancing a `*mut Subtree` before casting to `*mut SubtreeHeapData`, avoiding
  the previous intermediate `*mut u8` alignment warning.
- The two reported >5% per-case slowdowns are error-case fixtures that have
  shown sensitivity in earlier checkpoints. Given the flat absolute Rust
  throughput against the prior checkpoint and the lack of semantic parser
  changes, this is most likely benchmark/C-run variance rather than a source
  regression from this batch.
- No rollback was performed.

### 2026-06-25 06:55 EDT

- Repo head: `97927e34`
- Batch base: `de6d0d76`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small parser raw-pointer cleanup commits from
  `Use pointer casts in parser scanner helpers` through
  `Use pointer casts in parser logging buffers`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26754.7 | 25575.5 | +4.61% |
| TypeScript error parses | 24 | 1733.2 | 1714.2 | +1.11% |
| TSX normal parses | 1 | 5773.2 | 5776.3 | -0.06% |
| TSX error parses | 27 | 1750.0 | 1717.1 | +1.92% |
| Overall parser throughput | 63 | 2128.5 | 2097.4 | +1.48% |

Prior checkpoint at `b255b076` recorded Rust overall throughput of 2140.5
bytes/ms on the same TypeScript/TSX gate, so this batch was effectively flat
at -0.56% absolute Rust throughput. The Rust-vs-C delta moved from +1.46% to
+1.48%.

Per-case regression investigation:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal builderStatePublic.ts` | 19112.4 | 20742.8 | 7.86% |

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation sizes, parse-table data, generated parser templates, C headers,
  parser control flow, stack ownership, or subtree ownership.
- The runtime edits are pointer-cast spelling and raw-reference extraction
  cleanups in `parser.rs`: language pointer casts, `Array<T>` alias pointer
  extraction, external scanner buffer/data casts, `memcmp` pointer arguments,
  parser self-pointers for logging/state/control paths, and logging buffer
  casts.
- Integer payload casts and non-alias wrapper layout casts were deliberately
  left unchanged.
- The reported TypeScript normal per-case slowdown is isolated while aggregate
  TypeScript normal and overall Rust-vs-C throughput remain in line with prior
  checkpoints. Given the lack of semantic parser changes and the small
  aggregate movement, this is most likely benchmark/C-run variance rather than
  a source regression from this batch.
- No rollback was performed.

### 2026-06-25 07:35 EDT

- Repo head: `f5a30dbf`
- Batch base: `bd0dd7f3`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small clippy/raw-pointer cleanup commits from
  `Remove explicit deref in subtree clone` through
  `Use pointer casts in stack insert`
- Command:

```sh
cargo xtask perf-gate --language typescript --language tsx --repetitions 10 --error-limit 4 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 26440.1 | 25181.2 | +5.00% |
| TypeScript error parses | 24 | 1736.1 | 1722.6 | +0.78% |
| TSX normal parses | 1 | 5883.3 | 5768.1 | +2.00% |
| TSX error parses | 27 | 1756.0 | 1728.1 | +1.61% |
| Overall parser throughput | 63 | 2134.9 | 2108.3 | +1.26% |

Prior checkpoint at `97927e34` recorded Rust overall throughput of 2128.5
bytes/ms on the same TypeScript/TSX gate, so this batch was effectively flat
at +0.30% absolute Rust throughput. The Rust-vs-C delta moved from +1.48% to
+1.26%, mainly because this C run was slightly faster.

Per-case regression investigation:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error compound-statement-without-trailing-newline.py` | 916.8 | 973.9 | 5.86% |

Source-code analysis:

- This batch did not change exported FFI signatures, `#[repr(C)]` layouts,
  allocation sizes, parse-table data, generated parser templates, C headers,
  parser control flow, stack ownership, subtree ownership, or generated parser
  ABI.
- The first seven runtime edits are clippy-oriented readability cleanups:
  removing explicit auto-derefs in `subtree.rs`, `parser.rs`, and `stack.rs`,
  and collapsing equivalent nested conditions in subtree error-cost accounting,
  parser language setup, parser recovery, and stack splice copying.
- The final three runtime edits are pointer-cast spelling cleanups in generic
  internal stack array helpers: `array_splice`, `array_erase`, and
  `array_insert`. They keep the same byte offsets, lengths, and libc copy calls,
  replacing equivalent `as *mut/*const` casts with `.cast::<T>()`.
- The reported >5% case is the same TypeScript error fixture that has appeared
  in earlier checkpoint variance. Aggregate TypeScript error throughput
  improved slightly against the prior checkpoint, overall Rust throughput was
  effectively flat, and the source changes do not alter parser semantics, so no
  source regression is indicated.
- No rollback was performed.

### 2026-06-25 10:39 EDT

- Repo head: `445e9dac`
- Batch base: `63f93959`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Clippy/readability cleanup commits from
  `Make private callbacks const` through
  `Use C-string literals in subtree formatting`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25232.6 | 23617.1 | +6.84% |
| TypeScript error parses | 32 | 1635.9 | 1583.7 | +3.30% |
| JavaScript normal parses | 2 | 13703.9 | 13049.9 | +5.01% |
| JavaScript error parses | 37 | 1785.0 | 1637.0 | +9.04% |
| Overall parser throughput | 82 | 2154.1 | 2039.3 | +5.63% |

The most recent same-language TypeScript/JavaScript checkpoint at `4f4904c9`
used 30 repetitions and recorded Rust overall throughput of 2385.5 bytes/ms
and C overall throughput of 2322.1 bytes/ms. This 10-repetition run is -9.70%
absolute Rust throughput against that checkpoint, while the C run is -12.18%.
Because both implementations slowed substantially and the Rust-vs-C delta
improved from +2.73% to +5.63%, this checkpoint is treated as run/environment
variance rather than a confirmed Rust-source regression.

Per-case regression investigation:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error weird-exprs.rs` | 736.6 | 1120.6 | 34.27% |
| `javascript error malloc.c` | 866.7 | 1177.6 | 26.41% |
| `javascript error install.sh` | 375.9 | 432.5 | 13.10% |
| `typescript error relocate.sh` | 479.0 | 550.4 | 12.96% |
| `typescript normal builderStatePublic.ts` | 16203.6 | 18387.5 | 11.88% |
| `javascript error value.go` | 1101.7 | 1225.2 | 10.08% |
| `javascript error crlf-line-endings.py` | 1220.0 | 1336.7 | 8.73% |
| `javascript error performance.ts` | 2767.9 | 2982.0 | 7.18% |
| `javascript error atom.sh` | 576.4 | 616.1 | 6.44% |
| `javascript normal text-editor-component.js` | 13483.8 | 14217.3 | 5.16% |

Source-code analysis:

- This batch did not change exported FFI signatures, public C headers,
  `#[repr(C)]` layouts, allocation sizes, generated parser templates,
  parse-table data, parser control flow, stack ownership, subtree ownership, or
  query execution semantics.
- Runtime edits were low-level spelling/readability changes: private lexer
  callbacks marked `const`, equivalent raw pointer `as *const/*mut T` casts
  replaced with `.cast()` in lexer and language helpers, range checks rewritten
  to `Range::contains`, and nul-terminated byte literals replaced with
  `c"..."` literals while preserving the `*const i8` call shape.
- Documentation-only Clippy cleanups in `lexer.rs` and `language.rs` cannot
  affect generated code outside comments.
- The large absolute-throughput drop is not isolated to Rust and is larger for
  the C comparison run, so the data does not indicate a culprit commit in this
  batch. Keep `builderStatePublic.ts` and the JavaScript error-corpus outliers
  on the watchlist for the next same-gate checkpoint.
- No rollback was performed.

### 2026-06-25 10:56 EDT

- Repo head: `4f83c221`
- Batch base: `c46e668b`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: one broad mechanical Clippy-fix commit,
  `Apply tree-sitter library clippy fixes`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25702.6 | 24199.2 | +6.21% |
| TypeScript error parses | 32 | 1715.9 | 1621.9 | +5.79% |
| JavaScript normal parses | 2 | 17288.6 | 16486.1 | +4.87% |
| JavaScript error parses | 37 | 2052.4 | 1958.4 | +4.80% |
| Overall parser throughput | 82 | 2347.3 | 2226.8 | +5.41% |

Prior checkpoint at `c46e668b` recorded Rust overall throughput of 2154.1
bytes/ms on the same TypeScript/JavaScript gate, so this batch improved
absolute Rust throughput by 8.97%. The Rust-vs-C delta moved from +5.63% to
+5.41%, with no per-case regressions above the 5% threshold.

Source-code analysis:

- `cargo clippy --fix --lib -p tree-sitter --` was run, then rerun with
  `--allow-dirty` because the existing subtree formatting cleanup was already
  in the working tree.
- The generated patch was kept as one broad Clippy-fix commit for review. It
  mainly replaces equivalent raw pointer casts with `.cast()`/`addr_of!`/
  `from_ref`, removes redundant casts and trailing punctuation, adds constness
  to Rust-internal helpers, applies match guards, and converts selected
  nul-terminated byte literals to `c"..."` literals.
- Exported `extern "C"` function constness changes suggested by Clippy were
  reverted before testing, so exported FFI signatures, C headers, `#[repr(C)]`
  layouts, allocation sizes, generated parser templates, parse-table data,
  parser control flow, stack ownership, subtree ownership, and query execution
  semantics remain unchanged.
- The perf run shows no source regression signal. No rollback was performed.

### 2026-06-25 12:03 EDT

- Repo head: `4d6c2113`
- Batch base: `b607e4e4`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small Clippy/readability cleanup commits from
  `Use C-string literals in language helpers` through
  `Remove redundant macro trailing commas`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24718.6 | 24199.4 | +2.15% |
| TypeScript error parses | 32 | 1662.8 | 1618.1 | +2.76% |
| JavaScript normal parses | 2 | 16817.8 | 15908.5 | +5.72% |
| JavaScript error parses | 37 | 1921.7 | 1961.9 | -2.05% |
| Overall parser throughput | 82 | 2245.3 | 2223.9 | +0.96% |

Prior checkpoint at `4f83c221` recorded Rust overall throughput of 2347.3
bytes/ms on the same TypeScript/JavaScript gate, so this batch measured -4.35%
absolute Rust throughput. The Rust-vs-C delta moved from +5.41% to +0.96%,
while C overall throughput was effectively flat. This is below the 5% absolute
Rust regression threshold, but the JavaScript error outliers remain worth
watching in the next checkpoint.

Per-case regression investigation:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error release.sh` | 249.7 | 702.4 | 64.45% |
| `javascript error relocate.sh` | 393.7 | 593.3 | 33.65% |
| `javascript error compound-statement-without-trailing-newline.py` | 2645.9 | 3273.6 | 19.18% |
| `javascript error install.sh` | 386.2 | 459.8 | 16.02% |
| `javascript error proc.go` | 812.0 | 956.6 | 15.12% |
| `javascript error update-authors.sh` | 625.2 | 686.0 | 8.85% |

Source-code analysis:

- This batch did not change C headers, `#[repr(C)]` layouts, allocation sizes,
  generated parser templates, parse-table data, stack ownership, subtree
  ownership, or query execution semantics.
- Several exported Rust `extern "C"` functions were marked `const` by the
  Clippy const cleanup. This changes Rust source qualifiers only; exported
  symbol names, argument lists, return types, and the C ABI remain unchanged.
- The C-string literal commits replace nul-terminated byte strings with
  `c"..."` literals in language helpers and debug graph/logging paths. These
  keep the same pointer values at call sites and are outside normal benchmark
  parsing unless parser logging or dot graph generation is enabled.
- `Annotate parser range transmute` only spells the existing transmute source
  and destination types explicitly. It does not change the represented array
  layout or parser control flow.
- `Remove redundant tree cursor continues` affects tree cursor traversal, not
  the parser throughput benchmark hot loop.
- The non-core CLI/generate/loader cleanups remove redundant macro trailing
  commas or simplify style parsing/format strings; they do not participate in
  benchmark parser execution.
- The JavaScript error outliers are not tied to a plausible source-code change
  in this batch, and the aggregate result still shows Rust slightly ahead of C.
  No rollback was performed.

### 2026-06-25 12:34 EDT

- Repo head: `32de820d`
- Batch base: `3e27ae96`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small cleanup commits from
  `Rename tree cursor FFI parameter bindings` through
  `Use stack references for version helpers`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25802.0 | 24133.8 | +6.91% |
| TypeScript error parses | 32 | 1671.5 | 1625.6 | +2.82% |
| JavaScript normal parses | 2 | 17075.5 | 16169.0 | +5.61% |
| JavaScript error parses | 37 | 2055.2 | 1942.5 | +5.80% |
| Overall parser throughput | 82 | 2311.3 | 2222.3 | +4.01% |

Prior checkpoint at `4d6c2113` recorded Rust overall throughput of 2245.3
bytes/ms on the same TypeScript/JavaScript gate, so this batch measured +2.94%
absolute Rust throughput. The Rust-vs-C delta moved from +0.96% to +4.01%, and
the gate reported no per-case regressions above 5%.

Source-code analysis:

- The first seven commits are low-risk Clippy/readability cleanups: FFI parameter
  binding renames, raw-string hash/doc visibility cleanup in `xtask`, a
  highlight match simplification, an ordering-based comparison, and by-value
  receivers for 8-byte inline subtree metadata accessors.
- The three stack commits reduce internal Rust raw-pointer signatures for stack
  pop, renumber, and version helper paths. They keep exported symbols, C
  headers, `#[repr(C)]` layouts, allocation sizes, generated parser templates,
  parse-table data, stack ownership, and subtree ownership unchanged.
- The stack changes move raw-pointer-to-reference conversion to the parser/stack
  boundary only; parser control flow and stack mutation order remain the same.
- Full `cargo test --all` passed after each code commit in this checkpoint.
  No rollback was performed.

### 2026-06-25 13:01 EDT

- Repo head: `18a8eb92`
- Batch base: `b2650730`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small cleanup commits from
  `Trim unused subtree header helpers` through `Trim atomic header includes`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25464.8 | 23550.9 | +8.13% |
| TypeScript error parses | 32 | 1644.7 | 1626.3 | +1.13% |
| JavaScript normal parses | 2 | 16865.5 | 15630.3 | +7.90% |
| JavaScript error parses | 37 | 2006.4 | 1944.7 | +3.17% |
| Overall parser throughput | 82 | 2267.9 | 2222.0 | +2.07% |

Prior checkpoint at `32de820d` recorded Rust overall throughput of 2311.3
bytes/ms on the same TypeScript/JavaScript gate, so this batch measured -1.88%
absolute Rust throughput. The Rust-vs-C delta moved from +4.01% to +2.07%,
while C overall throughput was effectively unchanged.

The first checkpoint run at this same head measured Rust overall throughput of
2235.0 bytes/ms and C throughput of 2148.8 bytes/ms, still +4.01% overall for
Rust. It reported five per-case Rust slowdowns over 5% versus C:
`javascript error corePublic.ts` (80.81%),
`javascript error compound-statement-without-trailing-newline.py` (49.37%),
`typescript normal transform.ts` (9.23%),
`javascript error parser.c` (7.94%), and
`javascript error marker-index.h` (6.76%).

Because that outlier set did not match the prior checkpoint or the source-code
risk profile, the command was repeated before recording the checkpoint. The
repeat reduced the outlier list to one borderline case:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2831.6 | 3012.0 | 5.99% |

Source-code analysis:

- The batch has two Rust parser-stack signature cleanups:
  `ts_stack_merge`/`ts_stack_can_merge` and `ts_stack_push` now take internal
  `Stack` references instead of raw `*mut Stack` parameters. The raw conversion
  remains at the parser/stack boundary, and stack ownership, mutation order, FFI
  exports, parser control flow, parse-table data, and `#[repr(C)]` layouts are
  unchanged.
- The remaining eight commits are C header cleanup: removing unused static
  inline helpers, stale internal macros, dead internal headers
  (`error_costs.h`, `portable/endian.h`, `unicode/utf16.h`), and two unused
  `atomic.h` includes. These files are not used by the Rust parser hot path.
- The generated parser headers `crates/generate/src/templates/alloc.h`,
  `crates/generate/src/templates/array.h`, and `crates/generate/src/parser.h.inc`
  were not edited. The public generated parser ABI remains unchanged.
- Full `cargo test --all` passed after every commit in this checkpoint.
- The per-case slowdown report was not stable across two runs, while aggregate
  Rust throughput stayed within a small band and remained ahead of C overall.
  No rollback was performed.

### 2026-06-25 13:27 EDT

- Repo head: `be4cc0fc`
- Batch base: `d553eab6`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small cleanup commits from
  `Use stack references for debug helpers` through
  `Remove stale subtree header types`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

First run:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24931.2 | 24865.2 | +0.27% |
| TypeScript error parses | 32 | 1657.7 | 1643.1 | +0.89% |
| JavaScript normal parses | 2 | 17330.2 | 15932.3 | +8.77% |
| JavaScript error parses | 37 | 2048.4 | 1972.3 | +3.86% |
| Overall parser throughput | 82 | 2296.7 | 2249.5 | +2.09% |

Per-case regressions over 5% in the first run:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal corePublic.ts` | 21593.8 | 27376.8 | 21.12% |
| `javascript error performance.ts` | 2838.5 | 3092.5 | 8.21% |
| `typescript normal packageJsonCache.ts` | 16875.5 | 17831.3 | 5.36% |

Because the per-case list did not match the previous checkpoint and the changed
source did not touch language-specific parsing behavior, the same command was
repeated before recording the batch:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25806.2 | 24897.9 | +3.65% |
| TypeScript error parses | 32 | 1633.6 | 1660.7 | -1.63% |
| JavaScript normal parses | 2 | 17704.9 | 16394.1 | +8.00% |
| JavaScript error parses | 37 | 2090.5 | 2006.3 | +4.20% |
| Overall parser throughput | 82 | 2294.7 | 2279.5 | +0.66% |

Per-case regressions over 5% in the repeat run:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error crlf-line-endings.py` | 1190.1 | 1309.2 | 9.10% |
| `typescript error compound-statement-without-trailing-newline.py` | 886.9 | 961.4 | 7.75% |

Prior checkpoint at `d553eab6` recorded Rust overall throughput of 2267.9
bytes/ms and C throughput of 2222.0 bytes/ms. This checkpoint measured Rust
overall throughput of 2296.7 bytes/ms in the first run and 2294.7 bytes/ms in
the repeat, so absolute Rust throughput moved by about +1.18% to +1.27% versus
that checkpoint. The Rust-vs-C delta moved from +2.07% to +2.09% in the first
run and +0.66% in the repeat.

Source-code analysis:

- Five commits convert internal stack-node helper signatures from raw
  `*mut StackNode` parameters to `&mut StackNode` parameters. These changes keep
  raw graph links as raw pointers, preserve allocation/layout, and convert back
  to raw pointers only where the graph stores identity. They do not change stack
  ownership, link traversal order, parser control flow, parse tables, or subtree
  retain/release order.
- Two commits convert internal stack debug/summary helpers and `ts_node_edit`'s
  point-edit call away from internal calls through FFI-shaped raw pointers. The
  exported FFI functions and signatures remain unchanged.
- Three commits remove unused C-header constants/types from `length.h`,
  `unicode.h`, and `subtree.h`. The active C query path still uses
  `ts_subtree_symbol`, `ts_subtree_is_repetition`, and `ts_decode_utf8`; those
  definitions were not changed. Generated parser templates
  (`crates/generate/src/templates/alloc.h`,
  `crates/generate/src/templates/array.h`, and
  `crates/generate/src/parser.h.inc`) were not edited.
- Full `cargo test --all` passed after every commit in this checkpoint.
- The >5% per-case slowdown sets changed completely between the first run and
  the repeat, while aggregate Rust throughput was stable and remained above the
  previous checkpoint. No rollback was performed.

### 2026-06-25 13:52 EDT

- Repo head: `26a7cc28`
- Batch base: `b1ba2d86`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: header cleanup commits from `Remove stale wasm store header`
  through `Restore array swap helper`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25054.9 | 24627.3 | +1.74% |
| TypeScript error parses | 32 | 1644.0 | 1644.3 | -0.02% |
| JavaScript normal parses | 2 | 17278.0 | 15820.8 | +9.21% |
| JavaScript error parses | 37 | 2069.4 | 1966.6 | +5.22% |
| Overall parser throughput | 82 | 2293.6 | 2247.6 | +2.05% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error compound-statement-without-trailing-newline.py` | 935.1 | 1006.1 | 7.06% |
| `typescript normal transform.ts` | 23211.6 | 24438.5 | 5.02% |

Prior checkpoint at `be4cc0fc` recorded Rust overall throughput of 2296.7
bytes/ms in the first run and 2294.7 bytes/ms in the repeat. This checkpoint
measured 2293.6 bytes/ms, so absolute Rust throughput stayed within about 0.1%
of the prior repeated measurement. The Rust-vs-C delta moved from +0.66% in the
prior repeat to +2.05%.

Source-code analysis:

- This batch is C-header cleanup only in net effect: it removes unused includes,
  stale runtime-only array macros, a stale runtime-only subtree macro, and the
  unused `wasm_store.h` declaration header. It does not touch Rust parser
  control flow, parse tables, stack/subtree ownership, or exported FFI
  signatures.
- A temporary removal of two `LookaheadIterator` fields was reverted in
  `Restore lookahead iterator fields`. The private `array_swap` helper and its
  `struct Swap` backing type were also restored in `Restore array swap helper`.
  The final net diff for this checkpoint removes no struct/union/enum fields.
- Generated parser templates (`crates/generate/src/templates/alloc.h`,
  `crates/generate/src/templates/array.h`, and
  `crates/generate/src/parser.h.inc`) were not edited.
- `typescript error compound-statement-without-trailing-newline.py` has appeared
  repeatedly as a >5% per-case outlier in earlier checkpoint logs, and
  `typescript normal transform.ts` is a borderline 5.02% outlier. Given the
  source delta and stable aggregate throughput, no rollback was performed.
- Full `cargo test --all` passed after every committed code/header change in
  this checkpoint.

### 2026-06-25 14:16 EDT

- Repo head: `6c0bc71e`
- Batch base: `9e28b120`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: internal Rust helper/import cleanup commits from
  `Use range edit helper internally` through
  `Use subtree changed range helper internally`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25806.5 | 24903.3 | +3.63% |
| TypeScript error parses | 32 | 1656.8 | 1655.9 | +0.06% |
| JavaScript normal parses | 2 | 17399.3 | 15770.9 | +10.33% |
| JavaScript error parses | 37 | 2059.0 | 1980.9 | +3.94% |
| Overall parser throughput | 82 | 2301.2 | 2263.4 | +1.67% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal builderStatePublic.ts` | 18082.8 | 19556.6 | 7.54% |

Prior checkpoint at `26a7cc28` recorded Rust overall throughput of 2293.6
bytes/ms and a Rust-vs-C delta of +2.05%. This checkpoint measured 2301.2
bytes/ms, so absolute Rust throughput increased by about 0.33%; the Rust-vs-C
delta moved to +1.67%.

Source-code analysis:

- Four commits remove Rust-side `extern "C"` declarations for functions already
  implemented in Rust or consolidate duplicate local type mirrors in favor of
  the canonical Rust definitions. These do not change the exported
  `#[no_mangle] extern "C"` symbols, C ABI, struct layout, or parser control
  flow.
- Three commits route internal Rust callers through crate-local helpers
  (`ts_range_edit_ref`, `ts_range_array_get_changed_ranges_ref`, and
  `ts_subtree_get_changed_ranges_ref`) while preserving the public C wrappers.
  The only changed runtime paths are tree editing and changed-range
  calculation, not fresh parser throughput.
- One commit changes the internal lookahead iterator step helper from a raw
  pointer parameter to `&mut LookaheadIterator`; the exported iterator API is
  unchanged.
- The only >5% outlier is a normal fresh-parse TypeScript case. This batch does
  not alter parsing tables, lexer/parser hot loops, stack/subtree ownership, or
  the TypeScript grammar inputs used by the benchmark. Given the higher
  aggregate Rust throughput and the mismatch between touched code paths and the
  outlier workload, no rollback was performed.
- Full `cargo test --all` passed after every committed code change in this
  checkpoint.

### 2026-06-25 15:55 EDT

- Repo head: `ae9c7b3d`
- Batch base: `768f0fe1`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: typed array/slice access helper cleanup
  commits from `Add typed array access helpers` through
  `Use slice for lexer included ranges`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

Primary checkpoint rerun:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24932.6 | 24607.1 | +1.32% |
| TypeScript error parses | 32 | 1710.0 | 1622.8 | +5.38% |
| JavaScript normal parses | 2 | 16645.2 | 15747.0 | +5.70% |
| JavaScript error parses | 37 | 1998.4 | 1957.1 | +2.11% |
| Overall parser throughput | 82 | 2316.9 | 2225.7 | +4.10% |

Per-case regressions over 5% in the checkpoint rerun:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error parser.c` | 1174.2 | 1510.3 | 22.25% |
| `javascript error malloc.c` | 1080.6 | 1327.9 | 18.62% |
| `javascript error compound-statement-without-trailing-newline.py` | 2864.5 | 3219.2 | 11.02% |
| `typescript error compound-statement-without-trailing-newline.py` | 906.8 | 975.1 | 7.00% |
| `typescript normal transform.ts` | 21924.9 | 23192.6 | 5.47% |

The first run of the same command reported a lower overall Rust throughput and
a different per-case regression set:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25412.0 | 23624.6 | +7.57% |
| TypeScript error parses | 32 | 1641.1 | 1591.8 | +3.10% |
| JavaScript normal parses | 2 | 16924.3 | 15576.5 | +8.65% |
| JavaScript error parses | 37 | 1997.3 | 1929.6 | +3.51% |
| Overall parser throughput | 82 | 2261.2 | 2187.2 | +3.38% |

First-run per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error update-authors.sh` | 420.5 | 609.6 | 31.02% |
| `javascript error test.sh` | 508.8 | 695.7 | 26.86% |
| `javascript error release.sh` | 537.5 | 730.4 | 26.41% |
| `javascript error relocate.sh` | 444.8 | 595.1 | 25.25% |
| `typescript error doc-build.sh` | 478.9 | 640.7 | 25.25% |
| `typescript error clean-old.sh` | 382.5 | 440.4 | 13.15% |
| `typescript normal performanceCore.ts` | 21894.7 | 23906.8 | 8.42% |

Prior checkpoint at `e6779351` recorded Rust overall throughput of 2357.7
bytes/ms and a Rust-vs-C delta of +5.25%. The checkpoint rerun measured 2316.9
bytes/ms, so absolute Rust throughput moved by about -1.73%; C throughput moved
from 2240.1 to 2225.7 bytes/ms, and the Rust-vs-C delta moved to +4.10%.

Source-code analysis:

- This batch centralizes raw array pointer access behind typed helpers and
  slice views. It does not change exported `#[no_mangle] extern "C"` symbols,
  C ABI, struct layout, parser tables, or parser control flow.
- The parser changes are helper-only: generic `Array<T>` reference helpers and
  local casts for `SubtreeArray`, `MutableSubtreeArray`, and `TSRangeArray`.
  The changed call sites preserve the same underlying pointer/index semantics.
- The `get_changed_ranges`, `tree_cursor`, and `lexer` changes convert local
  indexing helpers to `from_raw_parts`/`from_raw_parts_mut` plus unchecked
  indexing. These paths are outside fresh parser table execution except for
  lexer included-range lookup, whose helper still indexes the same
  `included_ranges` allocation and is guarded by the same range-count checks.
- The two full runs disagreed on the per-case regression set:
  `performanceCore.ts` in the first run did not reproduce in the rerun, and the
  shell-script JavaScript error cases were replaced by different C/Python
  fixtures. This points to benchmark noise rather than a stable source-level
  regression.
- Both full runs were aggregate-positive versus C. Given the non-reproducing
  per-case set and ABI/control-flow-neutral source diff, no rollback was
  performed.
- Full `cargo test --all` passed after every committed code change in this
  checkpoint.

### 2026-06-25 15:21 EDT

- Repo head: `e6779351`
- Batch base: `66cf1224`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: internal node/tree accessor cleanup
  commits from `Use cursor helpers for child navigation` through
  `Use node named child count helper`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25205.1 | 24594.4 | +2.48% |
| TypeScript error parses | 32 | 1714.2 | 1635.7 | +4.80% |
| JavaScript normal parses | 2 | 17499.0 | 16364.2 | +6.93% |
| JavaScript error parses | 37 | 2080.2 | 1962.7 | +5.99% |
| Overall parser throughput | 82 | 2357.7 | 2240.1 | +5.25% |

Per-case regressions over 5% in the full run:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2752.8 | 2965.0 | 7.16% |

Prior checkpoint at `66cf1224` recorded Rust overall throughput of 2155.8
bytes/ms and a Rust-vs-C delta of +1.87%. This checkpoint measured 2357.7
bytes/ms, so absolute Rust throughput moved by about +9.36%; C throughput also
moved from 2116.2 to 2240.1 bytes/ms in the same run series, and the Rust-vs-C
delta moved to +5.25%.

Source-code analysis:

- This batch only adds private Rust helpers and rewires internal callers to
  them. The exported `#[no_mangle] extern "C"` symbols, C ABI, struct layout,
  parser tables, and parser control flow are unchanged.
- The touched code is centered on `TSNode`/`TSTree` accessor paths: root lookup,
  null checks, child counts, position accessors, symbol/type metadata, state
  flags, and named-child counts.
- The single per-case JavaScript error slowdown is not explained by a parser
  algorithm change in this batch. The same run shows JavaScript normal and error
  aggregate throughput faster than C, and overall Rust throughput improved
  versus both C and the prior checkpoint.
- No rollback was performed because the regression is isolated to one error
  fixture, the batch is accessor-only, and the aggregate parser result is
  positive. If this same fixture stays over 5% in a later repeated run, inspect
  generated code around error recovery/node metadata access before changing
  parser control flow.
- Full `cargo test --all` passed after every committed code change in this
  checkpoint.

### 2026-06-25 14:38 EDT

- Repo head: `f89eaef3`
- Batch base: `ef39ecef`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: wrapper-boundary and internal child/range lookup cleanup commits
  from `Use tree cursor init helper internally` through
  `Extract empty tree cursor setup`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25374.0 | 24091.5 | +5.32% |
| TypeScript error parses | 32 | 1645.4 | 1625.0 | +1.26% |
| JavaScript normal parses | 2 | 17281.3 | 15914.6 | +8.59% |
| JavaScript error parses | 37 | 2052.8 | 1952.7 | +5.12% |
| Overall parser throughput | 82 | 2288.4 | 2225.6 | +2.82% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 2960.2 | 3121.1 | 5.16% |

Prior checkpoint at `6c0bc71e` recorded Rust overall throughput of 2301.2
bytes/ms and a Rust-vs-C delta of +1.67%. This checkpoint measured 2288.4
bytes/ms, so absolute Rust throughput moved by about -0.56%; C throughput moved
from 2263.4 to 2225.6 bytes/ms in the same run series, and the Rust-vs-C delta
improved to +2.82%.

Source-code analysis:

- This batch centralizes raw pointer casts at existing FFI wrapper boundaries
  for tree cursors, trees, range arrays, range edits, and subtree changed-range
  calls. Exported `#[no_mangle] extern "C"` symbols and signatures are
  unchanged.
- The internal helper extractions name existing raw pointer arithmetic for
  cursor child lookup, changed-range child lookup, tree included-range lookup,
  and temporary changed-range cursor initialization. They do not change parser
  tables, lexer/parser hot loops, stack/subtree ownership, or retain/release
  behavior.
- The changed runtime paths are tree cursor operations, tree edit/range edit,
  and changed-range calculation. The perf gate workload here measures parser
  throughput, so the single 5.16% JavaScript error outlier does not match the
  touched code paths and is close to the threshold. No rollback was performed.
- Full `cargo test --all` passed after every committed code change in this
  checkpoint.

### 2026-06-25 15:01 EDT

- Repo head: `d478e797`
- Batch base: `b52d8559`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: node pointer accessor cleanup and Rust-side libc import cleanup
  commits from `Extract node child lookup` through
  `Use node tree helper for parent lookup`
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 24389.8 | 23235.7 | +4.97% |
| TypeScript error parses | 32 | 1628.2 | 1565.0 | +4.04% |
| JavaScript normal parses | 2 | 14308.4 | 15584.0 | -8.18% |
| JavaScript error parses | 37 | 1799.1 | 1816.7 | -0.97% |
| Overall parser throughput | 82 | 2155.8 | 2116.2 | +1.87% |

Per-case regressions over 5% in the full run:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error mixed-spaces-tabs.py` | 124.3 | 296.8 | 58.12% |
| `typescript error compound-statement-without-trailing-newline.py` | 537.8 | 891.7 | 39.69% |
| `typescript error crlf-line-endings.py` | 664.3 | 1035.6 | 35.86% |
| `javascript error doc-build.sh` | 493.6 | 746.6 | 33.88% |
| `javascript error proc.go` | 787.6 | 921.4 | 14.52% |
| `typescript error jquery.js` | 12853.0 | 14885.8 | 13.66% |
| `typescript error text-editor-component.js` | 14273.0 | 16446.8 | 13.22% |
| `javascript normal text-editor-component.js` | 14254.5 | 15988.1 | 10.84% |
| `javascript error packageJsonCache.ts` | 3509.5 | 3819.3 | 8.11% |
| `javascript error parser.ts` | 5005.2 | 5433.0 | 7.87% |

Prior checkpoint at `f89eaef3` recorded Rust overall throughput of 2288.4
bytes/ms and a Rust-vs-C delta of +2.82%. This full checkpoint measured 2155.8
bytes/ms, so absolute Rust throughput moved by about -5.80%; C throughput also
moved down from 2225.6 to 2116.2 bytes/ms in the same run series, and the
Rust-vs-C delta moved to +1.87%.

Because the full run showed a JavaScript normal regression over 5% on only two
normal cases, a narrower JavaScript-only rerun was used to check noise:

```sh
cargo xtask perf-gate --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| JavaScript normal parses | 2 | 17359.7 | 16054.8 | +8.13% |
| JavaScript error parses | 37 | 2089.0 | 1992.6 | +4.84% |
| JavaScript overall | 39 | 2716.7 | 2588.0 | +4.97% |

The JavaScript rerun reported no per-case regressions over 5%.

Source-code analysis:

- The node commits centralize `TSNode` tree/language/subtree access behind
  existing internal helpers. They do not change exported
  `#[no_mangle] extern "C"` symbols, C ABI, struct layout, parsing tables, or
  parser control flow.
- The tree/tree-cursor/lexer changes remove libc `memcpy` imports and use typed
  Rust pointer copies. The changed paths copy included ranges and tree cursor
  stacks; these are not expected to affect fresh parser throughput materially.
- The stack change replaces generic array `memcpy`/`memmove` imports with
  `ptr::copy`, preserving overlapping-copy behavior. This code is closer to the
  parser hot path than the other import cleanups, so it remains the first
  source-code area to recheck if future repeated perf runs show a stable parser
  regression.
- The subtree scanner-state comparison and language string lookup commits
  preserve C semantics explicitly: zero-length scanner-state equality is
  handled before slice creation, and the language helper preserves
  `strncmp`-style ordering for the sorted field-name lookup.
- The full run's JavaScript regression did not reproduce in the targeted
  rerun, which measured JavaScript normal at +8.13% and reported no per-case
  regressions. Given that and the positive full-run overall delta, no rollback
  was performed.
- Full `cargo test --all` passed after every committed code change in this
  checkpoint.

### 2026-06-25 17:57 EDT

- Repo head: `8f3b555e`
- Batch base: `3330f77b`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small parser stack accessor cleanups:
  `Use parser stack accessors in reduce tail` through
  `Use parser stack accessors in stack condensing`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25463.9 | 24791.1 | +2.71% |
| TypeScript error parses | 32 | 1703.4 | 1649.3 | +3.28% |
| JavaScript normal parses | 2 | 17512.5 | 15781.5 | +10.97% |
| JavaScript error parses | 37 | 2075.7 | 1942.3 | +6.87% |
| Overall parser throughput | 82 | 2347.1 | 2241.3 | +4.72% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error jquery.js` | 14715.5 | 15682.6 | 6.17% |
| `typescript error compound-statement-without-trailing-newline.py` | 912.8 | 966.5 | 5.56% |
| `javascript error compound-statement-without-trailing-newline.py` | 2752.8 | 2908.3 | 5.35% |

Prior checkpoint at `a964b3ca` measured Rust overall throughput of 2314.7
bytes/ms and a Rust-vs-C delta of +2.76%. This checkpoint measured 2347.1
bytes/ms, so absolute Rust throughput moved by about +1.40%. C throughput moved
from 2252.6 to 2241.3 bytes/ms, and the Rust-vs-C delta moved to +4.72%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parser action or
  parsing table semantic changes.
- The parser commits replace direct raw stack reference formation
  (`&*self_.stack` and `&mut *self_.stack`) with the existing
  `parser_stack_ref` and `parser_stack_mut` accessors across reduce tail,
  accept, reduction loop, state recovery, recovery setup/tail, error handling,
  advance, isolated parser checks, and stack condensing.
- The final search for direct `self_.stack` raw-reference formation in
  `parser.rs` is empty. Remaining parser raw-pointer work is now in other
  parser-owned pointer fields and exported API/FFI boundary handling.
- The three per-case slowdowns are narrow error fixtures just over the 5%
  threshold, while every aggregate bucket is faster than C and absolute Rust
  throughput improved versus the prior checkpoint. The changed code is
  mechanical reference formation around identical stack operations, so these
  outliers do not currently point to a source-level regression in this batch.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 21:01 EDT

- Repo head: `17778178`
- Batch base: `34802f4a`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small tier-2 stack/array raw-pointer cleanup commits:
  `Take array init by mutable reference` through
  `Take array get by reference`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25595.5 | 23089.3 | +10.85% |
| TypeScript error parses | 32 | 1681.0 | 1599.7 | +5.08% |
| JavaScript normal parses | 2 | 17310.2 | 16259.9 | +6.46% |
| JavaScript error parses | 37 | 2031.4 | 1922.1 | +5.68% |
| Overall parser throughput | 82 | 2309.6 | 2191.4 | +5.39% |

Per-case regressions over 5%: none.

Prior checkpoint at `34802f4a` measured Rust overall throughput of 2296.3
bytes/ms and a Rust-vs-C delta of +5.19%. This checkpoint measured 2309.6
bytes/ms, so absolute Rust throughput moved by about +0.58%. C throughput moved
from 2183.0 to 2191.4 bytes/ms, and the Rust-vs-C delta moved to +5.39%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parser action or
  parsing table semantic changes.
- The stack callback commit removes an unnecessary internal C ABI callback type
  in tier 2. The callbacks are only passed within Rust stack traversal code, so
  changing them to Rust ABI does not affect generated parser, CLI, library, or
  external C ABI behavior.
- The array commits change internal helper signatures from raw array pointers to
  `&Array<T>` / `&mut Array<T>` at existing ownership boundaries. The helpers
  still perform the same pointer arithmetic and preserve the `Array<T>` memory
  layout used by FFI-facing data structures.
- The final tier-0/1/2 extern audit found no other removable C ABI use in those
  tiers without crossing an external ABI or C-library boundary: remaining
  externs are allocator/libc/stdio imports, exported tree-sitter API symbols,
  generated language/scanner callback ABI, lexer variadic log shim, or wasm
  store C imports.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 22:32 EDT

- Repo head: `dbf2a3cc`
- Batch base: `6cc97d59`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small stack/parser raw-pointer cleanup commits:
  `Remove unused array front helper` through
  `Use from_mut for stack callback payloads`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

Initial run:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25146.8 | 23786.3 | +5.72% |
| TypeScript error parses | 32 | 1699.1 | 1588.3 | +6.98% |
| JavaScript normal parses | 2 | 13725.3 | 16052.9 | -14.50% |
| JavaScript error parses | 37 | 2048.7 | 1941.1 | +5.55% |
| Overall parser throughput | 82 | 2321.9 | 2190.3 | +6.01% |

Initial per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript normal jquery.js` | 12017.7 | 15712.6 | 23.52% |
| `javascript error compound-statement-without-trailing-newline.py` | 2799.3 | 3267.7 | 14.34% |
| `typescript normal builderStatePublic.ts` | 17413.5 | 19519.7 | 10.79% |
| `typescript error compound-statement-without-trailing-newline.py` | 865.5 | 951.7 | 9.05% |
| `javascript error mixed-spaces-tabs.py` | 356.3 | 389.6 | 8.54% |
| `javascript error corePublic.ts` | 2374.7 | 2588.5 | 8.26% |
| `typescript error text-editor-component.js` | 14967.4 | 16005.3 | 6.48% |
| `javascript error utilities.ts` | 2332.1 | 2483.1 | 6.08% |

Rerun:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25858.2 | 24190.3 | +6.89% |
| TypeScript error parses | 32 | 1742.6 | 1653.8 | +5.37% |
| JavaScript normal parses | 2 | 17975.2 | 16107.4 | +11.60% |
| JavaScript error parses | 37 | 2088.7 | 2029.7 | +2.91% |
| Overall parser throughput | 82 | 2386.3 | 2282.2 | +4.56% |

Rerun per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error no_newline_at_eof.go` | 1208.5 | 1315.6 | 8.14% |

Prior checkpoint at `6cc97d59` measured Rust overall throughput of 2309.6
bytes/ms and a Rust-vs-C delta of +5.39%. This checkpoint's rerun measured
2386.3 bytes/ms, so absolute Rust throughput moved by about +3.32%. C
throughput moved from 2191.4 to 2282.2 bytes/ms, and the Rust-vs-C delta moved
to +4.56%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parser action or
  parsing table semantic changes.
- The array commits remove a dead helper, use initialized source slices for
  bulk copy/assignment, and change internal helper signatures/casts from raw
  pointer spelling to reference or pointer-adapter spelling. Destination writes
  that append into reserved but not-yet-sized storage remain raw pointer writes.
- The parser and stack array-view commits centralize repr-compatible array view
  casts behind local helpers or pointer adapter methods. They do not alter the
  underlying `Array<T>`, `SubtreeArray`, `MutableSubtreeArray`, or
  `TSRangeArray` layout.
- The summary and callback payload commits replace repeated inline casts or
  `ptr::addr_of_mut!` on local variables with existing helper/reference forms.
  The callback payload ABI remains `*mut c_void`.
- The initial run's broad JavaScript normal regression did not reproduce on the
  rerun: JavaScript normal moved from -14.50% to +11.60%, and the eight initial
  per-case regressions collapsed to one different TypeScript error fixture.
  Given this instability, the initial per-case list is treated as benchmark
  noise rather than a stable source-level regression for this mechanical batch.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

### 2026-06-25 23:01 EDT

- Repo head: `12ef4dcd`
- Batch base: `f08d0c20`
- C core revision: `c9f80282ad355a88a389d75173d918de84ef3e79`
- Change batch: 10 small tier-3/reference and clippy cleanup commits:
  `Use split slice for stack head pair` through
  `fix clippy`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

Initial run:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25118.9 | 25443.4 | -1.28% |
| TypeScript error parses | 32 | 1701.2 | 1685.4 | +0.94% |
| JavaScript normal parses | 2 | 16110.0 | 16177.2 | -0.42% |
| JavaScript error parses | 37 | 2134.3 | 1994.5 | +7.01% |
| Overall parser throughput | 82 | 2365.7 | 2294.7 | +3.09% |

Initial per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal performanceCore.ts` | 17266.1 | 23290.9 | 25.87% |
| `javascript error release.sh` | 614.6 | 712.4 | 13.72% |
| `typescript normal builderStatePublic.ts` | 18833.5 | 20616.3 | 8.65% |
| `javascript normal text-editor-component.js` | 16710.5 | 17784.4 | 6.04% |
| `javascript error atom.sh` | 824.3 | 871.0 | 5.36% |
| `typescript error update-authors.sh` | 527.5 | 556.2 | 5.16% |

Rerun:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25025.3 | 25216.2 | -0.76% |
| TypeScript error parses | 32 | 1695.1 | 1669.7 | +1.53% |
| JavaScript normal parses | 2 | 18162.4 | 17004.7 | +6.81% |
| JavaScript error parses | 37 | 2135.4 | 2042.6 | +4.54% |
| Overall parser throughput | 82 | 2365.5 | 2303.9 | +2.67% |

Rerun per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript normal packageJsonCache.ts` | 17551.7 | 18966.1 | 7.46% |
| `typescript error update-authors.sh` | 544.0 | 576.9 | 5.71% |
| `typescript normal performance.ts` | 19650.2 | 20838.9 | 5.70% |

Prior checkpoint at `f08d0c20` measured Rust overall throughput of 2386.3
bytes/ms and a Rust-vs-C delta of +4.56%. This checkpoint's rerun measured
2365.5 bytes/ms, so absolute Rust throughput moved by about -0.87%. C
throughput moved from 2282.2 to 2303.9 bytes/ms, and the Rust-vs-C delta moved
to +2.67%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header field changes, and no parser table or
  parser action semantic changes.
- The parser diff only adds slice conversion for included-range diffing on the
  old-tree reuse path and marks a child-slice helper `const`. Normal initial
  parse benchmarks do not exercise the old-tree included-range diff path, so
  this does not explain the normal-parse per-case outliers.
- The stack diff changes `stack_head_array_pair_mut` from two raw element
  pointers to one mutable slice split with `split_at_mut`. This is the only
  plausible hot-path codegen change in the batch, but the largest initial
  outlier (`performanceCore.ts` at 25.87%) did not reproduce on the rerun, and
  the rerun's per-case regressions were different and much smaller.
- The tree, node, tree-cursor, and changed-range commits are internal
  reference/slice boundary cleanups or visibility/clippy cleanups in a private
  module. They do not change FFI layout, exported symbols, parser tables, or
  generated language behavior.
- Because aggregate Rust throughput remains faster than C overall in both runs
  and the per-case slowdown list changed substantially between runs, this
  checkpoint is treated as benchmark noise rather than a confirmed source-level
  parser regression. The absolute Rust overall throughput moved by less than 1%
  versus the prior checkpoint.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint, and the current pushed HEAD `12ef4dcd` also passed a final full
  `cargo test --all` after the clippy-fix commit landed.

## 2026-06-25 raw-pointer and clippy cleanup checkpoint

- Head: `e698a87c` (`Remove raw array back helper`)
- Base checkpoint: `1767d637` (`Record tier three cleanup perf checkpoint`)
- Change batch: 10 small internal cleanup commits:
  `Rename tree cursor step variants` through
  `Remove raw array back helper`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

Initial run:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 27263.8 | 25783.1 | +5.74% |
| TypeScript error parses | 32 | 1804.4 | 1706.5 | +5.74% |
| JavaScript normal parses | 2 | 18278.7 | 16642.4 | +9.83% |
| JavaScript error parses | 37 | 2156.1 | 2062.0 | +4.56% |
| Overall parser throughput | 82 | 2467.9 | 2342.3 | +5.36% |

Initial per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `javascript error compound-statement-without-trailing-newline.py` | 3067.7 | 3287.8 | 6.69% |

Rerun:

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 27206.2 | 22167.5 | +22.73% |
| TypeScript error parses | 32 | 1666.9 | 1598.0 | +4.31% |
| JavaScript normal parses | 2 | 18402.2 | 16959.7 | +8.51% |
| JavaScript error parses | 37 | 2155.0 | 2020.4 | +6.66% |
| Overall parser throughput | 82 | 2351.5 | 2231.0 | +5.40% |

Rerun per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error multiple-newlines.py` | 333.7 | 375.5 | 11.15% |
| `typescript error compound-statement-without-trailing-newline.py` | 884.9 | 983.2 | 10.00% |
| `typescript error python2-grammar.py` | 1032.2 | 1138.4 | 9.33% |
| `typescript error python2-grammar-crlf.py` | 1067.4 | 1168.8 | 8.67% |
| `typescript error python3-grammar-crlf.py` | 2301.1 | 2514.4 | 8.48% |
| `typescript error python3-grammar.py` | 2246.2 | 2451.4 | 8.37% |
| `typescript error ast.rs` | 1534.3 | 1673.9 | 8.34% |
| `typescript error weird-exprs.rs` | 1117.9 | 1204.7 | 7.21% |
| `typescript error mixed-spaces-tabs.py` | 301.0 | 323.1 | 6.84% |
| `typescript error parser.ts` | 24619.3 | 26303.3 | 6.40% |

Prior checkpoint rerun measured Rust overall throughput of 2365.5 bytes/ms,
C throughput of 2303.9 bytes/ms, and a Rust-vs-C delta of +2.67%. This
checkpoint's rerun measured 2351.5 bytes/ms for Rust, so absolute Rust
throughput moved by about -0.59% versus the prior checkpoint rerun. C
throughput moved by about -3.16%, and the Rust-vs-C delta moved to +5.40%.

Source-code analysis:

- The batch contains no struct layout changes, no header changes, no parser
  table/action changes, and no exported FFI signature changes. Several
  exported implementation functions had Rust `const` removed to preserve the
  crate's `rust-version = 1.77` compatibility; their symbol names, calling
  convention, argument lists, and return types are unchanged.
- The tree cursor and changed-range enum commits are internal renames only.
  They preserve discriminants or replace forwarding wrappers with direct calls
  to the existing reference-based helpers.
- The subtree-array copy change replaces a by-value array-header parameter
  with a borrowed source header while keeping the same explicit source
  temporaries at call sites that overwrite the destination header.
- The changed-range wrapper removals delete uncalled or forwarding
  transitional APIs and route the parser to `ts_range_array_intersects_ref`
  directly. They do not change included-range diff logic.
- The only broadly hot helper touched in the batch is `array_back_ref` /
  `array_back_mut`, where the unused raw-pointer-returning `array_back` helper
  was inlined into the reference helpers with the same pointer arithmetic and
  debug assertion.
- The initial and rerun per-case regression lists do not overlap. The rerun's
  TypeScript normal C throughput also moved sharply relative to the initial
  run, while Rust remained faster overall in both runs. This pattern is
  consistent with benchmark noise rather than a source-level parser
  regression.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.

## 2026-06-25 helper cleanup checkpoint

- Head: `73aeef0c` (`Inline simple tree helpers`)
- Base checkpoint: `02205948` (`Record raw pointer cleanup perf checkpoint`)
- Change batch: 10 internal helper cleanup commits:
  `Remove raw array get helper` through `Inline simple tree helpers`.
- Command:

```sh
cargo xtask perf-gate --language typescript --language javascript --repetitions 10 --error-limit 8 --report-only --offline
```

| Workload | Cases | Rust bytes/ms | C bytes/ms | Rust delta vs C |
| --- | ---: | ---: | ---: | ---: |
| TypeScript normal parses | 11 | 25473.8 | 25727.0 | -0.98% |
| TypeScript error parses | 32 | 1789.4 | 1681.7 | +6.40% |
| JavaScript normal parses | 2 | 18188.6 | 17046.7 | +6.70% |
| JavaScript error parses | 37 | 2147.8 | 2050.1 | +4.77% |
| Overall parser throughput | 82 | 2450.0 | 2317.6 | +5.71% |

Per-case regressions over 5%:

| Case | Rust bytes/ms | C bytes/ms | Slowdown |
| --- | ---: | ---: | ---: |
| `typescript error update-authors.sh` | 510.6 | 546.2 | 6.51% |

Prior checkpoint rerun measured Rust overall throughput of 2351.5 bytes/ms,
C throughput of 2231.0 bytes/ms, and a Rust-vs-C delta of +5.40%. This
checkpoint measured 2450.0 bytes/ms for Rust, so absolute Rust throughput
moved by about +4.19% versus the prior checkpoint rerun. C throughput moved by
about +3.88%, and the Rust-vs-C delta moved to +5.71%.

Source-code analysis:

- The batch contains no exported `#[no_mangle] extern "C"` signature changes,
  no struct layout changes, no C header changes, and no parser table/action
  changes.
- The array and subtree commits remove redundant raw pointer helper wrappers or
  avoid temporary slice construction in assignment/copy paths. Destination
  writes into reserved array storage remain explicit raw pointer writes where
  the array size has not yet been advanced.
- The parser, stack, and tree-cursor commits inline transitional typed wrappers
  that only forwarded to raw array views or direct fields. The underlying
  `Array<T>`, stack, tree cursor, and subtree array layouts are unchanged.
- The language, node, and tree commits remove single-purpose accessor helpers
  or move raw pointer reference formation to API boundaries. Exported symbols
  keep the same signatures, and internal call sites preserve their previous
  null/zero-count handling.
- The only >5% per-case slowdown is one TypeScript error fixture. The same
  `update-authors.sh` fixture has appeared as a small outlier in earlier
  checkpoints, while this checkpoint improves both Rust and C aggregate
  throughput and keeps Rust faster overall. No source-level parser regression
  was identified, so no rollback was performed.
- Full `cargo test --all` passed before every committed code change in this
  checkpoint.
