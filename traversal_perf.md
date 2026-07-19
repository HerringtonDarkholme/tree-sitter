# Tree traversal and node-read performance streak

## Streak 4: batched child metadata is rejected

After retaining the parent child-slice cache, a separate candidate resolved
each compact or heap child once into a 36-byte cursor summary containing
padding, size, visibility counts, and flags. The iterator returned size and
visible-child count to its callers so they would not resolve the child again.
Focused forward, reverse, alias, and point-navigation tests passed.

The complete three-sample, 200 ms A/B/A traversal screen against immediate
Rust parent `342cb0b6` was a broad regression:

| Language | Traversal throughput change |
| --- | ---: |
| C++ | +2.07% |
| Go | +1.38% |
| Java | +6.43% |
| JavaScript | -11.21% |
| Python | -10.81% |
| Rust | -4.59% |
| TypeScript | +6.53% |
| **Equal-language geometric mean** | **-1.71%** |

All 40 fixture lengths, hashes, and node counts matched. The implementation
was removed. Unlike the retained four-byte parent handle replacement, this
form widens the per-child value flow and forces a large aggregate through the
iterator/caller boundary. Existing inlined accessors let LLVM keep only the
fields required by each control path. Future cursor work should cache narrow
addresses or scalars, not materialize a broad metadata snapshot.

## Streak 3: resolve each parent child slice once

The current cursor now resolves an internal parent's arena-backed child slice
once in `tree_cursor_iterate_children` and keeps that pointer only in the
operation-local `CursorChildIterator`. Published tree arenas are immutable and
cannot move, so the borrowed pointer remains valid for that iterator. The old
loop re-resolved the same parent handle for its end check, current-child lookup,
and next-child padding lookup.

The decisive current-Rust A/B/A comparison used five 500 ms CPU-time samples
per fixture and immediate parent `1dca0be6` as the control. All 40 fixture
lengths, hashes, and node counts matched:

| Language | Fixtures | Traversal throughput change |
| --- | ---: | ---: |
| C++ | 4 | +5.19% |
| Go | 5 | +5.14% |
| Java | 4 | +5.11% |
| JavaScript | 2 | +6.13% |
| Python | 12 | +5.34% |
| Rust | 2 | +6.30% |
| TypeScript | 11 | +4.80% |
| **Equal-language geometric mean** | **40** | **+5.43%** |

Maximum control/candidate/control CV was 3.37%, 5.58%, and 3.62%. The broad,
uniform result also reproduced in the shorter screen (+5.22%). A separate long
normal-parse guard found no regression: C++ was -0.18%, TypeScript -0.47%, and
the other five languages were positive. Because cursor iteration is not on the
fresh parse path, those parse movements are treated as link-layout/run-order
sensitivity rather than attributed parser gains.

The parser-cached application gate linked both endpoints into the same local
ast-grep source and ran `ast-grep outline` over opencode with one worker. Three
interleaved `B, C, C, B` cycles averaged 1.1933 s control versus 1.1800 s
candidate user CPU, **1.12% less user CPU**. Output was byte-identical at
253,174 bytes with SHA-256
`91dd98a31a6263396ce56b658ce3c641aa6eb3b11f92942a0c6961d5206a2872`.
Paired peak RSS averaged 43.35 MiB control and 43.97 MiB candidate, a noise-sized
+0.62 MiB difference. Full 123-sample core parity, the ABI tripwire, all seven
ast-grep gate packages, and focused forward/reverse/alias cursor tests passed.

## Streak 2: current arena endpoint and accessor attribution

A fresh matched Rust-to-Rust run at `610deea2` reverses the earlier result. The
current arena endpoint is **1.71% faster** by equal-language geometric mean for
the complete traversal kernel than the pre-arena `fa33cd20` control. The
harness, corpus, and benchmark code were identical at both endpoints; three
samples of at least 150 ms were collected for each of 40 fixtures.

| Language | Pre-arena nodes/ms | Current nodes/ms | Change |
| --- | ---: | ---: | ---: |
| C++ | 20,514 | 20,885 | +1.81% |
| Go | 20,809 | 20,868 | +0.28% |
| Java | 20,289 | 20,964 | +3.33% |
| JavaScript | 18,841 | 19,542 | +3.72% |
| Python | 20,187 | 20,369 | +0.90% |
| Rust | 18,860 | 18,994 | +0.71% |
| TypeScript | 20,222 | 20,474 | +1.25% |
| Equal-language geometric mean | — | — | **+1.71%** |

The committed `traversal-attribution` mode incrementally adds one public read
set to the same preorder cursor walk. Aggregated across the 40 fixtures, the
current endpoint measured:

| Kernel | Nodes/ms | ns/node | Increment over navigation |
| --- | ---: | ---: | ---: |
| navigation only | 23,382 | 42.77 | — |
| + kind ID | 22,420 | 44.60 | 1.83 ns |
| + byte range | 22,257 | 44.93 | 2.16 ns |
| + named/error flags | 21,832 | 45.80 | 3.03 ns |
| complete read set | 20,461 | 48.87 | 6.10 ns |

The pre-arena control measured 20,183 nodes/ms for the complete kernel, so the
current result is +1.38% when fixtures are averaged directly. Navigation,
kind, range, flags, and the complete kernel were all faster than the control.
Later arena changes—especially kind-specialized headers and deferred column
summaries—therefore recovered the indexed-handle traversal loss measured in
the first streak. A parser/published-tree representation split is not justified
by current traversal data; application profiling should now focus on the rule
engine and arena high-water behavior.

## Streak 1: initial Candidate D endpoint

At the initial Candidate D endpoint, the compact indexed subtree arena improved
parser construction throughput but did **not** improve syntax-tree consumer
throughput. Against the last Rust implementation before the arena, revision
`77ac1d5e` was slower in all seven measured languages when performing a
preorder tree walk and reading common node metadata. The equal-language
geometric-mean regression was **2.40%**.

This result is retained as historical evidence for revision `77ac1d5e`; it is
not a description of the current endpoint. Streak 2 above supersedes it for
current design decisions.

This is a separate performance axis from parsing. Candidate D remains a parser
throughput win because four-byte handles reduce parser-stack and child-array
traffic. Once a tree has been built, however, every compact leaf and heap node
must be resolved through an arena base plus a physical byte index. The previous
representation read compact leaves directly from the eight-byte handle and
used direct pointers for heap nodes.

## Compared endpoints

| Endpoint | Tree-sitter revision | Subtree representation |
| --- | --- | --- |
| Pre-arena Rust baseline | `fa33cd20` | eight-byte inline-leaf-or-pointer union |
| Current Rust arena | `77ac1d5e` | four-byte tagged physical arena index |

Both endpoints were linked into the same local ast-grep 0.44.1 source snapshot
at revision `94bc9582`. The compatibility crates exposed tree-sitter version
0.26.11 so Cargo resolved an otherwise identical ast-grep dependency graph.
The C core was not used as a performance baseline.

The measurements were collected on 2026-07-18 on macOS ARM64. Both executables
were Cargo release builds.

## Measured operations

The primary kernel matches ast-grep's preorder `TsPre` cursor walk. For every
visited node it consumes:

- kind ID;
- start byte and end byte;
- named-node status; and
- error-node status.

Parsing, source loading, tree construction, and process startup are outside the
timed region. The tree is built once and traversed repeatedly. A checksum is
passed through `black_box` so metadata reads cannot be removed by the optimizer.

A secondary Rust-only kernel performs the same preorder traversal but reads
only the kind ID. It helps distinguish the cursor/index cost from the additional
metadata accessor cost.

## Corpus and sampling

The six non-Rust inputs are the checked-in perf-gate examples. The Rust input is
the local ast-grep `crates` source tree, concatenated in stable path order.

| Language | Files | Source bytes | Nodes per traversal | Visits per sample |
| --- | ---: | ---: | ---: | ---: |
| C++ | 4 | 19,030 | 6,688 | 30,096,000 |
| Go | 5 | 208,148 | 54,971 | 38,479,700 |
| Java | 4 | 8,525 | 2,893 | 37,609,000 |
| JavaScript | 2 | 398,884 | 104,366 | 41,746,400 |
| Python | 12 | 177,377 | 54,635 | 38,244,500 |
| Rust | 152 | 1,212,636 | 383,425 | 38,342,500 |
| TypeScript | 11 | 404,705 | 65,162 | 39,097,200 |

C++, Go, Java, JavaScript, Python, and TypeScript used five measured samples.
Rust used fifteen. Rust therefore measured 575,137,500 total node visits per
endpoint; each other language measured roughly 150-209 million. Coefficients of
variation were at most 1.23%.

## Traversal plus metadata-read results

Lower nanoseconds per node is better. The change column is current arena versus
the pre-arena Rust baseline.

| Language | Pre-arena ns/node | Current arena ns/node | Change |
| --- | ---: | ---: | ---: |
| C++ | 51.729 | 52.700 | **1.88% slower** |
| Go | 52.421 | 53.671 | **2.38% slower** |
| Java | 51.643 | 52.645 | **1.94% slower** |
| JavaScript | 54.328 | 55.486 | **2.13% slower** |
| Python | 56.068 | 57.242 | **2.09% slower** |
| Rust | 54.806 | 57.117 | **4.22% slower** |
| TypeScript | 53.397 | 54.547 | **2.15% slower** |
| Equal-language geometric mean | — | — | **2.40% slower** |

## Minimal traversal result

The Rust-only preorder traversal with a kind-ID read showed the same direction:

| Endpoint | Median ns/node | CV |
| --- | ---: | ---: |
| Pre-arena Rust | 49.749 | 0.58% |
| Current Rust arena | 51.365 | 1.07% |
| Change | **3.25% slower** | — |

The extra range and flag accessors widen the current regression from 3.25% to
4.22% on the same Rust tree. Index resolution is therefore already visible in
the cursor walk, and metadata reads add further arena-dependent loads.

## Interpretation

The result explains why dense handles can help construction while hurting
consumers:

1. Pre-arena compact leaves carry their hot fields inside the handle. Candidate
   D stores compact leaves in the arena, so even a leaf read needs base-plus-index
   resolution and a dependent record load.
2. Pre-arena heap handles are direct pointers. Candidate D resolves heap records
   with `arena_base + (index & !tag)` before reading the record.
3. Cursor navigation repeatedly reads parent headers and child handles. The
   smaller child references improve density, but the measured trees do not gain
   enough cache locality to repay the repeated resolutions.
4. Accessors such as range and flags touch additional record fields, which is
   why the read-heavy kernel regresses more than the minimal kind-only kernel.

The conclusion is deliberately narrow: Candidate D is a retained parse
throughput optimization with a measurable syntax-tree read penalty. Future
representation work must report both construction and consumer traversal; a
parse-only win is not evidence of a globally faster tree representation.

## Committed benchmark

The repository benchmark now accepts `traversal` as a benchmark kind. It parses
each checked-in valid fixture once, excludes parsing from the timed region, then
runs the same preorder cursor walk and metadata-read set described above. On
Unix it uses process CPU time, matching the existing parser performance harness.

Run all checked-in languages with calibrated 500 ms samples:

```bash
cargo xtask benchmark \
  --kind traversal \
  --repetition-count 15 \
  --min-sample-time-ms 500
```

Filter to one language or fixture with the existing `--language` and
`--example-file-name` options. Each `BENCHMARK_RESULT` record includes node count,
traversals per sample, sample durations, nodes per millisecond, nanoseconds per
node, input size and hash, and peak RSS.

The committed harness is intentionally consumer-neutral: it calls the public
tree cursor and node accessors directly rather than depending on a sibling
ast-grep checkout. Its traversal order and read set match the experiment above,
while its checked-in inputs make future comparisons reproducible from this
repository alone.
