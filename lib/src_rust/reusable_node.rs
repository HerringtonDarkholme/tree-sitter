#![allow(dead_code)]

use super::stack::{
    Array, array_back_ref, array_clear, array_delete, array_new, array_pop, array_push,
};
use super::subtree::{
    NULL_SUBTREE, Subtree, ts_subtree_child_count, ts_subtree_children,
    ts_subtree_has_external_tokens, ts_subtree_last_external_token, ts_subtree_total_bytes,
};

/// `StackEntry` — for `ReusableNode` (from `reusable_node.h`)
#[repr(C)]
#[derive(Clone, Copy)]
struct StackEntry {
    tree: Subtree,
    child_index: u32,
    byte_offset: u32,
}

/// `ReusableNode` — for incremental reparsing (from `reusable_node.h`)
#[repr(C)]
pub struct ReusableNode {
    stack: Array<StackEntry>,
    pub last_external_token: Subtree,
}

pub const unsafe fn reusable_node_new() -> ReusableNode {
    ReusableNode {
        stack: array_new(),
        last_external_token: NULL_SUBTREE,
    }
}

pub unsafe fn reusable_node_clear(self_: &mut ReusableNode) {
    array_clear(&mut self_.stack);
    self_.last_external_token = NULL_SUBTREE;
}

unsafe fn reusable_node_last_entry(self_: &ReusableNode) -> Option<&StackEntry> {
    if self_.stack.size > 0 {
        Some(array_back_ref(&self_.stack))
    } else {
        None
    }
}

pub unsafe fn reusable_node_tree(self_: &ReusableNode) -> Subtree {
    reusable_node_last_entry(self_).map_or(NULL_SUBTREE, |entry| entry.tree)
}

pub unsafe fn reusable_node_byte_offset(self_: &ReusableNode) -> u32 {
    reusable_node_last_entry(self_).map_or(u32::MAX, |entry| entry.byte_offset)
}

pub unsafe fn reusable_node_delete(self_: &mut ReusableNode) {
    array_delete(&mut self_.stack);
}

pub unsafe fn reusable_node_advance(self_: &mut ReusableNode) {
    let Some(last_entry) = reusable_node_last_entry(self_).copied() else {
        return;
    };
    let byte_offset = last_entry.byte_offset + ts_subtree_total_bytes(last_entry.tree);
    if ts_subtree_has_external_tokens(last_entry.tree) {
        self_.last_external_token = ts_subtree_last_external_token(last_entry.tree);
    }

    let mut tree;
    let mut next_index;
    loop {
        let popped_entry = array_pop(&mut self_.stack);
        next_index = popped_entry.child_index + 1;
        if self_.stack.size == 0 {
            return;
        }
        tree = reusable_node_last_entry(self_).map_or(NULL_SUBTREE, |entry| entry.tree);
        if ts_subtree_child_count(tree) > next_index {
            break;
        }
    }

    array_push(
        &mut self_.stack,
        StackEntry {
            tree: *reusable_node_subtree_child(tree, next_index),
            child_index: next_index,
            byte_offset,
        },
    );
}

pub unsafe fn reusable_node_descend(self_: &mut ReusableNode) -> bool {
    let Some(last_entry) = reusable_node_last_entry(self_).copied() else {
        return false;
    };
    if ts_subtree_child_count(last_entry.tree) > 0 {
        array_push(
            &mut self_.stack,
            StackEntry {
                tree: *reusable_node_subtree_child(last_entry.tree, 0),
                child_index: 0,
                byte_offset: last_entry.byte_offset,
            },
        );
        true
    } else {
        false
    }
}

pub unsafe fn reusable_node_advance_past_leaf(self_: &mut ReusableNode) {
    while reusable_node_descend(self_) {}
    reusable_node_advance(self_);
}

pub unsafe fn reusable_node_reset(self_: &mut ReusableNode, tree: Subtree) {
    reusable_node_clear(self_);
    array_push(
        &mut self_.stack,
        StackEntry {
            tree,
            child_index: 0,
            byte_offset: 0,
        },
    );

    // Never reuse the root node, because it has a non-standard internal structure
    // due to transformations that are applied when it is accepted: adding the EOF
    // child and any extra children.
    if !reusable_node_descend(self_) {
        reusable_node_clear(self_);
    }
}

#[inline]
unsafe fn reusable_node_subtree_child<'a>(parent: Subtree, index: u32) -> &'a Subtree {
    std::slice::from_raw_parts(
        ts_subtree_children(parent),
        ts_subtree_child_count(parent) as usize,
    )
    .get_unchecked(index as usize)
}
