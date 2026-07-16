# Rust Core Performance

This is a living summary, not a chronological experiment log. It records:

1. the current Rust-versus-C result;
2. techniques that produced useful results;
3. techniques that were rejected or reverted; and
4. the next profiling targets.

Old measurements are included only when they still inform a decision. They are
clearly marked when the measured implementation is no longer present.

## Current status

Measured on 2026-07-16 at `325ece7b`. The Rust runtime is based on
`fe2605c1`; the comparison C core is
`c9f80282ad355a88a389d75173d918de84ef3e79`.

```sh
cargo xtask perf-gate --offline
```

The table reports the geometric mean of the per-fixture median Rust/C
throughput ratios. Positive values favor Rust.

| Language | Fixtures | Rust vs C | Rust wins |
| --- | ---: | ---: | ---: |
| C++ | 4 | +0.36% | 3/4 |
| Go | 5 | +1.01% | 4/5 |
| Java | 4 | +1.61% | 4/4 |
| JavaScript | 2 | +0.64% | 2/2 |
| Python | 12 | **-2.87%** | 0/12 |
| Rust | 2 | +0.29% | 1/2 |
| TypeScript | 11 | **-1.35%** | 1/11 |

Two summaries are useful:

- Equal weight per language: **Rust -0.05%**. This is practical parity.
- Equal weight per fixture: **Rust -0.87%** across 40 fixtures. Python and
  TypeScript have more fixtures, so they dominate this number.

Peak RSS is also effectively tied:

| Language | Rust | C |
| --- | ---: | ---: |
| C++ | 11.00 MiB | 10.70 MiB |
| Go | 12.67 MiB | 13.17 MiB |
| Java | 8.36 MiB | 8.53 MiB |
| JavaScript | 21.53 MiB | 21.11 MiB |
| Python | 11.00 MiB | 10.73 MiB |
| Rust | 12.70 MiB | 12.59 MiB |
| TypeScript | 21.64 MiB | 21.34 MiB |

### Interpretation

- The Rust core does **not** currently have a 20% throughput advantage.
- C++, Go, Java, JavaScript, and Rust are tied or modestly favor Rust.
- Python is the clearest regression: C wins every fixture.
- TypeScript also favors C on nearly every fixture, though by a smaller amount.
- Optimize Python and TypeScript first. Improvements elsewhere are unlikely to
  move the overall result materially.

## Benchmark method

All normal-parse fixtures live in
`crates/cli/benches/examples`. The benchmark does not read
environment-specific source repositories or directories outside this
repository.

For each fixture and core:

1. warm the parser;
2. calibrate the number of parses;
3. record 10 samples of at least 500 ms each;
4. use process CPU time on Unix, avoiding scheduler-pause noise;
5. report median throughput plus sample mean, standard deviation, and
   coefficient of variation (CV); and
6. fail if CV exceeds 5%.

There are deliberately no retries, rejected samples, pooled processes,
checked-in throughput baselines, or Rust-versus-C pass/fail threshold.

The latest complete run passed the 5% stability limit. Most fixture CVs were
between 0.5% and 2%. Two consecutive Go runs had all CVs below 2.6%, and their
fixture medians differed by at most 1.5%.

Useful commands:

```sh
# Complete in-repo corpus
cargo xtask perf-gate --offline

# One or more languages
cargo xtask perf-gate --language python --language typescript --offline

# Longer samples for a noisy machine
cargo xtask perf-gate --min-sample-time-ms 1000 --offline
```

## What worked

### Current and retained

| Technique | Evidence | Decision |
| --- | --- | --- |
| Repository-owned fixtures | Every machine measures the same 40 inputs; Go, C++, and Java now cover multiple parse shapes | Keep |
| Calibrated CPU-time samples | Reduced formerly double-digit CV outliers to mostly 0.5–2% without filtering | Keep |
| Median plus standard deviation/CV | Gives a robust headline number while exposing noisy measurements | Keep |
| Compact `Subtree` handles | An explicit 16-byte Rust enum increased parse time by 19.74%; the compact handle is 8 bytes | Keep the compact private representation |
| One-pass parsing and compact stack links | Removed stale incremental/reuse machinery and shrank `StackLink` from 24 to 16 bytes and `StackNode` from 232 to 168 bytes | Keep; old speed figures were noisy, so claim simplicity/layout rather than a precise gain |
| Focused ownership and module cleanup | Behavior remained green and direct throughput changes stayed within measurement noise | Keep for readability |

The `Subtree` result is the strongest representation lesson. A readable API
should hide the compact tagged representation, not double every hot subtree
handle and child-array element.

### Worked experimentally, but not in the current runtime

These ideas produced useful measurements on the later NodeTable branch. That
branch was reverted to `fe2605c1`, so these are candidates, not current wins.

| Experiment | Result | Reuse condition |
| --- | --- | --- |
| ASCII fast path in `lexer_do_advance` | 95.85% hit rate; +1.26 Rust/C percentage points in a paired run | Reimplement only after a fresh current-core profile identifies the same hot path |
| Stack-history compaction | About 6 MiB lower peak RSS for large fixtures at roughly -0.12 throughput points | Relevant only to an append-only indexed stack |
| Bounded publish-in-place | Avoided a final compacting copy for low-waste trees | Relevant only if a flat NodeTable design returns |

## What did not work

### Directly measured rejections

| Experiment | Result | Decision |
| --- | --- | --- |
| 16-byte Rust `Subtree` enum | +19.74% parse time; more allocation and copying | Do not retry without preserving an 8-byte handle |
| Decoder-function selection hoist | +0.16% throughput / +0.31 Rust/C points | Too small; keep decoder selection local |
| Small-state symbol slice scan | -1.86% throughput / -1.01 Rust/C points | Keep the compact generated-table pointer loop |
| External-scanner state reuse by token identity | Only 8.11% overall would-be hit rate; Python 19.94% was the best | Do not add this cache |
| Incremental reduction summaries | Regressed the normalized gate by about 4.2 points; paid an 88-byte payload on discarded paths | Do not attach broad summaries to every pop path |
| Post-finalization column shrinking | Increased peak RSS by 346% because old and new allocations coexisted | Do not shrink by reallocating after construction |

### Repeatedly unproductive categories

The following families were tried in multiple forms and either regressed,
produced only noise-sized gains, or added more complexity than their measured
benefit:

- local reduction and child-summarization branches;
- deterministic-reduction runners that retained the same underlying work;
- linear-tail, segmented-stack, and compact-stack-node variants;
- allocator pools, larger arena pages, leaf arenas, and small child-block
  free lists;
- refcount ordering and retain/release micro-optimizations;
- broad `#[cold]`, inline, pointer-layout, and prefetch changes;
- balance candidate lists, progress guards, and deferred balancing;
- lexer callback, logging, and decode micro-optimizations without profile
  evidence; and
- dormant lazy-node or parse-forest foundations that did not remove a complete
  phase.

The common lesson is that making one branch cheaper rarely moves total parse
time. A future change should remove a full traversal, allocation, copy, or
materialization phase.

### Reverted NodeTable redesign

The flat NodeTable/indexed-stack branch reached strong experimental throughput
and enabled useful lifetime measurements, but it was reverted because the
implementation and its memory policies became substantially more complex than
the current pointer-sized subtree model. Its old +8–9% results used an earlier,
noisier harness and must not be compared with today's near-parity result.

Keep only its general lessons:

- measure aliasing before deleting ownership checks;
- preserve public node identity across edits;
- compact append-only history only at safe graph boundaries;
- use swap-then-free rather than in-place shrink when peak RSS matters; and
- do not introduce a second node representation merely to satisfy an arbitrary
  RSS target.

## Profiling status and next targets

There is no post-reset flamegraph of the exact current runtime. The detailed
NodeTable/S8 flamegraphs from the reverted branch are historical and should not
drive a new patch directly.

The last profile on an ancestor of the current runtime showed:

- generated lexer work as the largest C++ and Java leaf cost;
- approximately one third of time in the complete reduction/stack lifecycle;
  and
- child collection, parent construction, metadata summarization, and stack
  push/release as a connected pipeline rather than isolated bottlenecks.

The next optimization cycle should:

1. profile current Python and TypeScript fixtures;
2. separate generated lexer, external scanner, reusable lexer plumbing,
   reduction, stack traversal, subtree construction, and finalization;
3. choose a target only if it is large in both a flamegraph and the stable
   throughput corpus; and
4. use a same-session control/candidate comparison.

Example profile:

```sh
CARGO_PROFILE_BENCH_DEBUG=true \
TREE_SITTER_CORE_IMPL=rust \
TREE_SITTER_BENCHMARK_LANGUAGE_FILTER=python \
TREE_SITTER_BENCHMARK_EXAMPLE_FILTER=python3-grammar.py \
TREE_SITTER_BENCHMARK_KIND_FILTER=normal \
TREE_SITTER_BENCHMARK_REPETITION_COUNT=20 \
TREE_SITTER_BENCHMARK_MIN_SAMPLE_TIME_MS=500 \
cargo flamegraph -p tree-sitter-cli --bench benchmark \
  --output /tmp/tree-sitter-python.svg --deterministic
```

Profiler-run throughput is context only; use `cargo xtask perf-gate` for
performance decisions.

## Updating this document

- Replace the current-status table after a meaningful accepted change.
- Add one short row to “What worked” or “What did not work.”
- Record exact numbers only when the benchmark is below the 5% CV limit.
- Do not append raw command transcripts or a diary of every attempted patch.
- Do not compare results produced by different benchmark protocols as if they
  were one time series.
