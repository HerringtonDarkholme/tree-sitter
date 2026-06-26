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
