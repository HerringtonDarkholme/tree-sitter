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

### Candidate Ranking

| Rank | Direction | Decision |
| ---: | --- | --- |
| 1 | Full reduce-construction redesign | Still the top parser-core candidate. It targets `ts_parser__reduce`, node construction, child-array allocation/copying, summarization, and stack push as one pipeline. It must not be a linear-pop fast path, buffer-adoption variant, or standalone merged-candidate selection rewrite. |
| 2 | Lexer/external scanner work | Now co-equal for C++ and material for all languages. Needs separate direction triage because generated grammar lexers may limit core-library leverage. |
| 3 | Balancing/compress redesign | Important for Rust and moderate for JS/TS/Go/Python. Do not remove balancing; contains-repetition pruning and single-pass compression both regressed or failed to improve Rust. Only consider a correctness-preserving redesign with tree-shape evidence, not schedule tuning. |
| 4 | Summarization during reduce construction | Only after a real builder changes selection/ownership. A direct arena copy-plus-summary loop regressed and is closed. |

### Next Parser Trial

The next trial should be a design sketch for the full reduce-construction
redesign, then a no-code review against closed history before implementation:

- Hypothesis: a full reduce-construction redesign is the only remaining
  parser-core direction with enough shared leverage to pair with later lexer or
  balancing work toward the 20% target.
- History check: direct linear stack-pop, stack-pop buffer adoption, child-array
  adoption, summarizer micro-optimizations, combined copy-plus-summary, page-size
  tuning, refcount ordering, and allocation pools are closed.
- Evidence status: collected for TypeScript, JavaScript, Python, Go, Rust, C++,
  and Java.
- Kill criteria: if the design collapses into a linear fast path, buffer
  adoption, or isolated summarizer tweak, do not implement it.
- Implementation boundary if evidence passes: redesign the reduce construction
  protocol as a coherent replacement for the current `StackSliceArray` to node
  construction pipeline, not as a special fast path.

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
