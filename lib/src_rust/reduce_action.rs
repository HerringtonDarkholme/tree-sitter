#![allow(dead_code)]

// Reduce action deduplication.

use crate::ffi::TSSymbol;

/// Compact reduce-action identity used while gathering recovery candidates.
#[derive(Clone, Copy, Debug)]
pub struct ReduceAction {
    /// Number of stack entries consumed.
    pub count: u32,
    /// Symbol produced by the reduction.
    pub symbol: TSSymbol,
    /// Dynamic precedence delta.
    pub dynamic_precedence: i32,
    /// Production id for fields/aliases.
    pub production_id: u16,
}

/// Small growable set that deduplicates equivalent reduce actions.
#[derive(Default)]
pub struct ReduceActionSet {
    /// Stored unique actions.
    pub actions: Vec<ReduceAction>,
}

impl ReduceActionSet {
    /// Add `new_action` unless an action with the same symbol/count exists.
    pub fn add(&mut self, new_action: ReduceAction) {
        for action in &self.actions {
            if action.symbol == new_action.symbol && action.count == new_action.count {
                return;
            }
        }
        self.actions.push(new_action);
    }

    pub fn clear(&mut self) {
        self.actions.clear();
    }
}
