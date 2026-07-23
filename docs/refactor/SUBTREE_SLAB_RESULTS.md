# Contiguous Subtree Slab: Implementation and Results

> Historical endpoint: this records the direct-pointer contiguous-arena
> experiment. Candidate C subsequently replaced the heap pointer arm with an
> arena-relative physical index; see `SUBTREE_INDEX_RESULTS.md` for the current
> representation and measurements.

## Outcome

This experiment replaces per-node subtree allocation with one pointer-stable,
contiguous arena per parser/tree generation. It is an upper-bound measurement
of removing `malloc`/`free` from heap subtree records while retaining the
existing `Subtree` handle, `SubtreeHeapData` record, child layout, and intrusive
reference counts.

The arena is demand-backed. A 64-bit process reserves an 8 GiB virtual address
range as a guard ceiling, but it does **not** allocate or commit 8 GiB of RAM.
The implementation commits 64 KiB chunks only when the bump cursor reaches
them. The corresponding 32-bit ceiling is 512 MiB. On native Unix this uses
`mmap(PROT_NONE)` plus `mprotect`; Windows uses reserved and committed
`VirtualAlloc` regions. Unused virtual capacity does not contribute to RSS.

Across the 40 perf-gate fixtures, median throughput improved by **7.71% geometric
mean** versus the pre-slab branch baseline. Every fixture improved. Peak RSS was
mixed: effectively unchanged for four languages, +1.23 MiB for Python, and
+3.22 MiB for Go. This is expected for a no-reuse bump arena: records that reach
reference count zero remain committed until the whole arena can be rewound.

## Allocation layout

Inline `Subtree` values are unchanged. Heap values remain pointers; no public
handle or node-navigation representation changed.

```text
reserved virtual mapping
┌───────────────────────────────────────────────────────────────────────┐
│ 64 KiB arena header/alignment region                                 │
├───────────────────────────────────────────────────────────────────────┤
│ leaf SubtreeHeapData                                                 │
├───────────────────────────────────────────────────────────────────────┤
│ internal node: [Subtree child handles ...][SubtreeHeapData]          │
├───────────────────────────────────────────────────────────────────────┤
│ cloned internal node: [copied child handles ...][SubtreeHeapData]    │
├───────────────────────────────────────────────────────────────────────┤
│ temporary slab-backed child buffers and abandoned growth capacity    │
├──────────────────────────── bump cursor ──────────────────────────────┤
│ committed high-water space, available after the next safe rewind     │
├───────────────────────────────────────────────────────────────────────┤
│ reserved but inaccessible/uncommitted virtual address space          │
└───────────────────────────────────────────────────────────────────────┘
```

The internal-node layout is still exactly the existing combined allocation:
`child_count * size_of(Subtree)` bytes followed by one aligned
`SubtreeHeapData`. The only semantic allocation change is where this byte range
comes from.

| Entry | Before | Slab implementation | Refcount-zero behavior |
|---|---|---|---|
| Inline leaf | Packed in the 8-byte handle | Unchanged | No allocation |
| Heap leaf | One pooled or direct heap record | Bump-allocated `SubtreeHeapData` | Cascade ownership as before; record remains in slab |
| Internal subtree | One allocation containing children and header | Same byte layout, bump-allocated in the slab | Children are released as before; combined record remains in slab |
| Copy-on-write clone | New heap allocation, children retained | Same clone in the owning tree's slab | Same intrusive refcount behavior |
| Reduction/pop child buffer | Generic array allocation/growth | Slab-backed while building an owned node | Buffer is transferred into the node or abandoned |
| Large external scanner bytes | `malloc`/`free` | Unchanged | Freed immediately when the owning leaf reaches zero |
| Stack nodes and unrelated runtime arrays | Existing pools/allocator | Unchanged | Unchanged |

## Lifetime and reuse

The subtree refcount algorithm is unchanged: reaching zero recursively
decrements child references and releases any separately allocated external
scanner bytes. It no longer returns the subtree record itself to a free list or
calls `free` for an internal node's combined child/header allocation.

The mapping has an arena-level owner count. A returned `TSTree` retains the
mapping, and tree copies retain it again. The parser can rewind the bump cursor
only when it is the sole remaining arena owner, which means no published tree
can still reference the records. If an older tree is still alive, the parser
detaches and lazily reserves a new mapping for the next parse.

Rewind reuses addresses from offset zero. It does not decommit the high-water
pages, so repeated parses up to the same size avoid both system calls and page
faults. Persistent scratch arrays carry an arena generation number and discard
cached capacity after a rewind, preventing a stale buffer from overlapping a
new record.

This is whole-generation reuse, not per-record reuse. Dead records within a
live tree's generation remain holes until every tree owning that mapping is
deleted. That deliberately measures the speed ceiling of removing allocator
bookkeeping; a production arena would need free blocks, tracing/compaction, or
another reclamation policy to reduce the Go/Python high-water cost.

## Performance

Command: `cargo xtask perf-gate --offline`. Values compare the Rust-core median
throughput from the final slab run against the pre-slab Rust-core baseline.
All final-run coefficients of variation were below 2.1% (the gate is 5%).

| Language | Fixtures | Throughput geometric mean | Fixture range | Baseline peak RSS | Slab peak RSS | RSS delta |
|---|---:|---:|---:|---:|---:|---:|
| C++ | 4 | +2.10% | +0.27% to +4.55% | 10.56 MiB | 11.16 MiB | +0.60 MiB |
| Go | 5 | +9.20% | +5.97% to +11.21% | 12.58 MiB | 15.80 MiB | +3.22 MiB |
| Java | 4 | +7.82% | +7.14% to +8.98% | 8.42 MiB | 8.42 MiB | 0.00 MiB |
| JavaScript | 2 | +12.65% | +12.31% to +12.99% | 21.47 MiB | 21.48 MiB | +0.01 MiB |
| Python | 12 | +9.06% | +6.74% to +11.07% | 10.89 MiB | 12.12 MiB | +1.23 MiB |
| Rust | 2 | +8.53% | +6.63% to +10.47% | 12.56 MiB | 12.42 MiB | -0.14 MiB |
| TypeScript | 11 | +6.60% | +3.24% to +9.56% | 21.91 MiB | 20.67 MiB | -1.24 MiB |
| **Overall** | **40** | **+7.71%** | **+0.27% to +12.99%** | — | — | — |

Peak RSS is a process-level maximum and has more run-to-run noise than the
throughput medians. The same final run's Rust-minus-C RSS differences were
0.00 MiB (C++), +1.75 MiB (Go), -0.28 MiB (Java), +0.04 MiB (JavaScript),
+0.56 MiB (Python), -0.10 MiB (Rust), and -0.60 MiB (TypeScript). Go remains
the clear adverse memory case under either comparison.

## Correctness and compatibility

Passed:

- `cargo clippy -p tree-sitter --lib --tests -- -D warnings`
- `cargo test -p tree-sitter --test abi_surface`
- full Bash corpus, including error recovery and missing-token paths
- `cargo xtask core-parity` for all 15 configured TypeScript/TSX samples
- `cargo xtask ast-grep-gate` for all 4 packages
- all 16 Rust-core unit tests, including slab retention/rewind coverage

`cargo test --all` completed the arena/core/parser/corpus coverage successfully.
It retained the repository baseline's four unrelated language-detection
failures: the double-extension, first-line-regex, extensionless, and
filename-less cases returned `None` instead of their configured scope names.

## Important experimental constraints

- Unix/Windows arena mappings bypass Tree-sitter's configurable allocator
  hooks because those hooks cannot reserve a stable range and commit it on
  demand. External scanner buffers and all unrelated runtime allocations still
  use the hooks.
- The 8 GiB value is a virtual address-space ceiling, not a physical-memory
  request. It is appropriate for this 64-bit upper-bound experiment but would
  need configuration or a different failure strategy in a production design.
- Platforms without native reserve/commit support use the allocator-backed
  fallback and therefore depend on that platform allocator's demand-paging
  behavior.
- Per-record space is intentionally not reclaimed. The measured RSS deltas
  quantify the cost of that decision and should be treated as part of the
  result, not hidden allocator overhead.
