# Rust Core Rewrite Status

This document records the current state of the Rust core rewrite. It replaces
the original step-by-step migration plan, which is now mostly complete and had
become stale.

## Current State

The active Rust core implementation lives in `lib/src_rust` and is wired through
`lib/binding_rust/lib.rs`.

Most runtime components are Rust-backed:

| Module | Status |
| --- | --- |
| `alloc.rs` | Active |
| `point.rs` | Active |
| `length.rs` | Active |
| `error_costs.rs` | Active |
| `unicode.rs` | Active |
| `subtree.rs` | Active |
| `language.rs` | Active |
| `lexer.rs` | Active |
| `stack.rs` | Active |
| `tree_cursor.rs` | Active |
| `get_changed_ranges.rs` | Active |
| `tree.rs` | Active |
| `node.rs` | Active |
| `parser.rs` | Active |
| `reduce_action.rs` | Active helper |
| `reusable_node.rs` | Active helper |
| `query.rs` | Legacy/inactive Rust port; live query runtime remains C-backed |

`query.rs` is intentionally excluded from readability and cleanup work unless a
task explicitly targets the query runtime.

## Compatibility Rules

- Preserve public C ABI symbols and calling conventions.
- Keep `lib/include/tree_sitter/api.h` and `lib/src/parser.h` compatible with
  generated parsers and external C users.
- Preserve `#[repr(C)]` layout for FFI-facing and layout-sensitive runtime
  types.
- Use compile-time size assertions for layout-sensitive structs.
- Keep the C-backed query and WASM paths separate from parser-core cleanup.

## Current Cleanup Policy

- Prefer removing stale code over adding broad `dead_code` allowances.
- Keep naming-style allowances only where the Rust code intentionally mirrors C
  runtime names.
- Avoid broad performance scaffolding unless a benchmark shows a reproducible
  win.
- Keep changes focused and behavior-preserving unless the task explicitly asks
  for an architecture change.

## Testing

For Rust code changes, run:

```bash
cargo test --all
```

Useful focused checks while iterating:

```bash
cargo fmt --check --all
cargo clippy -p tree-sitter --lib --tests -- -D warnings
cargo test -p tree-sitter --test abi_surface
cargo test -p tree-sitter --lib
```

If `cargo test --all` fails for an unrelated workspace fixture/setup reason,
record the exact failing tests and continue only with that risk made explicit.

## Open Migration Work

- Keep the active Rust core green under workspace tests.
- Keep C ABI and Rust API behavior stable for parser, tree, node, cursor,
  language, lexer, stack, and changed-range surfaces.
- Treat query and WASM migration as separate projects.
- Keep performance work grounded in `PERFORMANCE.md`.
