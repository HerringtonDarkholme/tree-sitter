# How Tree-sitter Works

This chapter builds a mental model of Tree-sitter from the smallest useful
idea to the full runtime. It assumes no knowledge of parser implementation.

For concrete Rust types, memory layouts, function-by-function control flow,
and the complete recovery algorithm, continue afterward with the
[Runtime Implementation Deep Dive](./5-implementation-deep-dive.md).

Tree-sitter has two main jobs:

1. Turn a grammar into tables and lexer functions. This happens once, when
   `tree-sitter generate` creates a language's `parser.c`.
2. Use those tables to turn source text into a syntax tree. This happens every
   time an application calls `ts_parser_parse`.

The generated parser describes one language. The Tree-sitter runtime supplies
the reusable parsing algorithm, syntax-tree storage, incremental updates, and
tree navigation.

```text
grammar.js
    |
    | tree-sitter generate
    v
generated language (parser.c)       source text
    |                                   |
    +---------------+-------------------+
                    |
                    v
             Tree-sitter runtime
                    |
                    v
                syntax tree
```

Keeping the generated language separate from the runtime is the first useful
idea: generated code mostly contains *data*, while the runtime contains the
algorithm that interprets that data.

## The vocabulary, with one example

Consider a small expression grammar:

```javascript
expression: choice(
  $.number,
  seq("(", $.expression, ")"),
  seq($.expression, "+", $.expression),
  seq($.expression, "*", $.expression),
)
```

The important terms are:

| Term | Meaning | Example |
| --- | --- | --- |
| token or terminal | A unit produced by the lexer | `12`, `+`, `(` |
| non-terminal | A grammar rule built from other symbols | `expression` |
| production | One way to build a non-terminal | `expression + expression` |
| state | A number summarizing what the parser has recognized | “an expression has just ended” |
| lookahead | The next token, not consumed yet | `*` |
| parse table | A mapping from `(state, lookahead)` to actions | shift, reduce, accept, or recover |
| subtree | A token or completed grammar production stored in the syntax tree | `(number)` |

Tree-sitter generates both lexical tables, which turn characters into tokens,
and parse tables, which turn tokens into a tree. The runtime moves between the
two: the current parse state tells the lexer which tokens are valid, and the
returned token selects a parse-table entry.

## First: ordinary LR parsing

Before GLR, it helps to understand the ordinary case. An LR parser owns a
stack. Each stack entry records a parse state and the subtree that led to that
state. The parser repeats this loop:

1. Read, or reuse, one lookahead token.
2. Find the parse-table entry for `(current state, lookahead symbol)`.
3. Perform the selected action.

There are four actions worth remembering:

- **Shift** consumes the lookahead, pushes its subtree, and enters another
  state.
- **Reduce** recognizes the end of a grammar production. It pops that
  production's children, makes one parent subtree, and follows the table's
  `goto` state for that parent. It does *not* consume the lookahead.
- **Accept** records a complete syntax tree.
- **Recover** starts explicit error recovery.

### What an LR parse table looks like

Consider these productions:

```text
R1: expression -> number
R2: expression -> expression "+" number
```

An LR table has terminal columns for actions and non-terminal columns for
gotos. Here is a small illustrative table for this grammar:

| State | `number` | `+` | EOF | `expression` |
| ---: | --- | --- | --- | --- |
| 0 | shift 2 |  |  | goto 1 |
| 1 |  | shift 3 | accept |  |
| 2 |  | reduce R1 | reduce R1 |  |
| 3 | shift 4 |  |  |  |
| 4 |  | reduce R2 | reduce R2 |  |

The state numbers have no meaning outside this generated table. Each row
describes one parser configuration:

- For a terminal lookahead, read the ACTION part: shift, reduce, accept, or an
  error represented by an empty cell.
- After reducing to a non-terminal, read the GOTO part to find the next state.

For `1 + 2`, treating each number as the token `number`, the lookups are:

```text
stack state 0, lookahead number -> shift 2
stack state 2, lookahead +      -> reduce R1

    pop one grammar value
    predecessor state is 0
    goto[0, expression] = 1

stack state 1, lookahead +      -> shift 3
stack state 3, lookahead number -> shift 4
stack state 4, lookahead EOF    -> reduce R2

    pop three grammar values
    predecessor state is 0
    goto[0, expression] = 1

stack state 1, lookahead EOF    -> accept
```

Notice that reduction consults the table twice in different ways. The current
state and terminal lookahead select the reduction. After popping, the exposed
predecessor state and the reduced non-terminal select the goto state.

An ordinary deterministic LR table expects one action in each terminal cell.
A conflicting cell might instead look like:

```text
shift 3 / reduce R2
```

A parser generator can resolve that conflict using precedence, or a GLR table
can preserve both actions. Tree-sitter stores the cell as a list of actions and
lets the runtime follow the alternatives.

The deep dive shows how this logical table becomes Tree-sitter's
[dense `u16` rows, compressed groups, and action arrays](./5-implementation-deep-dive.md#concrete-dense-representation).

### What “subtree” means in an LR parser

A subtree is not a separate part of LR theory. The essential LR algorithm
needs grammar symbols and parse states; an implementation can attach a value
to each recognized symbol. Tree-sitter's attached value is a `Subtree`.

The distinction is:

- a **parse state** records what the LR algorithm can do next; and
- a **subtree** records the syntax already recognized on the way to that
  state.

A textbook LR stack is often written as alternating states and grammar
symbols:

```text
state 0, number, state 4, "+", state 7, number, state 4
```

For syntax-tree construction, each grammar symbol also needs a value. In
Tree-sitter that value is its subtree, so the same stack is better read as:

```text
state 0,
    (number subtree for "1", state 4),
    ("+" subtree,             state 7),
    (number subtree for "2", state 4)
```

Each pair says: “starting in the previous state, recognizing this subtree led
to the next state.” The state does not contain the subtree. State 4 might be
reached after many different numbers at many source positions; it only
summarizes which actions are legal next.

A useful analogy is a program counter and a runtime value. The LR state is like
the program counter: it selects the next instruction. The subtree is like the
value being carried through that execution. Reaching the same instruction
does not imply that two executions carry the same value.

A shifted token produces a leaf subtree. For example, after shifting the
tokens in `1 + 2`, one parse path conceptually contains:

```text
state 0 -- "1" --> state 4 -- "+" --> state 7 -- "2" --> state 4
           ^                    ^                    ^
       leaf subtree         leaf subtree         leaf subtree
```

The real graph-structured stack stores states in stack nodes and subtrees on
the links between them. The pointer on a link faces backward, so the operation
“state 0 recognizes `number "1"` and reaches state 4” is stored as:

```text
node(state 4) -- number subtree "1" --> node(state 0)
```

Reading the arrow backward reconstructs the parse history. A linear LR stack
has one predecessor link at each node. GLR permits several links when several
histories reach a compatible current state.

When the parser reduces `expression + expression` to `expression`, it pops the
three corresponding subtrees, makes a parent subtree containing them, and
pushes that parent with the next LR state:

```text
expression   "+"   expression       expression
     \        |        /       ->    /    |    \
      existing subtrees         expression "+" expression
```

The accepted root subtree eventually becomes the root of the public syntax
tree. During parsing, a subtree can therefore be a token, a completed grammar
production, a missing node, or an `ERROR` node. In all cases it means “the
piece of syntax already recognized by this parse path.” The distinction
between shift and reduce explains most of the runtime:
lexing supplies a lookahead, shifting moves forward in the input, and reducing
builds larger tree nodes without moving forward.

### One token can cause several reductions

Suppose the parser has recognized `1 + 2` and the lookahead is `)`. It may
reduce `2` to an expression and then reduce `1 + 2` to another expression,
all while keeping `)` as the lookahead. Only a later shift consumes `)`.

This is why Tree-sitter's action interpreter has an inner reduction loop
inside the outer loop that advances through the input.

## Why Tree-sitter needs GLR

An ordinary LR parse-table cell contains one action. Some useful grammars need
more than one.

With the expression grammar above, after `1 + 2` and before `*`, two actions
can make sense:

- Reduce `1 + 2` first, eventually producing `(1 + 2) * 3`.
- Shift `*` first, eventually producing `1 + (2 * 3)`.

This is a **shift/reduce conflict**. Two reductions can also compete; that is a
**reduce/reduce conflict**.

Static precedence and associativity normally resolve known conflicts while
the parser is generated. For example, giving `*` higher precedence than `+`
makes the table choose the second interpretation immediately. When a grammar
declares a genuine conflict, however, the generated table keeps multiple
actions. The runtime must try them.

That is where Tree-sitter uses **Generalized LR (GLR)** parsing. A helpful
definition is:

> GLR is LR parsing that can follow several valid stack histories at once.

Tree-sitter remains on one path during deterministic input. It branches only
when a parse-table entry contains competing actions or when recovery needs to
explore alternatives. This makes the common case look much like ordinary LR
parsing.

## A stack “version” is one possible parse

When an action conflicts, Tree-sitter creates multiple heads for its parse
stack. The code calls each head a **stack version**.

```text
shared history:  ... -> A -> B
                            |\
                            | +-> C   version 0
                            |
                            +----> D  version 1
```

Copying the whole stack at every conflict would be expensive. Instead,
Tree-sitter stores stack nodes as a persistent graph:

- A new push creates a node that points to the previous node.
- Versions with the same history share the old nodes.
- A version is mainly a pointer to one graph node plus a small amount of
  per-version state.
- Compatible versions can merge, giving a graph node more than one path back
  through the history.

This structure is commonly called a **graph-structured stack**. “Graph” sounds
more complicated than the operational rule: a push follows one edge forward;
a pop walks edges backward; if a pop encounters two predecessor paths, it
returns both results.

### How one node gets multiple predecessors

A conflict first creates separate versions. Each newly pushed node still has
one predecessor:

```text
version A                         version B

node(state S)                     node(state S)
     |                                 |
     | subtree X                       | subtree Y
     v                                 v
predecessor A                     predecessor B
```

After more parsing, the versions can arrive at the same current LR state and
input position. If their error and external-scanner states are also
compatible, future table lookups will be identical. Tree-sitter keeps one
current node and moves both histories onto it:

```text
                         shared head
                              |
                              v
                        node(state S)
                         /          \
              subtree X/            \subtree Y
                       v              v
                predecessor A   predecessor B
```

The current node now has two predecessor links. This does not mean the pasts
were equal. It means they have equivalent futures, so forward parsing can be
shared while the different pasts remain available to later reductions.

If a reduction pops one grammar child from this node, it gets two results:

```text
predecessor A with children [X]
predecessor B with children [Y]
```

This is how versions can merge without losing their alternative syntax trees.

### Reduction in the graph-structured stack

In a linear stack, reducing a three-symbol production has one result: pop
three entries. In the graph stack there may be several paths of length three.
Tree-sitter enumerates those paths. For each path it:

1. collects the popped subtrees;
2. builds the production's parent subtree;
3. finds the next parse state;
4. pushes the parent on the corresponding version; and
5. merges that version with a compatible one when possible.

The branching therefore stays localized. Shared prefixes remain shared, and
equivalent heads are brought back together.

## How Tree-sitter chooses among parses

GLR can preserve alternatives while more input is examined, but Tree-sitter's
public result is one concrete syntax tree, not a parse forest. The runtime
continually compares, merges, and prunes versions.

The main signals are:

- whether a version can continue without recovery;
- accumulated error cost;
- how much valid input it has consumed since an error;
- dynamic precedence supplied by the grammar; and
- whether two versions have reached compatible state and scanner state.

After advancing the active versions, the parser **condenses** the stack. It
removes halted or clearly worse versions, merges compatible versions, orders
the remaining versions by quality, and enforces a fixed upper bound on their
count. If every useful version is paused at an error, it resumes the best one
and performs recovery.

This is an important practical difference from the simplest description of
GLR: Tree-sitter does not keep every conceivable parse forever. It is designed
to produce a useful tree quickly while a file is being edited.

## Error recovery is part of normal parsing

Source files are often temporarily invalid. Tree-sitter still has to return a
tree, so failure of a table lookup does not normally end the parse.

At a high level:

1. A version that cannot handle its lookahead is paused.
2. Other versions get a chance to advance normally.
3. If no good version remains, the best paused version tries reductions that
   do not depend on the invalid lookahead.
4. The parser tries inserting one plausible missing token.
5. It records earlier stack states and tries returning to one that accepts the
   current lookahead.
6. If those repairs fail, it skips the lookahead into an `ERROR` node.
7. Every repair receives a cost so that a later error-free interpretation wins
   when possible.

For example, in `1 + * 2`, a state expecting an expression has no action for
`*`. The parser first pauses that stack version. Depending on the generated
table, it may insert a zero-width missing operand, or it may skip `*` and
continue at `2`:

```text
source:  1 + * 2
             ^ unexpected token

possible repair A               possible repair B

expression                      expression
|-- number "1"                  |-- number "1"
|-- "+"                         |-- "+"
`-- number MISSING              |-- ERROR "*"
                                `-- number "2"
```

Missing nodes mean that the grammar expected absent syntax. `ERROR` nodes own
source text that could not be used. Internally, Tree-sitter can also push a
zero-width recovery discontinuity on the parse stack; unlike missing and error
nodes, that marker never appears in the public tree.

The syntax tree exposes the result rather than hiding it: unexpected input is
represented by `ERROR` nodes and inferred omissions by missing nodes. Tools can
therefore keep navigating the parts of the file that are valid.

The deep dive's [recovery section](./5-implementation-deep-dive.md#recovery-is-a-search-over-stack-versions)
explains stack summaries, missing-token validation, error costs, and skipped
token grouping in implementation order.

## Lexing is guided by the parse state

Tree-sitter does not run a completely independent lexer over the whole file
first. The current parse state selects a lexical mode, so the generated lexer
only needs to distinguish tokens that are valid in that context.

The runtime lexer:

- buffers bytes from the application's `TSInput` callback;
- decodes UTF-8, UTF-16, or a custom encoding;
- tracks byte offsets and row/column positions;
- respects included ranges; and
- presents the stable `TSLexer` callback interface to generated lexers and
  external scanners.

External scanners participate when tokenization needs language-specific state
that the generated finite-state lexer cannot express conveniently, such as
indentation or nested delimiters. Their serialized state travels with stack
versions so that GLR alternatives can scan consistently.

Lexical precedence and parse precedence solve different problems. Lexical
precedence chooses which token the lexer returns for the same characters;
parse precedence chooses between grammar actions for already recognized
tokens.

## From accepted subtrees to a public tree

Tokens and reduced productions use the same internal `Subtree` handle. Small
leaf nodes can be stored inline; larger nodes refer to heap data containing
their children and cached measurements. Each subtree records enough summary
information for parsing and navigation, including its symbol, byte and point
extent, child counts, error cost, and external scanner state.

When a version accepts, its root subtree becomes a candidate final tree. The
runtime chooses the best accepted candidate, balances large child arrays when
needed, and wraps the root with the language and included ranges in a
`TSTree`. `TSNode` values are lightweight references into that tree, and a
`TSTreeCursor` keeps a path for efficient repeated navigation.

## Incremental parsing

Incremental parsing adds one idea to the same algorithm: an unchanged old
subtree can stand in for all of the shifts and reductions that originally
built it.

The application first describes an edit with `ts_tree_edit`, which moves old
node positions and marks affected regions. During the next parse, Tree-sitter
compares the new input with the edited old tree. When an old subtree:

- starts at the current input position;
- is outside the changed region;
- is valid in the current parse state; and
- has compatible external scanner state,

the parser can reuse it as one unit. If it cannot be reused, parsing falls back
to ordinary lexing and parse actions for that region.

```text
old tree:  [ unchanged declaration ][ edited function ][ unchanged class ]
new parse: [       reuse           ][ parse again     ][      reuse      ]
```

Afterward, changed-range comparison walks the old and new trees together. It
skips structurally identical regions and reports the ranges whose structure
differs. Incremental parsing and changed-range reporting are related but
separate: one builds the new tree efficiently; the other explains what changed
between two completed trees.

## The runtime loop as pseudocode

The whole runtime can now be summarized without C or Rust details:

```text
initialize one stack version

loop:
    for each active stack version:
        obtain a reusable subtree or lex one lookahead token

        loop:
            actions = parse_table[state, lookahead.symbol]

            if actions contain reductions:
                pop every matching stack path
                build and push parent subtrees
                branch or merge versions as needed
                continue with the same lookahead

            else if an action shifts:
                push lookahead and advance the input
                break

            else if an action accepts:
                save this root as a finished candidate
                stop this version

            else:
                pause this version at an error
                break

    merge compatible versions and prune worse versions
    recover if every useful version is paused
    stop when no in-progress version can beat the best accepted tree

balance and return the best tree
```

## Map of the Rust runtime

The Rust core rewrite lives in `lib/src_rust`. Its large modules follow the
runtime concepts above:

| Module | Responsibility |
| --- | --- |
| `parser` | Owns parser state and drives the outer GLR loop |
| `parser::advance` | Interprets parse-table actions and condenses versions |
| `parser::lexing` | Reuses or obtains lookahead tokens |
| `parser::reduction` | Turns GLR pop paths into parent subtrees |
| `parser::recovery` | Searches for a low-cost path past invalid input |
| `lexer` | Buffers and decodes input and implements the `TSLexer` interface |
| `language` | Reads generated language metadata and parse tables |
| `stack` | Stores stack versions and their shared graph of stack nodes |
| `subtree` | Stores tokens and internal syntax-tree nodes |
| `tree` | Owns a root subtree, language, and included ranges |
| `node` | Implements immutable `TSNode` inspection and navigation |
| `tree_cursor` | Implements stateful, repeated tree navigation |
| `get_changed_ranges` | Compares two trees while skipping unchanged structure |

For a first code-reading pass, follow one successful token through
`parser::lexing`, `parser::advance`, `parser::reduction`, `stack`, and
`subtree`. Read `parser::recovery` and `get_changed_ranges` afterward; both are
easier once the normal shift/reduce path is familiar.

## Where generation fits

The CLI's `generate` command performs the work that the runtime should not have
to repeat:

1. It evaluates `grammar.js` and converts the grammar to an internal rule
   representation.
2. It normalizes and validates those rules.
3. It separates syntactic rules from lexical rules.
4. It constructs parse states and detects conflicts.
5. It applies precedence and associativity, retaining declared GLR conflicts.
6. It emits parse tables, symbol metadata, lexer functions, and optional
   external-scanner bindings in `parser.c`.

Once `parser.c` has been compiled, an application only needs the generated
language and the Tree-sitter runtime. The CLI is a build-time tool, not a
runtime dependency.

## A practical reading strategy

When debugging a parse, avoid starting with the graph-structured stack. Start
with these questions in order:

1. What token did the lexer return?
2. What were the current parse state and lookahead symbol?
3. Which actions were stored in that parse-table entry?
4. Did the action shift, reduce, accept, or enter recovery?
5. Only if there were multiple actions: which stack versions were created,
   merged, or pruned?

Tree-sitter's parse logger reports this same sequence. Reading the log as a
series of table lookups keeps GLR from obscuring the ordinary LR work that
makes up most of a parse.
