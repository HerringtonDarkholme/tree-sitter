# Candidate C: Arena-Relative Subtree Index

## Outcome

Candidate C is implemented in the Rust core. `Subtree` remains an eight-byte
inline-or-reference value, but its heap arm is now a physical byte index in the
owning `SubtreeArena`, not a process pointer. Every heap access resolves the
record as `arena_data_base + index`.

This is intentionally a representation experiment, not a compacting collector.
It makes relocation of the entire arena base safe without rewriting handles and
establishes the storage-domain plumbing required by a future collector.
Compacting records *within* the arena would still require rewriting physical
indexes.

## Data layout

The handle retains the existing low-bit inline tag:

```text
8-byte Subtree word

inline leaf                         heap-backed subtree
┌─────────────────────────┐         ┌──────────────────────────┐
│ packed SubtreeInlineData│         │ arena-relative byte index│
│ low bit = 1             │         │ low bit = 0              │
└─────────────────────────┘         └──────────────────────────┘

index 0 = NULL_SUBTREE
heap record address = arena_data_base + index
```

Arena allocation begins at `align_of::<SubtreeHeapData>()`, leaving index zero
as a permanent null sentinel and ensuring every real heap index has a clear low
tag bit. The arena remains demand-backed: virtual capacity is reserved up front,
while physical memory is committed in 64 KiB increments as the bump cursor
advances.

Internal nodes preserve the existing variable-record layout:

```text
arena_data_base
      │
      ├── heap leaf: [SubtreeHeapData]
      │
      └── internal:  [child index/inline handles ...][SubtreeHeapData]
                                                        ▲
                                                        └─ handle index
```

The physical index points to the `SubtreeHeapData` header. For an internal node,
the child slice is recovered by subtracting `child_count * 8` bytes from that
header. Child handles are therefore also inline values or indexes in the same
arena domain.

## Resolution and ownership paths

An index has no meaning without its arena. The implementation obtains that
storage domain from the owner already present on each runtime path:

| Path | Arena source | Important consequence |
|---|---|---|
| Parser allocation, reductions, token cache | `TSParser.tree_pool` | Parser-private handles resolve in the active generation |
| GLR stack links and deterministic window | `Stack.subtree_pool` | Stack nodes remain separate; only subtree values are indexes |
| Published tree root | `TSTree.arena` | Tree copies share the arena and resolve the same indexes |
| Public `TSNode` | Its owning `TSTree` | ABI and occurrence identity remain unchanged |
| `TreeCursor` and query cursor | Cursor's owning `TSTree` | Navigation resolves child indexes through the tree domain |
| Changed-range comparison | Separate old/new arenas | Equal numeric indexes from different arenas are not interchangeable storage |
| Tree edit and copy-on-write | Pool backed by `TSTree.arena` | Clones remain in the edited tree's domain |

No thread-local arena, global index registry, or hidden arena identity is stored
in the handle. The domain dependency is explicit at heap access sites.

## Allocation, release, and reuse

Allocation and logical ownership are otherwise unchanged:

| Mechanism | Candidate C behavior |
|---|---|
| Inline leaf | Entirely in the handle; no arena record or refcount |
| Heap leaf | Bump-allocated record; handle stores its byte index |
| Internal subtree | Coallocated child handles and header; header index is returned |
| Copy-on-write clone | New arena record; child handles are retained as before |
| Refcount reaches zero | Child ownership cascades and external scanner bytes are released; arena bytes become dead but are not individually freed |
| Whole-generation reuse | Bump cursor rewinds when the parser is the arena's sole owner |
| Published tree still alive | Parser detaches and lazily creates another arena for the next parse |

Temporary scratch nodes required one Candidate-C-specific correction. A scratch
header cannot live in a generic `malloc` buffer because no valid arena index can
name it. The parser now rebinds its non-owning scratch child array to the current
arena before constructing the temporary header. The ast-grep gate exercised and
validated this path in C++, Elixir, and Swift grammars.

Large external-scanner byte strings remain separate `malloc` allocations. Their
owning leaf record is indexed in the arena, but moving those byte strings into
the arena was not part of this representation-axis experiment.

## Public identity and movement

`TSNode.id` still points to a `Subtree` occurrence slot in `TSTree.root` or a
parent's child array. Candidate C does not change the public ABI or equality
rules.

The index representation permits the arena's entire data base to move: handles
do not change because their byte offsets remain the same. In-arena compaction is
not implemented. Such compaction would change physical indexes and must rewrite
parser roots, stack-held handles, child slots, token caches, and other registered
internal roots. Caller-held `TSNode` occurrence pointers remain a separate
publication constraint, as described in `SUBTREE_ARENA_PLAN.md`.

## Performance

Command: `cargo xtask perf-gate --offline`, with TypeScript repeated using
`--min-sample-time-ms 1000` because three C-core measurements in the first run
exceeded the 5% CV gate. The longer TypeScript run stayed below 5% for every
fixture.

The table reports the geometric mean of per-fixture median Rust/C throughput
ratios. Positive values favor Rust.

| Language | Fixtures | Rust vs C | Rust peak RSS | C peak RSS |
|---|---:|---:|---:|---:|
| C++ | 4 | +5.68% | 9.56 MiB | 11.09 MiB |
| Go | 5 | +6.25% | 15.83 MiB | 13.70 MiB |
| Java | 4 | +5.95% | 8.31 MiB | 8.48 MiB |
| JavaScript | 2 | +11.71% | 21.48 MiB | 21.31 MiB |
| Python | 12 | +12.71% | 12.14 MiB | 10.59 MiB |
| Rust | 2 | +10.02% | 12.36 MiB | 12.45 MiB |
| TypeScript, 1 s samples | 11 | +12.30% | 20.23 MiB | 19.86 MiB |

Equal weighting per language gives Rust **+9.19%**. Because the direct-pointer
arena and Candidate C were not preserved as separately benchmarkable commits,
this run does not claim a paired index-versus-pointer delta. A future
representation comparison should commit both endpoints before measuring.

### Throughput conclusion

Arena-relative indexes do **not** show a throughput benefit over the
direct-pointer arena. The earlier window-plus-direct-pointer-arena result was
about **+12.25% versus C**, while Candidate C measured roughly **+9–10% versus
C** after accounting for the stable TypeScript rerun. These were separate,
unpaired sessions, so the size of the apparent regression is not reliable, but
there is no evidence that replacing pointers with `arena_base + index` made the
parser faster. Treat Candidate C as relocation/compaction infrastructure, not a
throughput optimization; a future paired comparison may determine whether its
extra resolution work is neutral or slightly costly.

The memory pattern is essentially unchanged from the bump arena: Go remains the
clearest adverse RSS case because refcount-zero records are reclaimed only at a
whole-generation rewind.

## Packed metadata follow-up

The arena byte index currently occupies an eight-byte `Subtree` heap arm even
though the practical index can be encoded in 32 bits. A follow-up experiment
used the other 32 bits to cache fields frequently read while reducing:

- symbol (16 bits);
- child count with an overflow sentinel (8 bits);
- visible, named, extra, changed, missing, and keyword flags (6 bits).

The low half used an alignment-scaled arena reference so the existing low-bit
inline tag remained available. Mutations of cached fields updated both the
authoritative heap header and the handle copy.

### Rust-only A/B/A result

The comparison baseline for this experiment is the immediately preceding Rust
index-only implementation, not the C core. C uses a materially different
allocator, representation, and reduction path and therefore cannot determine
whether caching fields in the Rust handle helps.

The packed implementation and index-only implementation were measured on the
nine reduction-heavy Go and Java perf-gate fixtures. The packed implementation
lost on every fixture. Relative to index-only Rust, its geometric-mean
throughput was **-2.31%** in the first packed run and **-2.76%** in the repeated
packed run, averaging **-2.54%**. The packed cache was therefore reverted.

### Why this was not equivalent to true inline data

| Property | True `SubtreeInlineData` | Packed arena index |
|---|---|---|
| Complete subtree state | Yes, for a constrained leaf | No; only selected metadata |
| Arena resolution | Never | Still required for the header and children |
| Heap record and allocation | None | Unchanged |
| Refcount and release traversal | None | Unchanged |
| Child storage | Implicitly empty | Still arena-backed |
| Metadata ownership | One authoritative value | Duplicated between handle and header |

True inline data succeeds because it represents only small leaves. Its
constraints make child count, descendant counts, dynamic precedence, external
scanner state, and most error state implicit. Size, padding, symbol, parse
state, lookahead, and flags then fit in the complete eight-byte value. An
arbitrary internal node cannot use those assumptions: it must retain identity,
child storage, ownership, and aggregate metadata.

The packed experiment cached symbol, child count, and six flags, but
`subtree_summarize_children` still needed each child's padding, size, lookahead,
error cost, dynamic precedence, visible descendant and child counts, external
token/scanner flags, column dependence, repeat depth, and sometimes its child
list. `stack_push` then needed error cost, total size, node count, and dynamic
precedence. The heap header was consequently resolved anyway. Because the
header was already needed and hot, the cache did not remove the memory
dependency; it added index decoding, masks and shifts, cache-population work,
register pressure, and mutation-coherence work.

### Consequence for the design space

The unused handle bits do not make arbitrary internal nodes "almost inline."
A genuinely inline internal node would have to eliminate the heap object and
its lifetime operations, which cannot encode arbitrary child relationships and
exact metadata in eight bytes.

Do not retry the broad upper-half metadata cache. The next representation-local
experiment, if pursued, should resolve each child index once in
`subtree_summarize_children` and operate through a reduction-local resolved view
or summary. That preserves one authoritative heap header and targets repeated
index resolution without duplicating state. A much narrower cached predicate,
such as `extra` for loops that otherwise need no heap fields, is separately
testable but should not be treated as an inline-node representation.

## Reduction-local resolved summary

The recommended resolved-view experiment was subsequently implemented. At the
start of each `subtree_summarize_children` iteration, the child is classified as
inline or heap-backed. A heap handle performs `arena_base + index` once, and the
fields used by that reduction are copied into a local
`ReductionSubtreeSummary`. The parent-aggregation loop then reads the local
summary rather than repeatedly resolving the handle while interleaving writes
to the parent header.

This changes no persistent representation. The eight-byte handle, arena
record, child layout, allocation, refcounting, and mutation paths remain the
same. Inline leaves are expanded into the same summary using their existing
implicit-zero rules. Heap data remains the single authoritative persistent
copy; the summary exists only for one loop iteration.

### Rust-only full-corpus comparison

The kept resolved-summary endpoint was compared directly with the immediately
preceding Rust index-only implementation using `cargo xtask perf-gate
--offline`. The table uses the geometric mean of raw Rust throughput ratios for
the same fixtures; C measurements are deliberately excluded from this design
comparison.

| Language | Fixtures | Resolved summary vs index-only Rust |
|---|---:|---:|
| C++ | 4 | +0.48% |
| Go | 5 | +2.37% |
| Java | 4 | +3.06% |
| JavaScript | 2 | +0.27% |
| Python | 12 | +1.51% |
| Rust | 2 | +3.34% |
| TypeScript | 11 | +2.75% |
| **All fixtures** | **40** | **+2.04%** |

Equal weighting per language gives **+1.96%**. Every language-level result is
positive, and every individual endpoint measurement remained below the 5% CV
gate. The earlier Go/Java A/B/A screening also favored the resolved summary by
an average of +0.96% overall, with Go positive and Java effectively flat,
before the complete corpus produced the stronger paired result above.

This result supports the narrower diagnosis from the packed-cache failure:
arena indexes were being resolved repeatedly, but persisting duplicate metadata
inside every handle was the wrong remedy. A short-lived complete summary lets
the compiler keep child values independent of parent writes, avoids coherence
work, and pays no persistent handle-decoding penalty outside reduction.

## Candidate D: four-byte uniform physical indexes

Candidate D was implemented on top of the kept reduction-local summary. The
parser-facing `Subtree` and `MutableSubtree` handles are four-byte physical byte
indexes for every node. Index zero is null. The low bit classifies the target:

```text
0                         null
nonzero index, low bit 1  compact 8-byte leaf record
nonzero index, low bit 0  full leaf or internal SubtreeHeapData
```

Compact leaves preserve the former packed `SubtreeInlineData` fields, but the
eight bytes now live in the arena instead of directly in every copied handle.
They remain non-refcounted and are cloned before mutation to preserve the old
value semantics. Full leaves and all internal nodes retain their intrusive
reference counts.

Internal children remain coallocated with the parent. Four-byte handles make
an odd-sized child prefix insufficiently aligned for the heap header, so the
physical layout explicitly rounds the child prefix to the header alignment:

```text
[u32 child handles][0 or 4 bytes padding][aligned SubtreeHeapData]
```

The header computes its child address by subtracting the rounded prefix size.
This preserves the existing child-buffer transfer optimization rather than
forcing every reduction to allocate and copy a second child block.

### Rust-to-Rust throughput and RSS

The immediately preceding eight-byte hybrid-index implementation and Candidate
D were measured from separate preserved Rust benchmark binaries in the same
session. Both endpoints used five repetitions, a 250 ms minimum sample time,
and the same 40 fixtures across all seven perf-gate languages. The maximum CV
was 1.96% for the control and 2.74% for Candidate D; no fixture exceeded the 5%
gate.

| Language | Fixtures | Candidate D throughput | Control RSS | Candidate D RSS | RSS delta |
|---|---:|---:|---:|---:|---:|
| C++ | 4 | +1.26% | 11.22 MiB | 11.06 MiB | -0.16 MiB |
| Go | 5 | +3.88% | 15.84 MiB | 15.80 MiB | -0.05 MiB |
| Java | 4 | +2.74% | 8.42 MiB | 8.34 MiB | -0.08 MiB |
| JavaScript | 2 | +4.38% | 21.44 MiB | 21.58 MiB | +0.14 MiB |
| Python | 12 | +2.68% | 12.12 MiB | 12.22 MiB | +0.09 MiB |
| Rust | 2 | +0.96% | 12.47 MiB | 12.52 MiB | +0.05 MiB |
| TypeScript | 11 | -0.23% | 21.89 MiB | 21.67 MiB | -0.22 MiB |
| **All fixtures** | **40** | **+1.88%** |  |  |  |

Equal weighting per language gives **+2.23%**. Six languages improved;
TypeScript's -0.23% remained within ordinary pair noise. RSS was effectively
neutral. Candidate D therefore clears the throughput gate and is retained.

## Global child-range experiment

A second experiment separated Candidate D's child handles from their parent
headers. Internal metadata stored `{child_start: u32, child_count: u32}` and
resolved children as a consecutive range in the same virtual arena. Node
records grew upward from the arena base while a child-reference domain grew
downward from the end; both sides committed pages on demand.

This endpoint was compared directly with Candidate D using the same 40
fixtures, five repetitions, and 250 ms minimum sample time. Its maximum CV was
4.75%, still within the 5% gate.

| Language | Fixtures | Global ranges vs D | D RSS | Global-range RSS | RSS delta |
|---|---:|---:|---:|---:|---:|
| C++ | 4 | +1.63% | 11.06 MiB | 11.61 MiB | +0.55 MiB |
| Go | 5 | +0.24% | 15.80 MiB | 22.12 MiB | +6.33 MiB |
| Java | 4 | -0.26% | 8.34 MiB | 8.58 MiB | +0.23 MiB |
| JavaScript | 2 | -1.58% | 21.58 MiB | 33.20 MiB | +11.62 MiB |
| Python | 12 | +0.61% | 12.22 MiB | 15.03 MiB | +2.81 MiB |
| Rust | 2 | +1.02% | 12.52 MiB | 15.67 MiB | +3.16 MiB |
| TypeScript | 11 | -1.50% | 21.67 MiB | 31.22 MiB | +9.55 MiB |
| **All fixtures** | **40** | **-0.09%** |  |  |  |

Equal weighting per language is **+0.02%**: no throughput benefit. The large
RSS increase has a specific source. Candidate D transfers an arena-backed
temporary child buffer directly into the parent's coallocated record. A
separate global range must instead copy those handles into a second allocation;
because the arena is bump-only, the temporary source buffer also remains live
physically until the whole arena rewinds. The experiment therefore retained
both copies and added a dependent child-range lookup.

The global-range layer was reverted. It should not be retried with the same
construction path. A materially different retry must construct children in
their final range from the start, or use scratch storage that is actually
reused rather than retained in the subtree arena.

### Direct-to-final global-range retry

The materially different retry was implemented. Internal nodes stored a
half-open `[child_start, child_end)` range of four-byte Candidate D handles in a
global child-index domain. Node/leaf records grew upward from the start of the
same virtual arena and child ranges grew downward from its end; both directions
committed pages only on demand.

Deterministic-window reductions know their exact physical child count. They
reserved the final global range first and moved the existing handles directly
into it. A compact leaf's eight-byte record was not copied or rebuilt: only its
four-byte tagged arena index moved into the final range.

GLR path enumeration can contain an unknown number of extras and can produce
several candidate child lists before ambiguity selection chooses one. Those
candidates were kept in separately reclaimable buffers carrying the correct
subtree arena for retain/release. Only the selected list was copied into a
permanent global range. This avoided retaining both temporary and final buffers
in the bump-only subtree arena. A first prototype incorrectly conflated buffer
storage with the subtree-resolution arena; full parity exposed that bug as a
null-arena retain, after which the domains were separated and all 15 parity
samples passed.

The fixed endpoint was compared directly with retained Candidate D over the
same 40 fixtures, five repetitions, and 250 ms minimum sample time:

| Language | Fixtures | Direct-final ranges vs D | D RSS | Direct-final RSS | RSS delta |
|---|---:|---:|---:|---:|---:|
| C++ | 4 | -4.12% | 11.06 MiB | 11.16 MiB | +0.09 MiB |
| Go | 5 | -5.81% | 15.80 MiB | 15.36 MiB | -0.44 MiB |
| Java | 4 | -5.54% | 8.34 MiB | 8.42 MiB | +0.08 MiB |
| JavaScript | 2 | -1.17% | 21.58 MiB | 21.17 MiB | -0.41 MiB |
| Python | 12 | -2.61% | 12.22 MiB | 10.62 MiB | -1.59 MiB |
| Rust | 2 | -4.76% | 12.52 MiB | 12.42 MiB | -0.09 MiB |
| TypeScript | 11 | +0.28% | 21.67 MiB | 21.69 MiB | +0.02 MiB |
| **All fixtures** | **40** | **-2.72%** |  |  |  |

Equal weighting per language was **-3.41%**. One endpoint reached 6.62% CV,
above the nominal 5% noise gate, but the regressions were broad and much larger
than that isolated instability. Direct construction fixed the earlier RSS
amplification, proving that retained temporary buffers caused the first
experiment's memory growth. It did not fix throughput: the extra child-domain
address calculation and loss of parent/child coallocation and locality dominate
the common reduction/traversal paths.

The direct-final global-range layer was also reverted. Pooling GLR candidate
buffers or using a reduction-scoped scratch arena could remove fallback
`malloc/free`, but deterministic-window reductions did not use those buffers
and still participated in the broad regression. Candidate-buffer reuse is
therefore not expected to recover the measured loss by itself.

#### Regression attribution: candidate malloc is not the primary cause

The GLR fallback's candidate-buffer `malloc/free` was not separately measured
against a pooled-buffer variant, so its cost is not proven to be zero. The
experiment does, however, rule it out as the principal explanation:

- deterministic-window reductions allocated their exact final global child
  range directly and never used the malloc-backed candidate path;
- the largest regressions occurred in Go (-5.81%), Java (-5.54%), Rust
  (-4.76%), and C++ (-4.12%), where the deterministic path is important; and
- conflict-heavy TypeScript, which has more opportunity to exercise generalized
  bookkeeping, was the only positive language result at +0.28%.

The common-path representation changed more substantially. Candidate D needs
one transferable allocation containing `[children][padding][header]`.
Direct-final global ranges need one reservation for the child range and another
for the standalone header. They also place the two on distant arena pages and
add a global-range address calculation to every child traversal. The measured
pattern is therefore most consistent with the extra reservation, lost
parent/child locality, and additional child-domain lookup—not fallback candidate
allocation.

A reusable candidate pool or reduction-scoped scratch arena remains a valid
fallback cleanup, but it should not be presented as a likely recovery of the
global-range throughput loss without a new isolated measurement.

## Refcount synchronization and copying-collection experiments

Three temporary endpoints priced the remaining ownership/reclamation axes
against retained Candidate D and were reverted. A fourth, phase-split endpoint
was retained after measurement.

### Synchronization-free refcount ceiling

A literal removal of subtree counts is not behavior-preserving: `make_mut`,
tree editing, keyword-token mutation, recovery mutation, and final repeat-tree
balancing use `ref_count == 1` as their copy-on-write uniqueness oracle. The
controlled experiment therefore retained the exact count and all ownership
decisions, but replaced subtree `fetch_add`/`fetch_sub` operations with
single-threaded relaxed load/store pairs. Arena-owner counts remained atomic.

This endpoint is deliberately not safe for concurrent copies/deletions of the
same tree. It is a parse-throughput ceiling for removing atomic read-modify-
write synchronization, not a proposed production ownership model. All 16 core
tests and all 15 core-parity samples passed before measurement.

The control and experimental Rust benchmark executables were preserved and run
over the same 40 fixtures with five repetitions and a 250 ms minimum sample
time. Maximum CV was 4.03% for the control and 2.20% for the experiment.

| Language | Fixtures | Relaxed count vs atomic control | Control RSS | Relaxed-count RSS | RSS delta |
|---|---:|---:|---:|---:|---:|
| C++ | 4 | +2.18% | 11.12 MiB | 11.05 MiB | -0.08 MiB |
| Go | 5 | +1.40% | 15.48 MiB | 15.67 MiB | +0.19 MiB |
| Java | 4 | +2.21% | 8.33 MiB | 8.34 MiB | +0.02 MiB |
| JavaScript | 2 | -2.32% | 21.58 MiB | 21.58 MiB | 0.00 MiB |
| Python | 12 | +0.80% | 11.95 MiB | 11.94 MiB | -0.02 MiB |
| Rust | 2 | +5.47% | 12.41 MiB | 12.34 MiB | -0.06 MiB |
| TypeScript | 11 | +0.97% | 22.02 MiB | 21.94 MiB | -0.08 MiB |
| **All fixtures** | **40** | **+1.27%** |  |  |  |

RSS was neutral. The modest and uneven throughput result says that atomic
subtree counts have a measurable possible cost, but they are not a large
remaining throughput axis by themselves. Complete refcount removal would also
need a replacement uniqueness model and therefore cannot claim this entire
1.27% as an immediately recoverable production gain.

### Literal count-field removal

A follow-up removed `SubtreeHeapData.ref_count` entirely. Retain became an
idempotent atomic `shared = true` mark, release became a no-op, and copy-on-write
cloned any record ever observed as shared. Long external-scanner states were
temporarily moved into the arena so count-free release could not leak their
standalone allocations. The marker remained atomic so concurrent readers did
not race on the record.

The first endpoint appeared extremely promising: an A/B/A comparison against
the atomic-count control measured **+8.28%** across all 40 fixtures, with every
language positive. That number is invalid as a performance claim. The sticky
sharing mark did not become unique again after temporary parse-stack owners
were released. Final repeat-tree balancing therefore treated the accepted root
as shared and skipped work that the exact-count implementation performs. The
full test suite exposed this as a progress-callback behavior failure in
`test_parsing_with_timeout_during_balancing`.

To restore semantics, a parser-private accepted-DAG pass replaced stale sharing
marks with exact sharing information immediately before balancing. This made
the timeout test pass, but added one complete accepted-tree traversal. The
corrected endpoint was measured against the geometric mean of the same two A
controls:

| Language | Fixtures | Count-free, exact balancing | Control RSS A/A2 | Count-free RSS |
|---|---:|---:|---:|---:|
| C++ | 4 | +1.53% | 11.12 / 11.09 MiB | 11.09 MiB |
| Go | 5 | +0.62% | 15.48 / 14.23 MiB | 15.66 MiB |
| Java | 4 | -0.72% | 8.33 / 8.31 MiB | 8.36 MiB |
| JavaScript | 2 | -0.93% | 21.58 / 21.58 MiB | 21.66 MiB |
| Python | 12 | -0.33% | 11.95 / 12.05 MiB | 12.14 MiB |
| Rust | 2 | +1.81% | 12.41 / 12.34 MiB | 12.36 MiB |
| TypeScript | 11 | -1.52% | 22.02 / 21.98 MiB | 21.66 MiB |
| **All fixtures** | **40** | **-0.32%** |  |  |

Maximum CV for the corrected run was 2.32%. RSS was neutral within run-to-run
variation. Removing the four-byte count also did **not** shrink the allocation:
`SubtreeHeapData` remained 88 bytes because its remaining fields and eight-byte
alignment still round to the same size. At least six additional bytes would
have to be removed or packed before this record could fall to 80 bytes.

The apparent +8% was therefore the cost of omitted balancing, not the cost of
reference counting. With equivalent balancing semantics, this single atomic
sharing-oracle design was throughput-neutral to slightly negative and provided
no RSS benefit. It was reverted before testing the phase-split design below.

### Parser-private sharing and published atomic sharing

The successful follow-up separated the two synchronization domains instead of
using one atomic marker everywhere:

- while the arena is parser-private, each heap record has a `Cell<bool>`
  sharing marker; retain sets it, release is a no-op, and copy-on-write treats a
  set marker conservatively;
- immediately before balancing, one accepted-DAG traversal replaces stale
  parse-time marks with exact sharing marks, so balancing work and cancellation
  behavior remain unchanged;
- publication freezes the parser-only cells and switches accessors to a
  separate relaxed `AtomicBool`, preserving concurrent published-tree copying
  and copy-on-write;
- the arena's atomic owner count remains the physical lifetime authority, and
  long external-scanner states live in the arena so no per-record destructor is
  required;
- when the parser is again the sole arena owner, rewind resets the arena to its
  parser-private phase.

This removes subtree atomic read-modify-write operations and release cascades
from parsing without using non-atomic state after publication. The candidate
was measured between two runs of the preserved exact-refcount Rust executable.
Each fixture compares candidate throughput with the geometric mean of those
bracketing controls.

| Language | Fixtures | Phase split vs refcount | Control RSS pre/post | Phase-split RSS |
|---|---:|---:|---:|---:|
| C++ | 4 | +2.75% | 11.09 / 11.08 MiB | 11.11 MiB |
| Go | 5 | +2.41% | 14.23 / 15.80 MiB | 15.55 MiB |
| Java | 4 | +1.35% | 8.31 / 8.33 MiB | 8.44 MiB |
| JavaScript | 2 | +3.67% | 21.58 / 21.50 MiB | 21.56 MiB |
| Python | 12 | +3.01% | 12.05 / 12.14 MiB | 12.11 MiB |
| Rust | 2 | +6.00% | 12.34 / 12.53 MiB | 12.38 MiB |
| TypeScript | 11 | +4.56% | 21.98 / 21.66 MiB | 21.94 MiB |
| **All fixtures** | **40** | **+3.35%** |  |  |

One Python fixture exceeded the 5% CV gate in the first candidate run. Its
one-second rerun measured 0.47% CV; after replacement, maximum candidate CV was
3.75%. RSS remained neutral. `SubtreeHeapData` also remains 88 bytes, so this
is a throughput/lifetime result rather than a record-density improvement.

This phase-split endpoint is retained. It also explains why the earlier
non-atomic exact-count ceiling understated the opportunity: the kept design
removes not only atomic synchronization, but the entire rejected-subtree
release cascade. The accepted-DAG normalization traversal is paid once, after
the GLR stack has been discarded.

### Scanner-state inline capacity reduction

The first post-phase-split throughput experiment reduced the inline serialized
external-scanner state from 24 to 16 bytes. This crossed the intended layout
boundary: a 64-bit compile-time assertion confirmed that `SubtreeHeapData`
shrank from 88 to 80 bytes. Scanner states longer than 16 bytes remained valid
and moved into the existing demand-backed arena.

The candidate and retained 24-byte control were measured in an A/B/A sequence
over all 40 fixtures. No endpoint exceeded the 5% CV gate; maximum candidate CV
was 3.57%.

| Language | Fixtures | 16-byte capacity vs control | Control RSS pre/post | Candidate RSS |
|---|---:|---:|---:|---:|
| C++ | 4 | -1.21% | 11.17 / 11.05 MiB | 11.11 MiB |
| Go | 5 | -1.98% | 15.69 / 15.83 MiB | 15.19 MiB |
| Java | 4 | +1.46% | 8.42 / 8.44 MiB | 8.36 MiB |
| JavaScript | 2 | +1.33% | 21.61 / 21.56 MiB | 20.64 MiB |
| Python | 12 | -0.17% | 12.17 / 12.09 MiB | 11.77 MiB |
| Rust | 2 | +1.16% | 12.48 / 12.55 MiB | 12.22 MiB |
| TypeScript | 11 | +1.76% | 21.98 / 22.00 MiB | 21.08 MiB |
| **All fixtures** | **40** | **+0.32%** |  |  |

The smaller record reduced peak RSS in the larger JavaScript, TypeScript, Go,
and Python runs, but did not produce a credible throughput win and regressed
the important C++/Go paths. Because this is the throughput program, the
24-byte capacity and 88-byte header were restored. The result establishes that
crossing one eight-byte header boundary is insufficient by itself; subsequent
record-shape experiments need a larger reduction or less initialization work.

### Kind-specialized internal records

The next experiment separated the common heap header from its kind-specific
tail. Full leaves retain the existing 88-byte record and scanner-state payload,
while internal nodes now use a 72-byte record containing the 48-byte common
header followed directly by the 20-byte child summary and alignment padding.
Every heap handle still resolves to the common header at offset zero, so the
four-byte Candidate D handle and arena lookup are unchanged.

`child_count` cannot discriminate these shapes: empty productions create valid
internal nodes with zero children. The first prototype used that invalid
assumption and ast-grep parity found three assertion failures. The corrected
layout consumes one previously unused heap-flag bit as an explicit internal
record-kind tag. This supports zero-child internal nodes without adding a byte
or changing either physical record size.

The corrected candidate passed the 16 core tests, the ABI surface test,
`clippy -D warnings`, 15 core-parity samples, and all four ast-grep packages
before measurement. It was compared with the committed parser-private sharing
baseline in an A/B/A run over all 40 fixtures. Each fixture uses the geometric
mean of the two bracketing controls; maximum CV was 2.61% / 1.95% / 2.03% for
control, candidate, and control respectively.

| Language | Fixtures | Specialized internal record | Control RSS | Candidate RSS |
|---|---:|---:|---:|---:|
| C++ | 4 | +1.79% | 10.38 MiB | 10.39 MiB |
| Go | 5 | +0.72% | 11.50 MiB | 11.01 MiB |
| Java | 4 | +0.18% | 8.35 MiB | 8.25 MiB |
| JavaScript | 2 | +1.04% | 21.58 MiB | 19.58 MiB |
| Python | 12 | +1.26% | 10.37 MiB | 10.06 MiB |
| Rust | 2 | -0.71% | 12.43 MiB | 11.83 MiB |
| TypeScript | 11 | +0.39% | 17.16 MiB | 15.82 MiB |
| **All fixtures** | **40** | **+0.79%** |  |  |

The throughput gain is modest but broad, no language crosses the -1% guard,
and the larger JavaScript/TypeScript fixtures show the expected RSS reduction.
The specialized record is retained as the baseline for subsequent throughput
experiments.

### Every-parse live-tree copying collection

The GC endpoint retained normal atomic refcounts during parsing and performed
a stop-the-world copying collection at the publication boundary. After parser
scratch roots were reset, it recursively copied only the accepted tree into a
fresh arena, rebuilt physical indexes, copied external scanner state, released
the old root, and left the parser as the sole owner of from-space so it could
rewind that space on the next parse. Internal DAG sharing was conservatively
expanded into separate occurrences, making every copied record uniquely owned.

This is a real reclamation endpoint but an intentionally aggressive schedule:
one complete collection after every successful parse. It measures the cost of
the simplest safe copying boundary, not an amortized pressure-triggered policy.
All 16 core tests, 15 core-parity samples, and four ast-grep packages passed.

The same atomic control binary and a separately preserved collector binary were
measured over all 40 fixtures. One short Python fixture exceeded the 5% CV gate
in the first collector run; repeating that exact pair with 1 s minimum samples
reduced CV to 0.92%/2.26%, and the replacement result is used below.

| Language | Fixtures | Copying GC vs atomic control | Control RSS | Copying-GC RSS | RSS delta |
|---|---:|---:|---:|---:|---:|
| C++ | 4 | -16.06% | 11.12 MiB | 11.33 MiB | +0.20 MiB |
| Go | 5 | -25.09% | 15.48 MiB | 17.67 MiB | +2.19 MiB |
| Java | 4 | -26.27% | 8.33 MiB | 8.41 MiB | +0.08 MiB |
| JavaScript | 2 | -19.73% | 21.58 MiB | 27.67 MiB | +6.09 MiB |
| Python | 12 | -58.61% | 11.95 MiB | 13.86 MiB | +1.91 MiB |
| Rust | 2 | -18.89% | 12.41 MiB | 14.12 MiB | +1.72 MiB |
| TypeScript | 11 | -30.24% | 22.02 MiB | 27.38 MiB | +5.36 MiB |
| **All fixtures** | **40** | **-37.45%** |  |  |  |

Peak RSS increased because from-space and the copied live tree coexist during
collection. This benchmark also deletes each returned tree before the next
iteration, so the control can already rewind its arena without collection;
copying therefore provides no memory benefit for this corpus schedule. Compact
leaves being out of line makes the collector copy every syntax occurrence,
which is especially expensive for the small Python fixtures where publication
copying becomes a large fraction of total parse work.

Do not collect unconditionally at publication. A future collector experiment
must first measure arena high water, live bytes, and estimated dead bytes, then
trigger only under demonstrated pressure. Semispace still has a two-space peak;
if rejected GLR records dominate and the accepted tree has high survival, a
non-moving sweep/free-list design is a more plausible next reclamation axis
than copying the entire live tree after every parse.

## Verification

Passed:

- `cargo fmt --check --all`
- `cargo clippy -p tree-sitter --lib --tests -- -D warnings`
- `cargo test -p tree-sitter --lib` (16/16)
- `cargo test -p tree-sitter --test abi_surface`
- `CARGO_NET_OFFLINE=true cargo xtask core-parity` (15/15 samples)
- `CARGO_NET_OFFLINE=true cargo xtask ast-grep-gate` (4/4 packages)
- all parser, node, cursor, query, corpus, and tree tests reached by
  `cargo test --all`

`cargo test --all` still reports the repository's four known
`tests::detect_language` fixture failures; the remaining 265 CLI library tests
pass. These failures are unrelated to subtree storage and match the pre-existing
baseline.
