#![allow(dead_code)]
#![allow(non_snake_case)]

use core::ffi::c_void;
use std::ptr;

use crate::ffi::{TSFieldId, TSNode, TSPoint, TSSymbol, TSTreeCursor};

use super::alloc::{ts_calloc, ts_free, ts_realloc};
use super::length::{
    length_add, length_is_undefined, length_zero, Length, LENGTH_UNDEFINED,
};
use super::point::point_gt;
use super::subtree::{
    ts_subtree_children, ts_subtree_child_count, ts_subtree_extra,
    ts_subtree_padding, ts_subtree_size, ts_subtree_symbol,
    ts_subtree_total_size, ts_subtree_visible, ts_subtree_visible_child_count,
    ts_subtree_visible_descendant_count, Subtree, TSFieldMapEntry, NULL_SUBTREE,
};
use super::language::{
    ts_language_alias_at, ts_language_alias_sequence, ts_language_field_map,
    ts_language_symbol_metadata, TSLanguageFull,
};
use super::node::{ts_node_new, ts_node_start_byte, ts_node_start_point};
use super::tree::TSTree;

use crate::ffi::TSPoint as POINT_ZERO_TYPE;
const POINT_ZERO: POINT_ZERO_TYPE = POINT_ZERO_TYPE { row: 0, column: 0 };

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// `TreeCursorEntry` — mirrors `tree_cursor.h`
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TreeCursorEntry {
    pub subtree: *const Subtree,
    pub position: Length,
    pub child_index: u32,
    pub structural_child_index: u32,
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

/// Array(TreeCursorEntry) — inline repr
#[repr(C)]
pub struct TreeCursorEntryArray {
    pub contents: *mut TreeCursorEntry,
    pub size: u32,
    pub capacity: u32,
}

/// `TreeCursor` — internal cursor (cast to/from `TSTreeCursor`)
#[repr(C)]
pub struct TreeCursor {
    pub tree: *const TSTree,
    pub stack: TreeCursorEntryArray,
    pub root_alias_symbol: TSSymbol,
}

#[inline]
unsafe fn tree_cursor_ref<'a>(cursor: *const TSTreeCursor) -> &'a TreeCursor {
    cursor.cast::<TreeCursor>().as_ref().unwrap_unchecked()
}

#[inline]
unsafe fn tree_cursor_mut<'a>(cursor: *mut TSTreeCursor) -> &'a mut TreeCursor {
    cursor.cast::<TreeCursor>().as_mut().unwrap_unchecked()
}

/// `TreeCursorStep` — result of internal navigation
#[repr(C)]
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TreeCursorStep {
    None = 0,
    Hidden = 1,
    Visible = 2,
}

/// `CursorChildIterator` — internal iterator for children
struct CursorChildIterator {
    parent: Subtree,
    tree: *const TSTree,
    position: Length,
    child_index: u32,
    structural_child_index: u32,
    descendant_index: u32,
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
unsafe fn tree_cursor_entry_array_get(
    arr: &TreeCursorEntryArray,
    index: u32,
) -> &TreeCursorEntry {
    tree_cursor_entry_slice(arr).get_unchecked(index as usize)
}

#[inline]
unsafe fn tree_cursor_entry_array_back(arr: &TreeCursorEntryArray) -> &TreeCursorEntry {
    debug_assert!(arr.size > 0);
    tree_cursor_entry_slice(arr).get_unchecked(arr.size as usize - 1)
}

#[inline]
unsafe fn tree_cursor_entry_array_back_mut(arr: &mut TreeCursorEntryArray) -> &mut TreeCursorEntry {
    debug_assert!(arr.size > 0);
    let index = arr.size as usize - 1;
    tree_cursor_entry_slice_mut(arr).get_unchecked_mut(index)
}

#[inline]
const unsafe fn tree_cursor_entry_slice(arr: &TreeCursorEntryArray) -> &[TreeCursorEntry] {
    std::slice::from_raw_parts(arr.contents, arr.size as usize)
}

#[inline]
unsafe fn tree_cursor_entry_slice_mut(arr: &mut TreeCursorEntryArray) -> &mut [TreeCursorEntry] {
    std::slice::from_raw_parts_mut(arr.contents, arr.size as usize)
}

#[inline]
unsafe fn array_clear(arr: &mut TreeCursorEntryArray) {
    arr.size = 0;
}

unsafe fn array_reserve(arr: &mut TreeCursorEntryArray, new_capacity: u32) {
    if new_capacity > arr.capacity {
        if arr.contents.is_null() {
            arr.contents = ts_calloc(
                new_capacity as usize,
                std::mem::size_of::<TreeCursorEntry>(),
            ).cast::<TreeCursorEntry>();
        } else {
            arr.contents = ts_realloc(
                arr.contents.cast::<c_void>(),
                new_capacity as usize * std::mem::size_of::<TreeCursorEntry>(),
            ).cast::<TreeCursorEntry>();
        }
        arr.capacity = new_capacity;
    }
}

unsafe fn array_grow(arr: &mut TreeCursorEntryArray, count: u32) {
    let new_size = arr.size + count;
    if new_size > arr.capacity {
        let mut new_capacity = if arr.capacity > 0 { arr.capacity } else { 8 };
        while new_capacity < new_size {
            new_capacity *= 2;
        }
        array_reserve(arr, new_capacity);
    }
}

unsafe fn array_push(arr: &mut TreeCursorEntryArray, entry: TreeCursorEntry) {
    array_grow(arr, 1);
    ptr::write(arr.contents.add(arr.size as usize), entry);
    arr.size += 1;
}

unsafe fn array_pop(arr: &mut TreeCursorEntryArray) -> TreeCursorEntry {
    arr.size -= 1;
    ptr::read(arr.contents.add(arr.size as usize))
}

unsafe fn array_delete(arr: &mut TreeCursorEntryArray) {
    if !arr.contents.is_null() {
        ts_free(arr.contents.cast::<c_void>());
    }
    arr.contents = ptr::null_mut();
    arr.size = 0;
    arr.capacity = 0;
}

unsafe fn array_init(arr: &mut TreeCursorEntryArray) {
    arr.contents = ptr::null_mut();
    arr.size = 0;
    arr.capacity = 0;
}

unsafe fn array_push_all(dst: &mut TreeCursorEntryArray, src: &TreeCursorEntryArray) {
    if src.size > 0 {
        array_grow(dst, src.size);
        ptr::copy_nonoverlapping(
            src.contents,
            dst.contents.add(dst.size as usize),
            src.size as usize,
        );
        dst.size += src.size;
    }
}

// ---------------------------------------------------------------------------
// Internal helper functions
// ---------------------------------------------------------------------------

#[inline]
unsafe fn ts_tree_cursor_is_entry_visible(
    self_: &TreeCursor,
    index: u32,
) -> bool {
    let entry = tree_cursor_entry_array_get(&self_.stack, index);
    if index == 0 || ts_subtree_visible(*entry.subtree) {
        return true;
    }
    if !ts_subtree_extra(*entry.subtree) {
        let parent_entry = tree_cursor_entry_array_get(&self_.stack, index - 1);
        return ts_language_alias_at(
            (*self_.tree).language,
            u32::from((*(*parent_entry.subtree).ptr).data.children.production_id),
            entry.structural_child_index,
        ) != 0;
    }
    false
}

#[inline]
unsafe fn ts_tree_cursor_iterate_children(
    self_: &TreeCursor,
) -> CursorChildIterator {
    let last_entry = tree_cursor_entry_array_back(&self_.stack);
    if ts_subtree_child_count(*last_entry.subtree) == 0 {
        return CursorChildIterator {
            parent: NULL_SUBTREE,
            tree: self_.tree,
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
            alias_sequence: ptr::null(),
        };
    }
    let alias_sequence = ts_language_alias_sequence(
        (*self_.tree).language,
        u32::from((*(*last_entry.subtree).ptr).data.children.production_id),
    );

    let mut descendant_index = last_entry.descendant_index;
    if ts_tree_cursor_is_entry_visible(self_, self_.stack.size - 1) {
        descendant_index += 1;
    }

    CursorChildIterator {
        tree: self_.tree,
        parent: *last_entry.subtree,
        position: last_entry.position,
        child_index: 0,
        structural_child_index: 0,
        descendant_index,
        alias_sequence,
    }
}

#[inline]
unsafe fn cursor_child<'a>(parent: Subtree, index: u32) -> &'a Subtree {
    cursor_children(parent).get_unchecked(index as usize)
}

#[inline]
const unsafe fn cursor_children<'a>(parent: Subtree) -> &'a [Subtree] {
    std::slice::from_raw_parts(
        ts_subtree_children(parent),
        ts_subtree_child_count(parent) as usize,
    )
}

unsafe fn ts_tree_cursor_child_iterator_next(
    self_: &mut CursorChildIterator,
) -> Option<CursorChild> {
    if self_.parent.ptr.is_null() || self_.child_index == (*self_.parent.ptr).child_count {
        return None;
    }
    let child = cursor_child(self_.parent, self_.child_index);
    let entry = TreeCursorEntry {
        subtree: child,
        position: self_.position,
        child_index: self_.child_index,
        structural_child_index: self_.structural_child_index,
        descendant_index: self_.descendant_index,
    };
    let mut visible = ts_subtree_visible(*child);
    let extra = ts_subtree_extra(*child);
    if !extra {
        if !self_.alias_sequence.is_null() {
            visible |= *self_.alias_sequence.add(self_.structural_child_index as usize) != 0;
        }
        self_.structural_child_index += 1;
    }

    self_.descendant_index += ts_subtree_visible_descendant_count(*child);
    if visible {
        self_.descendant_index += 1;
    }

    self_.position = length_add(self_.position, ts_subtree_size(*child));
    self_.child_index += 1;

    if self_.child_index < (*self_.parent.ptr).child_count {
        let next_child = *cursor_child(self_.parent, self_.child_index);
        self_.position = length_add(self_.position, ts_subtree_padding(next_child));
    }

    Some(CursorChild { entry, visible })
}

#[inline]
const unsafe fn length_backtrack(a: Length, b: Length) -> Length {
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

unsafe fn ts_tree_cursor_child_iterator_previous(
    self_: &mut CursorChildIterator,
) -> Option<CursorChild> {
    if self_.parent.ptr.is_null() || self_.child_index == u32::MAX {
        return None;
    }
    let child = cursor_child(self_.parent, self_.child_index);
    let entry = TreeCursorEntry {
        subtree: child,
        position: self_.position,
        child_index: self_.child_index,
        structural_child_index: self_.structural_child_index,
        descendant_index: 0, // not used in previous iteration
    };
    let mut visible = ts_subtree_visible(*child);
    let extra = ts_subtree_extra(*child);

    self_.position = length_backtrack(self_.position, ts_subtree_padding(*child));
    self_.child_index = self_.child_index.wrapping_sub(1);

    if !extra && !self_.alias_sequence.is_null() {
        visible |= *self_.alias_sequence.add(self_.structural_child_index as usize) != 0;
        if self_.structural_child_index > 0 {
            self_.structural_child_index -= 1;
        }
    }

    // unsigned can underflow so compare it to child_count
    if self_.child_index < (*self_.parent.ptr).child_count {
        let previous_child = *cursor_child(self_.parent, self_.child_index);
        let size = ts_subtree_size(previous_child);
        self_.position = length_backtrack(self_.position, size);
    }

    Some(CursorChild { entry, visible })
}

#[inline]
unsafe fn ts_tree_cursor_goto_first_child_for_byte_and_point(
    cursor: &mut TreeCursor,
    goal_byte: u32,
    goal_point: TSPoint,
) -> i64 {
    let initial_size = cursor.stack.size;
    let mut visible_child_index: u32 = 0;

    loop {
        let mut did_descend = false;

        let mut iterator = ts_tree_cursor_iterate_children(cursor);
        while let Some(child) = ts_tree_cursor_child_iterator_next(&mut iterator) {
            let entry = child.entry;
            let entry_end = length_add(entry.position, ts_subtree_size(*entry.subtree));
            let at_goal = entry_end.bytes > goal_byte && point_gt(entry_end.extent, goal_point);
            let visible_child_count = ts_subtree_visible_child_count(*entry.subtree);
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

unsafe fn ts_tree_cursor_goto_sibling_internal(
    cursor: &mut TreeCursor,
    advance: unsafe fn(&mut CursorChildIterator) -> Option<CursorChild>,
) -> TreeCursorStep {
    let initial_size = cursor.stack.size;

    while cursor.stack.size > 1 {
        let entry = array_pop(&mut cursor.stack);
        let mut iterator = ts_tree_cursor_iterate_children(cursor);
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

            if ts_subtree_visible_child_count(*entry.subtree) > 0 {
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
    ts_tree_cursor_init_ref(tree_cursor_mut(&mut self_), node);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_reset(self_: *mut TSTreeCursor, node: TSNode) {
    ts_tree_cursor_init_ref(tree_cursor_mut(self_), node);
}

pub unsafe fn ts_tree_cursor_init_ref(cursor: &mut TreeCursor, node: TSNode) {
    cursor.tree = node.tree.cast::<TSTree>();
    cursor.root_alias_symbol = node.context[3] as TSSymbol;
    array_clear(&mut cursor.stack);
    array_push(&mut cursor.stack, TreeCursorEntry {
        subtree: node.id.cast::<Subtree>(),
        position: Length {
            bytes: ts_node_start_byte(node),
            extent: ts_node_start_point(node),
        },
        child_index: 0,
        structural_child_index: 0,
        descendant_index: 0,
    });
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_delete(self_: *mut TSTreeCursor) {
    let cursor = tree_cursor_mut(self_);
    array_delete(&mut cursor.stack);
}

// ---------------------------------------------------------------------------
// Navigation: children
// ---------------------------------------------------------------------------

unsafe fn tree_cursor_goto_first_child_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    let mut iterator = ts_tree_cursor_iterate_children(cursor);
    while let Some(child) = ts_tree_cursor_child_iterator_next(&mut iterator) {
        let entry = child.entry;
        if child.visible {
            array_push(&mut cursor.stack, entry);
            return TreeCursorStep::Visible;
        }
        if ts_subtree_visible_child_count(*entry.subtree) > 0 {
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
    tree_cursor_goto_first_child_internal(tree_cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child(
    self_: *mut TSTreeCursor,
) -> bool {
    tree_cursor_goto_first_child(tree_cursor_mut(self_))
}

unsafe fn tree_cursor_goto_last_child_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    let mut iterator = ts_tree_cursor_iterate_children(cursor);
    if iterator.parent.ptr.is_null() || (*iterator.parent.ptr).child_count == 0 {
        return TreeCursorStep::None;
    }

    let mut last_entry = TreeCursorEntry::empty();
    let mut last_step = TreeCursorStep::None;
    while let Some(child) = ts_tree_cursor_child_iterator_next(&mut iterator) {
        let entry = child.entry;
        if child.visible {
            last_entry = entry;
            last_step = TreeCursorStep::Visible;
        } else if ts_subtree_visible_child_count(*entry.subtree) > 0 {
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
pub unsafe extern "C" fn ts_tree_cursor_goto_last_child(
    self_: *mut TSTreeCursor,
) -> bool {
    tree_cursor_goto_last_child(tree_cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_byte(
    self_: *mut TSTreeCursor,
    goal_byte: u32,
) -> i64 {
    let cursor = tree_cursor_mut(self_);
    ts_tree_cursor_goto_first_child_for_byte_and_point(cursor, goal_byte, POINT_ZERO)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_point(
    self_: *mut TSTreeCursor,
    goal_point: TSPoint,
) -> i64 {
    let cursor = tree_cursor_mut(self_);
    ts_tree_cursor_goto_first_child_for_byte_and_point(cursor, 0, goal_point)
}

// ---------------------------------------------------------------------------
// Navigation: siblings, parent, descendant
// ---------------------------------------------------------------------------

unsafe fn tree_cursor_goto_next_sibling_internal(cursor: &mut TreeCursor) -> TreeCursorStep {
    ts_tree_cursor_goto_sibling_internal(cursor, ts_tree_cursor_child_iterator_next)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling_internal(
    self_: *mut TSTreeCursor,
) -> TreeCursorStep {
    tree_cursor_goto_next_sibling_internal(tree_cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling(
    self_: *mut TSTreeCursor,
) -> bool {
    let cursor = tree_cursor_mut(self_);
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
    let step = ts_tree_cursor_goto_sibling_internal(
        cursor,
        ts_tree_cursor_child_iterator_previous,
    );
    if step == TreeCursorStep::None {
        return step;
    }

    // if length is already valid, there's no need to recompute it
    if !length_is_undefined(tree_cursor_entry_array_back(&cursor.stack).position) {
        return step;
    }

    // restore position from the parent node
    let parent = tree_cursor_entry_array_get(&cursor.stack, cursor.stack.size - 2);
    let mut position = parent.position;
    let child_index = tree_cursor_entry_array_back(&cursor.stack).child_index;
    let children = cursor_children(*parent.subtree);

    if child_index > 0 {
        // skip first child padding since its position should match the position of the parent
        position = length_add(position, ts_subtree_size(*children.get_unchecked(0)));
        for i in 1..child_index {
            position = length_add(
                position,
                ts_subtree_total_size(*children.get_unchecked(i as usize)),
            );
        }
        position = length_add(
            position,
            ts_subtree_padding(*children.get_unchecked(child_index as usize)),
        );
    }

    tree_cursor_entry_array_back_mut(&mut cursor.stack).position = position;

    step
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_previous_sibling(
    self_: *mut TSTreeCursor,
) -> bool {
    let cursor = tree_cursor_mut(self_);
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
pub unsafe extern "C" fn ts_tree_cursor_goto_parent(
    self_: *mut TSTreeCursor,
) -> bool {
    let cursor = tree_cursor_mut(self_);
    let mut i = cursor.stack.size as i32 - 2;
    while i + 1 > 0 {
        if ts_tree_cursor_is_entry_visible(cursor, i as u32) {
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
    let cursor = tree_cursor_mut(self_);

    // Ascend to the lowest ancestor that contains the goal node.
    loop {
        let i = cursor.stack.size - 1;
        let entry = tree_cursor_entry_array_get(&cursor.stack, i);
        let next_descendant_index =
            entry.descendant_index
            + u32::from(ts_tree_cursor_is_entry_visible(cursor, i))
            + ts_subtree_visible_descendant_count(*entry.subtree);
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
        let mut iterator = ts_tree_cursor_iterate_children(cursor);
        if iterator.descendant_index > goal_descendant_index {
            return;
        }

        while let Some(child) = ts_tree_cursor_child_iterator_next(&mut iterator) {
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
    let cursor = tree_cursor_ref(self_);
    let last_entry = tree_cursor_entry_array_back(&cursor.stack);
    last_entry.descendant_index
}

// ---------------------------------------------------------------------------
// Node info & copy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_node(
    self_: *const TSTreeCursor,
) -> TSNode {
    let cursor = tree_cursor_ref(self_);
    let last_entry = tree_cursor_entry_array_back(&cursor.stack);
    let is_extra = ts_subtree_extra(*last_entry.subtree);
    let alias_symbol = if is_extra {
        0
    } else if cursor.stack.size > 1 {
        let parent_entry = tree_cursor_entry_array_get(&cursor.stack, cursor.stack.size - 2);
        ts_language_alias_at(
            (*cursor.tree).language,
            u32::from((*(*parent_entry.subtree).ptr).data.children.production_id),
            last_entry.structural_child_index,
        )
    } else {
        cursor.root_alias_symbol
    };
    ts_node_new(
        cursor.tree,
        last_entry.subtree,
        last_entry.position,
        alias_symbol,
    )
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_status(
    self_: *const TSTreeCursor,
    field_id: *mut TSFieldId,
    has_later_siblings: *mut bool,
    has_later_named_siblings: *mut bool,
    can_have_later_siblings_with_this_field: *mut bool,
    supertypes: *mut TSSymbol,
    supertype_count: *mut u32,
) {
    let cursor = tree_cursor_ref(self_);
    let language = (*cursor.tree).language;
    let field_id = field_id.as_mut().unwrap_unchecked();
    let has_later_siblings = has_later_siblings.as_mut().unwrap_unchecked();
    let has_later_named_siblings = has_later_named_siblings.as_mut().unwrap_unchecked();
    let can_have_later_siblings_with_this_field =
        can_have_later_siblings_with_this_field.as_mut().unwrap_unchecked();
    let supertype_count = supertype_count.as_mut().unwrap_unchecked();
    let max_supertypes = *supertype_count;
    *field_id = 0;
    *supertype_count = 0;
    *has_later_siblings = false;
    *has_later_named_siblings = false;
    *can_have_later_siblings_with_this_field = false;

    // Walk up the tree, visiting the current node and its invisible ancestors
    let mut i = cursor.stack.size - 1;
    while i > 0 {
        let entry = tree_cursor_entry_array_get(&cursor.stack, i);
        let parent_entry = tree_cursor_entry_array_get(&cursor.stack, i - 1);

        let alias_sequence = ts_language_alias_sequence(
            language,
            u32::from((*(*parent_entry.subtree).ptr).data.children.production_id),
        );

        // Inline subtree_symbol macro
        let subtree_symbol_fn = |subtree: Subtree, structural_child_index: u32| -> TSSymbol {
            if !ts_subtree_extra(subtree)
                && !alias_sequence.is_null()
                && *alias_sequence.add(structural_child_index as usize) != 0
            {
                *alias_sequence.add(structural_child_index as usize)
            } else {
                ts_subtree_symbol(subtree)
            }
        };

        // Stop walking up when a visible ancestor is found.
        let entry_symbol = subtree_symbol_fn(*entry.subtree, entry.structural_child_index);
        let entry_metadata = ts_language_symbol_metadata(
            language,
            entry_symbol,
        );
        if i != cursor.stack.size - 1 && entry_metadata.visible {
            break;
        }

        // Record any supertypes
        if entry_metadata.supertype && *supertype_count < max_supertypes {
            *supertypes.add(*supertype_count as usize) = entry_symbol;
            *supertype_count += 1;
        }

        // Determine if the current node has later siblings.
        if !*has_later_siblings {
            let sibling_count = (*(*parent_entry.subtree).ptr).child_count;
            let mut structural_child_index = entry.structural_child_index;
            if !ts_subtree_extra(*entry.subtree) {
                structural_child_index += 1;
            }
            let mut j = entry.child_index + 1;
            while j < sibling_count {
                let sibling = *cursor_child(*parent_entry.subtree, j);
                let sibling_metadata = ts_language_symbol_metadata(
                    language,
                    subtree_symbol_fn(sibling, structural_child_index),
                );
                if sibling_metadata.visible {
                    *has_later_siblings = true;
                    if *has_later_named_siblings {
                        break;
                    }
                    if sibling_metadata.named {
                        *has_later_named_siblings = true;
                        break;
                    }
                } else if ts_subtree_visible_child_count(sibling) > 0 {
                    *has_later_siblings = true;
                    if *has_later_named_siblings {
                        break;
                    }
                    if (*sibling.ptr).data.children.named_child_count > 0 {
                        *has_later_named_siblings = true;
                        break;
                    }
                }
                if !ts_subtree_extra(sibling) {
                    structural_child_index += 1;
                }
                j += 1;
            }
        }

        if !ts_subtree_extra(*entry.subtree) {
            let mut field_map: *const TSFieldMapEntry = ptr::null();
            let mut field_map_end: *const TSFieldMapEntry = ptr::null();
            ts_language_field_map(
                language,
                u32::from((*(*parent_entry.subtree).ptr).data.children.production_id),
                &mut field_map,
                &mut field_map_end,
            );

            // Look for a field name associated with the current node.
            if *field_id == 0 {
                let mut map = field_map;
                while map < field_map_end {
                    if !(*map).inherited && (*map).child_index == entry.structural_child_index as u8 {
                        *field_id = (*map).field_id;
                        break;
                    }
                    map = map.add(1);
                }
            }

            // Determine if the current node can have later siblings with the same field name.
            if *field_id != 0 {
                let mut map = field_map;
                while map < field_map_end {
                    if (*map).field_id == *field_id
                        && u32::from((*map).child_index) > entry.structural_child_index
                    {
                        *can_have_later_siblings_with_this_field = true;
                        break;
                    }
                    map = map.add(1);
                }
            }
        }

        i -= 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_depth(
    self_: *const TSTreeCursor,
) -> u32 {
    let cursor = tree_cursor_ref(self_);
    let mut depth: u32 = 0;
    for i in 1..cursor.stack.size {
        if ts_tree_cursor_is_entry_visible(cursor, i) {
            depth += 1;
        }
    }
    depth
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_parent_node(
    self_: *const TSTreeCursor,
) -> TSNode {
    let cursor = tree_cursor_ref(self_);
    let mut i = cursor.stack.size as i32 - 2;
    while i >= 0 {
        let entry = tree_cursor_entry_array_get(&cursor.stack, i as u32);
        let mut is_visible = true;
        let mut alias_symbol: TSSymbol = 0;
        if i > 0 {
            let parent_entry = tree_cursor_entry_array_get(&cursor.stack, i as u32 - 1);
            alias_symbol = ts_language_alias_at(
                (*cursor.tree).language,
                u32::from((*(*parent_entry.subtree).ptr).data.children.production_id),
                entry.structural_child_index,
            );
            is_visible = alias_symbol != 0 || ts_subtree_visible(*entry.subtree);
        }
        if is_visible {
            return ts_node_new(
                cursor.tree,
                entry.subtree,
                entry.position,
                alias_symbol,
            );
        }
        i -= 1;
    }
    ts_node_new(ptr::null(), ptr::null(), length_zero(), 0)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_field_id(
    self_: *const TSTreeCursor,
) -> TSFieldId {
    let cursor = tree_cursor_ref(self_);

    // Walk up the tree, visiting the current node and its invisible ancestors.
    let mut i = cursor.stack.size - 1;
    while i > 0 {
        let entry = tree_cursor_entry_array_get(&cursor.stack, i);
        let parent_entry = tree_cursor_entry_array_get(&cursor.stack, i - 1);

        // Stop walking up when another visible node is found.
        if i != cursor.stack.size - 1
            && ts_tree_cursor_is_entry_visible(cursor, i)
        {
            break;
        }

        if ts_subtree_extra(*entry.subtree) {
            break;
        }

        let mut field_map: *const TSFieldMapEntry = ptr::null();
        let mut field_map_end: *const TSFieldMapEntry = ptr::null();
        ts_language_field_map(
            (*cursor.tree).language,
            u32::from((*(*parent_entry.subtree).ptr).data.children.production_id),
            &mut field_map,
            &mut field_map_end,
        );
        let mut map = field_map;
        while map < field_map_end {
            if !(*map).inherited && (*map).child_index == entry.structural_child_index as u8 {
                return (*map).field_id;
            }
            map = map.add(1);
        }

        i -= 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_field_name(
    self_: *const TSTreeCursor,
) -> *const i8 {
    let id = ts_tree_cursor_current_field_id(self_);
    if id != 0 {
        let cursor = tree_cursor_ref(self_);
        let lang = (*cursor.tree).language.cast::<TSLanguageFull>();
        return *(*lang).field_names.add(id as usize);
    }
    ptr::null()
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_copy(
    cursor_ptr: *const TSTreeCursor,
) -> TSTreeCursor {
    let cursor = tree_cursor_ref(cursor_ptr);
    let mut res = TSTreeCursor {
        tree: ptr::null(),
        id: ptr::null(),
        context: [0, 0, 0],
    };
    let copy = tree_cursor_mut(&mut res);
    copy.tree = cursor.tree;
    copy.root_alias_symbol = cursor.root_alias_symbol;
    array_init(&mut copy.stack);
    array_push_all(&mut copy.stack, &cursor.stack);
    res
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_reset_to(
    dst: *mut TSTreeCursor,
    src: *const TSTreeCursor,
) {
    let cursor = tree_cursor_ref(src);
    let copy = tree_cursor_mut(dst);
    copy.tree = cursor.tree;
    copy.root_alias_symbol = cursor.root_alias_symbol;
    array_clear(&mut copy.stack);
    array_push_all(&mut copy.stack, &cursor.stack);
}
