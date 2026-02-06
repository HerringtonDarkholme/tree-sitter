#![allow(dead_code)]

// Stub for reduce_action.h — Reduce action deduplication.

use crate::ffi::TSSymbol;

#[derive(Clone, Copy, Debug)]
pub struct ReduceAction {
    pub count: u32,
    pub symbol: TSSymbol,
    pub dynamic_precedence: i32,
    pub production_id: u16,
}

#[derive(Default)]
pub struct ReduceActionSet {
    pub actions: Vec<ReduceAction>,
}

impl ReduceActionSet {
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
