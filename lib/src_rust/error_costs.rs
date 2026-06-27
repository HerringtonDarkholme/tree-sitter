#![allow(dead_code)]

//! Parser recovery cost constants.
//!
//! Error recovery compares stack versions by an accumulated cost. Smaller costs
//! are preferred, so skipped characters are cheap, skipped trees/inserted
//! missing nodes are more expensive, and entering recovery has a large fixed
//! penalty.

/// Synthetic parse state used while recovering from syntax errors.
pub const ERROR_STATE: u16 = 0;

/// Fixed cost for entering error recovery.
pub const ERROR_COST_PER_RECOVERY: u32 = 500;

/// Cost for inserting a missing syntax node.
pub const ERROR_COST_PER_MISSING_TREE: u32 = 110;

/// Cost for skipping a full subtree.
pub const ERROR_COST_PER_SKIPPED_TREE: u32 = 100;

/// Cost for skipping one line of source.
pub const ERROR_COST_PER_SKIPPED_LINE: u32 = 30;

/// Cost for skipping one character of source.
pub const ERROR_COST_PER_SKIPPED_CHAR: u32 = 1;
