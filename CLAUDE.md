# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Tree-sitter is a parser generator tool and incremental parsing library. The core library is written in C11 (dependency-free), with Rust bindings, a Rust CLI, and JavaScript/WASM bindings. It's designed for use in text editors — fast enough for per-keystroke parsing and robust with syntax errors.

## Build Commands

### Rust (primary build system)

```bash
cargo build --release                  # Build all workspace crates
cargo build --profile release-dev      # Faster dev builds (with debug info)
cargo install --path crates/cli        # Install CLI globally
```

### C library

```bash
make -j                                # Build static + shared libraries
cmake -S . -B build -DBUILD_SHARED_LIBS=OFF && cmake --build build  # CMake alternative
```

### WASM/JS bindings

```bash
cd lib/binding_web && npm install && npm run build    # Full build
cargo xtask build-wasm                                # Via xtask
```

## Testing

Tests require fetching and generating fixtures from real grammar repos first:

```bash
cargo xtask fetch-fixtures             # Download test grammars (one-time)
cargo xtask generate-fixtures          # Build test parsers from grammars
cargo xtask test                       # Run test suite
```

Or all at once: `make test`

### Running specific tests

```bash
cargo xtask test test_name             # Run a single test by name
cargo xtask test -l javascript         # Test a specific language corpus
cargo xtask test -l javascript -e Arrays  # Test a specific corpus example
cargo xtask test -g test_name          # Run under debugger (lldb/gdb)
```

### WASM tests

```bash
cargo xtask generate-fixtures --wasm
cargo xtask test-wasm
```

## Linting

```bash
make lint                              # Full Rust lint (check + fmt + clippy)
make lint-web                          # JavaScript/TypeScript lint
cargo fmt --all                        # Format Rust code
cargo xtask clippy --fix               # Auto-fix clippy issues
```

The workspace uses strict clippy: pedantic + nursery + cargo warnings enabled, `dbg_macro` and `todo` are denied.

## Architecture

### Component dependency flow

```
C Library (lib/src/)           ← Pure C11, ~40 source files, no dependencies
    ↓
Rust FFI Bindings (lib/binding_rust/)  ← tree-sitter crate, supports no_std
    ↓
Workspace Crates (crates/)
├── cli/       ← tree-sitter binary (generate, test, parse, build, query, highlight, tags, playground)
├── generate/  ← Compiles grammar.js → parser.c (uses quickjs runtime)
├── loader/    ← Runtime grammar discovery and on-demand compilation
├── highlight/ ← Syntax highlighting via query system
├── tags/      ← Code navigation (ctags-like)
├── language/  ← Language abstraction layer
├── config/    ← User config (~/.config/tree-sitter/config.json)
└── xtask/     ← Build automation and task runner
```

The JS/WASM bindings (`lib/binding_web/`) compile the C library to WASM via Emscripten, with TypeScript wrappers producing ESM and CommonJS outputs.

### How grammar compilation works

1. User writes `grammar.js` (JavaScript DSL)
2. `tree-sitter generate` evaluates the JS → produces parse tables → emits `parser.c` + `parser.h`
3. Generated C code is compiled to a shared library (`.so`/`.dylib`/`.dll`) or WASM
4. Libraries are loaded at runtime by the tree-sitter core, which links against `lib/src/`

### Key C library files

- `lib/src/parser.c` — LR parsing engine
- `lib/src/query.c` — Query matching system (the largest file)
- `lib/src/subtree.c` — Persistent tree representation
- `lib/src/lib.c` — Amalgamated build entry (includes all .c files)
- `lib/include/tree_sitter/api.h` — Public C API

### Feature flags

- `wasm` — Enables WASM runtime via wasmtime-c-api (on `tree-sitter` crate and CLI)
- `std` (default) — Standard library support for the core crate
- `qjs-rt` (default on CLI) — QuickJS runtime for grammar.js evaluation

### ABI versioning

- `TREE_SITTER_LANGUAGE_VERSION = 15` (current)
- `TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION = 13`

## Workspace Layout

- **Cargo workspace** with `default-members = ["crates/cli"]` — bare `cargo build` builds only the CLI
- Rust 1.85+ required, edition 2021
- Test fixtures live in `test/fixtures/test_grammars/` (57 grammar repos, git-ignored, fetched via xtask)

## Prerequisites

1. C compiler (core library + generated parsers)
2. Rust toolchain 1.85+
3. Node.js/NPM (for grammar.js parsing)
4. Emscripten, Docker, or Podman (for WASM only)
