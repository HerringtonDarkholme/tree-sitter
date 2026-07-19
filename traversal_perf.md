# Tree traversal and node-read performance streak

## Result

The compact indexed subtree arena improves parser construction throughput, but
it does **not** improve syntax-tree consumer throughput. Against the last Rust
implementation before the arena, the current implementation was slower in all
seven measured languages when performing a preorder tree walk and reading
common node metadata. The equal-language geometric-mean regression was
**2.40%**.

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
