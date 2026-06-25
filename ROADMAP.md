# Rust Core Migration Roadmap

This roadmap scopes the Rust rewrite of tree-sitter around upstream-compatible
core migration for normal tree-sitter users. ast-grep is a required consumer
gate, but it is not the whole migration scope.

## Summary

The milestone is full parity for the Rust-rewritten tree-sitter core across the
surfaces broader users rely on: parser, tree, node, cursor, language metadata,
edits, ranges, incremental parsing, generated parser ABI, C ABI, Rust crate,
CLI parse behavior, workspace crates, and package build surfaces.

ast-grep must be fully satisfied as an important downstream consumer. Upstream
tree-sitter behavior for rewritten surfaces must also be protected with
differential tests against the old C-backed implementation and with broad
workspace-level tests.

`query.c` and `wasm_store.c` remain C-backed for this milestone. They are
explicitly out of scope for the first Rust core parity milestone.

## Required Core Features

- Preserve upstream public C ABI behavior for parser/tree/node/cursor/language
  functions that are backed by the rewritten core. Generated C parsers and
  package consumers should not need source changes.

- Preserve Rust library behavior for ast-grep and upstream users:
  - `Parser`, `Tree`, `Node`, `TreeCursor`, `Language`, `InputEdit`, `Range`,
    and `Point`.
  - Incremental parsing with old trees.
  - Included ranges for injection-style parsing.
  - Node identity, lineage, traversal order, sibling navigation, field lookup,
    kind IDs, byte ranges, point ranges, error/missing/named flags, and
    `to_sexp`.

- Preserve CLI/library consistency:
  - `tree-sitter parse` and Rust `Node::to_sexp()` agree for the same
    grammar/source.
  - CLI edit/reparse behavior matches Rust incremental parse behavior.
  - Included-range behavior is consistent between CLI code paths and
    `Parser::set_included_ranges`.

- Preserve generated parser compatibility:
  - C generated parsers link unchanged.
  - `../tree-sitter-typescript` TypeScript and TSX grammars load through
    `tree-sitter-language::LanguageFn`.
  - ABI version checks behave like old tree-sitter.

- Preserve workspace crate behavior for broader users:
  - `tree-sitter-cli`, `tree-sitter-generate`, `tree-sitter-loader`,
    `tree-sitter-highlight`, `tree-sitter-tags`, `tree-sitter-config`,
    `tree-sitter-language`, and the `tree-sitter` Rust crate continue to build
    and test together.
  - Highlight, tags, query, language loading, corpus tests, grammar generation,
    text providers, cancellation/timeouts, included ranges, UTF-8/UTF-16 input,
    and pathological parser cases remain covered.

- Preserve packaging surfaces where the Rust core changes affect them:
  - SwiftPM/C package builds from `Package.swift`.
  - Crates publish contents include the Rust core files and C shims needed by
    existing Rust and C consumers.
  - Web/WASM package behavior remains separately gated because the first core
    migration milestone keeps `wasm_store.c` C-backed.

## ast-grep API Surface

The required ast-grep consumer surface is ast-grep's actual `tree_sitter` usage
in `../../ast-grep`:

- `Parser::new`, `Parser::set_language`, `Parser::parse`, parsing with an old
  edited tree, and `Parser::set_included_ranges`.
- `Tree::root_node`, `Tree::edit`, `Tree::included_ranges`, cloning/copying
  trees through the Rust API, and incremental reparsing.
- `Node::id` identity comparisons within one process, `Node::parent`,
  `Node::child`, `Node::children`, `Node::child_by_field_name`,
  `Node::child_by_field_id`, `Node::child_with_descendant`, sibling navigation,
  byte ranges, points, `kind`, `kind_id`, `grammar_id`, `utf8_text`,
  `to_sexp`, `is_named`, `is_missing`, `is_error`, `has_error`, and
  `has_changes`.
- `TreeCursor::node`, `field_name`, `field_id`, `goto_first_child`,
  `goto_next_sibling`, `goto_previous_sibling`, `goto_parent`,
  `goto_first_child_for_byte`, `goto_first_child_for_point`,
  `goto_descendant`, `depth`, and `descendant_index`.
- `Language::id_for_node_kind`, `node_kind_for_id`, field id/name conversion,
  ABI constants, and dynamic-language ABI compatibility checks.
- `Point`, `Range`, and `InputEdit` semantics, especially byte-column behavior
  for UTF-8 text.

The ast-grep core matching path does not require rewriting Tree-sitter query
execution for this milestone. Query behavior remains protected by leaving
`query.c` C-backed and by running ast-grep's test suite.

## Explicit Non-Goals

- Rewriting `query.c`.
- Rewriting `wasm_store.c`.
- Changing generated parser source layouts or requiring grammar repos to migrate.
- Web-tree-sitter/WASM runtime migration beyond proving the C-backed WASM path
  still builds in its own release gate.
- Rewriting broad parser internals just to chase benchmark wins before the
  perf gate identifies a reproducible hotspot.

## Test And Benchmark Plan

- Differential core parity tests:
  - Use `cargo xtask core-parity` as the first runnable comparison harness.
    It materializes the pre-rewrite C core from git history into `/tmp`, rebuilds
    one CLI binary with `TREE_SITTER_CORE_IMPL=c` and one with
    `TREE_SITTER_CORE_IMPL=rust`, then compares parse output for
    tree-sitter-typescript corpus examples, focused TypeScript source files, and
    a TSX smoke sample. The harness compares range-bearing output by default,
    no-range output as a secondary mode, and an incremental edit smoke case.
    It also builds a temporary Rust probe crate against both cores to compare
    library and direct public `tree_sitter::ffi` behavior: language metadata,
    node traversal, tree cursors, field lookup, byte/point ranges, incremental
    edits, changed ranges, and included ranges. It defaults to
    `../tree-sitter-typescript` and
    `../typescript`, with `--tree-sitter-typescript-path` and
    `--typescript-path` for alternate checkout layouts.
  - Compare old C-backed behavior vs Rust core behavior for parse success,
    sexps, node ranges, cursor traversal, field IDs, kind IDs, edit results,
    and changed ranges.
  - Add focused cases for UTF-8 byte-column behavior, emoji, sibling traversal,
    repeated fields, missing/error nodes, and incremental edits.

- TypeScript benchmark:
  - Use `../tree-sitter-typescript/test/corpus/*.txt`.
  - Use focused files from `../typescript/src/compiler`,
    `../typescript/src/services`, and `../typescript/src/server`.
  - Cover both TypeScript and TSX grammars.

- Performance gate:
  - Use `cargo xtask perf-gate` to compare the Rust core against the
    pre-rewrite C core on shared benchmark samples. The gate materializes the
    same historical C core revision used by `core-parity`, runs
    `cargo bench benchmark -p tree-sitter-cli` for each selected language, and
    compares machine-readable `BENCHMARK_RESULT` records emitted by the
    benchmark harness.
  - The strict default requires Rust parser throughput to meet or beat C
    overall and rejects per-file parser regressions above the configured
    threshold. Use `--report-only` for profiling runs while collecting data.
  - The gate compares parser work (`normal` and mismatched-language `error`
    cases). Query construction remains visible in raw benchmark output, but it
    is not a Rust-core performance signal while `query.c` stays C-backed.

- ast-grep acceptance gate:
  - Use `cargo xtask ast-grep-gate` to run ast-grep's core/config/language/CLI
    tests against this local tree-sitter rewrite. The command creates a
    temporary `tree-sitter` compatibility crate at ast-grep's locked
    `tree-sitter` version, copies ast-grep to `/tmp`, and patches the copied
    workspace's `tree-sitter` dependency to that path so the sibling checkout's
    lockfile is not mutated. Use `--tree-sitter-version` only when testing a
    checkout without a usable lockfile. Use `--package/-p` to narrow failures
    during iteration, or `--full` to include outline, dynamic, and LSP
    packages. Use `--bindings` to compile-check ast-grep's NAPI crate and
    Python crate with its `python` feature against the same local tree-sitter
    override.
  - Run ast-grep with a local dependency override to this tree-sitter repo.
  - Require ast-grep core/config/language/CLI tests to pass for all
    tree-sitter-backed behavior.
  - Prioritize TypeScript/TSX scan, pattern, contextual pattern, rewrite, field
    selector, nth-child, range, suppression, and injection scenarios.

- Broader tree-sitter CLI gate:
  - Fetch pinned fixture grammars with `cargo xtask fetch-fixtures`.
  - Run `cargo test --workspace --lib --tests` with `XDG_CACHE_HOME` and
    `TREE_SITTER_LIBDIR` pointed at temporary directories. This covers the
    CLI/library paths that ast-grep does not fully exercise, including corpus,
    parser, node, tree, query, highlight, tag, generated parser, loader,
    language, and grammar-generation behavior.

- Aggregate migration gate:
  - Use `cargo xtask migration-gate` as the normal broad compatibility command.
    It runs `core-parity`, validates fixture grammars, runs the workspace Rust
    test suite with isolated cache/libdir directories, and runs the ast-grep
    consumer gate.
  - Use `cargo xtask migration-gate --ast-grep-full --ast-grep-bindings` as the
    release profile when ast-grep outline, dynamic-language, LSP, NAPI, and
    Python binding compatibility should all be checked in one command.
  - Use `cargo xtask migration-gate --swift-build` on machines with SwiftPM to
    verify the C/Swift package surface. Swift is opt-in so Linux maintainers
    without SwiftPM can still run the default migration gate.

## Roadmap Order

1. Stabilize rewritten Rust core APIs already routed through `lib/src_rust`.
2. Build the old-vs-new differential harness for core parser/tree/node/cursor
   behavior.
3. Run focused TypeScript/TSX corpus benchmarks and fix parity failures.
4. Run ast-grep with the local tree-sitter override and treat every failure as a
   blocker unless it touches deferred WASM surfaces.
5. Lock CLI/library/workspace consistency with regression tests that exercise
   public Rust APIs, C-API-backed CLI paths, generated parser loading, and
   `tree-sitter parse`.
6. Verify package surfaces that compile the C/Rust core combination, especially
   the Rust crate publish contents and SwiftPM/C package build.
7. Leave `query.c` and `wasm_store.c` C-backed until the core milestone is
   green.
8. Run the perf gate in report-only mode during profiling, then make
   `cargo xtask perf-gate` part of the release checklist once parity is stable.

## Current Idiomatic Rust Cleanup Order

1. Fix Clippy warnings first in the active Rust core, prioritizing warnings that
   improve readability or remove C-port artifacts without changing exported
   FFI signatures, generated parser ABI, `#[repr(C)]` layouts, allocation
   behavior, or parser semantics.
2. Then clean up raw pointer and type-cast usage, starting with internal helper
   bodies and internal Rust-only function signatures. Keep public C ABI
   signatures and layout-sensitive structures unchanged.
3. Keep each cleanup single-goal and small. Run `cargo test --all` after every
   code change, use focused Clippy checks for touched warnings, and run the
   TypeScript/TSX perf gate after each ten code commits before pushing.

## Task Breakdown

1. `core-parity`: keep `cargo xtask core-parity` green. This compares the old C
   core and the Rust core for CLI parse output, no-range output, incremental
   edit output, TypeScript corpus samples, at least one TSX corpus sample, a TSX
   smoke sample, focused TypeScript repository files, Rust library APIs, and
   direct public `tree_sitter::ffi` calls.
2. `ast-grep-gate`: keep `cargo xtask ast-grep-gate` green for
   `ast-grep-core`, `ast-grep-config`, `ast-grep-language`, and the ast-grep
   CLI using a temporary compatibility crate whose version matches ast-grep's
   locked `tree-sitter` package.
3. `ast-grep-full-gate`: run `cargo xtask ast-grep-gate --full` before release
   to include outline, dynamic-language, and LSP packages.
4. `ast-grep-bindings-gate`: run `cargo xtask ast-grep-gate --bindings` before
   release to compile-check ast-grep's NAPI crate and ast-grep-py's `python`
   feature against the compatibility crate. This is a compile/API gate, not a
   replacement for ast-grep's language/runtime tests.
5. `workspace-gate`: run `cargo xtask fetch-fixtures` once per checkout, then
   keep `cargo test --workspace --lib --tests` green with isolated cache/libdir
   environment variables.
6. `migration-gate`: keep `cargo xtask migration-gate` green as the broad
   default gate. Before release, run it again with
   `--ast-grep-full --ast-grep-bindings`; on SwiftPM-capable machines, also add
   `--swift-build`.
7. `package-surface-gate`: verify `swift build --scratch-path /tmp/tree-sitter-swift-build`
   on macOS/SwiftPM machines and keep crate package includes in `lib/Cargo.toml`
   aligned with the Rust core files and C shims.
8. `parity-fix-loop`: for every `core-parity` diff, reduce to the smallest
   TypeScript/TSX input, add the failing case to the harness or existing Rust
   tests, then fix the Rust core.
9. `downstream-fix-loop`: for every ast-grep or broader-user failure, classify
   it as core, C ABI, Rust API, CLI, generated parser, loader/dynamic-language,
   query, WASM, or dependency/version behavior. Core failures block the
   milestone. Query/WASM failures should only block if they are regressions
   despite those modules staying C-backed.
10. `release-readiness`: once the parity, ast-grep, full ast-grep, binding,
    workspace, package-surface, and migration gates are green, add the gate
    commands to the maintainer checklist or CI job used for the Rust rewrite
    branch.
11. `perf-gate`: keep `cargo xtask perf-gate` green before release. During
    optimization work, run narrower report-only checks such as
    `cargo xtask perf-gate --language typescript --report-only --offline`, use
    flamegraphs or Instruments on the slowest reported files, and only then
    change parser hot paths.

## Maintainer Checklist

Run these before treating the Rust core rewrite as ready for broader users:

1. `cargo xtask fetch-fixtures`
2. `cargo xtask migration-gate`
3. `cargo xtask migration-gate --ast-grep-full --ast-grep-bindings`
4. `cargo xtask migration-gate --skip-core-parity --skip-workspace-tests --skip-ast-grep --swift-build`
5. `XDG_CACHE_HOME=/tmp/tree-sitter-workspace-test-cache TREE_SITTER_LIBDIR=/tmp/tree-sitter-workspace-test-libdir cargo test --workspace --lib --tests`
6. `cargo xtask core-parity`
7. `cargo xtask ast-grep-gate --full --bindings`
8. `cargo xtask perf-gate`

Use `--tree-sitter-typescript-path`, `--typescript-path`, and
`--ast-grep-path` if the benchmark repositories are not in the default sibling
locations.

## Assumptions

- ast-grep compatibility is the bottom-line release gate.
- Core upstream tree-sitter parity is required for rewritten surfaces.
- Broader-user migration means preserving public C ABI, Rust crate behavior,
  CLI behavior, generated parser compatibility, workspace crates, and package
  build surfaces, not only satisfying one downstream project.
- TypeScript and TSX are the primary benchmark grammars.
- Query and WASM behavior may remain correct by staying C-backed.
