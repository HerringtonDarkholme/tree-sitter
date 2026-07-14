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
