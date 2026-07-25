# Parser Runtime Components

This document describes the current Rust parser runtime at a high level. It is
not a performance trial log; performance history lives in `PERFORMANCE.md`.

## Parse Flow

Normal parsing is driven by `ts_parser_parse` in `lib/src_rust/parser.rs`:

```text
set input and parser state
initialize old-tree reuse or fresh-parse state
loop over active stack versions
  advance one version
  lex/reuse/cache lookahead
  apply parse actions
  reduce, shift, recover, or accept
  condense stack versions
balance accepted tree
return TSTree
reset parser scratch state
```

## `TSParser`

`TSParser` owns the runtime state for one parser instance:

- `lexer`: input buffering and `TSLexer` callback surface
- `stack`: GLR stack versions
- `tree_pool`: subtree free lists and release scratch
- `language`: active generated language tables
- `wasm_store`: optional wasm runtime
- `reduce_actions`: recovery reduction candidates
- `finished_tree`: best accepted root
- `reduce_builder`: parser-owned pop-result scratch for reductions
- `trailing_extras`, `trailing_extras2`: reduction scratch arrays
- `scratch_trees`: temporary child-list scratch
- `token_cache`: one-token cache keyed by byte position and external state
- `tree_arena`: tree-owned arena used for fresh normal parse internal nodes
- `reusable_node`: old-tree cursor for incremental reparsing
- `external_scanner_payload`: language scanner instance
- parse progress, cancellation, included-range, and error flags

The parser eagerly builds concrete `Subtree` values during reductions. Lazy
pending-reduction descriptors and stack payload descriptor paths were removed
after failing to produce a measured win.

## `Stack`

`Stack` in `lib/src_rust/stack.rs` is a persistent GLR graph:

- `heads`: active, paused, and halted stack versions
- `slices`: scratch results for pop APIs
- `iterators`: graph traversal scratch
- `node_pool`: small parser-local stack-node free list
- `base_node`: bottom of the stack
- `subtree_pool`: release target for link payloads

Each `StackNode` stores state, position, predecessor links, refcount, error
cost, node count, and dynamic precedence. Links carry concrete `Subtree`
payloads plus a pending-link flag used by pop traversal.

Important operations:

- `stack_push`: push a concrete subtree
- `stack_pop_count_into`: collect reduction children into `StackPopBuilder`
- `stack_merge` / `stack_can_merge`: fold equivalent versions
- `stack_remove_version`, `stack_renumber_version`, `stack_swap_versions`:
  maintain version order and ownership

The dormant segmented-stack storage trial has been removed.

## `Subtree` And `TreeArena`

`Subtree` in `lib/src_rust/subtree.rs` is an 8-byte value: either an inline leaf
or a pointer to `SubtreeHeapData`. Internal node storage keeps child `Subtree`
values immediately before the heap header.

`TreeArena` owns pages for arena-backed internal nodes created during fresh
normal parsing. `TSTree` retains the arena so copied trees share pages safely.
Arena-owned nodes still release their children, but the node block itself is
freed with the arena page.

## `Lexer`

`Lexer` in `lib/src_rust/lexer.rs` adapts `TSInput` chunks to generated lexers
through the `TSLexer` callback surface. It owns:

- current/token positions
- included ranges
- current input chunk
- decoded lookahead
- column cache
- debug/logging buffer

Generated lexers and external scanners call back through `advance`, `mark_end`,
`get_column`, range-start, and EOF callbacks.

## `TSLanguageFull`

`language.rs` mirrors generated language metadata and parse tables. Hot-path
helpers resolve parse actions, next states, lex modes, symbol metadata, field
maps, aliases, and external scanner definitions.

## Incremental Reuse

`ReusableNode` walks the old tree during incremental parsing. Fresh parses clear
it. Incremental parsing keeps concrete subtree metadata and external scanner
state compatibility as hard boundaries for future architecture changes.

## Current Architecture Pressure

The main parser cost remains the reduction construction loop:

```text
stack pop
child collection
arena node allocation
child summarization
stack push
merge/condense
```

Performance work should remove or defer a full phase of this loop. Small local
fast paths have not produced a universal win.
