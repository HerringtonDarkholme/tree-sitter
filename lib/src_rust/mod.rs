#![allow(dead_code)]

// Core Rust implementation of the tree-sitter library.
//
// This module is the Rust rewrite of lib/src/*.c.
// During the transition, both C and Rust implementations coexist:
// - C code is compiled via remaining_lib.c (through the cc crate in build.rs)
// - Rust modules here start as stubs and are activated one by one
// - As each module is activated, its corresponding #include is removed
//   from remaining_lib.c
//
// Module structure mirrors the C source files.

// Tier 0 — Pure leaf utilities
pub mod alloc;
pub mod error_costs;
pub mod length;
pub mod point;
pub mod unicode;

// Tier 1 — Core data structure
pub mod subtree;

// Tier 2 — Components depending on subtree
pub mod language;
pub mod lexer;
pub mod stack;

// Tier 3 — Tree navigation
pub mod get_changed_ranges;
pub mod node;
pub mod tree;
pub mod tree_cursor;

// Tier 4 — The engine
pub mod parser;
pub mod query;

// Internal helpers (no corresponding .c file)
pub mod reduce_action;
pub mod reusable_node;
