#![allow(dead_code)]

// Constants from error_costs.h

pub const ERROR_STATE: u16 = 0;
pub const ERROR_COST_PER_RECOVERY: u32 = 500;
pub const ERROR_COST_PER_MISSING_TREE: u32 = 110;
pub const ERROR_COST_PER_SKIPPED_TREE: u32 = 100;
pub const ERROR_COST_PER_SKIPPED_LINE: u32 = 30;
pub const ERROR_COST_PER_SKIPPED_CHAR: u32 = 1;
