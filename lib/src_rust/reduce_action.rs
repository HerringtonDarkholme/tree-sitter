use crate::ffi::TSSymbol;

use super::utils::Array;

/// Candidate reduction used while searching recovery actions.
///
/// Recovery can scan many lookahead symbols for a parse state. This compact
/// record deduplicates equivalent reduce actions before applying them.
#[derive(Clone, Copy)]
pub struct ReduceAction {
    /// Number of stack entries consumed by the reduce action.
    pub(super) count: u32,
    /// Grammar symbol produced by the reduction.
    pub(super) symbol: TSSymbol,
    /// Dynamic precedence delta for conflict resolution.
    pub(super) dynamic_precedence: i32,
    /// Production id used for alias/field metadata on the new subtree.
    pub(super) production_id: u16,
}

/// `ReduceActionSet` — Array(ReduceAction)
pub type ReduceActionSet = Array<ReduceAction>;

pub unsafe fn reduce_action_set_add(actions: &mut ReduceActionSet, new_action: ReduceAction) {
    for action in actions.as_slice() {
        if action.symbol == new_action.symbol && action.count == new_action.count {
            return;
        }
    }
    actions.push(new_action);
}
