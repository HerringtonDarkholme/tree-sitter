// This file replaces lib.c during the C-to-Rust transition.
// As each C file is rewritten in Rust, its #include line is removed here.
// When all files are rewritten, this file will be deleted entirely.

// alloc.c — replaced by src_rust/alloc.rs
#include "./get_changed_ranges.c"
#include "./language.c"
#include "./lexer.c"
#include "./node.c"
#include "./parser.c"
// point.c — replaced by src_rust/point.rs
#include "./query.c"
#include "./stack.c"
#include "./subtree.c"
#include "./tree_cursor.c"
#include "./tree.c"
#include "./wasm_store.c"
