# Current Rust Parser Performance Profile

This document records a multi-tool profile of the current Rust parser core at
`3155a36006b9` on 2026-07-18. It is a current-code profile and design triage,
not a Rust-versus-C scorecard. Any experiment proposed here must be compared
with this Rust implementation or its immediate parent in the same session;
the C runtime is too different to attribute a local change.

The profile answers four questions:

1. Which phases consume CPU across the complete seven-language performance
   corpus?
2. Is the processor limited by instruction delivery, data dependencies,
   speculation, allocation, or memory residency?
3. Which source and generated functions create those limits?
4. Which designs remove a complete hot operation rather than merely making a
   branch cheaper?

## Scope and method

The profiled executable was an optimized benchmark build with Rust debug
information:

```sh
TREE_SITTER_CORE_IMPL=rust \
CARGO_PROFILE_BENCH_DEBUG=true \
CARGO_TARGET_DIR=/private/tmp/tree-sitter-current-profile \
cargo bench -p tree-sitter-cli --bench benchmark --no-run
```

Normal-parse fixtures came from `crates/cli/benches/examples`. Each focused
profile repeatedly parsed one repository-owned fixture for at least 500 ms per
benchmark sample. Throughput printed while a profiler was attached is context
only; candidate decisions still use paired `cargo xtask perf-gate` runs.

The host was a 14-core Apple M3 Max MacBook Pro with 36 GiB RAM, running
macOS 26.5.1 (25F80). The compiler was Rust 1.97.1 with LLVM 22.1.6.
The following independent tools were used:

| Tool | Evidence collected |
| --- | --- |
| macOS `sample` | Seven-language statistical call trees with optimized Rust symbols |
| Instruments Time Profiler through `xctrace` | Independent Python call-tree confirmation |
| Instruments CPU Counters | Pipeline bottleneck fractions for representative C++, Go, Python, and TypeScript parses |
| Samply 0.13.1 | Independent 1 kHz Go call tree with a presymbolicated sidecar |
| malloc stack logging and `malloc_history` | Allocation call sites and historical allocation volume |
| `heap`, `vmmap`, and `leaks` | Live malloc blocks, virtual/resident mappings, and leak state |
| `cargo-bloat` 0.12.1 and `nm` | Linked Rust function sizes and generated grammar function sizes |
| `cargo-llvm-lines` 0.4.46 | LLVM IR expansion and generic-function duplication |
| `cargo-asm` and `llvm-objdump` | Source-correlated hot-path code generation and stack-frame shape |

The focused runs used the following benchmark controls, changing the language
and fixture filters for each recording:

```sh
BENCHMARK_BINARY=/private/tmp/tree-sitter-current-profile/release/deps/benchmark-0e624cd7c68860fb
LANGUAGE_NAME=go
FIXTURE_NAME=letter_test.go
export TREE_SITTER_BENCHMARK_LANGUAGE_FILTER="$LANGUAGE_NAME"
export TREE_SITTER_BENCHMARK_EXAMPLE_FILTER="$FIXTURE_NAME"
export TREE_SITTER_BENCHMARK_KIND_FILTER=normal
export TREE_SITTER_BENCHMARK_REPETITION_COUNT=20
export TREE_SITTER_BENCHMARK_MIN_SAMPLE_TIME_MS=500
```

`sample` attached at 1 ms intervals for its default ten-second window. Samply
recorded Go at 1 kHz with `--unstable-presymbolicate`. CPU Counters used the
Instruments `CPU Counters` template and the same launch environment; the table
below includes only precise 1 ms process rows at timestamps of at least two
seconds. A reproducible launch template is:

```sh
xcrun xctrace record \
  --template 'CPU Counters' \
  --time-limit 12s \
  --env TREE_SITTER_BENCHMARK_LANGUAGE_FILTER="$LANGUAGE_NAME" \
  --env TREE_SITTER_BENCHMARK_EXAMPLE_FILTER="$FIXTURE_NAME" \
  --env TREE_SITTER_BENCHMARK_KIND_FILTER=normal \
  --env TREE_SITTER_BENCHMARK_REPETITION_COUNT=20 \
  --env TREE_SITTER_BENCHMARK_MIN_SAMPLE_TIME_MS=500 \
  --launch -- "$BENCHMARK_BINARY"

samply record --rate 1000 --save-only --unstable-presymbolicate \
  --output /tmp/tree-sitter-go-samply.json.gz \
  "$BENCHMARK_BINARY"
```

The statistical evidence volume was:

| Recording | Accepted samples or rows |
| --- | ---: |
| C++ / Go / Java `sample` | 8,365 / 8,403 / 8,353 |
| JavaScript / Python `sample` | 8,354 / 12,356 |
| Rust / TypeScript `sample` | 8,351 / 8,402 |
| C++ / Go CPU Counters | 8,667 / 10,941 precise rows |
| Python / TypeScript CPU Counters | 7,281 / 8,811 precise rows |
| Go Samply | 5,675 samples |

Raw profiler throughput is intentionally not reported as a benchmark result.
Sampling tools perturb execution, and each focused trace represents one
fixture. The seven-language phase agreement selects design axes; paired
same-session perf-gate runs remain the acceptance authority.

The `sample` phase table below groups **exclusive leaf samples** by symbol
family. It is a phase decomposition, not an inclusive call-tree sum. Small
rounding error is expected.

## Cross-language CPU decomposition

| Language | Generated lexer | External scanner | Lexer runtime | Reduction and stack | Balancing | Action dispatch | Allocation | Other |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| C++ | **34.5%** | 0.0% | 16.7% | 30.5% | 2.8% | 11.2% | 1.6% | 2.7% |
| Go | 12.0% | 0.0% | 21.1% | **46.3%** | 4.1% | 11.1% | 2.1% | 3.4% |
| Java | 19.3% | 0.0% | 21.1% | **38.5%** | 3.4% | 12.3% | 2.0% | 3.5% |
| JavaScript | 12.7% | 2.8% | 25.9% | 30.4% | **6.9%** | 16.2% | 2.1% | 3.0% |
| Python | 7.8% | **5.7%** | 26.4% | 35.1% | 5.3% | 12.7% | **4.8%** | 2.1% |
| Rust | 14.7% | 1.2% | 23.4% | **40.1%** | 5.2% | 10.4% | 2.1% | 2.9% |
| TypeScript | 13.1% | 2.4% | **32.6%** | 29.0% | 4.8% | 14.1% | 1.7% | 2.3% |

This separates three materially different workloads:

- C++ is generated-lexer and instruction-footprint bound.
- Go and Java are reduction/stack bound.
- Python and TypeScript spend less time in generated `ts_lex`, but more in the
  reusable lexer/parser boundary. Python additionally pays external-scanner
  allocation and deserialization work.

There is no single allocator or subtree-representation change that directly
targets all three.

## Hardware-counter result

Apple's CPU Bottlenecks mode partitions sustainable instruction bandwidth into
four categories:

- **useful**: bandwidth that retired useful work;
- **processing**: the instruction window stopped accepting work, commonly due
  to data dependencies, memory latency, or insufficient instruction-level
  parallelism;
- **delivery**: fetch/decode could not supply the sustainable rate, commonly
  due to instruction-cache footprint, code layout, or front-end bandwidth;
- **discarded**: work was lost to branch misprediction or pipeline restart.

The table aggregates precise 1 ms samples after the first two seconds of each
recording, excluding loader and grammar-build startup.

| Fixture | Useful | Processing | Delivery | Discarded |
| --- | ---: | ---: | ---: | ---: |
| C++ `rule.cc` | 48.33% | 11.86% | **27.72%** | 12.09% |
| Go `letter_test.go` | 52.23% | **17.57%** | **18.58%** | 11.61% |
| Python `python3-grammar.py` | 50.78% | **17.15%** | 17.87% | **14.20%** |
| TypeScript `parser.ts` | 53.89% | 9.64% | **23.91%** | 12.56% |

Consequences:

1. C++ and TypeScript have a real instruction-delivery problem. A layout or
   generated-code-size experiment is justified; a data-layout-only experiment
   does not address their largest hardware loss.
2. Go has both front-end and dependent-data losses. Its reduction pipeline is
   a chain of arena-index resolution, child metadata loads, parent writes, and
   stack updates, matching the counter result.
3. Python has the highest discarded fraction. Its external-scanner and parser
   control flow are better targets than a global memory-capacity change.
4. Broad `#[cold]` annotations remain unjustified. Only measured rare paths
   should be outlined, and linked code size plus paired throughput must prove
   the effect.

## Independent Go call tree

Samply collected 5,675 presymbolicated steady-state samples. Its result agrees
with `sample`, but exposes several subphases more clearly:

| Function or path | Exclusive | Inclusive |
| --- | ---: | ---: |
| Complete `parser_reduce` call path | 3.33% | **38.50%** |
| `parser_finish_reduction` | **9.22%** | 13.43% |
| `subtree_summarize_children` | **7.93%** | 7.93% |
| materialized `stack_iter<pop_count>` | **6.82%** | 7.63% |
| `stack_node_new` | 4.09% | 4.11% |
| `stack_push` | 3.84% | 7.14% |
| `parser_balance_subtree` | 3.75% | 4.76% |
| `stack_pop_count_from_window` | 2.20% | 2.73% |
| `subtree_arena_allocate` | 1.46% | 1.46% |

The deterministic window is a large retained win, but it has not made the
remaining reducer cheap. Eligible reductions still:

1. scan the window backward;
2. reserve and fill a temporary `SubtreeArray`;
3. remove trailing extras in another pass;
4. initialize an internal header;
5. resolve and summarize every child in another pass; and
6. finish the goto and push through shared reducer code.

Reductions that cannot remain inside the window still reach the 3.3 KiB
materialized `stack_iter<pop_count>` implementation and 160-byte stack nodes.
The profile does not by itself identify whether ambiguity, a straddling pop,
or another materialization trigger dominates those calls. That distinction
must be measured before changing the generalized iterator.

## Code-generation and instruction-footprint findings

### Generated lexers

The grammar functions were measured directly from the linked benchmark
dynamic libraries. Sizes are machine-code bytes to the next text symbol.

| Language | `ts_lex` | `ts_lex_keywords` | generated `ts_lex` cases |
| --- | ---: | ---: | ---: |
| C++ | **92,000 B** | **32,960 B** | **659** |
| Go | 8,768 B | 2,860 B | 165 |
| Java | 10,704 B | 6,568 B | 194 |
| JavaScript | 18,412 B | 4,436 B | 279 |
| Python | 8,412 B | 3,584 B | 169 |
| Rust | 13,312 B | 5,232 B | 196 |
| TypeScript | 15,020 B | 7,720 B | 219 |

C++'s generated C `ts_lex` body is 168,220 source bytes and 6,007 lines before
compilation. Address-level samples place hot PCs near the beginning, around
49 KiB, and around 87.5 KiB of the 92 KiB machine function. The front end is
therefore executing hot regions spread across most of the function, not merely
carrying unreachable cold bytes in the binary.

This proves that generated lexer layout contributes materially to C++ and
TypeScript performance. It does **not** make generator changes an active
candidate for the current runtime program. Tree-sitter does not control the
size, state topology, corpus, compiler, or regeneration cadence of user
grammars, so a layout that wins on the seven repository fixtures cannot be
assumed to win across the language ecosystem. The result is retained as a
diagnosis and deferred below.

### Rust runtime

`cargo-bloat` on the linked CLI and direct symbol deltas on the benchmark agree
on the important sizes:

| Function family | Linked text size |
| --- | ---: |
| `parser_lex_lookahead` | about 4.3 KiB |
| four `stack_iter` monomorphizations | **about 12.4 KiB total** |
| `parser_advance` | about 2.3 KiB |
| `parser_balance_subtree` | about 1.9 KiB |
| `subtree_summarize_children` | about 1.4 KiB |
| `parser_reduce` | about 1.4 KiB |
| `stack_node_new` | 652 B |
| `lexer_do_advance` | 608 B |

`cargo-llvm-lines` shows four separate `stack_iter` instances containing 615,
616, 618, and 618 LLVM IR lines. The duplication is real, but only the
`pop_count` instance is hot in the normal Go profile. Deduplicating all four
through an indirect callback could shrink the binary while slowing the hot
instance; code size alone is not an acceptance argument.

The source-correlated assembly reveals a more actionable reducer issue:
In the exact profiled benchmark binary, `parser_reduce` reserves a **304-byte
stack frame** and saves ten general-purpose registers at entry. The
deterministic-window eligibility check happens only after that prologue. The
GLR path's local state therefore increases stack traffic and register pressure
on every deterministic reduction even though the function returns early from
the window path. A separate non-LTO library assembly view produced an even
larger frame, so the exact acceptance witness must always come from the linked
benchmark binary.

This supports separating deterministic and generalized reduction into
different compiled functions. It is more precise than applying `#[cold]` to a
large existing function: the desired result is a small dispatcher that
tail-calls a small deterministic body or an out-of-line GLR body, verified in
assembly.

## Allocation, virtual memory, and RSS

The Python allocation snapshot was taken during repeated parsing with malloc
stack logging enabled:

| Observation | Result |
| --- | ---: |
| Process physical footprint | about 7.2 MiB |
| Live malloc nodes | 1,180 blocks / about 215 KiB |
| Live runtime-recorded malloc nodes | 61 blocks / about 12 KiB |
| Live arena VM reservation | about 8 GiB virtual |
| Resident/dirty arena VM pages | about 2.1 MiB |
| Leaks | 0 |

The virtual reservation is not RSS. Candidate D reserves address space in
4 GiB units but commits 64 KiB regions on demand. The syntax tree's ordinary
records consequently do not appear as one malloc per node, and the 8 GiB
mapping observed at the snapshot consumed only about 2.1 MiB of resident/dirty
pages.

Malloc history still showed stack-node allocations and Python external-scanner
allocation. The normal stack-node pool absorbs most stack nodes; historical
entries included 39 reduce-path stack allocations (7,488 B) and four
advance-path allocations (768 B) in the inspected snapshot. Python's external
scanner calls malloc/free while deserializing its state, which explains why
Python's allocation sample fraction is higher than the other languages.

The conclusions are narrow but strong:

- GC, arena paging, and malloc replacement are not the next fresh-parse
  throughput target.
- The large virtual mapping is a capacity/address-space question, not an RSS
  regression in the measured corpus.
- Removing atomic arena bumping has a profile ceiling near the observed 1.5%
  Go allocator fraction and must preserve concurrent copy/edit allocation from
  published arenas.
- External-scanner allocation is a Python-specific ABI/design question, not a
  reason to redesign all subtree storage.

## Ranked optimization designs

The ranks combine profile size, hardware evidence, implementation scope, and
the experiment ledger in `PERFORMANCE.md` and `SUBTREE_ARENA_PLAN.md`.

| Priority/status | Design | Work removed | Main evidence | Main risk |
| --- | --- | --- | --- | --- |
| Retained | UTF-8 ASCII advance fast path | Decoder, range-seek, and callback work for an ordinary in-chunk ASCII byte | +2.70% current-Rust throughput, all languages positive | Boundary/newline/included-range parity |
| Rejected | Dedicated direct-final deterministic reducer | Large shared frame, temporary child-array lifecycle, trailing-extra pass, and separate child-summary pass | +0.58% longer confirmation; three languages below -1% | Code placement and dependency chains offset removed work |
| 1 | Reuse accepted-DAG discovery for balancing | Second child-edge discovery traversal and its work stack | Balance is 3-7%; exact sharing already requires one accepted-DAG scan | Candidate writes may cost more than the saved traversal |
| 2 | Single-action parser interpreter fast path | Generic action loop and multi-action bookkeeping for the common one-action entry | Dispatch is 10-16%; discarded bandwidth is 10-14% | A branch-only optimization may remain below noise |
| 3 | Parser-private arena bump cursor with published atomic fallback | CAS loop on allocations made before publication | Arena allocation is 1.5% exclusive in Go | Published tree copies may allocate concurrently in the same arena |
| 4 | Versioned external-scanner snapshot ABI | Repeated deserialize and grammar-owned malloc/free | Python external scanner is 5.7%, allocation 4.8% | ABI and grammar complexity; identity cache already had low reuse |

### 1. UTF-8 ASCII advance

The prior successful implementation used a conservative fast path only when:

- input encoding was UTF-8;
- current lookahead was positive ASCII but not newline;
- `lookahead_size == 1`;
- the next byte stayed in the current input chunk and included range; and
- the next byte was also ASCII, otherwise it fell back to normal decoding.

It updated bytes, columns, token-start position, and the next lookahead directly.
The current profile independently reopens this exact design: `lexer_do_advance`
and its callers remain hot in every language. Reimplement the known conditions
without its old statistics feature, then compare current control and candidate
on all seven languages.

Acceptance gate: full behavior/parity tests, all seven paired language results
inside the normal regression bound, and at least +0.8% overall current-Rust
throughput. This is intentionally lower than the historical +1.26 points
because the runtime and subtree representation have changed.

Implemented result, measured against immediate Rust parent `0e98e639` in an
interleaved A/B/A run with five 200 ms CPU-time samples per fixture:

| Language | Fixtures | Throughput change |
| --- | ---: | ---: |
| C++ | 4 | +2.55% |
| Go | 5 | +1.33% |
| Java | 4 | +1.21% |
| JavaScript | 2 | +2.85% |
| Python | 12 | +1.91% |
| Rust | 2 | +1.83% |
| TypeScript | 11 | +4.94% |
| **All fixtures** | **40** | **+2.70%** |

Each fixture compares the candidate median with the geometric mean of its two
bracketing Rust controls; the final rows are geometric means. Source byte
lengths and hashes matched for every comparison. Maximum CV was 4.60%, 3.75%,
and 1.65% for control, candidate, and control. Peak RSS was neutral: the
largest per-language increase was 0.15 MiB, while five of seven candidate peaks
were lower. Five focused path witnesses, the Rust core tests, ABI test, Clippy,
core parity, and the four-package ast-grep gate passed. The candidate clears
the throughput and per-language gates and is retained.

### 2. Direct-final deterministic reducer

This should be one coherent fast path, not another accumulator layered over
the existing reducer:

```text
parser_reduce
  if deterministic window reduction is eligible:
      tail-call parser_reduce_window_final
  else:
      tail-call parser_reduce_glr
```

`parser_reduce_window_final` would:

1. scan the window to find the logical pop start;
2. identify trailing extras before allocating;
3. allocate the exact final `[children][internal header]` block;
4. initialize that final header in place;
5. move child handles directly into their final slots while updating that
   header's summaries; and
6. truncate the window and push the parent plus trailing extras.

This differs from the rejected stack-accumulator fusion. That experiment
created a large temporary summary object and then copied its result. The new
design writes the already-allocated final header and eliminates the temporary
`SubtreeArray`, `subtree_take_children`, and second summary traversal together.

The first checkpoint is code generation, before timing: the deterministic body
must have a substantially smaller frame than 304 B and contain no GLR version,
slice, merge, or iterator locals. If outlining does not change the fast-path
frame or linked hot code, stop before a full implementation.

Acceptance gate: exact summary-field witness against `subtree_new_node` in
debug builds; all deterministic-window ownership tests; full parity; at least
+1.0% overall paired throughput; no language below -1.0%; and no material RSS
increase. Go, Java, and Ruby recovery are mandatory individual gates.

Implemented result: **rejected**. The implementation did achieve the intended
code shape: the deterministic reducer frame fell from 304 B to 80 B, GLR
locals moved out of line, child handles moved directly into the exact final
arena block, and the final header was summarized during the move. Focused
ownership, trailing-extra, straddle-fallback, and summary-parity witnesses
passed.

The first five-sample, 200 ms current-Rust A/B/A gate was narrowly positive at
+1.09% overall, but the longer 500 ms confirmation did not reproduce the
required win:

| Language | Fixtures | Confirmed throughput change |
| --- | ---: | ---: |
| C++ | 4 | -2.39% |
| Go | 5 | -1.43% |
| Java | 4 | -0.27% |
| JavaScript | 2 | +2.80% |
| Python | 12 | +1.80% |
| Rust | 2 | -1.33% |
| TypeScript | 11 | +1.55% |
| **All fixtures** | **40** | **+0.58%** |

All 40 source hashes and byte lengths matched. Final-process peak RSS was
24.98 MiB for the candidate versus 24.95/24.92 MiB for the bracketing
controls, which is neutral. Maximum sample CV was 2.05%, 9.71%, and 5.45% for
control, candidate, and control, so the confirmation also failed the 5%
stability limit. The candidate fails the +1.0% overall gate and the -1.0%
per-language floor. The result shows that shrinking a hot caller's frame is
not sufficient evidence: moving the direct summary loop and final allocation
into a new helper changed code placement and dependency chains without
reliably improving the reduction-heavy languages.

A July 2026 reimplementation independently reproduced that decision after the
retained ASCII lexer fast path. Its short five-sample, 200 ms Rust A/B/A gate
again looked positive at +1.42%. A 500 ms whole-suite confirmation was also
positive but unstable, with maximum CV above 7%. The decisive 500 ms run then
bracketed each language separately to limit cross-language thermal drift:

| Language | Fixtures | Interleaved retry |
| --- | ---: | ---: |
| C++ | 4 | +0.25% |
| Go | 5 | +0.41% |
| Java | 4 | **-1.64%** |
| JavaScript | 2 | +2.99% |
| Python | 12 | -0.05% |
| Rust | 2 | +1.97% |
| TypeScript | 11 | +3.18% |
| **All fixtures** | **40** | **+1.01%** |

All source lengths and hashes matched, and RSS remained neutral. Java crossed
the -1.0% regression guard, while Python's candidate and final-control maximum
CV values were 7.27% and 5.17%. The retry is therefore rejected too. The
direct-final family should not be reopened merely because another short run
looks positive; it needs a materially different dependency chain or removal
of more work.

### 3. Accepted-DAG balancing worklist reuse

`subtree_prepare_for_balancing` must already traverse the accepted DAG to turn
conservative parser sharing marks into exact accepted sharing. It can, in the
same traversal, append reachable unshared internal candidates in the order
needed by `parser_balance_subtree`. Balancing can then iterate that reusable
worklist instead of discovering child edges again.

This is not the rejected parse-time candidate list: that list recorded dead
nodes from abandoned paths and paid a write during every construction. The
proposed list contains only nodes found by the required accepted-root traversal.
It is still a risky trade because it writes one handle per reachable candidate.

Gate it first with edge and candidate counts. If the resulting worklist write
volume is comparable to the edge scan it replaces, do not implement it. Any
prototype must preserve progress-callback resume behavior and exact sharing
before mutation.

### 4. Single-action dispatch

The action interpreter dynamically loops over `action_count` and carries GLR
reduction state even when an entry contains one shift or one reduction. A
dedicated `action_count == 1` path could directly dispatch the first action and
leave multi-action iteration out of line.

This remains measurement-gated because past local branch simplifications were
usually noise. Count dynamic action-entry shapes first; require at least 95%
single-action coverage before building it. Inspect the resulting
`parser_advance` frame and text size before benchmarking.

### 5. Parser-private arena bumping

The arena's atomic cursor is required after publication because separate tree
copies may perform copy-on-write edits concurrently. Parsing itself is
single-threaded and the arena is explicitly marked unpublished.

A safe design therefore needs two phases, not a global replacement:

- a parser-owned plain cursor/commit watermark in `SubtreePool` before
  publication; and
- synchronization into the arena's atomic cursor at publication, after which
  public copy/edit allocation continues using atomics.

This is a small-ceiling candidate. Do it only after the ASCII fast path and
deterministic-reducer work, and reject it if routing every `SubtreeArray`
growth through the private cursor expands the change beyond a focused
allocation layer.

### 6. External-scanner snapshots

The current scanner ABI exposes one mutable grammar-owned object plus serialized
bytes. The runtime must deserialize a stack version's bytes before scanning,
and a scanner may mutate even when it returns no token. That prevents a safe
runtime-only "already loaded" cache. The measured token-identity cache had only
8.11% overall reuse and 19.94% for Python, so that design remains closed.

A real solution requires a versioned optional ABI in which the grammar can
provide cheap immutable or copy-on-write scanner snapshots. Stack versions
would own snapshot handles and scanning would fork a snapshot only on mutation.
The existing serialize/deserialize callbacks remain the compatibility fallback.

This is only justified after a Python-specific prototype proves that scanner
snapshot creation is cheaper than deserialize plus malloc/free and that memory
does not grow with abandoned GLR versions.

## Deferred axis: generated lexer layout

The instruction-delivery diagnosis remains valid: C++'s 92 KiB `ts_lex` and
widely separated hot PCs make generated code layout a plausible explanation
for its front-end stalls. It is deferred because it sits outside the runtime's
controlled optimization surface:

- users generate parsers from grammars whose lexer-state graphs and hot-token
  distributions are not represented by the seven-language corpus;
- generated parser sources are compiled by different C/C++ toolchains and
  flags, which can transform the same source layout differently;
- changing generator output affects checked-in parser artifacts and therefore
  requires users to regenerate or upgrade them; and
- a corpus-trained PGO layout cannot ship as a general policy for grammars that
  Tree-sitter has never observed.

Hot/cold state partitioning, a compiled-hot/table-cold hybrid, and generator
PGO remain possible research mechanisms, but they are not scheduled or ranked
for this throughput program. Revisit them only with a separately approved,
opt-in generator mode and a broad ecosystem corpus that includes large, small,
sparse, external-scanner-heavy, and conflict-heavy grammars. Runtime-owned
lexer improvements such as the conservative ASCII advance fast path are not
deferred because they preserve generated-language behavior and apply uniformly
to existing parser artifacts.

## Designs not reopened by this profile

- **GC or semispace collection for fresh-parse throughput:** live syntax data is
  arena-backed, allocation is a small CPU fraction, and semispace copying
  already regressed heavily in the arena ledger.
- **Global child ranges or another handle table:** both add dependent loads in
  the Go workload that already shows a processing/dependency bottleneck; the
  measured global-range forms regressed.
- **Further compact-handle bit packing:** Candidate D and lazy column summaries
  already captured the useful density win. The current hot costs are traversal,
  dependent resolution, generated code, and dispatch.
- **Larger stack or leaf pools:** live malloc volume is small and the existing
  stack pool already serves nearly all ordinary nodes.
- **Generic `stack_iter` deduplication solely for binary size:** only one
  monomorph is hot. An enum or function-pointer core must prove it does not add
  dispatch to `pop_count` before it is considered.
- **Broad cold annotations, prefetch, or field shuffles:** hardware counters
  justify targeted code separation and locality work, not unmeasured global
  hints.

## Recommended experiment order

1. Retain the completed conservative ASCII advance fast path.
2. Keep the direct-final deterministic reducer rejected unless a materially
   different design removes more work than the measured candidate.
3. Gate accepted-DAG balancing reuse with edge and worklist counts.
4. Measure single-action coverage before attempting a dispatch fast path.
5. Only then consider the small-ceiling parser-private arena cursor.

This order records the retained low-complexity win and the rejected reducer
experiment before moving to the next measured runtime phases. It stays within
behavior that the runtime controls for every existing generated parser and
deliberately postpones GC, new indirection, ABI expansion, and generated-lexer
changes.
