#![allow(dead_code)]

use super::stack::{
    array_back_ref, array_clear, array_delete, array_new, array_pop, array_push, Array,
};
use super::subtree::{
    subtree_child, subtree_child_count, subtree_has_external_tokens, subtree_last_external_token,
    subtree_total_bytes, Subtree, NULL_SUBTREE,
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

impl Default for ReusableNode {
    fn default() -> Self {
        Self::new()
    }
}

impl ReusableNode {
    pub const fn new() -> Self {
        Self {
            stack: array_new(),
            last_external_token: NULL_SUBTREE,
        }
    }

    pub fn clear(&mut self) {
        array_clear(&mut self.stack);
        self.last_external_token = NULL_SUBTREE;
    }

    unsafe fn last_entry(&self) -> Option<&StackEntry> {
        if self.stack.size > 0 {
            Some(array_back_ref(&self.stack))
        } else {
            None
        }
    }

    pub unsafe fn tree(&self) -> Subtree {
        self.last_entry().map_or(NULL_SUBTREE, |entry| entry.tree)
    }

    pub unsafe fn byte_offset(&self) -> u32 {
        self.last_entry()
            .map_or(u32::MAX, |entry| entry.byte_offset)
    }

    pub unsafe fn delete(&mut self) {
        array_delete(&mut self.stack);
    }

    /// Move from the current old-tree node to the next sibling in preorder.
    ///
    /// The current node is considered consumed. The cursor walks upward until it
    /// finds an ancestor with another child, then pushes that sibling. Reaching
    /// the top means the old tree has no more reusable candidates.
    pub unsafe fn advance(&mut self) {
        let Some(last_entry) = self.last_entry().copied() else {
            return;
        };
        let (byte_offset, last_external_token) = entry_after(last_entry);
        if !last_external_token.ptr.is_null() {
            self.last_external_token = last_external_token;
        }

        let mut tree;
        let mut next_index;
        loop {
            let popped_entry = array_pop(&mut self.stack);
            next_index = popped_entry.child_index + 1;
            if self.stack.size == 0 {
                return;
            }
            tree = self.last_entry().map_or(NULL_SUBTREE, |entry| entry.tree);
            if subtree_child_count(tree) > next_index {
                break;
            }
        }

        array_push(
            &mut self.stack,
            StackEntry {
                tree: *subtree_child(tree, next_index),
                child_index: next_index,
                byte_offset,
            },
        );
    }

    /// Descend from the current candidate to its first child.
    pub unsafe fn descend(&mut self) -> bool {
        let Some(last_entry) = self.last_entry().copied() else {
            return false;
        };
        if subtree_child_count(last_entry.tree) > 0 {
            array_push(
                &mut self.stack,
                StackEntry {
                    tree: *subtree_child(last_entry.tree, 0),
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
    pub unsafe fn advance_past_leaf(&mut self) {
        while self.descend() {}
        self.advance();
    }

    /// Reset the cursor to the first reusable child of an old root.
    ///
    /// The root is deliberately skipped because accepted roots contain
    /// parser-added structure such as EOF and trailing extras that should not be
    /// reused as a normal grammar node.
    pub unsafe fn reset(&mut self, tree: Subtree) {
        self.clear();
        array_push(
            &mut self.stack,
            StackEntry {
                tree,
                child_index: 0,
                byte_offset: 0,
            },
        );

        // Never reuse the root node, because it has a non-standard internal
        // structure due to transformations that are applied when it is accepted:
        // adding the EOF child and any extra children.
        if !self.descend() {
            self.clear();
        }
    }
}

/// Byte offset and external-scanner token immediately after a consumed entry.
#[inline]
unsafe fn entry_after(entry: StackEntry) -> (u32, Subtree) {
    let byte_offset = entry.byte_offset + subtree_total_bytes(entry.tree);
    let last_external_token = if subtree_has_external_tokens(entry.tree) {
        subtree_last_external_token(entry.tree)
    } else {
        NULL_SUBTREE
    };
    (byte_offset, last_external_token)
}
