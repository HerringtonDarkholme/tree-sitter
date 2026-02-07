// Tree-sitter C library entry point (amalgamated build)
// Most modules have been migrated to Rust (see lib/src_rust/).
// Only query.c and wasm_store.c remain in C.
//
// For Rust builds: see remaining_lib.c
// For pure C builds: use the individual .c files or link against the Rust library

#include "./query.c"
#include "./wasm_store.c"
