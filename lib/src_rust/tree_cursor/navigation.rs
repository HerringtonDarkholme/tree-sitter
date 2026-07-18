//! Movement algorithms for a path-based tree cursor.
//!
//! Cursor movement operates on raw stored children but reports steps in the
//! public tree. Hidden nodes may add path frames without becoming visible
//! results, extras do not advance structural indexes, and aliases come from
//! the parent production. [`CursorChildIterator`] centralizes that bookkeeping
//! for child, sibling, descendant, and byte-position navigation.

use core::ptr;

use crate::ffi::{TSNode, TSPoint, TSSymbol};

use super::super::language::{language_alias_at, language_alias_sequence_slice};
use super::super::length::{
    length_add, length_is_undefined, length_zero, Length, LENGTH_UNDEFINED,
};
use super::super::node::node_new;
use super::super::point::point_gt;
use super::super::subtree::{Subtree, SubtreeArena, NULL_SUBTREE};
use super::{TreeCursor, TreeCursorEntry, TreeCursorStep};

/// Child iterator that maintains cursor-specific position and alias state.
pub(super) struct CursorChildIterator {
    pub(super) arena: *mut SubtreeArena,
    /// Parent whose children are being scanned.
    pub(super) parent: Subtree,
    /// Start position of the next child.
    pub(super) position: Length,
    /// Raw child index of the next child.
    pub(super) child_index: u32,
    /// Non-extra child index of the next child.
    pub(super) structural_child_index: u32,
    /// Visible descendant index of the next child.
    pub(super) descendant_index: u32,
    /// Alias sequence for the parent production.
    pub(super) alias_sequence: &'static [TSSymbol],
}

#[derive(Clone, Copy)]
pub(super) struct CursorChild {
    pub(super) entry: TreeCursorEntry,
    pub(super) visible: bool,
}

// Internal helper functions
// ---------------------------------------------------------------------------

#[inline]
pub(super) unsafe fn tree_cursor_is_entry_visible(self_: &TreeCursor, index: u32) -> bool {
    let arena = self_.arena();
    let entries = self_.stack.as_slice();
    let entry = entries.get_unchecked(index as usize);
    if index == 0 || (*entry.subtree).visible(arena) {
        return true;
    }
    if !(*entry.subtree).extra(arena) {
        let parent_entry = entries.get_unchecked((index - 1) as usize);
        return language_alias_at(
            (*self_.tree).language,
            u32::from(
                (*parent_entry.subtree)
                    .heap_data(arena)
                    .children()
                    .production_id,
            ),
            entry.structural_child_index,
        ) != 0;
    }
    false
}

#[inline]
pub(super) unsafe fn tree_cursor_iterate_children(self_: &TreeCursor) -> CursorChildIterator {
    let arena = self_.arena();
    let last_entry = self_.stack.as_slice().last().unwrap_unchecked();
    if (*last_entry.subtree).child_count(arena) == 0 {
        return CursorChildIterator {
            arena,
            parent: NULL_SUBTREE,
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
            alias_sequence: &[],
        };
    }
    let alias_sequence = language_alias_sequence_slice(
        (*self_.tree).language,
        u32::from(
            (*last_entry.subtree)
                .heap_data(arena)
                .children()
                .production_id,
        ),
    );

    let mut descendant_index = last_entry.descendant_index;
    if tree_cursor_is_entry_visible(self_, self_.stack.size - 1) {
        descendant_index += 1;
    }

    CursorChildIterator {
        arena,
        parent: *last_entry.subtree,
        position: last_entry.position,
        child_index: 0,
        structural_child_index: 0,
        descendant_index,
        alias_sequence,
    }
}

pub(super) unsafe fn tree_cursor_child_iterator_next(
    self_: &mut CursorChildIterator,
) -> Option<CursorChild> {
    if self_.parent.is_null()
        || self_.child_index == self_.parent.heap_data(self_.arena).child_count
    {
        return None;
    }
    let child = (self_.parent).child(self_.arena, self_.child_index);
    let entry = TreeCursorEntry {
        subtree: child,
        position: self_.position,
        child_index: self_.child_index,
        structural_child_index: self_.structural_child_index,
        descendant_index: self_.descendant_index,
    };
    let mut visible = (*child).visible(self_.arena);
    let extra = (*child).extra(self_.arena);
    if !extra {
        visible |= self_
            .alias_sequence
            .get(self_.structural_child_index as usize)
            .is_some_and(|alias| *alias != 0);
        self_.structural_child_index += 1;
    }

    self_.descendant_index += (*child).visible_descendant_count(self_.arena);
    if visible {
        self_.descendant_index += 1;
    }

    self_.position = length_add(self_.position, (*child).size(self_.arena));
    self_.child_index += 1;

    if self_.child_index < self_.parent.heap_data(self_.arena).child_count {
        let next_child = *(self_.parent).child(self_.arena, self_.child_index);
        self_.position = length_add(self_.position, next_child.padding(self_.arena));
    }

    Some(CursorChild { entry, visible })
}

#[inline]
const fn length_backtrack(a: Length, b: Length) -> Length {
    if length_is_undefined(a) || b.extent.row != 0 {
        return LENGTH_UNDEFINED;
    }
    Length {
        bytes: a.bytes - b.bytes,
        extent: TSPoint {
            row: a.extent.row,
            column: a.extent.column - b.extent.column,
        },
    }
}

/// Step the child iterator backward.
///
/// Reverse traversal reconstructs each previous child's start position by
/// subtracting padding and size. If a multi-line span prevents precise column
/// backtracking, the returned position becomes `LENGTH_UNDEFINED`, matching the
/// C cursor behavior.
pub(super) unsafe fn tree_cursor_child_iterator_previous(
    self_: &mut CursorChildIterator,
) -> Option<CursorChild> {
    if self_.parent.is_null() || self_.child_index == u32::MAX {
        return None;
    }
    let child = (self_.parent).child(self_.arena, self_.child_index);
    let entry = TreeCursorEntry {
        subtree: child,
        position: self_.position,
        child_index: self_.child_index,
        structural_child_index: self_.structural_child_index,
        descendant_index: 0, // not used in previous iteration
    };
    let mut visible = (*child).visible(self_.arena);
    let extra = (*child).extra(self_.arena);

    self_.position = length_backtrack(self_.position, (*child).padding(self_.arena));
    self_.child_index = self_.child_index.wrapping_sub(1);

    if !extra {
        visible |= self_
            .alias_sequence
            .get(self_.structural_child_index as usize)
            .is_some_and(|alias| *alias != 0);
        if self_.structural_child_index > 0 {
            self_.structural_child_index -= 1;
        }
    }

    // unsigned can underflow so compare it to child_count
    if self_.child_index < self_.parent.heap_data(self_.arena).child_count {
        let previous_child = *(self_.parent).child(self_.arena, self_.child_index);
        let size = previous_child.size(self_.arena);
        self_.position = length_backtrack(self_.position, size);
    }

    Some(CursorChild { entry, visible })
}

/// Descend to the first visible child covering a byte/point target.
///
/// Hidden nodes are traversed when they contain visible descendants. If no
/// matching visible child exists, the cursor stack is restored to its original
/// depth.
#[inline]
pub(super) unsafe fn tree_cursor_goto_first_child_for_byte_and_point(
    cursor: &mut TreeCursor,
    goal_byte: u32,
    goal_point: TSPoint,
) -> i64 {
    let initial_size = cursor.stack.size;
    let mut visible_child_index: u32 = 0;

    loop {
        let mut did_descend = false;

        let mut iterator = tree_cursor_iterate_children(cursor);
        while let Some(child) = tree_cursor_child_iterator_next(&mut iterator) {
            let entry = child.entry;
            let entry_end = length_add(entry.position, (*entry.subtree).size(cursor.arena()));
            let at_goal = entry_end.bytes > goal_byte && point_gt(entry_end.extent, goal_point);
            let visible_child_count = (*entry.subtree).visible_child_count(cursor.arena());
            if at_goal {
                if child.visible {
                    cursor.stack.push(entry);
                    return i64::from(visible_child_index);
                }
                if visible_child_count > 0 {
                    cursor.stack.push(entry);
                    did_descend = true;
                    break;
                }
            } else if child.visible {
                visible_child_index += 1;
            } else {
                visible_child_index += visible_child_count;
            }
        }
        if !did_descend {
            break;
        }
    }

    cursor.stack.size = initial_size;
    -1
}

/// Shared sibling navigation implementation.
///
/// The `advance` callback chooses next-vs-previous traversal. The cursor walks
/// upward until it can find a visible sibling, or a hidden sibling that contains
/// visible descendants. On failure, it restores the original stack.
pub(super) unsafe fn tree_cursor_goto_sibling_internal(
    cursor: &mut TreeCursor,
    advance: unsafe fn(&mut CursorChildIterator) -> Option<CursorChild>,
) -> TreeCursorStep {
    let initial_size = cursor.stack.size;

    while cursor.stack.size > 1 {
        let entry = cursor.stack.pop();
        let mut iterator = tree_cursor_iterate_children(cursor);
        iterator.child_index = entry.child_index;
        iterator.structural_child_index = entry.structural_child_index;
        iterator.position = entry.position;
        iterator.descendant_index = entry.descendant_index;

        if let Some(child) = advance(&mut iterator) {
            if child.visible && cursor.stack.size + 1 < initial_size {
                break;
            }
        }

        while let Some(child) = advance(&mut iterator) {
            let entry = child.entry;
            if child.visible {
                cursor.stack.push(entry);
                return TreeCursorStep::Visible;
            }

            if (*entry.subtree).visible_child_count(cursor.arena()) > 0 {
                cursor.stack.push(entry);
                return TreeCursorStep::Hidden;
            }
        }
    }

    cursor.stack.size = initial_size;
    TreeCursorStep::None
}

pub unsafe fn tree_cursor_goto_first_child_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    let mut iterator = tree_cursor_iterate_children(cursor);
    while let Some(child) = tree_cursor_child_iterator_next(&mut iterator) {
        let entry = child.entry;
        if child.visible {
            cursor.stack.push(entry);
            return TreeCursorStep::Visible;
        }
        if (*entry.subtree).visible_child_count(cursor.arena()) > 0 {
            cursor.stack.push(entry);
            return TreeCursorStep::Hidden;
        }
    }
    TreeCursorStep::None
}

pub(super) unsafe fn tree_cursor_goto_first_child(cursor: &mut TreeCursor) -> bool {
    loop {
        match tree_cursor_goto_first_child_internal(cursor) {
            TreeCursorStep::Hidden => {}
            TreeCursorStep::Visible => return true,
            _ => return false,
        }
    }
}
unsafe fn tree_cursor_goto_last_child_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    let mut iterator = tree_cursor_iterate_children(cursor);
    if iterator.parent.is_null() || iterator.parent.heap_data(iterator.arena).child_count == 0 {
        return TreeCursorStep::None;
    }

    let mut last_entry = TreeCursorEntry::empty();
    let mut last_step = TreeCursorStep::None;
    while let Some(child) = tree_cursor_child_iterator_next(&mut iterator) {
        let entry = child.entry;
        if child.visible {
            last_entry = entry;
            last_step = TreeCursorStep::Visible;
        } else if (*entry.subtree).visible_child_count(cursor.arena()) > 0 {
            last_entry = entry;
            last_step = TreeCursorStep::Hidden;
        }
    }
    if !last_entry.subtree.is_null() {
        cursor.stack.push(last_entry);
        return last_step;
    }

    TreeCursorStep::None
}

pub(super) unsafe fn tree_cursor_goto_last_child(cursor: &mut TreeCursor) -> bool {
    loop {
        match tree_cursor_goto_last_child_internal(cursor) {
            TreeCursorStep::Hidden => {}
            TreeCursorStep::Visible => return true,
            _ => return false,
        }
    }
}
pub unsafe fn tree_cursor_goto_next_sibling_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    tree_cursor_goto_sibling_internal(cursor, tree_cursor_child_iterator_next)
}
pub(super) unsafe fn tree_cursor_goto_previous_sibling_internal(
    cursor: &mut TreeCursor,
) -> TreeCursorStep {
    let step = tree_cursor_goto_sibling_internal(cursor, tree_cursor_child_iterator_previous);
    if step == TreeCursorStep::None {
        return step;
    }

    // if length is already valid, there's no need to recompute it
    let entries = cursor.stack.as_slice();
    let last_entry = entries.last().unwrap_unchecked();
    if !length_is_undefined(last_entry.position) {
        return step;
    }

    // restore position from the parent node
    let parent = entries.get_unchecked(cursor.stack.size as usize - 2);
    let mut position = parent.position;
    let child_index = last_entry.child_index;
    let arena = cursor.arena();
    let children = (*parent.subtree).children(arena);

    if child_index > 0 {
        // skip first child padding since its position should match the position of the parent
        position = length_add(position, (*children.get_unchecked(0)).size(arena));
        for i in 1..child_index {
            position = length_add(
                position,
                (*children.get_unchecked(i as usize)).total_size(arena),
            );
        }
        position = length_add(
            position,
            (*children.get_unchecked(child_index as usize)).padding(arena),
        );
    }

    cursor
        .stack
        .contents
        .add(cursor.stack.size as usize - 1)
        .as_mut()
        .unwrap_unchecked()
        .position = position;

    step
}

pub(super) unsafe fn tree_cursor_goto_parent(cursor: &mut TreeCursor) -> bool {
    let mut index = cursor.stack.size as i32 - 2;
    while index >= 0 {
        if tree_cursor_is_entry_visible(cursor, index as u32) {
            cursor.stack.size = index as u32 + 1;
            return true;
        }
        index -= 1;
    }
    false
}

pub(super) unsafe fn tree_cursor_goto_descendant(
    cursor: &mut TreeCursor,
    goal_descendant_index: u32,
) {
    // Ascend to the lowest ancestor that contains the goal node.
    loop {
        let index = cursor.stack.size - 1;
        let entry = cursor.stack.as_slice().get_unchecked(index as usize);
        let next_descendant_index = entry.descendant_index
            + u32::from(tree_cursor_is_entry_visible(cursor, index))
            + (*entry.subtree).visible_descendant_count(cursor.arena());
        if entry.descendant_index <= goal_descendant_index
            && next_descendant_index > goal_descendant_index
        {
            break;
        }
        if cursor.stack.size <= 1 {
            return;
        }
        cursor.stack.size -= 1;
    }

    // Descend to the goal node.
    loop {
        let mut did_descend = false;
        let mut iterator = tree_cursor_iterate_children(cursor);
        if iterator.descendant_index > goal_descendant_index {
            return;
        }

        while let Some(child) = tree_cursor_child_iterator_next(&mut iterator) {
            let entry = child.entry;
            if iterator.descendant_index > goal_descendant_index {
                cursor.stack.push(entry);
                if child.visible && entry.descendant_index == goal_descendant_index {
                    return;
                }
                did_descend = true;
                break;
            }
        }
        if !did_descend {
            break;
        }
    }
}

pub(super) unsafe fn tree_cursor_current_descendant_index(cursor: &TreeCursor) -> u32 {
    cursor
        .stack
        .as_slice()
        .last()
        .unwrap_unchecked()
        .descendant_index
}

pub(super) unsafe fn tree_cursor_current_depth(cursor: &TreeCursor) -> u32 {
    let mut depth = 0;
    for index in 1..cursor.stack.size {
        if tree_cursor_is_entry_visible(cursor, index) {
            depth += 1;
        }
    }
    depth
}

pub(super) unsafe fn tree_cursor_parent_node(cursor: &TreeCursor) -> TSNode {
    let entries = cursor.stack.as_slice();
    let mut index = cursor.stack.size as i32 - 2;
    while index >= 0 {
        let entry = entries.get_unchecked(index as usize);
        let mut is_visible = true;
        let mut alias_symbol: TSSymbol = 0;
        if index > 0 {
            let parent_entry = entries.get_unchecked(index as usize - 1);
            alias_symbol = language_alias_at(
                (*cursor.tree).language,
                u32::from(
                    (*parent_entry.subtree)
                        .heap_data(cursor.arena())
                        .children()
                        .production_id,
                ),
                entry.structural_child_index,
            );
            is_visible = alias_symbol != 0 || (*entry.subtree).visible(cursor.arena());
        }
        if is_visible {
            return node_new(cursor.tree, entry.subtree, entry.position, alias_symbol);
        }
        index -= 1;
    }
    node_new(ptr::null(), ptr::null(), length_zero(), 0)
}
