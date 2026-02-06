# Plan: Rewrite tree-sitter Core C Library to Rust

## Context

The tree-sitter core library is ~16K lines of C11 across 13 source files in `lib/src/`. It implements an LR parsing engine, incremental parsing, query system, and tree data structures. The goal is to rewrite this to pure Rust while maintaining full ABI compatibility with:
- **Generated parsers** (compiled C code that links against the library via `parser.h` types)
- **The existing Rust binding layer** (`lib/binding_rust/lib.rs`) which calls the C API via FFI
- **External C consumers** (editors, language servers) that use `api.h`

After the rewrite, the binding layer can be simplified to call Rust directly (a follow-up effort).

### Design Decisions
- **Idiomatic Rust for internals**: Use `Vec`, `Option`, `Arc`, Rust enums for internal types. Only use `#[repr(C)]` for types that cross the FFI boundary (e.g., `TSNode`, `TSTreeCursor`, `TSLanguage`).
- **Implementation location**: `lib/src_rust/` — separate directory alongside `lib/src/` for clean separation.
- **WASM store**: Deferred — decide after core files are done whether to rewrite or keep as C.

---

## Critical Constraint: ABI Compatibility

`lib/src/parser.h` and `lib/include/tree_sitter/api.h` define the ABI contract:
- `TSLanguage` struct layout (read at runtime from generated parsers)
- `TSLexer` callback interface (called by generated parser C code)
- All `ts_*` public API function signatures (~120 functions)

These headers **must remain unchanged**. The Rust implementation must produce identical symbol names and calling conventions (`#[no_mangle] extern "C"`).

---

## Step 1: Scaffold — Rust Module Structure ✅ DONE

Create `lib/src_rust/` as an internal module within the `tree-sitter` crate:

```
lib/src_rust/
├── mod.rs                  # Module root, re-exports
├── alloc.rs                # Memory allocation (replaces alloc.c + alloc.h)
├── point.rs                # Point arithmetic (replaces point.c + point.h)
├── length.rs               # Byte+point position math (replaces length.h)
├── error_costs.rs          # Constants (replaces error_costs.h)
├── unicode.rs              # UTF-8/16 support (replaces unicode/ directory)
├── subtree.rs              # Tree nodes, pooling, ref counting (replaces subtree.c/h)
├── language.rs             # Language metadata & tables (replaces language.c/h)
├── lexer.rs                # Input buffering & decoding (replaces lexer.c/h)
├── stack.rs                # Parse stack with versions (replaces stack.c/h)
├── tree.rs                 # TSTree lifecycle (replaces tree.c/h)
├── tree_cursor.rs          # Cursor traversal (replaces tree_cursor.c/h)
├── node.rs                 # TSNode API (replaces node.c)
├── get_changed_ranges.rs   # Incremental diff (replaces get_changed_ranges.c/h)
├── reduce_action.rs        # Reduce dedup (replaces reduce_action.h)
├── reusable_node.rs        # Cached node (replaces reusable_node.h)
├── parser.rs               # LR parse engine (replaces parser.c)
└── query.rs                # Query parser & matcher (replaces query.c)
```

Each module starts as a **stub** with the correct type signatures and `todo!()` bodies.

### What the stubs contain
- Internal Rust types (`Subtree`, `SubtreePool`, `Stack`, `Lexer`, etc.) — idiomatic Rust
- `#[repr(C)]` only on FFI-facing types that must match C layout
- Public API functions as `#[no_mangle] pub unsafe extern "C" fn ts_*(...)` with `todo!()` bodies
- `#[allow(dead_code)]` — stubs are not activated until their C counterpart is removed

### Files to modify
- `lib/binding_rust/lib.rs` — add `#[path = "../src_rust/mod.rs"] mod core_impl;`
- `lib/Cargo.toml` — add `"src_rust/*.rs"` to the `include` list
- `lib/binding_rust/build.rs` — conditional C compilation (see Step 2)

---

## Step 2: Change Build Script ✅ DONE

Modify `lib/binding_rust/build.rs` to support **mixed C/Rust compilation**.

### Strategy
Create `lib/src/remaining_lib.c` as a copy of `lib.c`. As each C file is rewritten to Rust, remove its `#include` line from `remaining_lib.c`. Change build.rs to compile `remaining_lib.c` instead of `lib.c`.

```rust
// build.rs change: swap lib.c → remaining_lib.c
config.file(src_path.join("remaining_lib.c"));
```

### Why this works
- Rust `#[no_mangle] extern "C"` functions produce the same symbols as C originals
- The linker sees both C object files (from `cc` crate) and Rust object files
- C code can call Rust functions (they look like C functions to the linker)
- Rust code can call remaining C functions via `extern "C"` block declarations
- At link time, all symbols resolve regardless of source language

### Transition state example
After rewriting `alloc.c` and `point.c`:
```c
// remaining_lib.c
// #include "./alloc.c"       ← REMOVED (now in Rust)
#include "./get_changed_ranges.c"
#include "./language.c"
#include "./lexer.c"
#include "./node.c"
#include "./parser.c"
// #include "./point.c"       ← REMOVED (now in Rust)
#include "./query.c"
#include "./stack.c"
#include "./subtree.c"
#include "./tree_cursor.c"
#include "./tree.c"
#include "./wasm_store.c"
```

---

## Step 3: Make It Compile ✅ DONE

1. Create all stub files with correct signatures (all bodies are `todo!()`)
2. Ensure `cargo build` succeeds with the mixed C/Rust setup
3. Stubs are `#[allow(dead_code)]` — not yet exported/activated
4. Verify `cargo xtask test` still passes (all C still in place, stubs unused)
5. **Commit this scaffold** as the baseline

---

## Step 4: Topological Rewrite Order

### Dependency Graph (from `#include` and cross-file function calls)

```
                    ┌──────────────────────────────────────────────────┐
                    │              Tier 0: Leaf Utilities               │
                    │  alloc.c/h   point.c/h   error_costs.h           │
                    │  length.h (uses point.h)                         │
                    │  array.h (uses alloc.h)                          │
                    │  unicode/*.h                                      │
                    └──────────┬───────────────────────────────────────┘
                               │
            ┌──────────────────┼──────────────────────┐
            ▼                  ▼                      ▼
     ┌─────────────┐   ┌─────────────┐        ┌─────────────┐
     │  subtree.c   │   │   lexer.c   │        │ language.c  │
     │              │   │             │        │             │
     │ uses: alloc, │   │ uses:       │        │ uses:       │
     │ length,      │   │ length,     │        │ wasm_store  │
     │ language*,   │   │ unicode     │        │ (for wasm   │
     │ error_costs  │   │             │        │  ref count) │
     └──────┬───────┘   └──────┬──────┘        └──────┬──────┘
            │                  │                      │
            │   ┌──────────────┼──────────────────────┤
            ▼   ▼              │                      ▼
     ┌─────────────┐           │               ┌─────────────┐
     │   stack.c    │           │               │tree_cursor.c│
     │              │           │               │             │
     │ uses: alloc, │           │               │ uses:       │
     │ subtree,     │           │               │ language,   │
     │ language     │           │               │ tree,       │
     └──────┬───────┘           │               │ subtree     │
            │                   │               └──────┬──────┘
            │                   │                      │
            │    ┌──────────────┤──────────────────────┤
            │    ▼              │                      ▼
            │  ┌─────────────┐ │        ┌──────────────────────┐
            │  │   tree.c    │ │        │ get_changed_ranges.c │
            │  │             │ │        │                      │
            │  │ uses:       │ │        │ uses: subtree,       │
            │  │ subtree,    │ │        │ language, tree_cursor│
            │  │ tree_cursor,│ │        │ error_costs          │
            │  │ get_changed │ │        └──────────┬───────────┘
            │  │ _ranges,    │ │                   │
            │  │ language    │ │                   │
            │  └──────┬──────┘ │                   │
            │         │        │                   │
            │    ┌────┤        │                   │
            │    ▼    ▼        │                   │
            │  ┌─────────────┐ │                   │
            │  │   node.c    │ │                   │
            │  │             │ │                   │
            │  │ uses:       │ │                   │
            │  │ subtree,    │ │                   │
            │  │ language,   │ │                   │
            │  │ tree, point │ │                   │
            │  └─────────────┘ │                   │
            │                  │                   │
            ▼                  ▼                   ▼
     ┌─────────────────────────────────────────────────────┐
     │                     parser.c                         │
     │ uses: ALL (alloc, subtree, stack, lexer, language,   │
     │   tree, get_changed_ranges, reusable_node,           │
     │   reduce_action, error_costs, length, wasm_store)    │
     └─────────────────────────────────────────────────────┘

     ┌─────────────────────────────────────────────────────┐
     │                     query.c                          │
     │ uses: alloc, language, tree_cursor, point, unicode   │
     │ (relatively self-contained, can be done before       │
     │  or after parser.c)                                  │
     └─────────────────────────────────────────────────────┘

     ┌─────────────────────────────────────────────────────┐
     │                   wasm_store.c                       │
     │ uses: alloc, language, lexer, array                  │
     │ (feature-gated, deferred)                            │
     └─────────────────────────────────────────────────────┘
```

### Per-File Dependency List

| C File | Depends on (calls functions from) | Depended on by |
|--------|-----------------------------------|----------------|
| `alloc.c` | *(nothing)* | everything (via `ts_malloc`/`ts_free` macros) |
| `point.c` | *(nothing)* (inline point.h only) | node.c (calls `ts_point_edit`) |
| `length.h` | point.h (inline) | subtree, lexer, stack, tree, parser |
| `error_costs.h` | *(nothing)* | subtree, get_changed_ranges, parser |
| `unicode/*.h` | *(nothing)* | lexer, query |
| `array.h` | alloc.h (for `ts_malloc`/`ts_free`) | subtree, stack, tree, parser, query, wasm_store |
| `subtree.c` | alloc, length, language (symbol names), error_costs | stack, tree_cursor, tree, node, get_changed_ranges, parser |
| `lexer.c` | length, unicode | parser, wasm_store |
| `language.c` | wasm_store (for `ts_wasm_language_*` ref counting) | subtree, stack, tree_cursor, tree, node, get_changed_ranges, parser, query |
| `stack.c` | alloc, subtree, language, length | parser |
| `tree_cursor.c` | language, tree, subtree | get_changed_ranges, tree, query |
| `get_changed_ranges.c` | subtree, language, tree_cursor, error_costs | tree, parser |
| `tree.c` | subtree, tree_cursor, get_changed_ranges, language | node, parser |
| `node.c` | subtree, language, tree, point | parser |
| `parser.c` | **ALL**: alloc, subtree, stack, lexer, language, tree, node, get_changed_ranges, reusable_node, reduce_action, error_costs, length, wasm_store | *(top-level)* |
| `query.c` | alloc, language, tree_cursor, point, unicode | *(top-level)* |
| `wasm_store.c` | alloc, language, lexer, array | language (circular for ref counting) |

### Circular Dependency: language.c ↔ wasm_store.c

`language.c` calls `ts_wasm_language_retain`/`ts_wasm_language_release` (in wasm_store.c) for WASM ref counting. `wasm_store.c` calls `ts_language_*` functions. This cycle is broken during the transition by:
- Keeping `wasm_store.c` in C until last (deferred to Tier 5)
- Rust `language.rs` calls wasm functions via `extern "C"` declarations
- Only matters when the `wasm` feature is enabled

### Rewrite Order (Revised)

#### Tier 0 — Pure Leaf Utilities (no deps on other .c files) ✅ ALL DONE
| # | File | ~Lines | Status | Replaces |
|---|------|--------|--------|----------|
| 1 | `alloc.rs` | 113 | **DONE** | `alloc.c/h` |
| 2 | `point.rs` | 93 | **DONE** | `point.c/h` |
| 3 | `length.rs` | 80 | **DONE** (header-only, no .c to remove) | `length.h` |
| 4 | `error_costs.rs` | 11 | **DONE** (header-only, no .c to remove) | `error_costs.h` |
| 5 | `unicode.rs` | 170 | **DONE** (header-only, no .c to remove) | `unicode/*.h` |

#### Tier 1 — Core Data Structure
| # | File | ~Lines | Depends on | Replaces |
|---|------|--------|------------|----------|
| 6 | `subtree.rs` | 1000+ | alloc, length, language (symbol fns), error_costs | `subtree.c/h` |

**subtree.c is the hardest and most critical file.** It defines:
- `Subtree` / `MutableSubtree` — union of inline (small leaf) and heap-allocated nodes
- Bitfield packing for node metadata (visible, named, extra, has_changes, etc.)
- Atomic reference counting
- `SubtreePool` — free-list for recycling subtree allocations
- ~60 functions including `ts_subtree_new_leaf`, `ts_subtree_new_error`, `ts_subtree_balance`, `ts_subtree_edit`, `ts_subtree_string`

#### Tier 2 — Components Depending on Subtree
| # | File | ~Lines | Depends on | Replaces |
|---|------|--------|------------|----------|
| 7 | `language.rs` | 300 | wasm_store (via extern "C" during transition) | `language.c/h` |
| 8 | `lexer.rs` | 500 | length, unicode | `lexer.c/h` |
| 9 | `stack.rs` | 900 | alloc, subtree, language | `stack.c/h` |

**Note:** `subtree.c` calls `language.c` functions (e.g. `ts_language_symbol_name`), and `language.c` is needed by many downstream files. However, `language.c` itself only depends on `wasm_store.c` (for WASM ref counting). During the transition, `language.rs` can call the remaining C `wasm_store` functions via `extern "C"`. So language.c can be rewritten right after subtree.c.

#### Tier 3 — Tree Navigation
| # | File | ~Lines | Depends on | Replaces |
|---|------|--------|------------|----------|
| 10 | `tree_cursor.rs` | 720 | language, tree, subtree | `tree_cursor.c/h` |
| 11 | `get_changed_ranges.rs` | 560 | subtree, language, tree_cursor, error_costs | `get_changed_ranges.c/h` |
| 12 | `tree.rs` | 140 | subtree, tree_cursor, get_changed_ranges, language | `tree.c/h` |
| 13 | `node.rs` | 870 | subtree, language, tree, point | `node.c` |

**Note on ordering within Tier 3:** `tree.c` depends on `tree_cursor.c` and `get_changed_ranges.c`, so those must come first. `tree_cursor.c` depends on `tree.c` for the `TSTree` struct definition (but only reads the struct, doesn't call tree.c functions extensively), so we can provide the struct from Rust while `tree.c` is still in C. `node.c` depends on `tree.c` so it comes last.

#### Tier 4 — The Engine
| # | File | ~Lines | Depends on | Replaces |
|---|------|--------|------------|----------|
| 14 | `query.rs` | 4450 | alloc, language, tree_cursor, point, unicode | `query.c` |
| 15 | `parser.rs` | 2260 | **ALL** of the above | `parser.c` |

`query.c` and `parser.c` are independent of each other — either can be done first. `query.c` is larger but simpler in structure. `parser.c` must be last because it depends on everything.

#### Tier 5 — Optional (Deferred)
| # | File | ~Lines | Depends on | Decision |
|---|------|--------|------------|----------|
| 16 | `wasm_store.rs` | 1940 | alloc, language, lexer | Deferred. Feature-gated behind `wasm`. |

---

## Steps 5–9: Per-File Rewrite Loop

For **each file** in the order above, repeat:

### 5. Rewrite the file
- Read the C source **line by line**. Translate every function, every branch, every edge case.
- Use idiomatic Rust: `Option` for nullable, `Vec` for arrays, `enum` for tagged unions, `Arc`/`AtomicU32` for ref counting.
- Keep `#[repr(C)]` only for types that cross the FFI boundary.
- Export public `ts_*` API functions as `#[no_mangle] pub unsafe extern "C" fn`.
- For calls to not-yet-rewritten C modules, declare them in an `extern "C" { ... }` block.

### 6. Write tests
- Unit tests in the module: `#[cfg(test)] mod tests { ... }`
- Test every public function including edge cases visible in the C code
- Run `cargo xtask test` — full integration suite must pass

### 7. Verify the rewrite
- Read the Rust implementation **side-by-side** with the C original
- Ensure **no logic is skipped**: every `if`, every loop bound, every error path
- Check for: null pointer risks, integer overflow, alignment, off-by-one errors
- Verify `#[repr(C)]` correctness for any FFI types
- Check `unsafe` blocks are minimal and justified

### 8. Human review
- Present the rewrite for human review
- Address all feedback before proceeding
- **Do not proceed to the next file until human approves**

### 9. Activate and commit
- Remove the corresponding `#include` from `lib/src/remaining_lib.c`
- Run `cargo build && cargo xtask test` — both must pass
- Commit the change
- Proceed to next file

---

## Post-Rewrite: Simplify Binding Layer

After all core C files are rewritten:
1. Remove `lib/src/remaining_lib.c` and all `.c` source files
2. Remove `cc` build dependency from `build.rs`
3. Refactor `lib/binding_rust/lib.rs` to call Rust directly instead of through `ffi::ts_*`
4. Remove `bindings.rs` (auto-generated FFI types no longer needed)
5. **Keep** `api.h` and `parser.h` — still needed by generated parsers and external C consumers
6. Consider generating C header from Rust (via `cbindgen`) for maintainability

---

## Testing Strategy

At every step:
1. `cargo build` — must compile
2. `cargo xtask test` — full test suite must pass
3. New unit tests within each rewritten module
4. After full rewrite: add Miri tests for unsafe code validation

### Pre-requisite (one-time)
```bash
cargo xtask fetch-fixtures      # Download test grammars
cargo xtask generate-fixtures   # Build test parsers from grammars
```

---

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Subtle behavior differences in C→Rust translation | Side-by-side review of every function; full test suite at each step |
| `#[repr(C)]` struct layout mismatches | Add `static_assert!(size_of::<T>() == N)` checks; compare with C sizes |
| Undefined behavior in unsafe Rust | Minimize unsafe blocks; run Miri; careful review |
| Mixed C/Rust linking issues during transition | Test on macOS, Linux, Windows at each tier boundary |
| `subtree.h` bitfield layouts | Use explicit bit manipulation in Rust (no bitfield crate) |
| Performance regression | Benchmark parsing throughput before/after (especially parser.rs) |
| `array.h` C macro generics | Replace with `Vec<T>` — simpler and idiomatic |
| Circular deps between C and Rust during transition | Linker resolves all symbols at link time; declare `extern "C"` as needed |

---

## Key Files Reference

| Role | Path |
|------|------|
| C source (being rewritten) | `lib/src/*.c` |
| C internal headers | `lib/src/*.h` |
| Public C API (keep) | `lib/include/tree_sitter/api.h` (1,445 lines) |
| Generated parser ABI (keep) | `lib/src/parser.h` (286 lines) |
| Rust binding layer (keep, later simplify) | `lib/binding_rust/lib.rs` (3,908 lines) |
| FFI type bindings (keep during transition) | `lib/binding_rust/bindings.rs` (959 lines) |
| Build script (modify) | `lib/binding_rust/build.rs` (125 lines) |
| Crate manifest (modify) | `lib/Cargo.toml` |
| **New Rust implementation** | `lib/src_rust/` |
