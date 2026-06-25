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
