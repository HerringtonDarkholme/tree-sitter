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
