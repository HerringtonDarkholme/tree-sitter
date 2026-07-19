# Rust Core Performance

This is a living summary, not a chronological experiment log. It records:

1. the current Rust-versus-C result;
2. techniques that produced useful results;
3. techniques that were rejected or reverted; and
4. the next profiling targets.

Old measurements are included only when they still inform a decision. They are
clearly marked when the measured implementation is no longer present.

## Latest Rust-versus-C checkpoint

Measured on 2026-07-16 at `325ece7b`. The Rust runtime is based on
`fe2605c1`; the comparison C core is
`c9f80282ad355a88a389d75173d918de84ef3e79`.

This table predates the retained deterministic-window and subtree-arena work
on the current branch. It is the latest complete cross-core checkpoint, not a
measurement of current HEAD. Use same-session Rust control/candidate pairs to
attribute the later representation changes; see
[CURRENT_PERFORMANCE_PROFILE.md](CURRENT_PERFORMANCE_PROFILE.md) for the exact
current-runtime profile.

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
| Conservative UTF-8 ASCII lexer advance | +2.70% current-Rust throughput across 40 fixtures; all seven languages positive; RSS neutral | Keep the guarded in-chunk/in-range fast path |
| Single-action parser dispatch | +2.78% current-Rust throughput across 40 fixtures; all seven languages positive; maximum CV 4.04%; RSS neutral | Keep the direct one-action interpreter and outline generalized action iteration |
| Sparse parser-private goto index | +2.18% confirmed current-Rust throughput; all seven languages positive, Go +7.09%; maximum CV 2.85%; at most +0.19 MiB peak RSS | Keep a sorted four-byte entry per actual small-state nonterminal transition; leave terminal/action lookup unchanged |
| Sparse parser-private terminal/action index | +1.42% confirmed current-Rust throughput with all seven languages positive; parser-cached opencode outline used about 5.0% less user CPU; application RSS +5.88 MiB | Keep a sorted four-byte entry per actual small-state terminal mapping; preserve generated tables and large-state lookup |
| Cursor-local resolved child slice | +5.43% current-Rust traversal throughput across 40 fixtures; all seven languages positive; parser-cached opencode outline used 1.12% less user CPU; paired RSS +0.62 MiB | Resolve each published parent child slice once per cursor operation instead of repeatedly resolving its arena index |
| Pressure-triggered arena child-array reuse | Reduced one-worker ast-grep TypeScript-baseline peak RSS from 492.2 MiB after retired-generation removal to 91.2 MiB; current-Rust parse throughput -1.10% | Keep the 16 MiB pressure gate and exact-size small-buffer bins as an explicit memory-for-throughput tradeoff |

The `Subtree` result is the strongest representation lesson. A readable API
should hide the compact tagged representation, not double every hot subtree
handle and child-array element.

### Worked experimentally, but not in the current runtime

These ideas produced useful measurements on the later NodeTable branch. That
branch was reverted to `fe2605c1`, so these are candidates, not current wins.

| Experiment | Result | Reuse condition |
| --- | --- | --- |
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
| Direct-final deterministic reducer | +1.09% in the short current-Rust A/B/A run, but only +0.58% in the longer confirmation; C++ -2.39%, Go -1.43%, and Rust -1.33%; RSS neutral | Reject the combined outlining/direct-builder change; the smaller frame did not produce a stable cross-language win |
| Accepted-DAG balancing worklist reuse | An unsafe form appeared +2.01%, but it could mutate descendants through shared ancestors; preserving the old skip invariant produced -0.18% overall, C++ -1.49%, and Python -1.39% | Do not cache bare candidates without also representing shared-ancestor exclusion; the safe propagation pass recreates the removed traversal |
| Parser-private arena bump cursor | +0.52% current-Rust throughput overall, but JavaScript -3.01%; CV stable and RSS neutral | Keep the single atomic allocator path; its CAS loop is too small a fraction to justify phase-specialized allocator code |
| Small parse-table group rejection | A safe terminal/nonterminal group skip was +1.17% in the short gate but only +0.56% in the 500 ms confirmation; JavaScript -2.51% and TypeScript -1.22%. A nonterminal-only retry was +0.52% overall and Rust -2.33% | Do not add kind branches to the generated-table scan; rendered symbol IDs are not ordered within groups |
| Post-finalization column shrinking | Increased peak RSS by 346% because old and new allocations coexisted | Do not shrink by reallocating after construction |
| Exact subtree counts plus node-record free lists | Pathological RSS remained roughly 494 MiB; instrumentation attributed 468.2 MiB of 470.1 MiB bump progress to temporary child-array capacities | Do not add ownership/release work to reclaim the wrong allocation class; reuse child-array blocks under pressure instead |
| Cached ASCII chunk/range boundary | -0.89% current-Rust throughput in a complete 40-fixture A/B/A screen; Go -2.08%, Java -1.54%, Python -3.55% | Keep the existing two comparisons; refreshing a derived boundary on chunk/range transitions did not pay for itself |
| Batched cursor child metadata | -1.71% traversal throughput after the parent-slice cache; JavaScript -11.21%, Python -10.81%, Rust -4.59% | Keep child accessors narrow and inlined; do not return a 36-byte metadata aggregate through each iterator step |

The direct-final reducer was independently reimplemented after the retained
ASCII lexer change. It again passed a short gate (+1.42%), but the decisive
per-language 500 ms A/B/A retry was only +1.01% overall, regressed Java by
1.64%, and retained Python CV outliers above 5%. This confirms the rejection;
short-run positives are not sufficient evidence to reopen this family.

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

## Current profiling result and next targets

The exact Rust runtime at `3155a36006b9` was profiled on 2026-07-18 with
seven-language CPU sampling, Instruments Time Profiler and CPU Counters,
Samply, malloc stack logging, `heap`, `vmmap`, `leaks`, `cargo-bloat`,
`cargo-llvm-lines`, and source-correlated assembly. The complete evidence and
ranked designs are in [CURRENT_PERFORMANCE_PROFILE.md](CURRENT_PERFORMANCE_PROFILE.md).

The current phase split is workload-dependent:

- reduction and stack work consume 29-46% of exclusive samples;
- lexer runtime consumes 17-33%;
- generated lexer code consumes 8-35%; and
- balancing consumes 3-7%.

Hardware counters add an important distinction. C++ and TypeScript lose 27.7%
and 23.9% of sustainable instruction bandwidth to delivery, while Go has the
largest reduction-oriented processing/dependency loss. C++'s generated
`ts_lex` is 92 KiB and has hot addresses spread across most of that function.
Generated-lexer layout remains a valid diagnosis but is deferred: the runtime
cannot assume that the repository corpus represents the state graphs, token
distributions, compiler choices, or regeneration practices of user grammars.

The current experiment order is:

1. retain the conservative UTF-8 ASCII advance fast path;
2. keep the direct-final reducer and accepted-DAG balancing worklist rejected;
3. retain the single-action dispatch fast path;
4. keep parser-private arena bumping rejected; and
5. keep small parse-table group rejection, parser-private goto caching, and
   simple `stack_push` hot/cold splitting rejected;
6. keep deferred subtree-summary commits rejected: broad aggregation increases
   register pressure, while a counter-only retry was throughput-neutral and
   regressed Python and TypeScript; and
7. retain the sparse parser-private nonterminal goto index; and
8. use the refreshed accepted-head profile to select another runtime-owned
   candidate.

Allocator/GC tuning is not the next throughput target. A Python snapshot had
about 7.2 MiB physical footprint and 2.1 MiB resident/dirty arena pages despite
an 8 GiB virtual reservation, with no leaks. Profiler-run throughput remains
context only; use same-session current-Rust control/candidate
`cargo xtask perf-gate` runs for performance decisions.

## Historical pre-arena allocation and data-layout audit (2026-07-16)

This audit describes the pointer-backed runtime at its recorded checkpoint. Its
event shapes still explain the motivation for the deterministic window and
arena work, but the layout table below is not the current Candidate D layout.
The current parser-facing `Subtree` is a four-byte tagged arena index; current
storage is documented in `SUBTREE_ARENA_PLAN.md`, `SUBTREE_INDEX_RESULTS.md`,
and `docs/src/5-runtime-memory.md`.

Temporary atomic counters instrumented `stack.rs`, `stack/stack_node.rs`,
`stack/pop.rs`, and subtree construction/storage. The counters were removed
after measurement. They deliberately distort timing, so this section reports
shapes, counts, and allocation behavior—not throughput.

The optimized Rust core parsed all 40 normal fixtures with one validation parse
and one measured parse per fixture. Absolute event counts therefore represent
two passes; percentages and maxima are the useful results. A focused Ruby
corpus run covered scanner states and ambiguous/recovery behavior absent from
the normal performance corpus.

64-bit layouts at that checkpoint:

| Type | Bytes | Important contents |
| --- | ---: | --- |
| `Subtree` | 8 | Inline leaf bits or heap pointer |
| `SubtreeHeapData` | 88 | 48-byte common prefix plus 40-byte content enum |
| `SubtreeChildrenData` | 20 | Internal-node summary fields |
| `ExternalScannerState` | 40 | 24 inline bytes or heap pointer, plus length/tag |
| `StackLink` | 16 | Predecessor pointer and `Subtree` |
| `StackNode` | 160 | Eight links (128 bytes) plus configuration fields |
| `StackHead` | 40 | Version head and recovery/scanner state |
| `StackIterator` | 32 | Graph-pop cursor and collected-child array |

### Stack findings

- 1,139,623 logical stack nodes were created. The 50-node pool served 99.855%
  of creations; only 1,650 reached `malloc`. Increasing the pool is not a
  meaningful target.
- 98.898% of released nodes had one predecessor. Average link count was 1.011,
  so only 12.64% of the eight fixed link slots carried data.
- Across the two-pass corpus, initialization wrote about 127.4 MB of link slots
  that were never used. This is cumulative write/cache traffic, not peak RSS.
- Go was the important exception: 4.10% of its nodes had multiple links and it
  caused 10,132 alternate-path subtree-array copies. Other normal languages
  ranged from 0% to 1.07% multi-link nodes.
- The Ruby corpus reached all eight link slots and recorded attempts to add a
  ninth. Therefore eight is a real ambiguity/recovery bound; simply reducing
  `MAX_LINK_COUNT` is incorrect.

The fixed link array is the largest obvious layout inefficiency, but removing
it is not a local cleanup. A primary inline link plus rare alternate storage
would halve the common node size, while adding allocation/indirection exactly
where Go and high-ambiguity recovery are sensitive. Treat that as a separately
profiled representation experiment, not a field shuffle.

### Subtree findings

- Of 449,050 leaves, 92.14% fit in the 8-byte inline representation. Inline
  leaves are working well and should remain pointer-sized.
- Internal nodes were 59.09% of all 1,097,636 created subtrees. They owned
  1,117,772 child slots and averaged 1.72 children.
- Unary nodes were 57.08% of internals; 94.84% had at most three children.
- Combined child/header allocations totaled 66.0 MB across two passes. The
  88-byte header accounted for 86.45% of those bytes; child handles accounted
  for only 13.55%.
- Only 0.90% of internal-node constructions needed the final header `realloc`.
  Reserving pop storage for children plus the header is effective; splitting
  the allocation would likely be worse.
- Heap leaves were only 3.21% of all subtrees. Their pool reused little, but
  their allocation volume was small compared with internal headers. Enlarging
  the leaf pool is not worthwhile.
- Copy-on-write clones and heap scanner-byte allocations were negligible in
  normal parsing. Wide symbols forced only two heap leaves.

The heap header is the most plausible subtree target. Its 40-byte content enum
is sized by the 24-byte inline scanner buffer, even for every internal node.
Normal JavaScript and TypeScript states were empty; Rust used one byte; Python
used at most eight. However, the Ruby corpus reached 80 bytes, proving that an
8-byte buffer is not generally safe for allocation behavior.

A layout-only trial found:

| Scanner inline capacity | Scanner state | Heap header | Projected internal allocation change |
| ---: | ---: | ---: | ---: |
| 24 bytes (current) | 40 bytes | 88 bytes | baseline |
| 16 bytes | 32 bytes | 80 bytes | -7.86% (-5.19 MB in the two-pass corpus) |
| 8 bytes | 24 bytes | 72 bytes | -15.72%, but rejected below |

In the Ruby corpus, 47,416 serialized states were observed: 47,274 were at
most 16 bytes, 67 were 17–24 bytes, and 75 exceeded 24 bytes. An 8-byte buffer
would have caused 2,959 scanner allocations instead of 75. Do not adopt it.
A 16-byte buffer would add only the 67 allocations in the 17–24-byte band and
is a bounded candidate for a clean, paired performance and full-corpus trial.

### Audit priorities

1. Trial a 16-byte scanner inline capacity with no instrumentation; require
   stable Python/TypeScript results and full corpus/parity coverage.
2. Keep the compact 8-byte `Subtree` handle and combined child/header
   allocation.
3. Do not tune stack or leaf pool sizes; allocation calls are not the dominant
   cost.
4. Revisit stack link storage only as a full representation experiment with Go
   and Ruby ambiguity/recovery as mandatory gates.
5. Prefer shrinking metadata paid by every internal node over optimizing rare
   heap leaves or copy-on-write paths.

## Updating this document

- Replace the current-status table after a meaningful accepted change.
- Add one short row to “What worked” or “What did not work.”
- Record exact numbers only when the benchmark is below the 5% CV limit.
- Do not append raw command transcripts or a diary of every attempted patch.
- Do not compare results produced by different benchmark protocols as if they
  were one time series.
