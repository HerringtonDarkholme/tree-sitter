//! Core runtime implementation for the tree-sitter library.
//!
//! Most modules correspond to one of the runtime's established components.
//! The only C source retained by the Rust core is the variadic lexer logging
//! shim, because stable Rust cannot define C-variadic functions.

// Leaf utilities.
pub mod alloc;
pub mod error_costs;
pub mod length;
pub mod point;
pub mod unicode;
pub mod utils;

// Core syntax-tree storage.
pub mod subtree;

// Parsing components.
pub mod language;
pub mod lexer;
pub mod stack;

// Tree navigation and change tracking.
pub mod get_changed_ranges;
pub mod node;
pub mod tree;
pub mod tree_cursor;

// Parser engine.
pub mod parser;

// Kept separate from the active-runtime readability work.
pub mod query;

// Internal helpers for the active Rust runtime (no corresponding .c file).
mod reduce_action;
