# Parser Runtime Components

Working inventory of the Rust parser runtime architecture. This is not a perf
trial log. It records what each component owns, which fields matter, and how the
components interact on the normal parsing hot path.

Target context: raw normal parsing for TypeScript, JavaScript, Python, Go,
Rust, C++, and Java.

## High-Level Flow

Normal parsing is driven by `ts_parser_parse`:

```text
initialize parser input/tree arena/reusable node
loop over active stack versions
  ts_parser__advance(version)
    reuse old node or cached token, else lex
    lookup parse-table actions for state + lookahead
    run reduce actions until shift/accept/recover
  condense stack versions
accept finished tree
balance accepted tree
return TSTree
reset parser scratch state
```

The current architecture eagerly constructs concrete `Subtree` nodes during
every reduction. The stack is a persistent graph, so a reduction first walks
backward through stack links, collects child payloads into arrays, builds a
parent subtree, and pushes that parent back into the graph.

## `TSParser`

File: `lib/src_rust/parser.rs`

### Role

`TSParser` is the top-level runtime coordinator. It owns lexer state, parse
stack state, parse scratch arrays, old-tree reuse state, accepted tree state,
and parse progress/cancellation state.

### Fields

- `lexer: Lexer`: buffered input scanner and `TSLexer` callback surface used by
  generated lexers and external scanners.
- `stack: *mut Stack`: GLR parse stack with active/paused/halted versions.
- `tree_pool: SubtreePool`: short-lived free lists used by subtree release and
  mutable subtree operations.
- `language: *const TSLanguage`: active generated language metadata and parse
  tables.
- `wasm_store: *mut TSWasmStore`: wasm language support.
- `reduce_actions: Array<ReduceAction>`: temporary deduped reductions used by
  recovery when scanning possible reductions.
- `finished_tree: Subtree`: best accepted root so far.
- `reduce_builder: StackPopBuilder`: parser-owned scratch storage for reduction
  pop results; avoids allocating a separate `StackSliceArray` in the fresh
  no-old-tree reduce path.
- `pending_reductions: Array<*mut PendingReduction>`: owner list for lazy
  reduction descriptors currently allocated by the parser.
- `trailing_extras`, `trailing_extras2`: scratch arrays used when stripping and
  selecting trailing extra nodes during reduce.
- `scratch_trees`: temporary children array used when comparing alternative
  child lists.
- `token_cache: TokenCache`: one-token cache keyed by byte position and last
  external token.
- `tree_arena: *mut TreeArena`: tree-owned arena used for normal parse internal
  nodes.
- `reusable_node: ReusableNode`: cursor over an old tree for incremental
  reparsing.
- `external_scanner_payload: *mut c_void`: external scanner instance state.
- `dot_graph_file: *mut c_void`: optional debug graph output target.
- `accept_count`: number of accepted trees seen.
- `operation_count`: progress-callback accounting.
- `old_tree: Subtree`: retained root of the old tree during reparsing.
- `included_range_differences: TSRangeArray`: changed included ranges between
  old and new parse.
- `parse_options`, `parse_state`: progress callback and public parse state.
- `included_range_difference_index`: cursor into included range differences.
- `has_scanner_error`, `canceled_balancing`, `has_error`: parse status flags.

### Hot-Path Behavior

- `ts_parser__advance` is the parser action interpreter. It obtains a lookahead,
  gets a `TableEntry`, processes actions, and repeats after reductions.
- `ts_parser__reduce` is the dominant parser-side hot function. It pops stack
  children, constructs a parent subtree, computes next state, pushes the parent,
  and tries to merge versions.
- `ts_parser__accept` pushes EOF, pops the full stack path, rebuilds the root,
  and chooses the best finished tree.
- `ts_parser__balance_subtree` runs after accept and can still be visible in
  profiles for repeat-heavy trees.

## Parser Scratch, Caches, And Limits

File: `lib/src_rust/parser.rs`

### Role

These are small parser-owned buffers and thresholds that avoid repeated
allocation or bound GLR explosion. They are not independent subsystems, but they
are important architecture constraints because many hot-path APIs write into
these scratch arrays.

### Fields And Types

- `ReduceAction`: compact recovery candidate:
  `count`, `symbol`, `dynamic_precedence`, `production_id`.
- `reduce_actions: Array<ReduceAction>`: temporary deduped set used when error
  recovery scans all possible lookaheads for reductions.
- `TokenCache`: one cached lookahead:
  `token`, `last_external_token`, `byte_index`.
- `reduce_builder: StackPopBuilder`: reusable reduction pop storage. It carries
  stack result spans plus child payload arrays and is central to current reduce
  allocation reduction.
- `scratch_trees`: temporary array used to build comparison-only subtrees in
  `ts_parser__select_children`.
- `trailing_extras`, `trailing_extras2`: temporary arrays for removing and
  comparing trailing extras across alternative reductions.

### Limits

- `MAX_VERSION_COUNT = 6`: normal cap for active stack versions.
- `MAX_VERSION_COUNT_OVERFLOW = 4`: extra temporary overflow before pruning.
- `MAX_SUMMARY_DEPTH = 16`: recovery stack-summary depth.
- `MAX_COST_DIFFERENCE`: error-cost pruning threshold.
- `OP_COUNT_PER_PARSER_CALLBACK_CHECK = 100`: progress callback cadence.

### Architecture Notes

The scratch model assumes one parser thread mutates one parser instance. This
is good for locality, but it also means new architecture work should prefer
parser-owned builders over hidden global caches or benchmark-only state.

## `Lexer`

File: `lib/src_rust/lexer.rs`

### Role

`Lexer` adapts `TSInput` chunks to generated lexers via the `TSLexer` vtable. It
tracks source positions, included ranges, chunk boundaries, decoded lookahead,
column data, logging, and external scanner serialization scratch space.

### Fields

- `data: TSLexer`: public callback struct passed to generated lexers. Contains
  `lookahead`, `result_symbol`, and callbacks for advance, mark_end,
  get_column, range start, eof, and log.
- `current_position`, `token_start_position`, `token_end_position`: byte/point
  positions for scanning and token bounds.
- `included_ranges: *mut TSRange`, `included_range_count`,
  `current_included_range_index`: range filtering state.
- `chunk: *const i8`, `chunk_start`, `chunk_size`: current source chunk from
  `TSInput::read`.
- `input: TSInput`: source reader and encoding/decode configuration.
- `logger: TSLogger`: optional parse logging.
- `lookahead_size`: decoded character width.
- `did_get_column`, `column_data`: column caching for external scanners and
  lexers that call `get_column`.
- `debug_buffer`: fixed logging/serialization buffer.

### Hot-Path Behavior

- `ts_lexer_start` prepares a token scan and ensures a chunk/lookahead exists.
- Generated `lex_fn`, `keyword_lex_fn`, and external scanners call back through
  `TSLexer::advance`, which updates byte/point position and decodes the next
  codepoint.
- `ts_lexer_finish` computes token end and lookahead bytes.
- The reusable-runtime cost is mostly position/chunk/decode/callback overhead.
  For C++ and JS-like languages, generated lexer and keyword code are also a
  major cost center outside the reusable parser logic.

## External Scanner Boundary

Files: `lib/src_rust/language.rs`, `lib/src_rust/lexer.rs`,
`lib/src_rust/parser.rs`

### Role

External scanners are language-provided callbacks for tokens that cannot be
recognized by the generated lexer alone. They receive the same `TSLexer`
surface as generated lexers and carry serialized state through tokens and stack
versions.

### Fields

`TSExternalScanner` fields:

- `states: *const bool`: valid external scanner states.
- `symbol_map: *const TSSymbol`: external token symbol mapping.
- `create`, `destroy`: scanner instance lifetime callbacks.
- `scan`: token scanning callback.
- `serialize`, `deserialize`: external scanner state persistence callbacks.

Parser/Lexer state involved:

- `external_scanner_payload`: scanner instance held by `TSParser`.
- `Lexer::debug_buffer`: serialization buffer.
- `StackHead::last_external_token`: last token with external scanner state for
  each stack version.
- Subtree flags/data for external tokens and scanner-state changes.

### Architecture Notes

Any lazy tree or linear-stack design must preserve external scanner state on
stack versions. Merge equivalence checks include external scanner state, so
descriptor or frame payloads need enough metadata to answer those checks without
forcing broad materialization.

## `TSLanguageFull` And Parse Tables

File: `lib/src_rust/language.rs`

### Role

`TSLanguageFull` is a `repr(C)` mirror of generated language metadata. It owns
no memory itself; generated code provides the pointed-to tables and callbacks.
Runtime helpers use it for parse-table lookup, lex modes, symbol metadata,
aliases, field maps, and external scanner dispatch.

### Fields

- Counts: `abi_version`, `symbol_count`, `alias_count`, `token_count`,
  `external_token_count`, `state_count`, `large_state_count`,
  `production_id_count`, `field_count`, `max_alias_sequence_length`.
- Parse tables: `parse_table`, `small_parse_table`,
  `small_parse_table_map`, `parse_actions`.
- Symbol/field metadata: `symbol_names`, `field_names`, `field_map_slices`,
  `field_map_entries`, `symbol_metadata`, `public_symbol_map`.
- Aliases: `alias_map`, `alias_sequences`.
- Lexing: `lex_modes`, `lex_fn`, `keyword_lex_fn`,
  `keyword_capture_token`, `reserved_words`, `max_reserved_word_set_size`.
- External scanner: `external_scanner`.
- State metadata: `primary_state_ids`.
- Metadata and names: `name`, `supertype_count`, `supertype_symbols`,
  `supertype_map_slices`, `supertype_map_entries`, `metadata`.

### Runtime Types

- `TableEntry`: resolved action list for a `(state, terminal_symbol)` pair:
  `actions`, `action_count`, `is_reusable`.
- `TSParseAction`: union of shift, reduce, accept, and recover actions.
- `LookaheadIterator`: iterates valid lookahead symbols for a state, used by
  recovery and public APIs.

### Hot-Path Behavior

- `ts_language_table_entry` calls `ts_language_lookup` and maps the resulting
  action index into `parse_actions`.
- Large states use direct table indexing. Small states scan compressed groups in
  `small_parse_table`.
- Reductions often call `ts_language_next_state`; for nonterminals this uses
  `ts_language_lookup`.

## `Stack`

File: `lib/src_rust/stack.rs`

### Role

`Stack` is a persistent GLR stack graph. Each stack version is a head pointing
to a `StackNode`. Each node points backward to predecessor nodes through up to
eight links. Links carry either concrete subtrees or pending-reduction payloads.

### Fields

- `heads: Array<StackHead>`: active/paused/halted version heads.
- `slices: Array<StackSlice>`: scratch result storage for pop APIs that return
  owned subtree arrays.
- `iterators: Array<StackIterator>`: scratch graph traversal state.
- `node_pool: Array<*mut StackNode>`: small parser-local pool of freed stack
  nodes.
- `base_node: *mut StackNode`: bottom of the stack.
- `subtree_pool: *mut SubtreePool`: release target for link payloads.

### Related Types

- `StackHead`: version metadata:
  `node`, `summary`, `node_count_at_last_error`, `last_external_token`,
  `lookahead_when_paused`, `status`.
- `StackNode`: graph node:
  `state`, `position`, `links`, `link_count`, `ref_count`, `error_cost`,
  `node_count`, `dynamic_precedence`.
- `StackLink`: predecessor pointer plus `StackLinkPayload`.
- `StackLinkPayload`: tagged union of `Subtree` or `*mut PendingReduction`,
  plus flags for pending link and pending reduction.
- `StackIterator`: graph traversal state for concrete subtree pops.
- `StackPayloadIterator`: graph traversal state for payload pops.
- `StackSlice`: owned concrete subtree pop result and resulting version.
- `StackSliceSpan`: slice into `StackPopBuilder` scratch arrays and resulting
  version.
- `StackPopBuilder`: parser-owned scratch arrays:
  `slices`, `subtrees`, `payloads`.
- `StackSummaryEntry`: recovery summary entry:
  `position`, `depth`, `state`.

### Hot-Path Behavior

- `ts_stack_push` creates a new `StackNode` with one link back to the previous
  version head.
- `ts_stack_pop_count_into` has a linear one-link fast path, then falls back to
  graph traversal for branching paths.
- Pop traversals retain child payloads/subtrees, reverse them into parse order,
  and create or reuse stack versions at the predecessor node.
- `ts_stack_merge` and `stack_node_add_link` merge equivalent paths by comparing
  payload shape, state, position, error cost, and dynamic precedence.
- `ts_stack_renumber_version`, `ts_stack_remove_version`, and
  `ts_parser__condense_stack` maintain version ordering and pruning.

## `PendingReduction`

File: `lib/src_rust/stack.rs` and `lib/src_rust/parser.rs`

### Role

`PendingReduction` is the current lazy-reduction descriptor. It stores enough
subtree-like metadata for stack accounting and future materialization without
immediately creating a concrete `SubtreeHeapData` node.

### Fields

- Identity/state: `symbol`, `production_id`, `parse_state`, `child_count`.
- Ownership: `ref_count`, `children`, `payload_children`, `materialized`.
- Layout metadata: `padding`, `size`, `lookahead_bytes`.
- Cost/count metadata: `error_cost`, `node_count`, `visible_child_count`,
  `named_child_count`, `visible_descendant_count`, `dynamic_precedence`,
  `repeat_depth`.
- First-leaf metadata: `first_leaf_symbol`, `first_leaf_parse_state`.
- Flags: extra, visible, named, fragile left/right, external tokens, external
  scanner state change, depends on column.

### Current Boundary

The descriptor can summarize concrete children or payload children and can
materialize recursively into a concrete subtree. Partial broad wiring has been
error-prone because reduce, merge, recovery, accept, and public tree publication
all assume concrete subtree semantics at different boundaries.

## `Subtree` And `TreeArena`

File: `lib/src_rust/subtree.rs`

### Role

`Subtree` is the parse tree value stored on stack links and inside tree nodes.
It is an 8-byte union: either inline leaf data or a pointer to heap node data.
Internal nodes store child `Subtree` values immediately before their
`SubtreeHeapData` header.

### Fields

- `SubtreeInlineData`: packed leaf:
  flags, small symbol, parse state, padding columns/rows/bytes, lookahead
  bytes, and size bytes.
- `SubtreeHeapData`: heap/internal node:
  `ref_count`, `padding`, `size`, `lookahead_bytes`, `error_cost`,
  `child_count`, `symbol`, `parse_state`, packed flags, and union content.
- `SubtreeChildrenData`: internal-node metadata:
  `visible_child_count`, `named_child_count`, `visible_descendant_count`,
  `dynamic_precedence`, `repeat_depth`, `production_id`, `first_leaf`.
- `SubtreeArray`: dynamic array of `Subtree`.
- `SubtreePool`: free lists and traversal scratch used by release.
- `TreeArena`: refcounted page owner for arena-backed nodes:
  `ref_count`, `pages`, `current_page`.

### Hot-Path Behavior

- `ts_subtree_new_node_in_arena` allocates one block for children plus
  `SubtreeHeapData`, copies children, initializes metadata, then calls
  `ts_subtree_summarize_children`.
- `ts_subtree_summarize_children` walks all children to calculate size,
  padding, visible/named counts, error cost, dynamic precedence, first leaf,
  external-token flags, fragility, and lookahead bytes.
- `ts_subtree_release` walks children and releases descendants; arena-owned
  blocks skip freeing the block itself and rely on `TreeArena` page lifetime.
- `ts_subtree_compare` and selection paths may recursively inspect trees when
  choosing between ambiguous reductions.

## `TSTree`

File: `lib/src_rust/tree.rs`

### Role

`TSTree` is the public parse result owner. It retains the root subtree,
language, included ranges, and arena pages for arena-backed internal nodes.

### Fields

- `root: Subtree`: accepted parse root.
- `language: *const TSLanguage`: retained language pointer.
- `included_ranges: *mut TSRange`: copied included ranges.
- `included_range_count: u32`: number of included ranges.
- `arena: *mut TreeArena`: optional refcounted owner for arena-backed node
  pages.

### Lifetime

`ts_tree_copy` retains the root and tree arena. `ts_tree_delete` releases the
root, releases the arena, deletes the language reference, and frees included
ranges.

## `ReusableNode`

File: `lib/src_rust/reusable_node.rs`

### Role

`ReusableNode` walks an old tree during incremental reparsing. It is mostly out
of scope for raw no-old-tree parsing, but it strongly constrains lazy tree
design because reused nodes must remain concrete and comparable.

### Fields

- `stack: Array<StackEntry>`: cursor stack through old-tree children.
- `last_external_token: Subtree`: external-token state associated with the
  current reusable position.

`StackEntry` fields:

- `tree: Subtree`: current old-tree node.
- `child_index: u32`: index within parent.
- `byte_offset: u32`: byte offset of the current node.

### Boundary

`ts_parser__reuse_node` consults `ReusableNode` from `ts_parser__advance`.
Normal fresh parsing clears this component; incremental parsing requires
concrete subtree metadata and external scanner state compatibility.

## Component Interaction On A Normal Reduction

```text
TSParser
  has current stack version and lookahead
  reads TableEntry from TSLanguageFull
  calls ts_parser__reduce

Stack
  pop count from version
  walk links backward
  retain payload subtrees
  write child slice into StackPopBuilder
  create/reuse resulting version at predecessor node

TSParser
  remove trailing extras
  call ts_subtree_new_node_in_arena

Subtree / TreeArena
  allocate children + heap header block
  copy children into arena block
  summarize children immediately

TSParser / Stack
  compute goto state with language lookup
  set fragility/parse_state/dynamic precedence
  push parent subtree back onto stack
  try to merge resulting version
```

The expensive architectural pattern is not one function. It is the loop of
graph traversal, child-array formation, immediate concrete node creation,
metadata summarization, and graph reinsertion.

## Architecture Pressure Points

- A linear-stack normal parser would change `Stack` usage: most fresh parses
  would avoid persistent graph nodes until a fork/recovery boundary.
- A stack-native parse forest would change the `Subtree` boundary: reductions
  would push descriptors and materialize only at forced concrete boundaries.
- Action-trace execution would change the `TSParser`/`TSLanguageFull` boundary:
  deterministic reduce chains would be interpreted as one compiled action run.
- Generated lexer work would change the `Lexer`/generated-code boundary:
  lexers would need a faster bulk scan or keyword path, likely outside this
  reusable runtime module alone.
