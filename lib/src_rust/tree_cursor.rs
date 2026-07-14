use core::ptr;

use crate::ffi::{TSNode, TSPoint, TSSymbol, TSTreeCursor};

use super::language::{language_alias_at, language_alias_sequence, ts_language_symbol_metadata};
use super::length::{length_add, length_is_undefined, length_zero, Length, LENGTH_UNDEFINED};
use super::node::{node_new, ts_node_start_byte, ts_node_start_point};
use super::point::point_gt;
use super::subtree::{
    subtree_child, subtree_child_count, subtree_children_slice, subtree_extra, subtree_padding,
    subtree_size, subtree_symbol, subtree_total_size, subtree_visible, subtree_visible_child_count,
    subtree_visible_descendant_count, Subtree, NULL_SUBTREE,
};
use super::tree::TSTree;
use super::utils::{
    array_assign, array_clear, array_delete, array_init, array_pop, array_push, Array,
};
use super::utils::{ptr_mut, ptr_ref};

mod status;
pub use status::{ts_tree_cursor_current_node, ts_tree_cursor_current_status};

use crate::ffi::TSPoint as POINT_ZERO_TYPE;
const POINT_ZERO: POINT_ZERO_TYPE = POINT_ZERO_TYPE { row: 0, column: 0 };

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One stack frame in a tree cursor traversal.
///
/// A cursor stores the path from the root node to the current node. Each frame
/// keeps enough sibling/position bookkeeping to move sideways or back upward
/// without parent pointers in subtrees.
#[derive(Clone, Copy)]
pub struct TreeCursorEntry {
    /// Current subtree pointer for this depth.
    pub subtree: *const Subtree,
    /// Start position of `subtree`.
    pub position: Length,
    /// Raw child index in the parent, including hidden/extra children.
    pub child_index: u32,
    /// Index among non-extra children, used for alias and field tables.
    pub structural_child_index: u32,
    /// Visible descendant index for public cursor APIs.
    pub descendant_index: u32,
}

impl TreeCursorEntry {
    #[inline]
    const fn empty() -> Self {
        Self {
            subtree: ptr::null(),
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
        }
    }
}

pub type TreeCursorEntryArray = Array<TreeCursorEntry>;

/// Internal cursor representation cast to/from public `TSTreeCursor`.
#[repr(C)]
pub struct TreeCursor {
    /// Tree that owns all subtree pointers in `stack`.
    pub tree: *const TSTree,
    /// Path from root to current cursor node.
    pub stack: TreeCursorEntryArray,
    /// Alias to apply to the root node, or zero.
    pub root_alias_symbol: TSSymbol,
}

// `TreeCursor` is stored directly in the public `TSTreeCursor` value. Keep the
// outer layout fixed while allowing heap-allocated cursor entries to use Rust
// layout.
const _: () = assert!(core::mem::size_of::<TreeCursor>() == core::mem::size_of::<TSTreeCursor>());
const _: () = assert!(core::mem::align_of::<TreeCursor>() == core::mem::align_of::<TSTreeCursor>());

#[inline]
unsafe fn cursor_ref<'a>(cursor: *const TSTreeCursor) -> &'a TreeCursor {
    ptr_ref(cursor.cast::<TreeCursor>())
}

#[inline]
unsafe fn cursor_mut<'a>(cursor: *mut TSTreeCursor) -> &'a mut TreeCursor {
    ptr_mut(cursor.cast::<TreeCursor>())
}

#[inline]
unsafe fn out_param_mut<'a, T>(ptr: *mut T) -> &'a mut T {
    ptr_mut(ptr)
}

/// Result of internal navigation.
/// Result returned by the two exported internal cursor navigation functions.
///
/// These symbols are consumed by the C query implementation, so the enum's
/// integer representation is part of that internal C boundary.
#[repr(C)]
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TreeCursorStep {
    None = 0,
    Hidden = 1,
    Visible = 2,
}

/// Child iterator that maintains cursor-specific position and alias state.
struct CursorChildIterator {
    /// Parent whose children are being scanned.
    parent: Subtree,
    /// Start position of the next child.
    position: Length,
    /// Raw child index of the next child.
    child_index: u32,
    /// Non-extra child index of the next child.
    structural_child_index: u32,
    /// Visible descendant index of the next child.
    descendant_index: u32,
    /// Alias sequence for the parent production.
    alias_sequence: *const TSSymbol,
}

#[derive(Clone, Copy)]
struct CursorChild {
    entry: TreeCursorEntry,
    visible: bool,
}

// ---------------------------------------------------------------------------
// Array helpers for TreeCursorEntry stack
// ---------------------------------------------------------------------------

#[inline]
pub const unsafe fn tree_cursor_entry_slice(arr: &TreeCursorEntryArray) -> &[TreeCursorEntry] {
    core::slice::from_raw_parts(arr.contents, arr.size as usize)
}

// ---------------------------------------------------------------------------
// Internal helper functions
// ---------------------------------------------------------------------------

#[inline]
unsafe fn tree_cursor_is_entry_visible(self_: &TreeCursor, index: u32) -> bool {
    let entries = tree_cursor_entry_slice(&self_.stack);
    let entry = entries.get_unchecked(index as usize);
    if index == 0 || subtree_visible(*entry.subtree) {
        return true;
    }
    if !subtree_extra(*entry.subtree) {
        let parent_entry = entries.get_unchecked((index - 1) as usize);
        return language_alias_at(
            (*self_.tree).language,
            u32::from((*(*parent_entry.subtree).ptr).data.children.production_id),
            entry.structural_child_index,
        ) != 0;
    }
    false
}

#[inline]
unsafe fn tree_cursor_iterate_children(self_: &TreeCursor) -> CursorChildIterator {
    let last_entry = tree_cursor_entry_slice(&self_.stack)
        .last()
        .unwrap_unchecked();
    if subtree_child_count(*last_entry.subtree) == 0 {
        return CursorChildIterator {
            parent: NULL_SUBTREE,
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
            alias_sequence: ptr::null(),
        };
    }
    let alias_sequence = language_alias_sequence(
        (*self_.tree).language,
        u32::from((*(*last_entry.subtree).ptr).data.children.production_id),
    );

    let mut descendant_index = last_entry.descendant_index;
    if tree_cursor_is_entry_visible(self_, self_.stack.size - 1) {
        descendant_index += 1;
    }

    CursorChildIterator {
        parent: *last_entry.subtree,
        position: last_entry.position,
        child_index: 0,
        structural_child_index: 0,
        descendant_index,
        alias_sequence,
    }
}

unsafe fn tree_cursor_child_iterator_next(self_: &mut CursorChildIterator) -> Option<CursorChild> {
    if self_.parent.ptr.is_null() || self_.child_index == (*self_.parent.ptr).child_count {
        return None;
    }
    let child = subtree_child(self_.parent, self_.child_index);
    let entry = TreeCursorEntry {
        subtree: child,
        position: self_.position,
        child_index: self_.child_index,
        structural_child_index: self_.structural_child_index,
        descendant_index: self_.descendant_index,
    };
    let mut visible = subtree_visible(*child);
    let extra = subtree_extra(*child);
    if !extra {
        if !self_.alias_sequence.is_null() {
            visible |= *self_
                .alias_sequence
                .add(self_.structural_child_index as usize)
                != 0;
        }
        self_.structural_child_index += 1;
    }

    self_.descendant_index += subtree_visible_descendant_count(*child);
    if visible {
        self_.descendant_index += 1;
    }

    self_.position = length_add(self_.position, subtree_size(*child));
    self_.child_index += 1;

    if self_.child_index < (*self_.parent.ptr).child_count {
        let next_child = *subtree_child(self_.parent, self_.child_index);
        self_.position = length_add(self_.position, subtree_padding(next_child));
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
unsafe fn tree_cursor_child_iterator_previous(
    self_: &mut CursorChildIterator,
) -> Option<CursorChild> {
    if self_.parent.ptr.is_null() || self_.child_index == u32::MAX {
        return None;
    }
    let child = subtree_child(self_.parent, self_.child_index);
    let entry = TreeCursorEntry {
        subtree: child,
        position: self_.position,
        child_index: self_.child_index,
        structural_child_index: self_.structural_child_index,
        descendant_index: 0, // not used in previous iteration
    };
    let mut visible = subtree_visible(*child);
    let extra = subtree_extra(*child);

    self_.position = length_backtrack(self_.position, subtree_padding(*child));
    self_.child_index = self_.child_index.wrapping_sub(1);

    if !extra && !self_.alias_sequence.is_null() {
        visible |= *self_
            .alias_sequence
            .add(self_.structural_child_index as usize)
            != 0;
        if self_.structural_child_index > 0 {
            self_.structural_child_index -= 1;
        }
    }

    // unsigned can underflow so compare it to child_count
    if self_.child_index < (*self_.parent.ptr).child_count {
        let previous_child = *subtree_child(self_.parent, self_.child_index);
        let size = subtree_size(previous_child);
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
unsafe fn tree_cursor_goto_first_child_for_byte_and_point(
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
            let entry_end = length_add(entry.position, subtree_size(*entry.subtree));
            let at_goal = entry_end.bytes > goal_byte && point_gt(entry_end.extent, goal_point);
            let visible_child_count = subtree_visible_child_count(*entry.subtree);
            if at_goal {
                if child.visible {
                    array_push(&mut cursor.stack, entry);
                    return i64::from(visible_child_index);
                }
                if visible_child_count > 0 {
                    array_push(&mut cursor.stack, entry);
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
unsafe fn tree_cursor_goto_sibling_internal(
    cursor: &mut TreeCursor,
    advance: unsafe fn(&mut CursorChildIterator) -> Option<CursorChild>,
) -> TreeCursorStep {
    let initial_size = cursor.stack.size;

    while cursor.stack.size > 1 {
        let entry = array_pop(&mut cursor.stack);
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
                array_push(&mut cursor.stack, entry);
                return TreeCursorStep::Visible;
            }

            if subtree_visible_child_count(*entry.subtree) > 0 {
                array_push(&mut cursor.stack, entry);
                return TreeCursorStep::Hidden;
            }
        }
    }

    cursor.stack.size = initial_size;
    TreeCursorStep::None
}

// ---------------------------------------------------------------------------
// Lifecycle: ts_tree_cursor_new, reset, init, delete
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_new(node: TSNode) -> TSTreeCursor {
    let mut self_ = TSTreeCursor {
        tree: ptr::null(),
        id: ptr::null(),
        context: [0, 0, 0],
    };
    tree_cursor_init_ref(cursor_mut(&mut self_), node);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_reset(self_: *mut TSTreeCursor, node: TSNode) {
    tree_cursor_init_ref(cursor_mut(self_), node);
}

pub unsafe fn tree_cursor_init_ref(cursor: &mut TreeCursor, node: TSNode) {
    cursor.tree = node.tree.cast::<TSTree>();
    cursor.root_alias_symbol = node.context[3] as TSSymbol;
    array_clear(&mut cursor.stack);
    array_push(
        &mut cursor.stack,
        TreeCursorEntry {
            subtree: node.id.cast::<Subtree>(),
            position: Length {
                bytes: ts_node_start_byte(node),
                extent: ts_node_start_point(node),
            },
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_delete(self_: *mut TSTreeCursor) {
    let cursor = cursor_mut(self_);
    array_delete(&mut cursor.stack);
}

// ---------------------------------------------------------------------------
// Navigation: children
// ---------------------------------------------------------------------------

pub unsafe fn tree_cursor_goto_first_child_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    let mut iterator = tree_cursor_iterate_children(cursor);
    while let Some(child) = tree_cursor_child_iterator_next(&mut iterator) {
        let entry = child.entry;
        if child.visible {
            array_push(&mut cursor.stack, entry);
            return TreeCursorStep::Visible;
        }
        if subtree_visible_child_count(*entry.subtree) > 0 {
            array_push(&mut cursor.stack, entry);
            return TreeCursorStep::Hidden;
        }
    }
    TreeCursorStep::None
}

unsafe fn tree_cursor_goto_first_child(cursor: &mut TreeCursor) -> bool {
    loop {
        match tree_cursor_goto_first_child_internal(cursor) {
            TreeCursorStep::Hidden => {}
            TreeCursorStep::Visible => return true,
            _ => return false,
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_internal(
    self_: *mut TSTreeCursor,
) -> TreeCursorStep {
    tree_cursor_goto_first_child_internal(cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child(self_: *mut TSTreeCursor) -> bool {
    tree_cursor_goto_first_child(cursor_mut(self_))
}

unsafe fn tree_cursor_goto_last_child_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    let mut iterator = tree_cursor_iterate_children(cursor);
    if iterator.parent.ptr.is_null() || (*iterator.parent.ptr).child_count == 0 {
        return TreeCursorStep::None;
    }

    let mut last_entry = TreeCursorEntry::empty();
    let mut last_step = TreeCursorStep::None;
    while let Some(child) = tree_cursor_child_iterator_next(&mut iterator) {
        let entry = child.entry;
        if child.visible {
            last_entry = entry;
            last_step = TreeCursorStep::Visible;
        } else if subtree_visible_child_count(*entry.subtree) > 0 {
            last_entry = entry;
            last_step = TreeCursorStep::Hidden;
        }
    }
    if !last_entry.subtree.is_null() {
        array_push(&mut cursor.stack, last_entry);
        return last_step;
    }

    TreeCursorStep::None
}

unsafe fn tree_cursor_goto_last_child(cursor: &mut TreeCursor) -> bool {
    loop {
        match tree_cursor_goto_last_child_internal(cursor) {
            TreeCursorStep::Hidden => {}
            TreeCursorStep::Visible => return true,
            _ => return false,
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_last_child(self_: *mut TSTreeCursor) -> bool {
    tree_cursor_goto_last_child(cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_byte(
    self_: *mut TSTreeCursor,
    goal_byte: u32,
) -> i64 {
    let cursor = cursor_mut(self_);
    tree_cursor_goto_first_child_for_byte_and_point(cursor, goal_byte, POINT_ZERO)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_point(
    self_: *mut TSTreeCursor,
    goal_point: TSPoint,
) -> i64 {
    let cursor = cursor_mut(self_);
    tree_cursor_goto_first_child_for_byte_and_point(cursor, 0, goal_point)
}

// ---------------------------------------------------------------------------
// Navigation: siblings, parent, descendant
// ---------------------------------------------------------------------------

pub unsafe fn tree_cursor_goto_next_sibling_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    tree_cursor_goto_sibling_internal(cursor, tree_cursor_child_iterator_next)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling_internal(
    self_: *mut TSTreeCursor,
) -> TreeCursorStep {
    tree_cursor_goto_next_sibling_internal(cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling(self_: *mut TSTreeCursor) -> bool {
    let cursor = cursor_mut(self_);
    match tree_cursor_goto_next_sibling_internal(cursor) {
        TreeCursorStep::Hidden => {
            tree_cursor_goto_first_child(cursor);
            true
        }
        TreeCursorStep::Visible => true,
        _ => false,
    }
}

unsafe fn tree_cursor_goto_previous_sibling_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    let step = tree_cursor_goto_sibling_internal(cursor, tree_cursor_child_iterator_previous);
    if step == TreeCursorStep::None {
        return step;
    }

    // if length is already valid, there's no need to recompute it
    let entries = tree_cursor_entry_slice(&cursor.stack);
    let last_entry = entries.last().unwrap_unchecked();
    if !length_is_undefined(last_entry.position) {
        return step;
    }

    // restore position from the parent node
    let parent = entries.get_unchecked(cursor.stack.size as usize - 2);
    let mut position = parent.position;
    let child_index = last_entry.child_index;
    let children = subtree_children_slice(*parent.subtree);

    if child_index > 0 {
        // skip first child padding since its position should match the position of the parent
        position = length_add(position, subtree_size(*children.get_unchecked(0)));
        for i in 1..child_index {
            position = length_add(
                position,
                subtree_total_size(*children.get_unchecked(i as usize)),
            );
        }
        position = length_add(
            position,
            subtree_padding(*children.get_unchecked(child_index as usize)),
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

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_previous_sibling(self_: *mut TSTreeCursor) -> bool {
    let cursor = cursor_mut(self_);
    match tree_cursor_goto_previous_sibling_internal(cursor) {
        TreeCursorStep::Hidden => {
            tree_cursor_goto_last_child(cursor);
            true
        }
        TreeCursorStep::Visible => true,
        _ => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_parent(self_: *mut TSTreeCursor) -> bool {
    let cursor = cursor_mut(self_);
    let mut i = cursor.stack.size as i32 - 2;
    while i + 1 > 0 {
        if tree_cursor_is_entry_visible(cursor, i as u32) {
            cursor.stack.size = i as u32 + 1;
            return true;
        }
        i -= 1;
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_descendant(
    self_: *mut TSTreeCursor,
    goal_descendant_index: u32,
) {
    let cursor = cursor_mut(self_);

    // Ascend to the lowest ancestor that contains the goal node.
    loop {
        let i = cursor.stack.size - 1;
        let entry = tree_cursor_entry_slice(&cursor.stack).get_unchecked(i as usize);
        let next_descendant_index = entry.descendant_index
            + u32::from(tree_cursor_is_entry_visible(cursor, i))
            + subtree_visible_descendant_count(*entry.subtree);
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
                array_push(&mut cursor.stack, entry);
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

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_descendant_index(
    self_: *const TSTreeCursor,
) -> u32 {
    let cursor = cursor_ref(self_);
    let last_entry = tree_cursor_entry_slice(&cursor.stack)
        .last()
        .unwrap_unchecked();
    last_entry.descendant_index
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_depth(self_: *const TSTreeCursor) -> u32 {
    let cursor = cursor_ref(self_);
    let mut depth: u32 = 0;
    for i in 1..cursor.stack.size {
        if tree_cursor_is_entry_visible(cursor, i) {
            depth += 1;
        }
    }
    depth
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_parent_node(self_: *const TSTreeCursor) -> TSNode {
    let cursor = cursor_ref(self_);
    let entries = tree_cursor_entry_slice(&cursor.stack);
    let mut i = cursor.stack.size as i32 - 2;
    while i >= 0 {
        let entry = entries.get_unchecked(i as usize);
        let mut is_visible = true;
        let mut alias_symbol: TSSymbol = 0;
        if i > 0 {
            let parent_entry = entries.get_unchecked(i as usize - 1);
            alias_symbol = language_alias_at(
                (*cursor.tree).language,
                u32::from((*(*parent_entry.subtree).ptr).data.children.production_id),
                entry.structural_child_index,
            );
            is_visible = alias_symbol != 0 || subtree_visible(*entry.subtree);
        }
        if is_visible {
            return node_new(cursor.tree, entry.subtree, entry.position, alias_symbol);
        }
        i -= 1;
    }
    node_new(ptr::null(), ptr::null(), length_zero(), 0)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_copy(cursor_ptr: *const TSTreeCursor) -> TSTreeCursor {
    let cursor = cursor_ref(cursor_ptr);
    let mut res = TSTreeCursor {
        tree: ptr::null(),
        id: ptr::null(),
        context: [0, 0, 0],
    };
    let copy = cursor_mut(&mut res);
    copy.tree = cursor.tree;
    copy.root_alias_symbol = cursor.root_alias_symbol;
    array_init(&mut copy.stack);
    array_assign(&mut copy.stack, &cursor.stack);
    res
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_reset_to(dst: *mut TSTreeCursor, src: *const TSTreeCursor) {
    let cursor = cursor_ref(src);
    let copy = cursor_mut(dst);
    copy.tree = cursor.tree;
    copy.root_alias_symbol = cursor.root_alias_symbol;
    array_clear(&mut copy.stack);
    array_assign(&mut copy.stack, &cursor.stack);
}
