# Subtree Arena Architecture Decision Space

## Status

This document explains the representation choices for replacing independent
subtree allocations with arena storage. It is a design comparison, not an
implementation plan. It deliberately stops before choosing a final candidate,
specifying a collector algorithm, listing code changes, or defining an
implementation schedule.

The scope is syntax-subtree storage in `lib/src_rust`. Stack nodes, generic
runtime arrays, lexer buffers, query storage, and generated language tables are
separate allocation domains. The deterministic stack window is described in
`DETERMINISTIC_WINDOW.md`.

## Why this decision has several layers

“Put subtrees in an arena” sounds like one decision, but it is at least four:

1. **What value does the parser copy?** Today an eight-byte `Subtree` is either
   a complete small leaf or a pointer to a heap record.
2. **What does an out-of-line node record contain?** The record may be uniform
   for every node or specialized for compact leaves, full leaves, and internal
   nodes.
3. **Where are records stored and how are they addressed?** A record can live
   in an independent allocation, a stable page, a contiguous byte arena, a
   fixed slab, or a kind-specific table.
4. **What identity escapes through the public API?** `TSNode` identifies a
   particular occurrence in a tree. That identity is not necessarily the same
   thing as the reference used internally to reach the node record.

These decisions interact, but they are not interchangeable. For example, an
arena does not imply indexes: a paged arena can preserve pointers. An index does
not imply compaction: it can address an append-only buffer. A tracing collector
does not imply removing refcounts: refcounts can continue to govern sharing and
copy-on-write while tracing reclaims arena space in batches.

## Current representation, from parser value to public node

The current runtime is easiest to understand as four layers.

### 1. `Subtree` handle

`Subtree` is the hot value copied through stack links, child arrays, token
caches, reduction arrays, and `TSTree.root`. It is eight bytes:

```text
small leaf                         heap-backed node
+-------------------------+        +-------------------------+
| SubtreeInlineData       |        | *const SubtreeHeapData |
| complete leaf metadata  |        | low tag bit is clear    |
+-------------------------+        +-------------------------+
```

The handle therefore has two jobs:

- store a qualifying leaf directly; or
- refer to an out-of-line record.

Copying the eight bytes does not by itself create ownership. Callers explicitly
retain a referenced heap node when the copy becomes another owner.

### 2. Heap record

Every internal syntax node and the minority of leaves that do not fit inline
use `SubtreeHeapData`. An internal allocation currently looks like:

```text
[child Subtree handles...][SubtreeHeapData]
```

The heap record stores measurements and flags used by parsing, recovery,
incremental comparison, and public navigation. Its atomic refcount belongs to
the node record, not to the eight-byte handle.

### 3. Allocation and ownership

Each internal node owns one variable-sized allocator block containing its child
handles and heap record. A parent owns its child handles; stack links and tree
roots can also own handles. The resulting object graph is a directed acyclic
graph rather than necessarily a unique tree because GLR paths and tree copies
can share subtrees.

When a refcount reaches zero, release recursively decrements children and frees
the allocation. The proposed arena work is motivated by the frequency of these
independent allocation and deallocation operations, especially for internal
nodes built on paths later rejected by GLR.

### 4. Public occurrence identity

`TSNode.id` currently points to a `Subtree` **slot**: either `TSTree.root` or one
element of a parent's child array. It does not simply point to
`SubtreeHeapData`.

That distinction is necessary because:

- an inline leaf has no separate record address;
- the same shared heap record can occur in two different child slots; and
- public equality distinguishes occurrences using `(tree, slot identity)`.

Any moving design must therefore answer two different questions:

1. How does an internal `Subtree` find its record after movement?
2. How does a caller-held `TSNode` continue to identify its child occurrence?

Solving only the first question is insufficient.

## Vocabulary used in the comparisons

| Term | Meaning | Example |
| --- | --- | --- |
| Handle or parser value | Small value copied throughout the parser | current eight-byte `Subtree` |
| Node record | Out-of-line metadata and optional children for one syntax node | `SubtreeHeapData` plus child handles |
| Child slot | One occurrence of a child handle inside a parent | `parent.children()[i]` |
| Physical address | Process address of a record or slot | current heap pointer |
| Physical index | Offset into a particular storage buffer | byte, word, node, or slot index |
| Logical ID | Stable name resolved through a table to the current physical location | `locations[id] -> offset` |
| Record identity | Identity of shared syntax data | heap allocation, physical index, or logical ID |
| Occurrence identity | Identity of one position within one public tree | current `TSNode.id` child-slot address |
| Storage domain | Allocation whose base and lifetime give meaning to an index | one arena, page set, node slab, or child array |

A physical index is only meaningful together with its storage domain. The
number `42` does not identify a subtree unless the parser or tree also tells us
which arena contains word, byte, node, or slot 42.

A logical ID adds a level of indirection:

```text
logical ID 42 -> location table entry -> current physical index -> record
```

Compaction changes the location-table entry without changing ID 42. A direct
physical-index design instead updates every reference that contains the old
index.

## Constraints that every candidate must confront

### Hot handle size

The handle is copied far more often than a public `TSNode` is created. The
performance ledger measured a 19.74% parse-time regression from a 16-byte
`Subtree`. A candidate must therefore use a native pointer, an index no wider
than eight bytes, or the existing eight-byte inline/reference union. A
16-byte `{arena, reference}` pair is not a useful baseline.

### Inline leaves are common

The allocation audit found approximately:

- 59.09% internal nodes;
- 3.21% existing heap leaves; and
- 37.70% inline leaves.

Turning every inline leaf into a full 88-byte heap-shaped record would remove a
branch from accessors but greatly increase live storage and writes. A uniform
reference is plausible only if its leaf records are substantially smaller than
the current full heap record or its narrower child references compensate.

### Internal nodes remain refcounted

Internal refcounts currently answer two questions:

- is another owner keeping this node alive? and
- is the node uniquely owned and therefore safe to mutate in place?

Arena reclamation may batch physical space recovery, but it must not silently
remove this sharing and copy-on-write contract. Whether compact leaf records
also need refcounts is a separate choice.

### The graph is shared but acyclic

Refcounts are sufficient to discover logical death because subtree edges point
from parent to child and do not form cycles. A collector is useful here mainly
to reclaim arena holes, relocate live blocks, and replace many allocator calls;
it is not needed to break reference cycles.

### Published identities can escape

Parser-private handles can be enumerated at a controlled safepoint. Public
`TSNode` values can be copied into arbitrary caller memory and cannot be found
or rewritten by the runtime. A candidate must either:

- keep the referenced public slot stable after publication;
- encode a stable occurrence ID and resolve it through the tree; or
- freeze movement once the tree becomes public.

### “One arena” has a precise meaning

A strict contiguous-arena design has one growable record buffer. A paged arena
is one logical owner but multiple physical allocations. A fixed node slab plus
child array has two storage domains. Separate kind tables have several storage
domains. These designs can still be useful controls, but they are not all
equivalent to the requested single contiguous block.

## How the design axes depend on one another

The main dependency chain is:

```text
handle model
    |
    +--> reference encoding and width
             |
             +--> storage can move or must stay stable
                      |
                      +--> reclamation choices
                               |
                               +--> public occurrence identity policy

record shape ---------> object size, scan rules, and leaf overhead
child representation --> locality and number of storage domains
ownership policy ------> retain/release and copy-on-write semantics
```

The top chain contains forcing relationships. A raw pointer requires the target
address to stay stable. A physical index permits the arena base to move, but
compaction changes the index and therefore requires reference rewriting. A
logical ID permits compaction without changing handles, but adds a location
lookup.

The bottom three axes are more independent. For example, both pointer and index
handles can refer to variable-sized records with coallocated children. Changing
child layout while changing handle encoding makes a benchmark difficult to
interpret because either change could explain the result.

## Subtree value models

| Model | Value copied by parser | Where leaves live | Where internals live | Essential property |
| --- | --- | --- | --- | --- |
| Hybrid inline/reference | inline bits or pointer/index | qualifying leaves in handle; others out of line | out-of-line record | preserves cheap common leaves |
| Uniform pointer | `*const NodeRecord` | every leaf out of line | every internal out of line | one access path, stable addresses |
| Uniform physical index | byte/word/node offset | every leaf in indexed storage | every internal in indexed storage | small references and relocatable base |
| Uniform logical ID | stable ID resolved through table | every leaf in managed storage | every internal in managed storage | movement without rewriting handles |
| Tagged table index | kind tag plus per-kind index | specialized leaf table | specialized internal table | dense records specialized by kind |
| Direct full value | complete record embedded in parser value | leaf can be embedded | recursive/shared children still require references | conceptual value-semantics extreme |

### Hybrid inline/reference

This is the current value model. Its primary advantage is that the common small
leaf requires no record allocation, refcount, or arena lookup. Its costs are a
tag branch in accessors and an eight-byte handle even when the reference arm
could fit in four bytes.

Changing the reference arm from pointer to index does not require removing the
inline arm. That distinction separates candidate C from candidate D below.

### Uniform pointer

Every node becomes an out-of-line record and the parser copies only a pointer.
Access is direct and code paths become more uniform, but every former inline
leaf now consumes record storage and must have a lifetime policy. Pointers also
prevent moving records, so storage must use independent allocations, stable
pages, a non-moving reserved region, or another stable-address scheme.

This model answers whether inline leaves themselves are worth their two-path
complexity when direct pointer access is retained.

### Uniform physical index

Every parser value is an offset interpreted relative to an arena or table.
Four-byte indexes can halve child arrays and stack-held subtree values, while an
eight-byte index can preserve the current handle width and capacity.

An arena `realloc` may change the base pointer without changing indexes. True
compaction is different: if a record moves from index 100 to index 60, every
handle containing 100 must be rewritten. This is straightforward for
registered parser roots and in-arena children, but not for unregistered stack
locals or escaped public occurrence identities.

### Uniform logical ID

The handle contains a stable ID rather than a physical location. A location
table supplies the current offset. Compaction changes the table, not the
handles.

This simplifies relocation and permits stable internal identity, but every
access becomes at least:

```text
arena -> location table[id] -> arena base + offset -> record
```

The location table also consumes memory, needs its own growth policy, and does
not by itself solve public occurrence identity: two parent slots containing the
same logical node ID are still two different public nodes.

### Tagged table index

The handle encodes both a record kind and an index into that kind's table. A
compact leaf can therefore use a dense small record without forcing internal
nodes into the same shape.

The tradeoff is multiple storage domains and more tag dispatch. Collection,
tree copying, and public-slot resolution must coordinate all tables. This is
closer to a compact node database than to one contiguous arena.

### Direct full value

Direct values work for leaves because leaves do not recursively contain
children. They cannot be the universal edge value in a shared recursive tree:
an internal value would need to embed descendants recursively, duplicate them,
or reintroduce references for children.

This row remains useful because it explains what inline leaves already are: a
limited direct-value optimization. It is not a complete replacement for
`Subtree`; the real design question is where the boundary lies between direct
leaf values and referenced node records.

## Reference width and units

| Reference | Approximate capacity | Important consequence |
| --- | --- | --- |
| `u32` byte index | 4 GiB per storage domain | simple units but lowest limit |
| `u32` eight-byte-word index | about 32 GiB per storage domain | four-byte handle with natural record alignment |
| `u64` index | effectively unlimited for one tree | current handle width, no child-array shrink |
| native pointer | platform address space | direct access but target address cannot move |

The unit is part of the representation. A word index needs a scale operation
but extends the range of `u32`; a node index works only if records are fixed
size or a location table maps node numbers to variable records.

On a 64-bit target, changing an eight-byte pointer or hybrid handle to an
eight-byte index does not reduce copied bytes. Its benefit would come from arena
growth and relocation. A four-byte index additionally changes cache density,
child-array size, stack-link layout, null/tag encoding, and possibly alignment.
Those effects make it a different candidate rather than a small variation.

## Record shape and node storage

Handle uniformity does not require record uniformity. These are separate
questions.

| Record/storage model | Physical organization | What is easy | What becomes difficult |
| --- | --- | --- | --- |
| Independent variable blocks | one allocation per node | direct pointers, individual free | allocator traffic and fragmentation |
| Pointer-stable pages | variable records packed into non-moving pages | pointers and bulk page ownership | reclaiming holes; page list traversal |
| Contiguous variable records | compact leaf/full leaf/internal blocks in one byte arena | one buffer, dense packing, sliding compaction | record-kind scan and index rewriting |
| Fixed node slab plus child array | `NodeRecord[]` and `ChildRef[]` | arithmetic node lookup, dense fixed records | second child domain and coordinated compaction |
| Separate kind tables | one array per leaf/internal kind | minimum record size for each kind | tags, several domains, cross-table collection |

### Variable-sized records

A uniform index can address several record shapes in the same byte arena:

```text
compact leaf: [kind/size][packed leaf fields]
full leaf:    [kind/size][full metadata][scanner bytes]
internal:     [kind/size][metadata][child handles...]
```

This avoids paying the current full heap-record size for every former inline
leaf. The arena must be able to scan from one record to the next, so each record
needs enough kind and size information to interpret its tail.

### Fixed node slab

A node index directly selects `NodeRecord[index]`, which makes lookup and node
movement simple. Variable child counts no longer fit inside the record, so
children must live in a separate `ChildRef[]` range or block. The apparent
simplicity of node lookup therefore moves complexity into child storage and
coordinated reclamation.

### Separate kind tables

This produces the densest records but changes “one arena” into a collection of
tables. A handle needs tag bits, and movement or growth of one table must not
invalidate interpretation of the others. It is valuable as an architectural
extreme, not equivalent to a single contiguous record buffer.

## Child representation

| Model | Parent representation | Strength | Cost |
| --- | --- | --- | --- |
| Coallocated tail | record followed by `child_count` handles | best current locality; parent and children move together | variable-sized records |
| Fixed inline capacity | record contains N slots plus overflow | excellent for nodes within N | unused capacity and overflow path |
| Global child range | `{child_start, child_count}` into one child array | dense sequential child storage | separate lifetime and compaction domain |
| Separate child block | pointer/index to variable block | record size independent of arity | extra allocation or arena block and lookup |
| First-child/next-sibling | references form a sibling chain | fixed fields per record | public indexed-child lookup becomes linear |

Coallocated children are the current control and isolate changes to handle and
record allocation. Global child ranges pair naturally with a fixed node slab,
but then node and child indexes have different domains. Fixed inline capacity
is attractive only if the arity distribution justifies paying unused slots;
otherwise it repeats the same space-for-branch tradeoff already seen in stack
node link arrays.

## Ownership and reclamation are separate decisions

Reference counting determines logical ownership and copy-on-write eligibility.
Reclamation determines when dead bytes become available for new allocations.
They can be combined in several ways:

| Policy | Internal refcount | Leaf refcount | When physical space returns |
| --- | ---: | ---: | --- |
| Current immediate free | yes | heap leaves only | zero-count cascade frees each block |
| Refcount plus bulk arena lifetime | yes | configurable | only when entire arena dies |
| Refcount plus free list | yes | configurable | zero-count blocks enter size-aware holes |
| Refcount plus mark-sweep | yes | configurable | collector identifies/reuses dead blocks without moving live ones |
| Refcount plus semispace copy | yes | configurable | live blocks copied to alternate space |
| Refcount plus mark-compact | yes | configurable | live blocks packed and references or locations updated |
| Pure tracing | no | no | collector alone determines liveness |

Pure tracing conflicts with the requirement that internal nodes remain
refcounted and would change copy-on-write semantics. It is included to show the
boundary of the design space, not because an arena forces it.

With exact cascading refcounts in an acyclic graph, a collector can mark from
roots as a safety authority or use zero-count state to identify garbage. Marking
still helps validate ownership and distinguish reachable nodes if releases are
ever deferred. The choice does not require changing the handle representation.

### Bulk-only arena

This removes individual `free` calls but retains every rejected path until the
arena dies. It is a useful allocation-throughput control, but may increase peak
memory substantially during GLR-heavy parses.

### Free list or non-moving mark-sweep

These preserve physical addresses or indexes. Variable record sizes make reuse
policy important: exact-size holes are simple but may reuse little; splitting
and coalescing recreate allocator-like complexity inside the arena.

### Semispace

Semispace collection copies reachable records into an alternate region and
then swaps spaces. It makes destination allocation simple, but requires enough
room for both the old live graph and its copy. Tree-sitter's successful syntax
tree has a high survival rate, so copying bandwidth and peak capacity are
material concerns.

### Mark-compact

Mark-compact packs live variable-sized records within one storage domain. With
physical indexes, roots and child handles must be rewritten. With logical IDs,
only the location table changes. Neither version can safely run while public
readers concurrently resolve the same mutable storage without an additional
synchronization scheme.

## Public occurrence identity choices

Internal record addressing and public node identity must be evaluated
separately.

| Public identity model | What `TSNode.id` means | Movement consequence |
| --- | --- | --- |
| Raw child-slot address | address of root/parent child handle | child storage must remain at that address |
| Physical child-slot index | offset of child handle in tree storage | arena base may move; compaction changes identity unless frozen |
| Stable occurrence ID | ID resolved to parent/slot location | occurrence table updated on movement; permanent lookup/metadata |
| Frozen post-publication index | physical slot index fixed after parse | parser-private compaction allowed; published compaction forbidden |

A record ID alone cannot serve as occurrence identity. If two parents share the
same heap record, the public nodes are different occurrences even though their
record identity is equal. A stable public scheme therefore needs a child-slot
ID, a parent-plus-child key, or another occurrence mapping.

Tree copying and editing add another constraint. Current tree copies share
immutable records and edits use path-level copy-on-write. Arena-local physical
indexes cannot refer into another arena unless the handle also carries an arena
identity. Avoiding a wider handle may require sharing the entire arena,
importing reused nodes, freezing storage, or copying more data during detach.
This tradeoff exists independently of the collector chosen during parsing.

## Architecture candidates

The candidates combine the axes into coherent systems. They are comparison
points, not an implementation sequence.

| Candidate | Parser value | Record storage | Children | Reclamation | Central question |
| --- | --- | --- | --- | --- | --- |
| A | current 8 B inline-or-pointer | pointer-stable pages | coallocated | bulk/page policy | What is gained by removing per-node allocator calls while preserving current access? |
| B | uniform pointer | variable leaf/internal records in stable pages | coallocated | bulk/page policy | What do uniform access and loss of inline leaves cost when pointer lookup remains direct? |
| C | 8 B inline-or-physical-index | one contiguous variable-record arena | coallocated | bulk, sweep, or compact | What does indexed heap access cost while preserving inline leaves? |
| D | 4 B uniform physical index | compact leaf/full leaf/internal records in one arena | coallocated | sweep or compact | Can denser references offset putting every leaf out of line? |
| E | 4 B uniform physical index | fixed `NodeRecord[]` plus `ChildRef[]` | global ranges | compact both domains | Does a fully flattened node database outperform variable records? |
| F | uniform logical ID | movable records plus location table | coallocated or global | moving collector | Is post-publication/stable movement worth permanent indirection? |

### Candidate A: allocation control

Candidate A changes storage ownership but deliberately preserves the current
hot representation: inline leaves remain inline, heap handles remain pointers,
and children remain coallocated. Stable pages are required because existing
pointers and public slot addresses cannot survive page movement.

Its result isolates the value of removing individual allocator calls. It does
not test one contiguous block or compaction. If A is slow, the cost is likely
page policy or arena bookkeeping rather than index decoding.

### Candidate B: uniform direct access

Candidate B keeps direct pointers but moves all leaves out of line into
kind-sized stable records. Compared with A, it measures the cost or benefit of
removing the inline/reference split while retaining direct dereference.

Its risk is clear from the audit: roughly 37.70% of created subtrees currently
fit entirely inside the handle. Candidate B adds record writes, storage, and
lifetime bookkeeping for all of them without gaining smaller child handles or
movability.

### Candidate C: conservative contiguous index

Candidate C preserves the current inline leaf format and eight-byte handle. It
changes only the heap arm from pointer to physical index and places heap records
in one contiguous arena.

This is the cleanest comparison for arena-aware indexed access because inline
leaf frequency, handle width, and internal record semantics remain familiar.
It permits the arena base to move, but record compaction still requires
rewriting indexes. Public identities must be indexed separately or frozen after
publication.

### Candidate D: dense uniform index

Candidate D replaces both arms of `Subtree` with one four-byte physical index.
Every node receives a record, but compact leaves use a small specialized shape
rather than the current full heap record. Child arrays and any stack fields that
store `Subtree` become denser.

This candidate mixes two major effects intentionally: loss of direct inline
leaves and halving of reference width. It answers whether one uniform,
relocatable representation produces a better total layout, not whether either
individual change wins alone.

### Candidate E: flattened node and child arrays

Candidate E uses fixed-size node records and a separate global child array.
Node lookup becomes arithmetic and node records compact uniformly, but child
ranges have their own offsets, lifetime, and compaction.

This is substantially more than an allocator replacement. It resembles the
reverted NodeTable family and must be judged against that history: dense layouts
can benchmark well while introducing broad identity and memory-policy
complexity.

### Candidate F: stable logical identity

Candidate F separates stable identity from physical position. Handles name
logical records; a table finds their current locations. This removes widespread
handle rewriting during compaction and is the natural basis for moving records
after publication.

The costs are permanent: another memory table, an additional dependent load on
record access, ID allocation/reuse rules, and a still-separate solution for
public occurrence identity. It is justified only if movement across long-lived
tree/edit lifetimes is important enough to pay those costs on every parse.

## Candidate comparison at a glance

| Property | A | B | C | D | E | F |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Preserves inline leaves | yes | no | yes | no | no | configurable |
| Parser reference width | 8 B | pointer | 8 B | 4 B | 4 B | 4 or 8 B |
| Direct record dereference | yes | yes | arena base + index | arena base + index | array index | table then location |
| Strict one contiguous record arena | no | no | yes | yes | no, node + child domains | record arena plus metadata table |
| Coallocated children | yes | yes | yes | yes | no | either |
| Arena base may move | no | no | yes | yes | each array may move | yes |
| Record compaction without stable-ID table | no | no | parser-private rewrite | parser-private rewrite | rewrite both domains | yes |
| Preserves current ownership most directly | yes | partial | yes | partial | partial | partial |
| Public identity complexity | lowest | low | medium | medium | high | high |
| Architectural change size | small | medium | medium | large | very large | large |

“Arena base may move” means relocation of the whole allocation while preserving
relative positions. It does not mean individual records may change indexes
without updating references.

## What the comparison should decide

The architecture discussion ultimately needs answers to these questions:

1. Is allocator-call removal valuable when the current handle and inline leaves
   are otherwise preserved? Candidate A isolates that effect.
2. Is one contiguous indexed arena affordable on hot subtree accessors?
   Candidate C isolates that effect without forcing leaves out of line.
3. If indexed access is affordable, does a four-byte uniform index improve the
   total layout enough to compensate for allocating compact leaf records?
   Candidate D answers that combined question.
4. Is a second child-storage domain justified? Candidate E should be considered
   only if variable-record packing is the demonstrated limitation.
5. Is movement after publication actually required? Candidate F should be
   considered only if frozen physical indexes or whole-arena sharing make tree
   copy/edit behavior unacceptable.
6. Does any reclamation policy reduce allocator traffic only by replacing it
   with comparable internal fragmentation, copying, or bookkeeping?

No collector or detailed code plan should be selected before the handle,
record, storage-domain, and public-identity choices are understood together.
