#![allow(dead_code)]

use super::stack::{
    array_back_ref, array_clear, array_delete, array_new, array_pop, array_push, Array,
};
use super::subtree::{
    ts_subtree_child_count, ts_subtree_children, ts_subtree_has_external_tokens,
    ts_subtree_last_external_token, ts_subtree_total_bytes, Subtree, NULL_SUBTREE,
};

/// One frame in the old-tree reuse cursor.
///
/// The parser uses this as a preorder cursor over the previous syntax tree.
/// Each frame records the current subtree, its index within the parent, and the
/// subtree's byte offset in the original input.
#[repr(C)]
#[derive(Clone, Copy)]
struct StackEntry {
    /// Old-tree subtree currently being considered for reuse.
    tree: Subtree,
    /// Child index of `tree` in its parent.
    child_index: u32,
    /// Absolute byte offset where `tree` starts.
    byte_offset: u32,
}

/// Cursor over an old syntax tree for incremental reparsing.
///
/// The parser advances this cursor in source order and asks whether the current
/// old subtree can replace freshly parsed input. The stack stores the path from
/// the old root to the current node so the cursor can descend, skip leaves, and
/// move to the next sibling without parent pointers in the tree.
#[repr(C)]
pub struct ReusableNode {
    /// Path from the old root to the current candidate subtree.
    stack: Array<StackEntry>,
    /// Last old-tree token with external scanner state encountered by advance.
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

#[inline]
unsafe fn reusable_node_entry_after(entry: StackEntry) -> (u32, Subtree) {
    let byte_offset = entry.byte_offset + ts_subtree_total_bytes(entry.tree);
    let last_external_token = if ts_subtree_has_external_tokens(entry.tree) {
        ts_subtree_last_external_token(entry.tree)
    } else {
        NULL_SUBTREE
    };
    (byte_offset, last_external_token)
}

/// Move from the current old-tree node to the next sibling in preorder.
///
/// The current node is considered consumed. The cursor walks upward until it
/// finds an ancestor with another child, then pushes that sibling. Reaching the
/// top means the old tree has no more reusable candidates.
pub unsafe fn reusable_node_advance(self_: &mut ReusableNode) {
    let Some(last_entry) = reusable_node_last_entry(self_).copied() else {
        return;
    };
    let (byte_offset, last_external_token) = reusable_node_entry_after(last_entry);
    if !last_external_token.ptr.is_null() {
        self_.last_external_token = last_external_token;
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

/// Descend from the current candidate to its first child.
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

/// Move past the current leaf, descending first if the candidate is a node.
pub unsafe fn reusable_node_advance_past_leaf(self_: &mut ReusableNode) {
    while reusable_node_descend(self_) {}
    reusable_node_advance(self_);
}

/// Reset the cursor to the first reusable child of an old root.
///
/// The root is deliberately skipped because accepted roots contain parser-added
/// structure such as EOF and trailing extras that should not be reused as a
/// normal grammar node.
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
    debug_assert!(index < ts_subtree_child_count(parent));
    ts_subtree_children(parent)
        .add(index as usize)
        .as_ref()
        .unwrap_unchecked()
}
