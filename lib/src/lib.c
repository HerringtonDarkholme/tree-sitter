// Tree-sitter C library entry point (amalgamated build).
//
// The parsing engine has been migrated to Rust (see lib/src_rust/), so the
// historical amalgamation of parser.c/query.c/subtree.c/... is gone. What
// remains in C are the pieces the Rust core links against:
//   - wasm_store.c       the wasmtime-backed WebAssembly store (and its
//                        no-wasmtime stubs when the `wasm` feature is off)
//   - lexer_log_shim.c   the variadic lexer log forwarder
//
// This file is the single C translation unit the build compiles, mirroring
// upstream's `lib.c` so the build scripts can reference `lib/src/lib.c`.
#include "./wasm_store.c"
#include "./lexer_log_shim.c"
