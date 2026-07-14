// Tree-sitter C library entry point (amalgamated build).
//
// The parsing engine has been migrated to Rust (see lib/src_rust/), so the
// historical amalgamation of parser.c/query.c/subtree.c/... is gone. The
// variadic lexer log forwarder remains in C because Rust cannot define a
// variadic function.
//
// This file is the single C translation unit the build compiles, mirroring
// upstream's `lib.c` so the build scripts can reference `lib/src/lib.c`.
#include "./lexer_log_shim.c"
