# Runtime Implementation Deep Dive

This chapter follows one parse through the implementation in `lib/src_rust`.
It assumes you have read [How Tree-sitter Works](./5-implementation.md), which
introduces LR actions, stack versions, and subtrees without depending on the
source code.

The goal here is different: connect those ideas to concrete types, functions,
data layouts, and ownership rules. Function and type names are written exactly
as they appear in the Rust runtime so that they can be searched directly.

## Scope of this chapter

The active parsing engine in this repository is Rust:

- `lib/src_rust` contains the runtime;
- `lib/src/lib.c` is only the C build entry point; and
- `lib/src/lexer_log_shim.c` implements the variadic `TSLexer::log` callback,
  which stable Rust cannot define.

The Rust rewrite preserves the public C API and the generated-language ABI.
Internal parser, stack, and tree types use Rust layout unless their module
documents a compatibility boundary. `lib/src_rust/query.rs` is legacy code and
is outside this chapter's scope.

## The implementation in one call graph

The public entry point is `ts_parser_parse` in `parser.rs`:

```text
ts_parser_parse
|
|-- parser_advance(version)
|   |
|   |-- parser_get_initial_lookahead
|   |-- parser_lex_lookahead
|   |   `-- parser_lex
|   |
|   |-- language_table_entry(state, lookahead.symbol)
|   |-- parser_shift
|   |-- parser_reduce
|   |   |-- stack_pop_count
|   |   |-- subtree_new_node
|   |   `-- stack_merge
|   |
|   |-- parser_accept
|   `-- pause version on error
|
|-- parser_condense_stack
|   |-- stack_merge
|   |-- discard inferior versions
|   `-- parser_handle_error when needed
|
|-- parser_balance_subtree
`-- TSTree::new
```

There are four long-lived kinds of data:

| Data | Owner | Purpose |
| --- | --- | --- |
| `TSLanguageFull` | generated parser | Immutable lexer, table, symbol, field, and alias data |
| `TSParser` | caller through the parser API | Mutable state reused between parses |
| `Stack` | `TSParser` | GLR histories and recovery state |
| `Subtree` hierarchy | parser, stack, then `TSTree` | Recognized syntax and cached measurements |

The generated language answers “what is legal?” The parser decides “what do I
do now?” The stack records “which histories are still possible?” Subtrees
record “what syntax did those histories recognize?”

## Generated languages are executable data

A generated `parser.c` does not contain a custom LR loop. It exports an
immutable `TSLanguage` value containing tables and function pointers. The Rust
runtime casts that pointer to the full `TSLanguageFull` layout defined in
`language/generated.rs`.

This cast is a real ABI boundary. The following types are `#[repr(C)]` and have
compile-time size assertions:

- `TSLanguageFull`;
- `TSParseAction` and `TSParseActionEntry`;
- `TSLexMode` and `TSLexerMode`;
- `TSLexer`; and
- `TSExternalScanner`.

The generated parser and runtime must agree on every field and union arm.
These C-compatible unions are not accidental C style: generated language
tables contain their byte representation directly.

### Symbol numbers divide tokens from non-terminals

`TSLanguageFull::token_count` is the boundary:

```text
0 .. token_count              terminal symbols (tokens)
token_count .. symbol_count   non-terminal grammar symbols
```

Symbol zero is the built-in end symbol. The runtime also reserves
`TS_BUILTIN_SYM_ERROR` and `TS_BUILTIN_SYM_ERROR_REPEAT` outside the ordinary
generated symbol range.

This division changes how a table value is interpreted:

- for a terminal, it identifies a list of parse actions; and
- for a non-terminal, it is the LR goto state after a reduction.

`ts_language_next_state` hides that distinction. For a token it inspects the
last shift action. For a non-terminal it returns the table's goto value.

### Dense and compressed parse tables

`language_lookup(language, state, symbol)` implements both generated table
encodings.

Frequently used states occupy the dense `parse_table`. Their lookup is direct:

```text
parse_table[state * symbol_count + symbol]
```

States at or above `large_state_count` use `small_parse_table`. A
`small_parse_table_map` entry points to groups shaped conceptually like:

```text
group count
    table value, symbol count, symbol, symbol, ...
    table value, symbol count, symbol, symbol, ...
```

Symbols with the same table value share one group. This saves generated binary
size for sparse states at the cost of a short linear scan.

### Action lists

For a terminal table cell, `language_table_entry` follows the table value into
`TSLanguageFull::parse_actions`. The first `TSParseActionEntry` is a header:

```text
count       number of following actions
reusable    whether the token can be reused across compatible lex modes
```

The following entries are `TSParseAction` union values:

| Action | Important fields |
| --- | --- |
| shift | next state, `extra`, `repetition` |
| reduce | symbol, child count, production id, dynamic precedence |
| accept | action type only |
| recover | action type only |

`language_table_entry` returns a lightweight `TableEntry` containing a pointer
to those actions, their count, and the reusable flag. It does not allocate or
copy the generated data.

A cell with several actions is the concrete representation of a GLR conflict.
The runtime does not have a separate “ambiguous grammar” mode; it discovers the
need to branch by seeing several actions in this list.

### Lex modes and tree metadata

Every parse state also selects a `TSLexerMode`:

- `lex_state` selects a state in the generated lexer;
- `external_lex_state` selects the valid-token row for an external scanner;
  and
- `reserved_word_set_id` selects the contextual reserved words.

The remaining language tables explain how internal subtrees become a public
tree:

- `symbol_metadata` says whether a symbol is visible, named, or a supertype;
- `public_symbol_map` maps internal symbols to public ones;
- `alias_sequences` applies production-specific aliases;
- `field_map_slices` and `field_map_entries` attach fields to children; and
- `primary_state_ids` identifies equivalent public parse states.

The parser uses action and lexer tables while parsing. `node` and
`tree_cursor` use symbol, alias, and field tables while presenting the result.

## `TSParser`: the owner of mutable parse state

`TSParser` is opaque to the public C API, so its fields use Rust layout. One
instance owns:

| Field | Responsibility |
| --- | --- |
| `lexer` | Input callback, decoding, included ranges, and `TSLexer` callbacks |
| `stack` | All active, paused, and halted GLR versions |
| `tree_pool` | Reusable subtree allocations and release scratch space |
| `language` | Borrowed generated-language tables |
| `reduce_actions` | Scratch reductions considered by recovery |
| `finished_tree` | Best accepted root found so far |
| `trailing_extras*` | Scratch arrays used while constructing parents |
| `scratch_trees` | Borrowed child storage for comparing candidate parents |
| `token_cache` | One lexed token shared by compatible stack versions |
| `external_scanner_payload` | Mutable state owned by the language's scanner |
| `parse_options`, `parse_state` | Progress and cancellation state |

`ts_parser_new` creates the lexer and subtree pool and then creates a stack
whose base node is state 1. `ts_parser_set_language` checks ABI compatibility
before storing a language. `ts_parser_reset` destroys scanner state, clears the
stack back to its base version, releases cached and finished subtrees, and
resets progress state.

The parser can preserve work only when cancellation occurs during final tree
balancing. `canceled_balancing` tells the next `ts_parser_parse` call to resume
that stage.

## The outer parse loop

`ts_parser_parse` first checks that a language and input callback exist, then
installs the new `TSInput`. A fresh parse creates the external scanner. The
main loop is organized around fairness between GLR versions:

```text
loop:
    for version in current stack versions:
        while version is Active:
            parser_advance(version)

            if this version moved beyond the last serviced position:
                stop advancing it for this pass

    min_error_cost = parser_condense_stack()

    if an accepted tree is cheaper than every unfinished version:
        clear the stack and stop

    if no versions existed in this pass:
        stop
```

The position check prevents one successful version from running far ahead
while alternatives at the same input position wait. Versions created during a
reduction are visible to later iterations of the same outer loop.

Once parsing ends, `parser_balance_subtree` compresses deeply nested repeat
nodes. `parser_take_finished_tree` transfers the root into `TSTree::new`, and
`ts_parser_reset` clears parser-owned scratch state without releasing the root
that was just transferred.

### Cancellation accounting

`parser_check_progress` charges operations rather than checking the callback
after every instruction. After `OP_COUNT_PER_PARSER_CALLBACK_CHECK` operations
(currently 100), it updates the public byte offset and error flag and calls the
progress callback. Lexing, parse actions, and balancing all use this mechanism.

## Lexing one lookahead

Tree-sitter lexing has two layers:

1. `Lexer` manages input bytes, decoding, positions, and included ranges.
2. Generated and external lexer callbacks decide which token matches.

### Why `Lexer` begins with `TSLexer`

`Lexer` is `#[repr(C)]`, and its first field is `data: TSLexer`. Generated
lexers and external scanners receive `*mut TSLexer`; callback implementations
cast that prefix pointer back to `*mut Lexer` to reach the surrounding input
state.

This cast is intentional external-scanner ABI plumbing. The `TSLexer` prefix
must remain at offset zero, and its callback signatures must match generated C
code.

The callbacks are:

- `advance(skip)` consumes a decoded character and optionally moves the token
  start past it;
- `mark_end()` records the accepted token boundary;
- `get_column()` computes a column, rescanning from the line start when the
  cache is invalid;
- `is_at_included_range_start()` reports discontinuities; and
- `eof()` reports the logical end of included input.

`TSLexer::log` points to the small C variadic shim.

### Input chunks and positions

`TSInput::read` returns borrowed chunks. `Lexer` records the chunk pointer,
its start byte, and its byte length; it never owns that memory. Depending on
`TSInputEncoding`, the next character is decoded as UTF-8, UTF-16LE, UTF-16BE,
or by a custom decode callback.

Positions use `Length`:

```text
Length {
    bytes,
    extent: TSPoint { row, column },
}
```

For a token scan:

- `current_position` is the read cursor;
- `token_start_position` follows skipped trivia;
- `token_end_position` is the last `mark_end`; and
- `lookahead_bytes` records how far past the token the lexer inspected to
  decide the match.

Included ranges make logical input discontinuous. Seeking and `mark_end` take
care to jump between ranges without claiming that excluded bytes belong to a
token.

### `parser_lex`: external, internal, then error lexing

`parser_lex` obtains the `TSLexerMode` for the current parse state and resets
the lexer to the stack version's position.

If `external_lex_state` is nonzero, it first:

1. restores scanner bytes from the version's last external token;
2. calls the external scanner with the valid-token bitmap;
3. finalizes the token;
4. serializes the new scanner state; and
5. rejects a zero-width token that made no scanner-state progress when that
   token could cause an infinite recovery loop.

If the external scanner does not produce a token, the generated `lex_fn` runs
with `lex_state`. If it returns the grammar's word token, `keyword_lex_fn` can
rescan the exact same span and refine it to a contextual keyword.

If ordinary lexing fails, `parser_lex` switches to the lex mode for
`ERROR_STATE`. It advances until lexing can resume or EOF is reached, then
creates an error leaf covering the skipped bytes.

The successful result is always a `Subtree`: an ordinary leaf, external-token
leaf with serialized scanner state, EOF leaf, or error leaf.

### The one-token GLR cache

Several versions often request a token at the same byte. `TokenCache` retains
one token together with:

- its byte position; and
- the last external token whose scanner state was restored.

`parser_can_reuse_token` verifies compatible lexer modes, reserved-word state,
external-scanner conditions, empty-token rules, and the generated action
entry's `reusable` flag. This cache avoids lexing the same position once per
GLR version.

This is not old-tree incremental reuse. In the current Rust rewrite,
`ts_parser_parse` accepts `old_tree` for API compatibility but does not yet use
it. Editing, subtree change flags, and changed-range comparison are
implemented; reusing unchanged old subtrees during the parse is not currently
part of this Rust driver.

## Interpreting one parse-table entry

`parser_advance(parser, version)` is the action interpreter. It snapshots the
version's state, byte position, and last external token, then obtains a cached
or freshly lexed lookahead and its `TableEntry`.

Its inner loop does not necessarily advance input:

```text
loop:
    actions = table[state, lookahead.symbol]
    apply actions

    if a reduction produced a version:
        replace the current version with that result
        recompute the state
        keep the same lookahead
        continue

    if a shift, accept, or recovery action completed:
        return to the outer parse loop

    if reductions merged entirely into another version:
        halt this version
        return

    try contextual-keyword fallback
    otherwise pause this version with its lookahead
```

### Applying the action list

`parser_apply_parse_actions` visits generated actions in order:

- A normal shift calls `parser_shift` and finishes this advance.
- A repetition shift ends action processing without shifting here.
- A reduction is converted to the smaller internal `ReduceAction` and passed
  to `parser_reduce`.
- Accept calls `parser_accept`.
- Recover calls `parser_recover`.

When a table entry has multiple actions, the reduction path invalidates the
parse state cached on newly built subtrees. A subtree created under a conflict
cannot later claim that one deterministic state is sufficient to reproduce
its parse.

### The null-lookahead case

The generated lex mode `u16::MAX` means the parser is at the end of a
non-terminal extra and must reduce without lexing another token.
`parser_lex` returns `NULL_SUBTREE`; `parser_lex_lookahead` deliberately looks
up the end-symbol table entry. After the fixed reduction, the parser requests
a real lookahead using the new state.

This sentinel is internal control flow, not a syntax node and not EOF. EOF is a
real leaf whose symbol is `TS_BUILTIN_SYM_END`.

## Worked parse: `1 + 2`

Consider this simplified grammar:

```text
document   -> expression
expression -> number
expression -> expression "+" number
```

The state names below are descriptive rather than generated numeric ids. The
operations are the ones performed by the runtime.

### 1. Shift `1`

The generated lexer returns a `number` leaf. The terminal table entry contains
a shift:

```text
input:   1 + 2
         ^

table[Start, number] = Shift(AfterNumber)

head -> [AfterNumber]
          |
          | Subtree(number, "1")
          v
        [Start]
```

`parser_shift` calls `stack_push`. The leaf is likely inline, so the stack link
carries all of its syntax data in one eight-byte `Subtree` word.

### 2. Reduce without consuming `+`

The next lookahead is `+`. The table requests
`expression -> number`:

```text
lookahead: "+"                         lookahead is unchanged

before                                    after

head -> [AfterNumber]                    head -> [AfterExpression]
          |                                        |
          | number "1"                             | expression
          v                                        v
        [Start]                                   [Start]
```

`stack_pop_count(..., 1)` returns the `number` subtree and a version whose head
is back at `Start`. `subtree_new_node` makes an `expression` parent. The goto
table entry `table[Start, expression]` yields `AfterExpression`, where the
parent is pushed. `parser_advance` then looks up the same `+` token again using
the new state.

### 3. Shift `+`, then shift `2`

```text
head -> [AfterRightNumber]
          |
          | number "2"
          v
        [AfterPlus]
          |
          | "+"
          v
        [AfterExpression]
          |
          | expression(number "1")
          v
        [Start]
```

The input position advanced only on shifts. Each reduction changed the stack
and state while reusing the current lookahead.

### 4. Reduce three children at EOF

At EOF, the table reduces
`expression -> expression "+" number`:

```text
stack_pop_count(..., 3)
              |
              v

[ expression("1"), "+", number("2") ]
              |
              | subtree_new_node
              v

expression
|-- expression
|   `-- number "1"
|-- "+"
`-- number "2"
```

The new parent is pushed in the goto state. Further reductions build
`document`; the accept action then moves the best root into `finished_tree`.

The same three objects have different roles throughout the trace:

```text
generated table       StackNode/StackLink        Subtree storage
----------------      --------------------       ---------------
chooses action   ->   records possible path  ->  records recognized syntax
```

## The GLR stack is a persistent directed graph

The `stack` module does not store one array of `(state, symbol)` pairs. Its
central types are:

```text
Stack
  heads: Array<StackHead>       one entry per GLR version

StackHead
  node: NonNull<StackNode>      top of this version
  status                        Active, Paused, or Halted
  last_external_token           scanner state for this version
  lookahead_when_paused         retained token awaiting recovery
  summary                       optional recovery index

StackNode
  state                         LR state at this point
  position                      input position reached here
  links[0..link_count]          predecessor edges
  ref_count                     owners of this graph node
  error_cost                    cached path cost
  node_count                    cached visible syntax progress
  dynamic_precedence            cached precedence total

StackLink
  node                          predecessor StackNode
  subtree                       syntax recognized along this edge
```

Edges point backward, from a newer state to its predecessor:

```text
head
 |
 v
state 8
 |  link subtree: expression
 v
state 3
 |  link subtree: "+"
 v
state 6
 |  link subtree: expression
 v
state 1
```

The subtree belongs to the transition, not to either state. This matches the
LR idea that a semantic value is pushed together with the state reached after
recognizing it.

### Push is persistent

`stack_push` allocates one `StackNode` whose first link points at the version's
current head. It then moves only that `StackHead` to the new node. The old node
is not changed.

`stack_copy_version` therefore does not copy the history. It copies a
`StackHead`, retains the shared top node and last external token, and appends
the head to `Stack::heads`:

```text
before copy                       after each version pushes

version 0 --+                     version 0 -> C --+
            v                                      |
            B -> A                 version 1 -> D --+-> B -> A
```

The common prefix stays allocated once. Reference counts on `StackNode` track
heads and successor links that own it. When a count reaches zero,
`stack_node_release` releases link subtrees and predecessor nodes iteratively.
Up to 50 released nodes are kept in `Stack::node_pool` for reuse.

### Cached path measurements

`stack_node_new` derives its aggregates from the predecessor and the pushed
subtree:

- `position += subtree.total_size()`;
- `error_cost += subtree.error_cost()`;
- `node_count +=` the subtree's public syntax contribution; and
- `dynamic_precedence += subtree.dynamic_precedence()`.

These are path properties cached at each node. Version comparison can read
them from a head without walking the whole graph.

A null subtree is different: it represents a recovery discontinuity and does
not contribute source geometry. `stack_push` uses it to reset the
`node_count_at_last_error` checkpoint.

### Version states

A `StackHead` has one of three states:

| Status | Meaning |
| --- | --- |
| `Active` | `parser_advance` may process this version normally |
| `Paused` | no table action worked; the head owns `lookahead_when_paused` |
| `Halted` | the version should be removed during condensation |

Pausing does not immediately enter recovery. Other versions at the same input
get a chance to succeed first. Halting is also lazy because version indexes
are used while action and pop results are still being processed.

## Popping a graph can return several stacks

`stack_pop_count` delegates to the generic `stack_iter` in `stack/pop.rs`.
`stack_iter` maintains a set of DFS cursors called `StackIterator`s. Each
cursor owns:

- its current graph node;
- the subtrees collected on that backward path; and
- a count of grammar children traversed.

At a node with several links, the iterator is cloned for alternate links. Link
zero reuses the current iterator so the common linear case avoids an array
copy. When a path reaches the requested child count, it becomes a `StackSlice`:

```text
StackSlice {
    version,     // head positioned at the predecessor after the pop
    subtrees,    // popped children in source order
}
```

If two paths stop at different predecessor nodes, `stack_add_slice` creates a
version for each. If they stop at the same node, their slices share the same
version and are considered competing child lists for one reduction result.

### What counts toward a reduction

The iterator collects every non-null subtree, but only non-extra subtrees
increment `subtree_count`. Extras therefore travel with a production without
changing the generated reduction's `child_count`. A null recovery link counts
as one stack entry even though it adds no subtree.

Collected children are reversed before returning because graph traversal
walks newest-to-oldest while a parent stores children in source order.

### Bounds on ambiguity

The implementation deliberately bounds graph expansion:

- one `StackNode` stores at most 8 predecessor links;
- a pop uses at most 64 active iterators;
- normal condensation retains at most 6 versions; and
- reduction may temporarily overflow that count by 4 versions while results
  are being merged.

These limits are part of Tree-sitter's latency strategy. The runtime is a GLR
parser, but it is intended for interactive parsing rather than construction of
an unbounded parse forest.

## Reduction turns pop slices into parent subtrees

`parser_reduce` connects the stack graph to syntax-tree construction. Given a
`ReduceAction { symbol, count, dynamic_precedence, production_id }`, it runs
these steps:

1. `stack_pop_count(version, count)` enumerates matching graph paths.
2. Over-limit versions and their retained child arrays are released.
3. `subtree_array_remove_trailing_extras` removes extras after the production's
   final structural child.
4. `subtree_new_node` takes ownership of the remaining child array.
5. Competing child lists for the same predecessor are compared, retaining the
   preferred parent.
6. The reduction's dynamic precedence is added to the parent.
7. The generated goto for `(predecessor state, reduced symbol)` is looked up.
8. The parent is pushed, followed by the trailing extras in their original
   order.
9. The new version is merged into an earlier compatible version if possible.

### Why trailing extras are removed and pushed again

Extras such as comments may occur between or after structural children. They
are present on the parse stack but are not counted by grammar productions.
If an extra lies after the last child of a reduction, making it a child of the
new parent would give it the wrong syntactic scope. The parser temporarily
removes those extras, pushes the parent, and then pushes the extras above it.

### Selecting between child lists

When graph popping yields several child lists for the same predecessor,
`parser_select_children` constructs a borrowed scratch parent and delegates to
`parser_select_tree`. Candidate selection considers, in order:

1. smaller total error cost;
2. larger dynamic precedence;
3. structural comparison when both are otherwise valid; and
4. a stable preference between errorful alternatives.

The scratch parent from `subtree_new_scratch_node` borrows
`TSParser::scratch_trees`. It must never be retained or released. This avoids a
temporary child allocation while preserving the normal `Subtree` accessor
interface for comparison.

### Recorded parse state

An unambiguous parent records the predecessor LR state in
`SubtreeHeapData::parse_state`. That cached state can support safe subtree reuse
when incremental parsing is implemented.

The value becomes `TS_TREE_STATE_NONE` when:

- the parse-table entry had several actions;
- the graph pop had several results; or
- more than one stack version already existed.

In those cases no single deterministic state describes how the subtree was
constructed.

### Merging versions

`stack_can_merge` requires both heads to be active and to have equal:

- LR state;
- byte position;
- accumulated error cost; and
- serialized external-scanner state.

`stack_merge` adds the second head node's predecessor links to the first head
node and removes the second version. `stack_node_add_link` further coalesces
equivalent link subtrees and recursively combines predecessor nodes that have
the same state, position, and error cost.

Merging does not claim that the syntax histories were identical. It records
several histories behind one compatible future parser configuration.

### Worked GLR branch: `1 + 2 * 3`

Suppose precedence has deliberately not resolved whether to reduce `1 + 2` or
shift `*`. The table cell contains both actions:

```text
table[AfterAddition, "*"] = [
    Reduce(expression -> expression "+" expression),
    Shift(AfterStar),
]
```

Before the conflict there is one shared history:

```text
                         head 0
                           |
                           v
... -> expression "1" -> "+" -> expression "2"
```

The reduction pop creates a version containing the reduced parent, while the
shift path retains the unreduced history and consumes `*`:

```text
version R:  ... -> expression("1 + 2")
                   ^
                   | reduce path
                   |
              shared older nodes
                   |
                   | shift path
                   v
version S:  ... -> expression("1") -> "+" -> expression("2") -> "*"
```

Both heads can now parse `3`. If they later reach compatible state, position,
error cost, and scanner state, `stack_merge` stores their histories as
alternate predecessor links behind one head. If they remain distinct,
condensation compares their costs and dynamic precedence and eventually keeps
the better result.

## Subtree is an eight-byte tagged handle

`Subtree` and `MutableSubtree` in `subtree/handle.rs` are `#[repr(C)]` unions:

```text
Subtree
  data: SubtreeInlineData
  ptr:  *const SubtreeHeapData

MutableSubtree
  data: SubtreeInlineData
  ptr:  *mut SubtreeHeapData
```

Compile-time assertions require both handles and `SubtreeInlineData` to remain
eight bytes. This matches the compact representation used by Tree-sitter's C
ABI-facing subtree design.

### The discriminator is inside the word

The low bit of `SubtreeInlineData::flags` is `INLINE_IS_INLINE`. The same bit
overlaps the low bit of the pointer arm. Heap allocation alignment keeps valid
pointers' low bit clear, so:

```text
low bit 1   packed inline leaf
low bit 0   heap pointer or null sentinel
```

There is no separate enum tag and therefore no second machine word. A zeroed
non-inline word is `NULL_SUBTREE`.

Union access is concentrated in `subtree/handle.rs`. Callers use methods such
as `is_inline`, `symbol`, `padding`, `children`, and `heap_data` rather than
reading an arm directly.

### What fits inline

Only leaves without external-scanner state can be inline. The constructor also
requires:

- symbol at most 255;
- padding bytes and column below 255;
- padding rows below 16;
- content bytes and column below 255;
- content entirely on one row; and
- lookahead bytes below 16.

The eight bytes store:

```text
logical byte layout of SubtreeInlineData

  0             1             2..3          4
+-------------+-------------+-------------+-----------------+
| flags       | symbol      | parse_state | padding_columns |
+-------------+-------------+-------------+-----------------+

  5                           6               7
+---------------------------+---------------+------------+
| lookahead:4 | pad_rows:4  | padding_bytes | size_bytes |
+---------------------------+---------------+------------+

flags bit 0 = 1 marks the word as inline
```

Inline leaves have no allocation and no reference-count work. An edit that
makes a measurement exceed these limits promotes the leaf to heap storage.

### Heap representation

`SubtreeHeapData` stores common fields:

```text
ref_count: AtomicU32
padding: Length
size: Length
lookahead_bytes: u32
error_cost: u32
child_count: u32
symbol: TSSymbol
parse_state: TSStateId
flags: u16
data: SubtreeHeapDataContent
```

`SubtreeHeapDataContent` is an internal Rust enum selected by node kind:

- `Children(SubtreeChildrenData)` for internal nodes;
- `ExternalScannerState` for external-token leaves; or
- `LookaheadChar` for ordinary and error leaves.

Unlike the handle and generated parse actions, this payload does not cross the
C ABI and does not need a C union representation.

The packed flags record public visibility and parser properties including
`named`, `extra`, `has_changes`, `has_external_tokens`, scanner-state changes,
column dependence, missingness, and keyword status.

### Internal node allocation

An ordinary internal node uses one allocation arranged as:

```text
+-----------+-----------+-----+-----------------+
| child 0   | child 1   | ... | SubtreeHeapData |
+-----------+-----------+-----+-----------------+
                              ^
                              handle points here
```

`subtree_new_node` consumes a `SubtreeArray`. `subtree_take_children` resizes
that allocation to append the header and returns a pointer to the header.
Child access uses `child_count` to find the slice immediately before it.

This layout needs only one allocation for a parent and its child handles. It
also explains why child-array ownership is explicit: after
`subtree_new_node`, the caller must not delete or reuse the consumed array.

### Cached child summaries

`SubtreeChildrenData` stores values used frequently by navigation and parse
selection:

- direct visible and named child counts;
- total visible descendant count;
- accumulated dynamic precedence;
- repeat depth for balancing; and
- production id for fields and aliases.

`subtree_summarize_children` also computes the parent's padding, size,
lookahead, error cost, external-token flags, and column dependency. Public
child-count and descendant-count queries can therefore avoid recursive walks.

### Padding, size, and lookahead

Each subtree separates three spans:

```text
[ leading padding ][ subtree content ][ bytes inspected as lookahead ]
```

- `padding` belongs before the node's visible content and includes skipped
  whitespace or extras as represented by the lexer.
- `size` is the node's content extent, excluding padding.
- `lookahead_bytes` is not owned source content; it records how far recognition
  inspected beyond the end.

The stack advances by `total_size = padding + size`, never by
`lookahead_bytes`. Edits and recovery still use lookahead bytes to decide
whether a change may invalidate a token.

### Intrusive ownership and copy-on-write

Copying the eight-byte `Subtree` handle does not create an owner. Ownership is
explicit:

- `retain` increments a heap node's intrusive atomic count;
- `release` decrements it and iteratively releases children when it reaches
  zero;
- inline and null handles need neither operation; and
- `make_mut` reuses a uniquely owned heap allocation or clones a shared one.

Released headers are stored in `SubtreePool::free_trees` for reuse. Recursive
release uses `SubtreePool::tree_stack` as an explicit work stack, avoiding a
call-stack overflow on deep trees.

`MutableSubtree` is a capability used after uniqueness has been established;
the type alone does not prove unique ownership. Mutation sites must still obey
the reference-count invariant.

### External-scanner bytes

`ExternalScannerState` stores up to 24 serialized bytes inline. Larger states
own a separate heap allocation. The bytes are immutable once attached to a
shared subtree, allowing GLR versions to compare scanner states without
sharing the scanner's mutable payload.

The scanner payload is live mutable state inside `TSParser`; the serialized
bytes in subtrees are checkpoints.

## Recovery is a search over stack versions

Recovery is not one “skip invalid token” branch. It is a staged search that
tries to preserve as much valid structure as possible while assigning costs to
repairs.

The relevant modules are:

- `parser/advance.rs` detects failure and chooses which version should recover;
- `parser/recovery.rs` creates repaired versions;
- `stack/pop.rs` records and revisits earlier states; and
- `error_costs.rs` defines the comparison weights.

### Stage 1: pause instead of recovering immediately

If an action list contains no shift, reduction, accept, or explicit recover
action, `parser_advance` calls `parser_pause_with_error`. The version becomes
`Paused` and retains its current lookahead.

The parser then advances other versions. During `parser_condense_stack`:

1. halted versions are removed;
2. active versions are compared and merged or pruned;
3. versions are ordered from most to least promising;
4. only the best six are kept; and
5. paused versions are discarded if an unpaused version can continue.

Only when the best remaining path is paused does condensation resume it and
call `parser_handle_error`.

This delay matters in GLR: an error on one interpretation should not damage the
tree if another interpretation accepts the same lookahead normally.

### Error costs and version comparison

The current costs are:

| Event | Cost |
| --- | ---: |
| enter recovery | 500 |
| insert a missing node | 110 |
| skip one subtree | 100 |
| skip one source line | 30 |
| skip one character | 1 |

`StackNode::error_cost` accumulates costs contributed by its subtrees.
`Stack::error_cost` adds the fixed recovery cost for paused versions and error
discontinuities. `parser_version_status` adds another skipped-tree cost while a
version is paused and records:

```text
ErrorStatus {
    cost,
    node_count since the last error,
    dynamic_precedence,
    is_in_error,
}
```

`parser_compare_versions` prefers a non-error version, then lower cost, then
higher dynamic precedence. A cost difference becomes decisive only when:

```text
cost difference * (1 + valid nodes since error) > MAX_COST_DIFFERENCE
```

This gives a repaired version time to demonstrate valid progress. A small
early cost difference should not permanently eliminate a path that soon
parses a long valid region.

### Stage 2: reductions independent of the lookahead

`parser_handle_error` first calls `parser_do_all_potential_reductions` with
lookahead symbol zero. This explores reductions that might have been delayed
by skipped invalid input.

The resulting versions represent plausible completed constructs at the same
source position. Recovery will merge their histories behind an error-state
discontinuity rather than throwing them away immediately.

### Stage 3: try one missing terminal

For each candidate version, `parser_try_insert_missing_token` scans terminal
symbols. A symbol is considered only if:

1. `ts_language_next_state(current_state, symbol)` reaches a different,
   nonzero state; and
2. from that state, the real lookahead permits a reduction.

The function copies the version head, creates a zero-width missing leaf, pushes
it, and performs every potential reduction for the real lookahead. If this
produces a usable state, the new version stays active alongside other
alternatives.

This is deliberately a narrow search: one plausible missing terminal followed
by reductions, not arbitrary insertion sequences.

### Stage 4: enter `ERROR_STATE` and record a summary

Recovery pushes `NULL_SUBTREE` with `ERROR_STATE` on each version created by
the preliminary reductions. That null link is a discontinuity: it records a
state transition without claiming source content or a syntax child.

The versions are then merged, and `stack_record_summary` walks predecessor
paths up to `MAX_SUMMARY_DEPTH` (currently 16). Each `StackSummaryEntry` stores:

```text
position
depth from the recovery head
LR state
```

Duplicate `(depth, state)` entries are suppressed. The summary is a compact
index of earlier parser configurations that might accept a future token.

### Stage 5: recover to a previous state

`parser_recover` receives the same lookahead that originally failed. If it is
not already an error token, it scans the recorded summary for an earlier state
that has actions for this lookahead.

For a candidate summary entry, the estimated cost is:

```text
current error cost
+ depth * skipped-tree cost
+ skipped bytes * skipped-character cost
+ skipped rows * skipped-line cost
```

If no clearly better version exists, `parser_recover_to_state` pops to that
depth, wraps the abandoned subtrees in an error node, pushes the error node in
the recovered state, and lets ordinary parsing try the lookahead again.

This strategy preserves a large valid prefix and gives the parser a chance to
resynchronize at a state where the current token makes grammatical sense.

### Stage 6: EOF recovery

If the failing lookahead is EOF, recovery cannot skip forward. It creates an
error parent, pushes it in the initial state, and calls `parser_accept`. The
result is a complete tree whose unfinished suffix is explicitly erroneous.

### Stage 7: skip the current token

If previous-state recovery did not make skipping unnecessary,
`parser_recover` computes the cost of skipping the lookahead:

```text
current error cost
+ one skipped-tree cost
+ token bytes * skipped-character cost
+ token rows * skipped-line cost
```

If another version or finished tree is already better, this version is halted.
Otherwise the token becomes a child of an internal
`TS_BUILTIN_SYM_ERROR_REPEAT` node and is pushed in `ERROR_STATE`.

Consecutive skipped tokens are combined: the parser pops an existing error
repeat, appends the new token, builds a replacement error repeat, and pushes it
back. This avoids a tall chain of one-token error wrappers.

If the skipped token carries external-scanner state, that token becomes the
version's new `last_external_token`. Recovery cannot allow the scanner's
logical position to drift away from the source position.

### Missing nodes and error nodes are different repairs

Both are visible through the public tree API, but their representation and
meaning differ:

| Repair | Source width | Representation | Meaning |
| --- | ---: | --- | --- |
| missing node | zero | leaf with `is_missing` flag | grammar expected a symbol that was absent |
| skipped input | nonzero | child of `ERROR_REPEAT`, later exposed as error structure | source text could not be used here |
| recovery discontinuity | zero | null stack link only | internal boundary between valid and recovery states |

The null discontinuity never appears in the final tree. Missing and error
subtrees do.

### Worked recovery: `1 + * 2`

Using an expression grammar, assume the parser has shifted `1 +` and is in a
state that expects the beginning of another expression. The lexer returns `*`:

```text
source:  1 + * 2
               ^ lookahead

table[ExpectExpression, "*"] = no action
```

The recovery procedure is:

```text
Active version
    |
    | no table action
    v
Paused version, owns lookahead "*"
    |
    | condense: no other version succeeds
    v
parser_handle_error
    |
    |-- try lookahead-independent reductions
    |-- try one missing terminal
    |-- push null discontinuity in ERROR_STATE
    |-- record up to 16 levels of earlier states
    v
parser_recover("*")
    |
    |-- try a summarized state that accepts "*"
    `-- otherwise skip "*" into ERROR_REPEAT
```

Depending on the generated table, missing-token insertion might find a token
that can stand for the absent right operand and allow a reduction before `*`.
That creates a zero-width missing leaf on a copied version:

```text
expression
|-- number "1"
|-- "+"
`-- number MISSING
```

If no such candidate works, the token is skipped:

```text
stack recovery representation

[ExpectExpression]
        |
        | NULL_SUBTREE                 internal discontinuity
        v
    [ERROR_STATE]
        |
        | ERROR_REPEAT
        |   `-- "*"                    owns the skipped source byte
        v
    [ERROR_STATE]
```

The next lookahead is `2`. Summary-based recovery can return to a state that
accepts a number, or error-state actions can continue until a valid expression
is built. The final public tree contains either a missing node or error
structure, never the null stack link:

```text
possible public result

expression
|-- number "1"
|-- "+"
|-- ERROR
|   `-- "*"
`-- number "2"
```

The exact shape depends on the grammar's generated states and actions. The
important implementation point is that recovery creates ordinary competing
stack versions with explicit costs; it does not mutate the grammar or return a
special partial-tree type.

## Accepting and choosing a root

`parser_accept` pushes the EOF subtree, pops every complete graph path, and
constructs one root candidate per slice. Leading or trailing extras are folded
into the root's child allocation so the finished tree covers all included
input.

`parser_select_tree` chooses between accepted roots using error cost, dynamic
precedence, and structural ordering. The winner is retained in
`TSParser::finished_tree`; the others are released.

Parsing can end before every version fails or accepts. After condensation,
`ts_parser_parse` compares the best finished tree's error cost with the minimum
unfinished cost. If the finished tree is strictly cheaper, no unfinished path
can improve the result enough to matter, so the stack is cleared.

## From internal storage to public tree values

The public tree API intentionally avoids parent pointers and per-node wrapper
allocations.

The relationship between public and internal values is:

```text
TSTree
  |
  | owns root handle
  v
Subtree slot --points to--> [ child handles ][ heap header ]
  ^                                  |
  |                                  `-- owns descendants recursively
  |
TSNode.id
  ^
  | borrowed; also carries start position and alias
  |
TreeCursorEntry.subtree
```

`TSTree` is the lifetime owner. `TSNode` and cursor entries are coordinates
into its immutable subtree storage.

### `TSTree` owns the hierarchy

`TSTree` contains:

```text
root: Subtree
language: *const TSLanguage
included_ranges: Array<TSRange>
```

The tree owns one retained root. Children are owned transitively by internal
subtree allocations. Copying a tree retains the same immutable root; editing a
shared tree uses subtree copy-on-write where necessary.

The language pointer gives meaning to symbol numbers, aliases, production ids,
and field ids. Included ranges are copied because changed-range comparison
must know which source regions were visible to each parse.

### `TSNode` is a borrowed coordinate

`node_new` fills the public `TSNode` as:

```text
context[0] = start byte
context[1] = start row
context[2] = start column
context[3] = alias symbol
id         = pointer to the Subtree handle
tree       = owning TSTree pointer
```

A node does not retain anything. It is valid only while its `TSTree` remains
alive. Storing the absolute start position avoids recomputing it for simple
accessors; navigation computes new positions as it walks children.

The `id` points to a `Subtree` handle slot, not necessarily directly to
`SubtreeHeapData`. This is important for inline leaves: their entire value
lives in that slot.

### Visible children are a projection

Stored child arrays preserve grammar structure, including invisible helper
rules and extras. Public child APIs expose a projection:

- visible children are returned directly;
- invisible children are recursively flattened;
- named-child APIs also filter anonymous symbols;
- production alias sequences can replace the child's symbol; and
- field maps are carried through hidden productions.

Cached visible and named counts in `SubtreeChildrenData` let the navigation
code skip entire hidden subtrees when the requested index lies elsewhere.

### `TreeCursor` caches a path

A tree cursor stores `TreeCursorEntry` frames from its cursor root to the
current subtree. Each frame contains:

```text
subtree pointer
absolute position
raw child index
structural child index excluding extras
public visible descendant index
```

This path supplies parent and previous-sibling navigation even though
subtrees have no parent pointers. The cursor's outer layout is `#[repr(C)]`
and compile-time checked against public `TSTreeCursor`; its heap entry array is
internal Rust-layout storage.

## Editing and changed ranges

`ts_tree_edit` converts `TSInputEdit` into old and new `Length` coordinates and
calls `subtree_edit`.

The edit traversal:

1. updates padding and size for overlapping nodes;
2. marks affected nodes and ancestors with `has_changes`;
3. adjusts child positions after the edit;
4. descends only where the edit intersects children or their lookahead; and
5. promotes inline leaves when new geometry no longer fits inline fields.

This prepares an old tree for comparison and, in a full incremental driver,
for subtree reuse.

Changed ranges are computed separately by `subtree_get_changed_ranges_ref`.
Two `DiffIterator`s walk old and new visible trees in lockstep:

- `Matches` skips a whole subtree;
- `MayDiffer` descends because cached state, changes, included ranges, or
  scanner state prevent a conclusive match; and
- `Differs` emits a range and advances past the incompatible span.

The comparison is structural, not a byte diff. Included-range differences are
merged into the decision because identical subtree bytes can have different
meaning when different parts of the document were parsed.

## Internal arrays and allocator behavior

`utils::Array<T>` stores:

```text
contents: *mut T
size: u32
capacity: u32
```

It resembles C's `Array` helper but is internal Rust-layout storage. It exists
instead of `Vec<T>` because Tree-sitter exposes configurable allocation
functions; `Array` consistently uses the runtime's `malloc`, `calloc`,
`realloc`, and `free` adapters.

`Array` does not automatically understand intrusive `Subtree` ownership.
Subtree arrays therefore use explicit operations:

- `subtree_array_copy` copies handles and retains each heap subtree;
- `subtree_array_clear` releases elements but keeps capacity; and
- `subtree_array_delete` releases elements and frees storage.

This division is easy to miss: `Array<T>` owns its buffer, while the operation
appropriate for its elements depends on `T`.

## Where raw pointers remain intentional

Most remaining raw pointers fall into a small number of representation needs:

| Boundary | Why a raw pointer is required |
| --- | --- |
| public C API arguments | functions receive opaque or nullable C pointers |
| `TSLanguageFull` tables | generated C data is immutable pointer-based storage |
| `TSLexer` prefix | generated lexers and external scanners call a fixed vtable |
| compact `Subtree` | one word is either inline bits or a heap pointer |
| graph links | stable node addresses survive array growth and shared histories |
| `TSNode::id` | borrowed identity points at a subtree handle inside a tree |
| custom `Array` | allocation must use Tree-sitter's configured allocator |

The implementation tries to contain interpretation near each boundary:

- `language/generated.rs` owns generated C layouts;
- `subtree/handle.rs` owns subtree union access;
- `lexer.rs` owns `TSLexer` prefix casts;
- `utils::ptr_ref` and `ptr_mut` adapt validated non-null API pointers; and
- module methods expose references and slices for ordinary internal logic.

## Invariants worth checking while reading code

Many unsafe operations are local consequences of a few global invariants:

1. A live `StackHead` owns one reference to its top `StackNode`.
2. A `StackLink` owns its predecessor node and non-null subtree.
3. Copying a `Subtree` handle creates no ownership until `retain` is called.
4. A heap subtree with reference count greater than one is immutable.
5. An internal subtree owns the child allocation immediately before its
   header.
6. A scratch subtree borrows its child array and is never retained or
   released.
7. `TSNode` and `TreeCursorEntry` subtree pointers cannot outlive their
   `TSTree`.
8. Stack versions can merge only when parser state, source position, error
   cost, and external-scanner state are compatible.
9. A paused stack head owns its saved lookahead until it is resumed or
   deleted.
10. Generated-layout types and exported cursor storage preserve their asserted
    C sizes and offsets.

When one of these is unclear at a call site, follow ownership before following
control flow. Most of the runtime's apparent complexity comes from preserving
these data relationships across GLR branching.

## Suggested source-reading passes

For an implementation-first reading, use this order:

1. `language/generated.rs` and `language_table_entry` — see the input data.
2. `parser.rs::ts_parser_parse` — see scheduling and termination.
3. `parser/lexing.rs::parser_lex` — see how one token becomes a subtree.
4. `parser/advance.rs::parser_advance` — see one table cell interpreted.
5. `stack.rs::stack_push`, `stack_copy_version`, and `stack_merge` — see graph
   persistence.
6. `stack/pop.rs::stack_iter` — see why one reduction can yield many paths.
7. `parser/reduction.rs::parser_reduce` — see paths become parents.
8. `subtree/data.rs`, `handle.rs`, and `storage.rs` — see representation and
   ownership.
9. `parser/recovery.rs::parser_handle_error` and `parser_recover` — see repair
   search.
10. `tree.rs`, `node.rs`, and `tree_cursor.rs` — see the public projection.
11. `get_changed_ranges.rs` and `subtree/edit.rs` — see post-parse incremental
    data handling.

That order keeps every new data structure motivated by an operation already
seen, while still exposing the implementation details that determine
performance and safety.
