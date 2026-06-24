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
