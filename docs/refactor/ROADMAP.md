# Rust Core Roadmap

This roadmap tracks remaining work for the Rust-backed tree-sitter core.

## Milestone

The current milestone is stable Rust-core parity for normal tree-sitter users:

- generated C parsers link unchanged
- public C ABI behavior remains compatible
- Rust crate APIs keep existing semantics
- CLI parse, edit, tree, node, cursor, highlight, tags, and query-facing paths
  remain compatible
- downstream users such as ast-grep keep working without source changes

`query.c` and `wasm_store.c` remain C-backed for this milestone.

## Required Gates

Run the full workspace test command for Rust code changes:

```bash
cargo test --all
```

Additional focused gates:

```bash
cargo fmt --check --all
cargo clippy -p tree-sitter --lib --tests -- -D warnings
cargo test -p tree-sitter --test abi_surface
cargo test -p tree-sitter --lib
```

When fixtures are available, broader migration checks should include:

```bash
cargo xtask fetch-fixtures
cargo xtask generate-fixtures
cargo xtask test
```

## Current Priorities

1. Keep the active Rust core small, readable, and testable.
2. Remove stale scaffolding instead of hiding it behind broad allowances.
3. Preserve parser/tree/node/cursor behavior under incremental parsing,
   included ranges, external scanner state, cancellation, and recovery.
4. Keep query and WASM as explicit non-goals unless a task targets them.
5. Use performance data from `PERFORMANCE.md` before changing parser hot paths.

## Deferred Work

- Rewrite query runtime.
- Rewrite WASM store/runtime.
- Simplify the Rust binding layer after C-backed surfaces are retired.
- Add broader differential old-C-core vs Rust-core harnesses.
- Promote performance architecture changes only after benchmark evidence.

## Downstream Acceptance

Before treating the Rust core as ready for broader users, validate:

- workspace tests
- parser ABI surface
- fixture grammars
- CLI parse behavior
- ast-grep or other downstream consumers with a local tree-sitter override
- package surfaces that include Rust core files and C shims
