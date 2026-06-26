# Parser Performance Trial Summary

This file tracks raw normal parsing performance work for the Rust runtime.

Target languages:

- TypeScript
- JavaScript
- Python
- Go
- Rust
- C++
- Java

Benchmark source files must not be changed. Profiling helpers may live outside
the repo under `/tmp`.

## Current Status

The 20% universal target is not met.

Current kept architecture changes:

- `9e843a09` - allocate parser reduction nodes in a tree arena.
- parser-owned stack-pop builder for reductions.

Measured same-session impact of that kept slice:

| Language | Baseline bytes/ms | Arena slice bytes/ms | Delta |
| --- | ---: | ---: | ---: |
| JavaScript | 17119 | 18072 | +5.6% |
| TypeScript | 22095 | 23024 | +4.2% |
| Python | 9031 | 9276 | +2.7% |
| Go | 15102 | 15265 | +1.1% |
| Rust | 13683 | 15139 | +10.6% |
| C++ | 7028 | 7068 | +0.6% |
| Java | 10588 | 11834 | +11.8% |

Mean of language averages:

- Baseline: `94646 bytes/ms`
- Arena slice: `99678 bytes/ms`
- Delta: `+5.3%`

Measured same-session impact of the parser-owned stack-pop builder on top of
the arena slice:

| Language | Baseline bytes/ms | Builder bytes/ms | Delta |
| --- | ---: | ---: | ---: |
| JavaScript | 18936 | 20127 | +6.3% |
| TypeScript | 24072 | 25684 | +6.7% |
| Python | 10311 | 11361 | +10.2% |
| Go | 16708 | 18571 | +11.1% |
| Rust | 16188 | 16487 | +1.8% |
| C++ | 8920 | 9924 | +11.3% |
| Java | 11535 | 12283 | +6.5% |

Mean of language averages:

- Baseline: `106670 bytes/ms`
- Builder: `114437 bytes/ms`
- Delta: `+7.3%`

Remaining weak spots: TypeScript, JavaScript, Python, and Java.

## Current Hotspots

Latest useful JavaScript `jquery.js` flamegraph after the kept arena-reduction
slice:

| Frame | Samples | Share |
| --- | ---: | ---: |
| `ts_parser__reduce` | 475 | 27.76% |
| `ts_lex` | 197 | 11.51% |
| `ts_subtree_new_node_in_arena` | 147 | 8.59% |
| `ts_stack_pop_count` | 147 | 8.59% |
| `tree_sitter_javascript_external_scanner_scan` | 137 | 8.01% |
| `ts_parser__balance_subtree` | 112 | 6.55% |
| `ts_subtree_compress` | 92 | 5.38% |
| `ts_subtree_summarize_children` | 91 | 5.32% |

Main shared bottleneck:

```text
ts_parser__advance
  -> ts_parser__reduce
     -> ts_stack_pop_count
     -> subtree node construction
     -> ts_subtree_summarize_children
     -> ts_stack_push / ts_stack_merge
```

SIMD is not the current primary target. The core runtime receives generated
lexer callbacks one codepoint at a time, so there is no obvious long contiguous
scan inside the core library to vectorize without grammar-level changes.

## What Worked

| Area | Kept result |
| --- | --- |
| Tree storage | Arena-backed `TSTree` foundation was added. |
| Reduction allocation | Parser reduction/accept/recovery parent nodes now allocate in the tree arena. |
| Stack pop materialization | Fresh parses now collect reduction stack-pop candidates into a parser-owned builder buffer instead of malloc-backed `SubtreeArray` values; reparses keep the original slice path for changed-range correctness. |
| Ownership correctness | Heap clones clear `arena_owned`, fixing edit/reparse leaks from cloned arena-backed nodes. |
| Earlier local parser fast paths | Several small wins were kept before the arena work, including linear stack-pop fast paths and direct nonterminal next-state lookup. |

The recent architecture-level wins are arena-backed reduction parent nodes and
parser-owned stack-pop builder materialization. They are useful, but the full
20% universal target is not proven yet.

## Itemized Trial Index

This section keeps one row per unique trial. Grouped summaries later in the file
may refer to these rows, but should not duplicate them as separate attempts.

### Kept

| Trial | Area | Result |
| --- | --- | --- |
| Avoid slice creation for subtree child access | Subtree access | Positive, kept |
| Compare lexer modes without `memcmp` | Token reuse / lexer mode | Positive, kept |
| Delay token reuse mode checks | Token reuse | Positive, kept |
| Inline hot array helpers | Array helpers | Positive, kept |
| Skip progress state updates without callback | Progress checks | Positive, kept |
| Avoid slice creation for lexer range access | Lexer ranges | Positive, kept |
| Fast path single lexer range reset | Lexer ranges | Positive, kept |
| Use direct lexer EOF checks internally | Lexer EOF | Positive, pushed |
| Fast path linear stack pops | Stack pop | Positive, kept |
| Direct nonterminal next-state lookup in reduce | Reduce path | Positive on JS/Go/TS canaries |
| Add arena-backed tree storage foundation | Tree storage | Positive foundation, kept |
| Allocate parser reduction nodes in tree arena | Reduce/node allocation | Positive architecture slice, `+5.3%` mean of seven language averages |
| Parser-owned stack-pop builder for fresh-parse reductions | Reduce/stack pop | Positive architecture slice, `+7.3%` mean of seven language averages on top of arena slice; reparses use the original slice path after `test_get_changed_ranges` exposed changed-range sensitivity; `cargo test --all` passed outside sandbox |

### Measurement And Design Trials

| Trial | Area | Result |
| --- | --- | --- |
| Cross-language reduce-construction profiling | Profiling/design | `cargo flamegraph` plus temporary reduce-shape counters across all seven target languages. Supports investigating a full reduce-construction redesign, but shows lexer and balancing are too large for reduce-only work to guarantee 20%. |
| Refreshed C++ raw parse flamegraph | Profiling/design | `cargo flamegraph` on C++ `rule.cc` with `/tmp/ts-raw-profile-harness-plain` produced `/tmp/tree-sitter-current-cpp.svg`: reduce `30.37%`, new node `10.59%`, summarize `7.94%`, stack pop `7.01%`, balance `4.05%`, `ts_lex` `22.90%`, keyword lex `6.07%`. Confirms reduce construction remains the largest library-owned target even on a lexer-heavy language. |
| Fresh-reduce candidate shape instrumentation | Profiling/design | Temporary parser-local counters across the seven target languages showed normal fresh parsing is almost entirely single-candidate. TypeScript, JavaScript, Python, Rust, and Java had zero merged groups; Go had `12 / 64540` merged groups; C++ had `5 / 4592` merged groups. This closes merged-candidate selection as a primary normal-case direction and shifts reduce work toward single-candidate collection/finalization. |
| C++ marker-index flamegraph | Profiling/design | `cargo flamegraph` on C++ `marker-index.h` with `/tmp/ts-raw-profile-harness-plain` produced `/tmp/tree-sitter-cpp-marker-current.svg`: reduce `27.81%`, `ts_lex` `21.93%`, keyword lex `5.88%`, new node in arena `9.63%`, summarize `8.56%`, stack pop into builder `5.88%`, stack push `5.35%` across visible frames, balance `3.74%`. Confirms C++ needs both reduce-construction and lexer/runtime-boundary work for a universal 20% target. |
| Lexer/runtime boundary counters | Profiling/design | Temporary parser and lexer counters across the seven target languages showed included-range stepping is zero in normal parsing and chunk reads are tiny. External scanner calls are high for JavaScript/TypeScript/Python and moderate for Rust, but absent for Go/C++/Java. Core runtime lookahead/advance callbacks are broad, but prior single-range and UTF-8/ASCII fast paths already failed, so lexer work needs narrower boundary evidence before code. |
| Reduce push/pop shape counters | Profiling/design | Temporary fresh-reduce counters showed trailing-extra stack pushes are too rare for batching to be a primary direction: `10,313` trailing pushes vs `319,371` parent pushes across the seven target languages. The broader signal is internal subtree churn: reductions popped `316,248` internal subtrees, almost one per reduction group. This supports pending/lazy reduction metadata as the next architecture design and deprioritizes parent-plus-extra push batching. |

### Rejected Or Closed

| Trial | Area | Result |
| --- | --- | --- |
| Broad metadata caching in `ts_subtree_summarize_children` | Subtree summarize | Regressed JavaScript, Go, and TypeScript |
| Single-child `ts_subtree_summarize_children` fast path | Subtree summarize | Flat or negative on JavaScript, Go, TypeScript |
| Smaller stack pop reserve count | Stack pop allocation | Large regression |
| Specialized `ts_stack_pop_count` graph walk without callback | Stack pop fallback | Mixed; Go improved once, JavaScript regressed |
| ASCII fast path in `ts_lexer__get_lookahead` | Lexer decode | Neutral or negative |
| Direct UTF-8 decode path avoiding decode function pointer | Lexer decode | Mixed or negative |
| Single-range per-character lexer advance fast path | Lexer advance | Negative |
| No-log lexer advance callback specialization | Lexer callback | Mixed/rejected. JavaScript average improved `18091` -> `18498`, but worst file regressed `17985` -> `16723`; C++ canary was noisy/inconclusive. |
| Alias-sequence condition reorder | Subtree summarize alias handling | Negative |
| Direct `as u8` casts replacing checked conversions in leaf creation | Leaf construction | Negative on JavaScript and Go |
| `#[inline]` on `ts_subtree_retain` | Refcount helper | Negative |
| Relaxed/release-acquire subtree/tree-arena refcount ordering | Refcount/lifetime | Failed twice; clean JavaScript regressed before and after arena work |
| Passing `is_leaf` into `ts_parser__shift` | Shift path | Negative |
| Direct cast for stack reserve count | Stack allocation | Negative |
| Accumulating subtree flags locally in summarizer | Subtree summarize flags | Negative |
| Caching `language_is_wasm` in `TSParser` | Parser state | Negative |
| Increasing `MAX_NODE_POOL_SIZE` from 50 to 128 | Stack node pool | Negative |
| Broad stack getter/push inlining | Stack helpers | Negative |
| Broad `ts_language_table_entry` inlining | Parse table lookup | Negative |
| Broad `ts_parser__check_progress` inlining | Parser progress check | Negative |
| Early no-callback return in `ts_parser__check_progress` | Parser progress check | Clean JavaScript benchmark regressed |
| Guard halted-version scans in `ts_parser__reduce` | Reduce version limiting | Clean JavaScript benchmark regressed |
| Pointer-equality fast path for `ts_stack_can_merge` last external tokens | Stack merge | Retested after reduce lookup win; remained below baseline |
| Guard no-op subtree-array reversals in stack pops | Stack pop | Warm JavaScript remained below baseline |
| Same-token fast path in `ts_stack_set_last_external_token` | External token tracking | Warm JavaScript remained below baseline |
| Skip summarize for zero-child non-error nodes | Subtree construction | Retested after reduce lookup win; remained below baseline |
| Guard zero dynamic-precedence writes in reduce | Reduce path | Retested after reduce lookup win; remained below baseline |
| Pointer-equality fast path in `ts_subtree_external_scanner_state_eq` | External scanner state comparison | Retested after reduce lookup win; remained below baseline |
| Hoist reduce nonterminal check out of pop-slice loop | Reduce path | Retested after reduce lookup win; remained below baseline |
| Specialized no-alias non-error subtree summarizer | Subtree summarize | Retested after reduce lookup win; remained below baseline |
| Combine arena child copy with summary calculation | Reduce/node construction | Regressed JavaScript same-session canary: patched `17447` avg bytes/ms vs reverted baseline `18091`; too close to the closed raw-pointer summarizer direction |
| Builder-specific copy plus summary finalization | Reduce/node construction | Mixed/rejected after parser-owned builder. JavaScript improved `20127` -> `21099` avg and TypeScript improved `25684` -> `26371`, but Python regressed `11361` -> `10754` avg and large grammar files regressed; not universal |
| Direct descriptor comparison for merged reduce candidates | Reduce/candidate selection | Mixed/rejected. JavaScript `-r 10` improved `18608` -> `18781` avg and Go `-r 5` improved `11531` -> `13017`, but TypeScript `-r 10` regressed `24076` -> `23567` avg and `20753` -> `20491` worst; large TypeScript `parser.ts` improved, but the aggregate gate failed |
| Fresh-parse direct graph builder collection | Reduce/stack pop | Rejected on Go warm same-session canary. Patched direct graph collection into `StackPopBuilder` avoided the old `StackSliceArray` conversion path for fresh reductions, but Go `-r 10` regressed from reverted baseline `18768` avg / `16707` worst bytes/ms to patched `17672` avg / `15865` worst. The graph fallback rate alone did not prove the conversion detour was worth replacing. |
| Single-group reduce control-flow split | Reduce/finalization | Rejected on JavaScript warm same-session canary. TypeScript looked positive in an initial `-r 10` canary (`26287` avg), but JavaScript regressed from reverted baseline `20395` avg / `19617` worst bytes/ms to patched `19648` avg / `19276` worst. Separating single-candidate control flow without removing child collection, arena copy, or summary work is insufficient. |
| Direct arena finalization for linear fresh reductions | Reduce/node construction | Rejected on JavaScript canary. The trial allocated an arena node block up front, filled it directly from the linear stack pop, removed trailing extras, and initialized the parent in place. TypeScript `-r 5` improved to `26333` avg / `22424` worst bytes/ms, but JavaScript `-r 10` regressed to `19811` avg / `18514` worst after a prior run at `20469` avg / `18584` worst. The second stack walk and branch/protocol overhead outweighed avoiding the builder-to-arena copy for JavaScript. |
| One-pass final-storage linear collection | Reduce/node construction | Rejected after same-session seven-language matrix. The trial added an arena reservation/finalization primitive, collected linear stack-pop children directly into reserved final storage, reversed in place, trimmed trailing extras, and initialized parent data after the non-extra prefix. Patched vs baseline `-r 10` avg/worst bytes/ms: JavaScript `19902/18114` vs `19381/18353`, TypeScript `26594/22976` vs `24519/19013`, Python `10418/520` vs `10308/542`, Go `16236/15277` vs `16818/15983`, Rust `18091/14136` vs `18293/14266`, C++ `7939/6831` vs `6816/5662`, Java `12497/9842` vs `13459/10449`. Removing the builder-to-arena copy helped TypeScript and C++, but direct arena reservation/locality and fallback costs regressed Go, Rust, and Java. |
| Propagated contains-repetition flag for balancing | Balance/compress | Regressed Rust same-session canary: patched `13149` avg / `11290` worst bytes/ms vs reverted baseline `14124` avg / `12362` worst; metadata overhead outweighed traversal pruning |
| 16-bit symbol inline leaf encoding | Subtree inline representation | Regressed JavaScript and did not reduce allocation counts |
| Global mutex slab for `SubtreeHeapData + children` blocks | Subtree block allocation | JavaScript benchmark stalled; global lock path not viable |
| Atomic global slab with `SubtreeArray.capacity` slab marker | Subtree block allocation | JavaScript benchmark stalled; ownership marker was too fragile |
| Zero-count fast path in linear stack pops | Stack pop | Warm JavaScript below baseline |
| Refcount-one direct release fast path | Subtree release | Regressed JavaScript |
| Terminal-only table-entry helper in advance loop | Parse table lookup | Warm JavaScript below baseline |
| Increase `TS_MAX_TREE_POOL_SIZE` from 32 to 128 | Childless subtree pool | Allocation counts unchanged; JS got slower/noisier |
| Pool-backed zero-child `ts_subtree_new_node` plus zero-count stack-pop reserve skip | Childless subtree allocation | Allocation counts unchanged; JS/TS/Go regressed |
| Raw-pointer child loop in `ts_subtree_summarize_children` | Subtree summarize | JS/TS/Go/Python regressed |
| Use `ts_malloc` instead of `ts_realloc(NULL, size)` in subtree array allocation | Subtree allocation | JS/TS/Go/Python regressed |
| Parser `SubtreePool` free lists for 1-4 child node blocks | Subtree block allocation | Allocation calls dropped, but harness and JS/TS/Go regressed |
| Arena-backed heap leaves during lexing | Subtree allocation | JS/TS/Python improved, but Go and Rust regressed |
| Increase `TREE_ARENA_PAGE_SIZE` from 16 KiB to 64 KiB | Tree arena page layout | JavaScript regressed to `17256` avg bytes/ms |
| Adopt stack-pop child arrays into `TreeArena` instead of copying into arena pages | Reduce/node construction | JavaScript roughly flat; TypeScript regressed |
| Embedded adopted-block headers in stack-pop arrays | Reduce/node construction | TypeScript improved, but JavaScript slipped; not universal |
| Direct linear reduce pop into parser scratch storage | Reduce/stack pop | Abandoned before benchmarking after history triage; too close to prior linear stack-pop and stack-pop adoption attempts |
| Stack-pop trailing-extra split before parent construction | Reduce/stack pop | Abandoned before coding. A useful version either becomes a linear-only scratch-buffer variant, or adds per-candidate trailing-extra arrays and ownership pressure before selection. |
| Skip post-parse subtree balancing entirely | Balance/compress upper bound | JavaScript improved, TypeScript regressed badly |
| Single-pass repeat compression schedule | Balance/compress | Mixed/rejected on Rust same-session canary: patched `13097` avg / `11222` worst bytes/ms vs reverted baseline `13199` avg / `11159` worst. Reducing the halving schedule did not improve the language with the largest balance share. |
| Reset benchmark allocator for raw parsing | Benchmark harness | Removed because benchmark source changes are out of scope |

## Closed Directions

Do not retry these without new profiler evidence that contradicts the recorded
result.

| Direction | Why closed |
| --- | --- |
| Relaxed/release-acquire subtree or arena refcount ordering | Failed twice. Earlier clean JavaScript benchmark regressed, and post-arena JavaScript canary regressed from `18072` to `17604` avg bytes/ms. Refcount frames are visible but not dominant. |
| Larger tree arena pages | 64 KiB pages regressed JavaScript to `17256` avg bytes/ms. Fewer page allocations did not offset worse locality/cache behavior. |
| Arena-backed lexer leaves | Helped JavaScript/TypeScript/Python but regressed Go to `14165` and Rust to `13219` avg bytes/ms. Not universal. |
| Stack-pop malloc-buffer adoption into `TreeArena` | Both metadata and embedded-header versions were mixed. JavaScript was flat/regressed while TypeScript moved differently. This is not a real builder path. |
| Direct linear reduce-pop scratch buffer | Closed before benchmarking. It is not identical to stack-pop buffer adoption, but it is still an incremental linear stack-pop fast path, not the requested architecture change. |
| Stack-pop trailing-extra split before selection | Rejected before implementation. It either repeats the linear scratch-buffer path or adds per-candidate trailing arrays that fight the builder ownership goal. |
| Skipping/deferring all balancing | JavaScript improved to `18728`, but TypeScript regressed to `22339` avg and `17610` worst bytes/ms. |
| Contains-repetition summary bit for balance pruning | Regressed Rust in same-session A/B. Do not retry branch-pruning balance metadata unless profiles show traversal overhead exceeds the metadata propagation cost. |
| Single-pass repeat compression schedule | Did not improve Rust, where balance/compress is largest. Do not retry compression-schedule tuning without a tree-shape proof and cross-language profile evidence. |
| Subtree allocation pools/slabs | Reduced some allocator counts, but bookkeeping, locking, or locality costs regressed benchmarks. |
| `TS_MAX_TREE_POOL_SIZE` tuning | Allocation counts were unchanged and benchmarks got noisier/slower. |
| Refcount-one release fast path | Regressed JavaScript. |
| Raw pointer summarizer loop | Regressed JS/TS/Go/Python. Existing iterator compiled better. |
| Combined arena copy plus summarizer loop | Regressed JavaScript in same-session A/B. Do not retry summary-loop rewrites unless the full builder design changes the ownership/selection protocol first. |
| Builder-specific copy plus summary finalization | Rejected after the builder protocol existed. It helped JS/TS but regressed Python, including large grammar files; do not retry summary-loop fusion without new Python-specific evidence. |
| Direct merged-candidate descriptor comparison | Mixed. It helped JavaScript/Go and the large TypeScript parser file, but regressed the TypeScript aggregate benchmark. Do not retry as a standalone replacement for temporary candidate parents. |
| Fresh-parse direct graph builder collection | Warm Go canary regressed despite the highest recorded graph fallback rate. Do not retry direct graph collection as an isolated `StackPopBuilder` completion; only revisit if a full reduce-construction protocol redesign removes more of the candidate materialization pipeline at the same time. |
| Single-group reduce control-flow split | Warm JavaScript regressed. Do not retry single-candidate branching unless the change removes a material phase such as builder child collection, builder-to-arena copy, or summary/finalization work. |
| Direct arena finalization for linear fresh reductions | JavaScript regressed even though TypeScript improved. Do not retry as a linear-only two-pass stack walk. A future direct-finalization design would need to collect directly in one pass without adding another stack traversal. |
| One-pass final-storage linear collection | Mixed after same-session seven-language matrix. It removed the builder-to-arena child copy and helped TypeScript/C++, but regressed Go/Rust/Java. Do not retry direct collection into upfront arena reservations as a linear-only path. Reopen only with evidence that reservation/fallback locality costs are removed or that the protocol also eliminates summary, stack-push, or graph-candidate materialization work. |
| Broad inlining/caching/check-progress fast paths | Repeatedly regressed or stayed below baseline. |
| Lexer ASCII/direct UTF-8 fast paths | Mixed or negative. |

## Reflection 1: Arena/Allocation Batch

Attempts covered:

- Allocation profiling and slab/pool trials.
- Arena-backed tree storage.
- Parser reduction-node arena allocation.
- Arena-backed leaves.
- Arena page-size tuning.
- Stack-pop buffer adoption variants.
- Refcount ordering.
- Skip-balancing upper-bound experiment.

What worked:

- Arena-backed normal tree storage plus reduction parent-node allocation.
- The `arena_owned` clone fix.

What failed:

- Most allocation-count-reduction ideas that did not improve locality.
- Leaf arena allocation because it was not universal.
- Page-size tuning.
- Partial stack-pop array adoption.
- Refcount ordering changes.
- Removing balancing.

Main lesson:

- Allocation count alone is not predictive. Several ideas reduced allocation
  pressure but lost on cache locality, branch layout, or language-specific parse
  shape.
- The next serious work should be a real parser-local reduce builder that writes
  child spans in the desired representation from the start. Do not try to rescue
  already-allocated `SubtreeArray` buffers after the fact.

## Reflection 2: Reduce Protocol And Boundary Evidence

Attempts covered:

- Direct merged-candidate descriptor comparison.
- Propagated contains-repetition balance flag.
- Single-pass repeat compression schedule.
- Builder-specific copy plus summary finalization.
- Fresh-parse direct graph builder collection.
- Reduce-construction equivalence map.
- Fresh-reduce candidate shape instrumentation.
- C++ `marker-index.h` flamegraph.
- Single-group reduce control-flow split.
- Direct arena finalization for linear fresh reductions.

Wins:

- Parser-owned stack-pop builder remained the last kept reduce-construction
  architecture slice.
- Fresh-reduce candidate shape instrumentation clarified that normal parsing is
  overwhelmingly single-candidate; merged-candidate work is not a primary
  normal-case lever.
- C++ profiling clarified that reduce and lexer are co-dominant, so the 20%
  universal target needs more than reduce construction alone.

Losses:

- Standalone merged-candidate selection was too narrow and regressed the
  TypeScript aggregate.
- Graph fallback collection regressed Go despite Go having the highest graph
  fallback rate.
- Control-flow-only single-candidate branching regressed JavaScript.
- Two-pass direct arena finalization helped TypeScript but regressed
  JavaScript; avoiding the copy did not offset the extra stack walk and protocol
  overhead.
- Balance metadata and schedule changes did not improve Rust, the language
  where balance/compress matters most.

Repeated mistakes:

- Several trials changed a local symptom of reduce construction without removing
  the full phase that the profiler shows: stack collection, parent allocation,
  child copy, summarization, and stack push still remained as separate costs.
- Candidate selection kept resurfacing, but normal fresh parsing does not spend
  enough time on merged candidates to justify it as a primary direction.

Closed directions:

- Standalone merged-candidate descriptor comparison.
- Isolated graph builder collection.
- Single-candidate control-flow branching without removing a material phase.
- Two-pass direct arena finalization for linear stack pops.
- Balance branch-pruning metadata and compression schedule tuning.

Open evidence:

- `ts_parser__reduce` remains the largest runtime-owned frame across the target
  languages, but the remaining viable reduce design must collect directly into
  final storage in one pass or remove summary/finalization work as part of a
  larger protocol.
- C++ and JavaScript still show large generated lexer/external scanner shares.
  Core-library lexer micro-fast paths failed before, and generated
  `set_contains` is not library runtime code in the benchmark fixtures.
- Stack push/node creation is visible, but pool sizing and broad helper inlining
  are already closed. A useful stack direction must batch or avoid pushes as
  part of reduce construction, not tune node allocation locally.

Next direction:

- Stop standalone reduce micro-architecture trials until there is a one-pass
  reduce-construction design that writes children into final storage without a
  second stack walk.
- In parallel, run lexer-boundary instrumentation to separate reusable runtime
  costs from generated `ts_lex`, keyword lexing, and external scanner costs.
  Only implement lexer work if the measured cost is in `ts_lexer__get_lookahead`,
  `ts_lexer__do_advance`, included-range handling, or another core runtime
  function, not generated fixture code.

Acceptance gate:

- Any kept library optimization must pass same-session `cargo xtask benchmark
  --kind normal -r 10 --language` for TypeScript, JavaScript, Python, Go, Rust,
  C++, and Java, with no average or meaningful worst-file regression.
- Kept library code must pass `cargo test --all` outside the sandbox.

## Algorithm Notes: What A Real Parser Revamp Must Change

The failed reduce trials show that the current bottleneck is not one branch or
one allocation call. The algorithmic cost is the shape of the pipeline:

```text
stack links -> retained child list -> final child list -> parent summary
            -> parent subtree -> stack links
```

The current successful builder only removed part of the temporary child-list
allocation. The remaining cost is still phase-oriented:

- Walk the stack to collect children.
- Retain each child.
- Reverse children into parser order.
- Trim trailing extras into a side array.
- Copy selected children into arena-backed parent storage.
- Walk selected children again to summarize the parent.
- Push the parent and trailing extras as new stack nodes.

### Algorithmic Constraints

- Parent layout requires child storage to be immediately before
  `SubtreeHeapData`, and the parent data pointer depends on the final child
  count after trailing extras are removed.
- Stack traversal discovers children from right to left. Parser-order children
  are left to right.
- Trailing extras are discovered at the right edge of the parser-order child
  list, but they still need to be pushed after the parent.
- Merged candidates are rare in normal fresh parsing, but correctness still
  requires preserving their ordering and selection behavior.
- Reparses remain more sensitive because changed-range behavior already broke
  when the builder path was used too broadly.

### Why Recent Designs Failed

- Single-group branching did not remove a phase. It only skipped the generic
  merged-candidate loop.
- Direct arena finalization removed the builder-to-arena copy but added a
  second stack walk to learn the allocation size. JavaScript lost more from the
  extra traversal and control flow than it gained from avoiding the copy.
- Direct graph collection removed a conversion detour for a rare path, but the
  normal workload did not spend enough time there.

### Viable Algorithm Families

1. One-pass final-storage reduce collection.
   - Required change: collect stack children directly into their final parent
     storage in one traversal.
   - Hard part: the current arena layout needs final child count before the
     parent data pointer is known.
   - Possible design: an arena reservation API that can append child slots,
     reverse the collected range in place, save trailing extras, then place
     `SubtreeHeapData` immediately after the non-extra prefix. This wastes the
     reserved trailing-extra slot space but avoids the second stack walk and the
     builder-to-arena copy.
   - Risk: placing parent data before the originally reserved end of the block
     creates internal holes in arena pages. This is acceptable only if the holes
     are bounded by trailing-extra counts, which are small in the measured
     normal cases.

2. Incremental parent summary during collection.
   - Required change: compute the parent summary while collecting children so
     `ts_subtree_summarize_children` is not a separate full traversal.
   - Hard part: collection is right-to-left, while summary logic is
     left-to-right and alias-sequence indexing depends on structural child
     order.
   - Possible design: maintain a reverse/prepend summary for layout-independent
     fields, and fall back to the existing summarizer for productions with alias
     sequences or error symbols. This is only worth trying if counters show the
     fallback rate is low across the seven target languages.
   - Risk: earlier summary-loop rewrites regressed. This must be a different
     algorithm that removes a traversal after ownership changes, not another
     hand-written variant of the same loop.

3. Lazy reduction nodes on the parse stack.
   - Required change: stack links hold a compact pending-node descriptor plus
     summary metadata instead of immediately materialized `Subtree` parents.
   - Benefit: chains of reductions could avoid copying children into intermediate
     parents until the tree is accepted or until a real subtree is needed for
     comparison, error recovery, or reuse.
   - Hard part: many parser operations expect `Subtree` identity and metadata:
     stack merging, error costs, dynamic precedence, external-token tracking,
     tree comparison, balancing, and public tree lifetime.
   - Risk: this is the only design that might change the asymptotic amount of
     intermediate tree construction, but it is also the highest correctness
     risk and would need a compatibility layer for every subtree query used by
     parsing.

4. Batched stack push for reduce finalization.
   - Required change: push parent plus trailing extras as a batch, creating a
     chain of stack nodes with one metadata walk/update instead of repeated
     `ts_stack_push` calls.
   - Benefit: targets visible stack-push/node-creation cost without changing
     child ownership.
   - Hard part: each pushed subtree affects position, error cost, node count,
     dynamic precedence, and last-error bookkeeping.
   - Risk: broad stack push/helper inlining and node-pool sizing already failed;
     batching must reduce repeated metadata work, not just reorganize the same
     calls.

### Algorithmic Triage Result

The reduce-shape counters below answered the two open questions:

- Alias-free small reductions are broad but not universal. They cover `79.2%`
  of `1-3` child candidates, but C++, JavaScript, TypeScript, and Go all have
  enough alias candidates that summary fusion should not be the first revamp.
- Trailing extras are rare and small. They are only `1.8%` of collected child
  slots and appear in only `1.6%` of candidates, so one-pass arena reservation
  with bounded holes is the next reduce-construction design to pursue.

Decision: prioritize one-pass final-storage reduce construction. Keep
incremental summary as a later layer only after final-storage ownership changes.
If the storage design fails without a second stack walk, move to lexer/runtime
boundary investigation before trying another reduce micro-design.

## Performance Trial Decision Process

Use this workflow for every performance attempt, independent of the current
optimization direction.

1. Define the target bottleneck.
   - Name the exact hot path, resource, data structure, or workflow cost.
   - Cite current evidence: flamegraph frame, sample output, allocation profile,
     benchmark result, or measured distribution.
   - If there is no current evidence, profile before coding.

2. Check history before coding.
   - Search this file and git history for the target area.
   - Classify the idea as `new`, `repeat`, or `near-repeat`.
   - For `repeat` or `near-repeat`, continue only if new profiler evidence
     directly contradicts the old result.
   - If the idea is only a small conditional fast path in an already-closed
     area, reject it.

3. Estimate leverage.
   - Prefer changes that remove or simplify a repeated phase, conversion,
     allocation family, ownership pattern, traversal, synchronization point, or
     other recurring cost.
   - Reject changes that only rearrange a small branch, annotation, guard, or
     special case unless profiler evidence shows that exact operation is a
     dominant cost in the target workload.

4. Write the trial hypothesis.
   - Expected win source.
   - Workloads expected to benefit.
   - Known risk from previous trials.
   - Kill criteria before benchmark time is spent.

5. Implement one scoped change.
   - Do not edit measurement fixtures or benchmark harnesses unless the trial
     explicitly targets measurement methodology.
   - Do not combine unrelated optimizations.
   - If instrumentation is needed, keep it temporary and remove it before
     committing production code.

6. Measure in increasing cost order.
   - Smoke benchmark the most relevant sample or workload first.
   - If the smoke result is negative or only noise, revert and log.
   - If positive, run the full benchmark matrix required by the current target.
   - For kept code, run the project validation required by the current
     acceptance gate.

7. Decide.
   - Keep only if the target workload holds or the net gain is large with a
     clear explanation for any localized regression.
   - Reject meaningful worst-file regressions unless there is an explicit
     reason they are noise or outside the target workload.
   - Revert failed code before moving to the next idea.
   - Log the result immediately, including why it should or should not be
     revisited.

### Trial Log Entry

Each trial entry should include:

- `Hypothesis`: what should become faster and why.
- `History check`: new/repeat/near-repeat, with matching prior rows or commits.
- `Change`: the implementation surface touched.
- `Evidence`: profiling, benchmark numbers, and test result if kept.
- `Decision`: kept, rejected, abandoned before benchmark, or needs follow-up.
- `Do not retry unless`: the specific new evidence required to reopen it.

## Performance Reflection Process

Write one reflection after every ten performance attempts, before starting the
next code experiment.

- `Attempts covered`: list the ten trials or measurements.
- `Wins`: what was kept and why it was real.
- `Losses`: what failed, grouped by failure mode.
- `Repeated mistakes`: any direction that was retried without enough new
  evidence.
- `Closed directions`: areas that should not be retried.
- `Open evidence`: profiler facts that still point at unresolved bottlenecks.
- `Next direction`: the single highest-priority direction and why it beats the
  alternatives.
- `Acceptance gate`: exact benchmarks/tests required before keeping it.

## Performance Next Direction Triage

Choose the next optimization direction by ranking candidates in this order:

1. Hotspot size in the current target workload.
2. Breadth across the required benchmark cases.
3. Leverage: removes or simplifies a phase, copy, allocation family, repeated
   traversal, synchronization point, or ownership transition rather than adding
   a special case.
4. Clean separation from closed trial history.
5. Correctness risk and testability.
6. Benchmark cost.

When two candidates are close, pick the one with stronger workload evidence. Do
not pick the easier patch if it mostly repeats a closed direction.

## Current Parser Next-Direction Plan

This section applies the generic process above to the current raw parser
performance goal. It is current work, not the reusable process.

### History Gate For Parser Work

Before writing parser optimization code, run at least:

```sh
rg -n "stack-pop|stack pop|linear|adopt|builder|SubtreeArray|reduce builder|scratch|child arrays" PERFORMANCE_TRIALS.md
git log --oneline --all --grep='stack' --grep='arena' --grep='builder' --grep='linear' --grep='adopt' --grep='reduce'
```

The `2026-06-26` direct linear reduce-pop scratch-buffer sketch failed this
gate. It was reverted before benchmarking because it was too close to recorded
linear stack-pop and stack-pop adoption attempts.

### Current Evidence

Collected on `2026-06-26` with `cargo flamegraph` using a plain temporary
raw-parse harness under `/tmp/ts-raw-profile-harness-plain`:

| Language sample | Reduce | New node | Summarize | Stack pop | Balance | Lex / scanner |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| JavaScript `jquery.js` | 28.52% | 9.13% | 6.62% | 8.44% | 6.39% | `ts_lex` 11.79%, external scan 8.29% |
| TypeScript `parser.ts` | 28.11% | 9.32% | 6.73% | 7.60% | 6.58% | `ts_lex` 10.57%, external scan 6.81% |
| Python `python3-grammar.py` | 23.91% | 8.61% | 6.19% | 5.43% | 5.69% | `ts_lex` 9.95%, external scan 9.62%, deserialize 7.11% |
| Go `proc.go` | 34.62% | 8.35% | 5.46% | 11.85% | 6.38% | `ts_lex` 11.42% |
| Rust `ast.rs` | 26.89% | 9.48% | 6.70% | 6.86% | 11.48% | `ts_lex` 15.49%, external scan 5.24% |
| C++ `rule.cc` | 25.84% | 8.24% | 5.76% | 6.00% | 3.92% | `ts_lex` 25.84% |
| Java `types.java` | 34.66% | 11.97% | 8.80% | 7.02% | 1.65% | `ts_lex` 15.82% |

Refreshed C++ `rule.cc` sample after later rejected trials still points at
the same priority order: `ts_parser__reduce` `30.37%`,
`ts_subtree_new_node_in_arena` `10.59%`,
`ts_subtree_summarize_children` `7.94%`, `ts_stack_pop_count` `7.01%`,
`ts_parser__balance_subtree` `4.05%`, `ts_lex` `22.90%`, and
`ts_lex_keywords` `6.07%`.

Refreshed C++ `marker-index.h` sample after single-candidate shape triage:
`ts_parser__reduce` `27.81%`, `ts_lex` `21.93%`,
`ts_lex_keywords` `5.88%`, `ts_subtree_new_node_in_arena` `9.63%`,
`ts_subtree_summarize_children` `8.56%`, `ts_stack_pop_count_into` `5.88%`,
visible `ts_stack_push` frames `5.35%`, `ts_parser__balance_subtree` `3.74%`,
and `ts_subtree_compress` `1.60%`. Output:
`/tmp/tree-sitter-cpp-marker-current.svg`.

Refreshed JavaScript `jquery.js` sample after the kept parser-owned stack-pop
builder still points at reduce construction as the largest parser-owned target:
`ts_parser__reduce` `25.51%`, `ts_subtree_new_node_in_arena` `9.42%`,
`ts_stack_pop_count_into` `7.29%`, `ts_subtree_summarize_children` `6.93%`,
`ts_parser__balance_subtree` `6.31%`, `ts_subtree_compress` `4.71%`,
`ts_lex` `13.07%`, external scanner scan `7.02%`, and
`ts_lex_keywords` `5.51%`.

Temporary reduce-shape instrumentation, removed before committing:

| Language sample | Linear pop-count calls | Graph fallback calls | Dominant reduce child counts |
| --- | ---: | ---: | --- |
| JavaScript `jquery.js` | `3,145,600` | `45,150` | 1-3 |
| TypeScript `parser.ts` | `2,962,600` | `11,700` | 1-3 |
| Python `python3-grammar.py` | `541,850` | `0` | 1-3 |
| Go `proc.go` | `1,519,800` | `125,000` | 1-3 |
| Rust `ast.rs` | `885,700` | `0` | 1-3 |
| C++ `rule.cc` | `577,800` | `1,200` | 1-3 |
| Java `types.java` | `85,000` | `0` | 1-3 |

Per-parse reduce-shape summary:

| Language sample | Reductions/parse | Child-array bytes/parse | Trailing extras removed/parse | Graph fallback rate |
| --- | ---: | ---: | ---: | ---: |
| JavaScript `jquery.js` | 63,815 | 6,081,992 | 2,737 | 1.4% |
| TypeScript `parser.ts` | 59,486 | 5,585,304 | 2,792 | 0.4% |
| Python `python3-grammar.py` | 10,837 | 1,013,144 | 115 | 0.0% |
| Go `proc.go` | 32,896 | 3,337,648 | 1,534 | 7.6% |
| Rust `ast.rs` | 17,714 | 1,655,384 | 836 | 0.0% |
| C++ `rule.cc` | 2,895 | 273,328 | 9 | 0.2% |
| Java `types.java` | 85 | 8,160 | 26 | 0.0% |

Temporary fresh-reduce candidate instrumentation, removed before committing:

| Language | Groups | Single groups | Merged groups | Candidates | Children | Trailing extras |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| TypeScript | 65,903 | 65,903 | 0 | 65,903 | 114,347 | 3,086 |
| JavaScript | 102,012 | 102,012 | 0 | 102,012 | 180,897 | 3,180 |
| Python | 59,977 | 59,977 | 0 | 59,977 | 102,447 | 659 |
| Go | 64,540 | 64,528 | 12 | 64,552 | 116,676 | 2,528 |
| Rust | 21,182 | 21,182 | 0 | 21,182 | 35,947 | 849 |
| C++ | 4,592 | 4,587 | 5 | 4,597 | 7,904 | 11 |
| Java | 1,165 | 1,165 | 0 | 1,165 | 2,047 | 0 |

Implication: merged-candidate comparison is not broad enough for the normal-case
target. A reduce-construction redesign must primarily remove work from the
single-candidate path: retained child collection, builder-to-parent copying,
summary/finalization, and parent/trailing-extra stack push.

Temporary reduce algorithm triage instrumentation, removed before committing:

| Language | Candidates | Alias candidates | 1-3 child candidates | No-alias 1-3 child candidates | Collected children | Non-extra prefix children | Trailing extras | Trailing-extra candidates |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| JavaScript | 102,012 | 23,694 | 96,791 | 74,871 | 180,897 | 177,717 | 3,180 | 2,230 |
| TypeScript | 65,903 | 15,452 | 62,790 | 48,414 | 114,347 | 111,261 | 3,086 | 1,147 |
| Python | 59,977 | 8,215 | 56,676 | 50,581 | 102,447 | 101,788 | 659 | 444 |
| Go | 64,552 | 17,070 | 61,271 | 46,441 | 116,676 | 114,148 | 2,528 | 1,090 |
| Rust | 21,182 | 4,320 | 19,862 | 16,412 | 35,947 | 35,098 | 849 | 311 |
| C++ | 4,597 | 1,986 | 4,445 | 2,548 | 7,904 | 7,893 | 11 | 11 |
| Java | 1,165 | 350 | 1,096 | 798 | 2,047 | 2,047 | 0 | 0 |
| Total | 319,388 | 71,087 | 302,931 | 240,065 | 560,265 | 549,952 | 10,313 | 5,233 |

Derived shape:

- `1-3` child candidates are `302,931 / 319,388` = `94.8%`.
- No-alias `1-3` child candidates are `240,065 / 302,931` = `79.2%`
  of small candidates, but C++ has a materially higher alias share.
- Trailing extras are `10,313 / 560,265` = `1.8%` of collected child slots.
- Candidates with any trailing extras are `5,233 / 319,388` = `1.6%`.
- Non-extra child-count buckets across the seven-language run: `0`: `0`,
  `1`: `182,714`, `2`: `66,175`, `3`: `54,042`, `4+`: `16,457`.

Algorithm implication: a one-pass final-storage design that over-reserves child
slots and leaves bounded holes for trailing extras has broad evidence now. The
hole cost is tiny in the measured normal cases, and it avoids the failed
two-pass direct-finalization shape. Incremental summary is still plausible for
the no-alias small majority, but C++, JavaScript, TypeScript, and Go have enough
alias candidates that summary fusion should follow storage redesign, not lead
it.

Temporary lexer/runtime boundary instrumentation, removed before committing:

| Language | Main lex | Keyword lex | External scan | External serialize | External deserialize | Lookahead | Advance | Skip advance | Consume advance | Mark end | Chunk reads | Included-range steps |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| JavaScript | 71,744 | 26,932 | 75,345 | 3,601 | 75,345 | 676,515 | 587,424 | 207,868 | 379,558 | 398,266 | 10 | 0 |
| TypeScript | 44,342 | 18,757 | 45,479 | 1,137 | 45,479 | 666,090 | 612,181 | 287,813 | 324,379 | 327,931 | 58 | 0 |
| Python | 33,557 | 11,731 | 41,922 | 8,365 | 41,922 | 451,415 | 412,976 | 272,829 | 140,159 | 150,310 | 84 | 0 |
| Go | 40,139 | 14,990 | 0 | 0 | 0 | 262,441 | 235,602 | 22,695 | 212,911 | 193,918 | 8 | 0 |
| Rust | 13,373 | 5,087 | 5,555 | 558 | 5,555 | 99,051 | 88,002 | 25,085 | 62,919 | 44,986 | 8 | 0 |
| C++ | 3,178 | 1,395 | 0 | 0 | 0 | 19,754 | 17,167 | 2,329 | 14,840 | 12,285 | 4 | 0 |
| Java | 871 | 365 | 0 | 0 | 0 | 5,131 | 4,487 | 1,046 | 3,443 | 2,835 | 4 | 0 |

Implications:

- Included-range stepping is not a normal-case target for the seven-language
  benchmark. Every measured language had `0` included-range steps.
- Input chunking is not a normal-case target; chunk reads are tiny relative to
  lookahead/advance counts.
- External scanners are important for JavaScript, TypeScript, Python, and Rust,
  but absent in Go, C++, and Java samples. Scanner-boundary work cannot be the
  primary universal lever.
- Runtime lookahead/advance/mark-end callbacks are broad, but previous
  single-range advance and UTF-8/ASCII decode fast paths regressed. Reopen lexer
  runtime work only with flamegraph evidence for a specific reusable operation,
  not generic callback-count evidence.

Temporary reduce push/pop shape instrumentation, removed before committing:

| Language | Groups | Merged groups | Candidates | Parent pushes | Trailing pushes | Groups with trailing | Popped subtrees | Popped internal | Internal visible | Internal named | Internal hidden | Popped leaves | Popped extras | Graph fallbacks |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| JavaScript | 102,012 | 0 | 102,012 | 102,012 | 3,180 | 2,230 | 181,515 | 101,972 | 34,129 | 93,274 | 67,843 | 74,875 | 4,668 | 1,007 |
| TypeScript | 65,903 | 0 | 65,903 | 65,903 | 3,086 | 1,147 | 114,540 | 65,221 | 21,965 | 60,463 | 43,256 | 44,811 | 4,508 | 271 |
| Python | 59,977 | 0 | 59,977 | 59,977 | 659 | 444 | 102,457 | 59,894 | 18,377 | 51,446 | 41,517 | 41,275 | 1,288 | 14 |
| Go | 64,540 | 12 | 64,552 | 64,540 | 2,528 | 1,090 | 119,443 | 62,917 | 26,787 | 55,922 | 36,130 | 52,172 | 4,354 | 4,913 |
| Rust | 21,182 | 0 | 21,182 | 21,182 | 849 | 311 | 35,947 | 20,665 | 5,972 | 14,473 | 14,693 | 13,929 | 1,353 | 0 |
| C++ | 4,592 | 5 | 4,597 | 4,592 | 11 | 11 | 7,918 | 4,485 | 1,931 | 4,148 | 2,554 | 3,420 | 13 | 16 |
| Java | 1,165 | 0 | 1,165 | 1,165 | 0 | 0 | 2,061 | 1,094 | 377 | 1,007 | 717 | 967 | 0 | 18 |
| Total | 319,371 | 17 | 319,388 | 319,371 | 10,313 | 5,233 | 563,881 | 316,248 | 109,538 | 280,733 | 206,710 | 231,449 | 16,184 | 6,239 |

Implications:

- Parent-plus-trailing-extra stack push batching is too narrow by itself.
  Trailing pushes are only `10,313 / 329,684` = `3.1%` of reduce-finalization
  pushes, and only `5,233 / 319,371` = `1.6%` of groups have trailing extras.
- Merged groups remain negligible: `17 / 319,371` groups.
- Internal subtree churn is broad: `316,248 / 319,371` = `99.0%` as many
  internal popped subtrees as reduction groups. This is the first evidence that
  a pending/lazy reduction descriptor could remove a repeated materialize-then-
  immediately-consume pattern.
- The internal nodes are not merely invisible wrappers. Across all languages,
  `280,733` internal popped nodes are named and `206,710` are hidden. A pending
  design must preserve metadata and tree identity lazily; it cannot just skip
  hidden nodes.

Pending-reduction stack inventory:

The stack currently consumes these `Subtree` properties immediately after a
reduce push:

- `stack_node_new`: `error_cost`, `total_size`, visible-descendant node count,
  visibility, symbol, and `dynamic_precedence`.
- `stack__subtree_is_equivalent` and stack link merging: symbol, error cost,
  padding bytes, size bytes, child count, extra flag, external scanner state
  equality, and dynamic precedence replacement.
- `ts_stack_has_advanced_since_error`: total bytes and error cost while walking
  links.
- External-token handling in parser shift/reduce paths still needs last external
  token/scanner state to remain equivalent.
- Merged candidate selection still needs full tree comparison when ambiguity is
  present, but merged groups are rare in normal parsing.

Implication: a pending-reduction descriptor is viable only if it stores or lazily
computes the same summary metadata as `SubtreeHeapData`, plus enough identity to
materialize before child iteration, external scanner state comparison, tree
comparison, or final tree output. This is not a small special case. The design
must introduce a stack-link payload abstraction or a descriptor-backed `Subtree`
query layer; otherwise the first forced metadata query will materialize the node
immediately and lose the intended phase removal.

### Candidate Ranking

| Rank | Direction | Decision |
| ---: | --- | --- |
| 1 | Multi-phase reduce protocol redesign | Back to the top implementation direction. Lexer counters ruled out included-range/chunking work and showed external scanners are not universal. Future reduce work must remove more than the builder-to-arena copy: summary, stack-push, or graph-candidate materialization must move too. |
| 2 | Lexer/runtime boundary investigation | Measurement remains useful, but not yet an implementation direction. Only pursue code if a flamegraph isolates reusable runtime work beyond generated `ts_lex`, keyword lexing, external scanner bodies, included-range checks, or chunk reads. |
| 3 | Balancing/compress redesign | Important for Rust and moderate for JS/TS/Go/Python. Do not remove balancing; contains-repetition pruning and single-pass compression both regressed or failed to improve Rust. Only consider a correctness-preserving redesign with tree-shape evidence, not schedule tuning. |
| 4 | Summarization during reduce construction | Only after a real builder changes selection/ownership. A direct arena copy-plus-summary loop regressed and is closed. |

### Next Direction Queue

1. Multi-phase reduce protocol redesign.
   - Goal: remove multiple reduce phases together: candidate materialization,
     builder-to-arena copying, separate child summarization, and repeated
     parent/trailing-extra stack pushes or immediate internal-parent
     materialization.
   - Why first: reduce is still the largest shared parser-owned frame, and
     lexer counters ruled out easy reusable lexer surfaces. The latest reduce
     storage trial proved that removing only one copy is insufficient, not that
     reduce is no longer the broad parser-core target.
   - Tooling: `cargo flamegraph` for post-builder samples, temporary counters
     for pending reduction metadata, stack-push batches, and summary fallback
     causes, same-session seven-language benchmarks.
   - Reject if it is a linear-only direct-storage path, buffer adoption, direct
     graph collection, candidate-selection-only change, or summary-loop rewrite.

2. Lexer/runtime boundary investigation.
   - Goal: determine whether the core runtime has a real library-owned lexer
     optimization surface after generated lexer and external scanner costs are
     separated.
   - Why second: C++ and JavaScript show large lexer/scanner shares, but the
     new counters ruled out included-range stepping and chunking. External
     scanner work is not universal, and prior core lexer ASCII/direct UTF-8
     fast paths failed.
   - Tooling: `cargo flamegraph`, temporary counters around
     `ts_lexer__get_lookahead`, `ts_lexer__do_advance`, included-range handling,
     generated `ts_lex`, and external scanner calls.
   - Reject if the remaining cost is dominated by generated grammar code or
     external scanners rather than reusable runtime code.

3. Balance/compress redesign.
   - Goal: reduce post-parse tree balancing/compression cost without removing
     correctness-preserving balancing.
   - Why third: Rust has the largest balance share, but previous pruning and
     schedule trials did not improve it.
   - Tooling: temporary tree-shape counters for repeat depth, child counts,
     compression depth, and balancing call outcomes; then Rust-first canaries
     followed by the full seven-language benchmark matrix.
   - Reject if the idea is only schedule tuning, branch pruning metadata, or
     skipping balancing.

4. Summary computation revisit.
   - Goal: reduce child-summary cost only after the reduce-builder protocol
     changes candidate ownership enough to avoid repeating closed loops.
   - Why fourth: direct summary-loop rewrites and builder-specific copy plus
     summary finalization already regressed at least one target language.
   - Tooling: use flamegraph evidence from the accepted reduce-builder shape,
     not the old `ts_subtree_new_node_in_arena` path.
   - Reject unless the new builder eliminates candidate parent construction or
     otherwise changes the ownership protocol materially.

### Next Parser Trial

The next parser-core trial should be a multi-phase reduce design measurement,
not a direct-storage implementation:

- Hypothesis: the remaining reduce opportunity requires removing repeated
  internal-parent materialization. Parent-plus-trailing-extra push batching is
  too narrow, but reductions almost always pop an already-materialized internal
  subtree soon after creating one.
- History check: direct graph collection, direct arena finalization, one-pass
  linear final storage, summary-loop rewrites, and candidate-selection-only
  changes are closed as standalone reduce directions.
- Evidence status: reduce remains the largest shared parser-owned frame, while
  lexer counters ruled out included-range/chunking and showed external scanner
  work is not universal.
- Kill criteria: reject any trial that is linear-only direct storage, buffer
  adoption, graph collection alone, summary-loop rewrite alone, or a branch-only
  fast path.
- Implementation boundary if evidence passes: design a pending-reduction
  descriptor that exposes subtree metadata lazily and materializes only when
  tree identity, child iteration, external scanner state, comparison, or final
  tree output requires it.

### Reduce-Construction Redesign Sketch

Current protocol:

1. `ts_stack_pop_count` materializes one or more `StackSlice` values.
2. Each `StackSlice` owns a malloc-backed `SubtreeArray`.
3. `ts_parser__reduce` removes trailing extras by shrinking that array and
   moving extras into parser scratch arrays.
4. `ts_parser__new_node` copies the selected children into the tree arena and
   deletes the malloc-backed array.
5. `ts_subtree_summarize_children` walks the copied children to fill parent
   metadata.
6. Merged slices may allocate scratch nodes via `ts_parser__select_children`
   just to choose the best candidate.
7. The selected parent and trailing extras are pushed back onto the stack.

Design target:

- Replace the `StackSliceArray` plus malloc-backed `SubtreeArray` protocol with
  a parser-owned reduce-construction context.
- The stack should report candidate slices into that context instead of owning
  child arrays.
- The context should represent each candidate as a child span plus metadata,
  support graph fallback, support merged-slice candidate selection, and allocate
  the final parent once after selection.
- Summary metadata may be computed while finalizing the candidate only if the
  builder changes the candidate-selection/ownership protocol first. A direct
  copy-plus-summary rewrite of `ts_subtree_new_node_in_arena` regressed.

Required properties:

- Handles both linear and graph stack pops.
- Preserves merged-slice selection semantics from `ts_parser__select_tree`.
- Preserves trailing-extra push order.
- Preserves retain/release behavior for children that are inspected but not
  selected.
- Does not reuse adopted malloc buffers.
- Does not create a linear-only fast path.
- Does not special-case zero-child or no-alias nodes.

Sketch:

```text
ReduceBuilder
  scratch children storage owned by TSParser
  candidate descriptors: stack version, base node, child span, trailing extras
  candidate summary metadata for selection

ts_stack_collect_pop_slices(stack, version, count, builder)
  walks linear and graph paths
  appends children into builder spans
  records destination stack node/version
  leaves ownership cleanup with builder

ts_parser__reduce
  asks stack to collect candidates into builder
  rejects versions over the max-version limit
  selects the winning candidate per destination version
  allocates exactly one arena parent for each winner
  fills child storage and summary metadata from the builder span
  pushes parent and trailing extras
  merges stack versions
  clears builder scratch state
```

Equivalence map for the next code slice:

- Collection equivalence:
  - A collected candidate must contain the same retained subtrees, in the same
    parser-order child sequence, as the current `StackSlice`.
  - The candidate destination version must be the version created for the same
    revealed stack node that `ts_stack__add_slice` would use.
  - Linear and graph paths must produce identical candidate ordering within each
    destination version; merged-candidate selection depends on that ordering for
    stable ties.
  - Reparses remain on `ts_parser__reduce_with_slices` until the new protocol is
    proven against changed-range tests.

- Trailing-extra equivalence:
  - Candidate children are the prefix remaining after removing trailing extras.
  - Trailing extras are the removed suffix, pushed after the parent in the same
    order as the current `trailing_extras` arrays.
  - Losing candidates release the entire retained collected span, including
    trailing extras. Winning candidates transfer ownership of the child prefix to
    the final parent and transfer ownership of trailing extras to the stack.

- Selection equivalence:
  - For a destination version with one candidate, allocate exactly one parent.
  - For merged candidates, do not allocate the final parent until the winner is
    known.
  - Candidate comparison must preserve `ts_parser__select_tree`: lower
    `error_cost`, higher `dynamic_precedence`, error-cost tie behavior, and then
    recursive tree-shape comparison.
  - A descriptor comparator is only valid if it compares the same synthetic
    parent shape that `ts_parser__select_children` currently constructs. The
    earlier standalone descriptor-comparison trial is closed because it regressed
    the TypeScript aggregate.

- Finalization equivalence:
  - The final parent uses the same symbol, production id, child sequence,
    summary metadata, dynamic-precedence adjustment, `extra` marking for
    `end_of_non_terminal_extra`, fragility flags, and parse state.
  - The stack push order remains parent first, then trailing extras, followed by
    the existing version-merge loop.
  - Builder cleanup must not release children after ownership has transferred to
    the final parent or stack.

Consequence of the direct graph-builder trial:

- Do not implement graph fallback collection as an isolated patch. It regressed
  Go even though Go had the highest graph fallback rate.
- Graph collection should only change as part of a broader protocol that also
  removes candidate parent construction, child-span conversion, or finalization
  work.

Implementation slices if this design survives review:

1. Add the builder data types behind parser-private APIs with no behavior
   change.
2. Add collection instrumentation/tests for candidate equivalence against
   `ts_stack_pop_count`.
3. Route graph and linear collection through the builder while still creating
   normal `SubtreeArray` nodes, proving semantic equivalence first.
4. Move final parent allocation to consume selected builder spans directly.
5. Revisit summary computation only after the builder path is correct and
   benchmark-positive. Do not repeat the direct copy-plus-summary loop.

Pre-implementation rejection criteria:

- If the first implementation slice needs a linear-only branch to be useful,
  reject it.
- If candidate selection still requires constructing every candidate as a full
  subtree in normal cases, reject it.
- If ownership requires overloading `SubtreeArray.capacity` or adopting malloc
  blocks, reject it.
- If the design cannot explain how it helps Go graph fallback and C++ lexer-heavy
  profiles, treat it as insufficient for the 20% target and pair it with a
  separate lexer/balance direction before implementation.

## Parser Acceptance Gate

Before keeping any new library optimization:

- Run same-session `cargo xtask benchmark --kind normal -r 10 --language` for
  all seven target languages.
- Reject a change if any target language has an average regression or meaningful
  worst-file regression, unless the universal explanation is strong and the net
  gain is clearly large.
- Run `cargo test --all` outside the sandbox before committing kept library
  code.
- Record failed trials here immediately with direction and canary numbers.

## Parser Tooling

Primary profiler:

```sh
cargo flamegraph --release -o /tmp/tree-sitter-js-jquery-flamegraph.svg -- \
  /Users/hd/code/test/tree-sitter/test/fixtures/grammars/javascript/src \
  /Users/hd/code/test/tree-sitter/test/fixtures/grammars/javascript/examples/jquery.js \
  1000
```

Run from `/tmp/ts-raw-profile-harness`.

Useful environment:

```sh
TREE_SITTER_HARNESS_SCRATCH=/tmp/ts-raw-profile-harness-cache
CARGO_NET_OFFLINE=true
```

Secondary profiler:

```sh
/usr/bin/sample <pid> <seconds> -file /tmp/sample.txt
```

Validation:

```sh
cargo test --all
```

Validation must run outside the sandbox.

## Parser Process Rules

- Do not edit benchmark source code.
- Do not use `cargo check` as validation.
- Check this trial history and relevant commit history before writing any
  optimization code.
- Do not implement near-duplicate attempts unless new profiler evidence directly
  contradicts the old result.
- Commit each kept optimization separately.
- Push after every 10 additional commits, unless explicitly asked otherwise.
- After every 10 performance attempts, write a reflection before the next code
  experiment.
