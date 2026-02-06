// This file replaces lib.c during the C-to-Rust transition.
// As each C file is rewritten in Rust, its #include line is removed here.
// When all files are rewritten, this file will be deleted entirely.

// alloc.c — replaced by src_rust/alloc.rs
// get_changed_ranges.c — replaced by src_rust/get_changed_ranges.rs
// language.c — replaced by src_rust/language.rs
// lexer.c — replaced by src_rust/lexer.rs
#include "./node.c"
#include "./parser.c"
// point.c — replaced by src_rust/point.rs
#include "./query.c"
// stack.c — replaced by src_rust/stack.rs
// subtree.c — replaced by src_rust/subtree.rs
// tree_cursor.c — replaced by src_rust/tree_cursor.rs
// tree.c — replaced by src_rust/tree.rs
#include "./wasm_store.c"
