//! Core runtime implementation for the tree-sitter library.
//!
//! A normal parse flows through these modules in one direction: [`parser`]
//! asks [`lexer`] for a token using tables exposed by [`language`], interprets
//! parse actions while storing alternatives in [`stack`], and builds values
//! owned by [`subtree`]. The accepted root becomes a [`tree`]; [`node`] and
//! [`tree_cursor`] provide public views over it, while [`get_changed_ranges`]
//! compares completed trees.
//!
//! Modules at this level correspond to the runtime's established components.
//! Their exported functions preserve the C API, but internal parser, stack,
//! and storage types use Rust layout unless a module documents an ABI boundary.
//!
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
