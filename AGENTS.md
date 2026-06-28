# Repository Agent Instructions

This file provides guidance for Codex and other coding agents working in this
repository.

## Scope

These instructions apply to the entire repository.

## Project Context

Tree-sitter is a parser generator tool and incremental parsing library. The
core library is written in C11, with Rust bindings, a Rust CLI, and
JavaScript/WASM bindings.

The Rust core rewrite lives under `lib/src_rust`. The module structure mirrors
the C source files while the rewrite is in progress.

## Working Rules

- Prefer small, behavior-preserving changes unless the user explicitly asks for
  a broader refactor.
- In `lib/src_rust`, treat `query.rs` as legacy/inactive code. Do not spend
  readability or cleanup effort there unless the user explicitly asks.
- Use existing local patterns and keep C ABI/layout compatibility in mind for
  `repr(C)` types and exported functions.
- Do not remove compatibility-oriented names just to satisfy style preferences
  when they intentionally mirror C runtime naming.

## Testing

For Rust changes, always run the full workspace test command before finalizing:

```bash
cargo test --all
```

Focused checks are still useful while iterating, especially:

```bash
cargo fmt --check --all
cargo clippy -p tree-sitter --lib --tests -- -D warnings
cargo test -p tree-sitter --test abi_surface
cargo test -p tree-sitter --lib
```

If `cargo test --all` cannot be run, report the reason clearly.

## Useful Commands

```bash
cargo build --release
cargo build --profile release-dev
make lint
make test
```
