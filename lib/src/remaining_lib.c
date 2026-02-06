// This file replaces lib.c during the C-to-Rust transition.
// As each C file is rewritten in Rust, its #include line is removed here.
// When all files are rewritten, this file will be deleted entirely.

// alloc.c — replaced by src_rust/alloc.rs
#include "./get_changed_ranges.c"
// language.c — replaced by src_rust/language.rs
// lexer.c — replaced by src_rust/lexer.rs
#include "./node.c"
#include "./parser.c"
// point.c — replaced by src_rust/point.rs
#include "./query.c"
#include "./stack.c"
// subtree.c — replaced by src_rust/subtree.rs
#include "./tree_cursor.c"
#include "./tree.c"
#include "./wasm_store.c"
