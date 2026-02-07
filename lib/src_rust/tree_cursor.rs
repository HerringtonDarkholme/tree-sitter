#![allow(dead_code)]
#![allow(non_snake_case)]

use core::ffi::c_void;
use std::ptr;

use crate::ffi::{
    TSFieldId, TSLanguage, TSNode, TSPoint, TSSymbol, TSTreeCursor,
};

use super::alloc::{ts_calloc, ts_free, ts_realloc};
use super::length::{
    length_add, length_is_undefined, length_zero, Length, LENGTH_UNDEFINED,
};
use super::point::point_gt;
use super::subtree::{
    ts_subtree_children, ts_subtree_child_count, ts_subtree_extra,
    ts_subtree_padding, ts_subtree_size, ts_subtree_symbol,
    ts_subtree_total_size, ts_subtree_visible, ts_subtree_visible_child_count,
    ts_subtree_visible_descendant_count, Subtree,
    TSFieldMapEntry, TSSymbolMetadata, NULL_SUBTREE,
};
use super::language::{
    ts_language_alias_at, ts_language_alias_sequence, ts_language_field_map,
    TSLanguageFull,
};
use super::tree::TSTree;

use crate::ffi::TSPoint as POINT_ZERO_TYPE;
const POINT_ZERO: POINT_ZERO_TYPE = POINT_ZERO_TYPE { row: 0, column: 0 };

// ---------------------------------------------------------------------------
// Extern C functions (still in C)
// ---------------------------------------------------------------------------

extern "C" {
    // node.c (still in C)
    fn ts_node_new(
        tree: *const TSTree,
        subtree: *const Subtree,
        position: Length,
        alias: TSSymbol,
    ) -> TSNode;
    fn ts_node_start_byte(self_: TSNode) -> u32;
    fn ts_node_start_point(self_: TSNode) -> TSPoint;

    // language.rs (exported)
    fn ts_language_symbol_metadata(
        self_: *const TSLanguage,
        symbol: TSSymbol,
    ) -> TSSymbolMetadata;

    fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// TreeCursorEntry — mirrors tree_cursor.h
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TreeCursorEntry {
    pub subtree: *const Subtree,
    pub position: Length,
    pub child_index: u32,
    pub structural_child_index: u32,
    pub descendant_index: u32,
}

/// Array(TreeCursorEntry) — inline repr
#[repr(C)]
pub struct TreeCursorEntryArray {
    pub contents: *mut TreeCursorEntry,
    pub size: u32,
    pub capacity: u32,
}

/// TreeCursor — internal cursor (cast to/from TSTreeCursor)
#[repr(C)]
pub struct TreeCursor {
    pub tree: *const TSTree,
    pub stack: TreeCursorEntryArray,
    pub root_alias_symbol: TSSymbol,
}

/// TreeCursorStep — result of internal navigation
#[repr(C)]
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TreeCursorStep {
    TreeCursorStepNone = 0,
    TreeCursorStepHidden = 1,
    TreeCursorStepVisible = 2,
}

/// CursorChildIterator — internal iterator for children
struct CursorChildIterator {
    parent: Subtree,
    tree: *const TSTree,
    position: Length,
    child_index: u32,
    structural_child_index: u32,
    descendant_index: u32,
    alias_sequence: *const TSSymbol,
}

// ---------------------------------------------------------------------------
// Array helpers for TreeCursorEntry stack
// ---------------------------------------------------------------------------

#[inline]
unsafe fn array_get(arr: &TreeCursorEntryArray, index: u32) -> *mut TreeCursorEntry {
    arr.contents.add(index as usize)
}

#[inline]
unsafe fn array_back(arr: &TreeCursorEntryArray) -> *mut TreeCursorEntry {
    arr.contents.add(arr.size as usize - 1)
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
            ) as *mut TreeCursorEntry;
        } else {
            arr.contents = ts_realloc(
                arr.contents as *mut c_void,
                new_capacity as usize * std::mem::size_of::<TreeCursorEntry>(),
            ) as *mut TreeCursorEntry;
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
        ts_free(arr.contents as *mut c_void);
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
        memcpy(
            dst.contents.add(dst.size as usize) as *mut c_void,
            src.contents as *const c_void,
            src.size as usize * std::mem::size_of::<TreeCursorEntry>(),
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
    let entry = &*array_get(&self_.stack, index);
    if index == 0 || ts_subtree_visible(*entry.subtree) {
        return true;
    } else if !ts_subtree_extra(*entry.subtree) {
        let parent_entry = &*array_get(&self_.stack, index - 1);
        return ts_language_alias_at(
            (*self_.tree).language,
            (*(*parent_entry.subtree).ptr).data.children.production_id as u32,
            entry.structural_child_index,
        ) != 0;
    } else {
        return false;
    }
}

#[inline]
unsafe fn ts_tree_cursor_iterate_children(
    self_: &TreeCursor,
) -> CursorChildIterator {
    let last_entry = &*array_back(&self_.stack);
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
        (*(*last_entry.subtree).ptr).data.children.production_id as u32,
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

unsafe fn ts_tree_cursor_child_iterator_next(
    self_: &mut CursorChildIterator,
    result: *mut TreeCursorEntry,
    visible: *mut bool,
) -> bool {
    if self_.parent.ptr.is_null() || self_.child_index == (*self_.parent.ptr).child_count {
        return false;
    }
    let child = &*ts_subtree_children(self_.parent).add(self_.child_index as usize);
    *result = TreeCursorEntry {
        subtree: child,
        position: self_.position,
        child_index: self_.child_index,
        structural_child_index: self_.structural_child_index,
        descendant_index: self_.descendant_index,
    };
    *visible = ts_subtree_visible(*child);
    let extra = ts_subtree_extra(*child);
    if !extra {
        if !self_.alias_sequence.is_null() {
            *visible |= *self_.alias_sequence.add(self_.structural_child_index as usize) != 0;
        }
        self_.structural_child_index += 1;
    }

    self_.descendant_index += ts_subtree_visible_descendant_count(*child);
    if *visible {
        self_.descendant_index += 1;
    }

    self_.position = length_add(self_.position, ts_subtree_size(*child));
    self_.child_index += 1;

    if self_.child_index < (*self_.parent.ptr).child_count {
        let next_child = *ts_subtree_children(self_.parent).add(self_.child_index as usize);
        self_.position = length_add(self_.position, ts_subtree_padding(next_child));
    }

    true
}

#[inline]
unsafe fn length_backtrack(a: Length, b: Length) -> Length {
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
    result: *mut TreeCursorEntry,
    visible: *mut bool,
) -> bool {
    if self_.parent.ptr.is_null() || self_.child_index as i8 == -1 {
        return false;
    }
    let child = &*ts_subtree_children(self_.parent).add(self_.child_index as usize);
    *result = TreeCursorEntry {
        subtree: child,
        position: self_.position,
        child_index: self_.child_index,
        structural_child_index: self_.structural_child_index,
        descendant_index: 0, // not used in previous iteration
    };
    *visible = ts_subtree_visible(*child);
    let extra = ts_subtree_extra(*child);

    self_.position = length_backtrack(self_.position, ts_subtree_padding(*child));
    self_.child_index = self_.child_index.wrapping_sub(1);

    if !extra && !self_.alias_sequence.is_null() {
        *visible |= *self_.alias_sequence.add(self_.structural_child_index as usize) != 0;
        if self_.structural_child_index > 0 {
            self_.structural_child_index -= 1;
        }
    }

    // unsigned can underflow so compare it to child_count
    if self_.child_index < (*self_.parent.ptr).child_count {
        let previous_child = *ts_subtree_children(self_.parent).add(self_.child_index as usize);
        let size = ts_subtree_size(previous_child);
        self_.position = length_backtrack(self_.position, size);
    }

    true
}

#[inline]
unsafe fn ts_tree_cursor_goto_first_child_for_byte_and_point(
    _self: *mut TSTreeCursor,
    goal_byte: u32,
    goal_point: TSPoint,
) -> i64 {
    let self_ = _self as *mut TreeCursor;
    let initial_size = (*self_).stack.size;
    let mut visible_child_index: u32 = 0;

    let mut did_descend;
    loop {
        did_descend = false;

        let mut visible = false;
        let mut entry = std::mem::zeroed::<TreeCursorEntry>();
        let mut iterator = ts_tree_cursor_iterate_children(&*self_);
        while ts_tree_cursor_child_iterator_next(&mut iterator, &mut entry, &mut visible) {
            let entry_end = length_add(entry.position, ts_subtree_size(*entry.subtree));
            let at_goal = entry_end.bytes > goal_byte && point_gt(entry_end.extent, goal_point);
            let visible_child_count = ts_subtree_visible_child_count(*entry.subtree);
            if at_goal {
                if visible {
                    array_push(&mut (*self_).stack, entry);
                    return visible_child_index as i64;
                }
                if visible_child_count > 0 {
                    array_push(&mut (*self_).stack, entry);
                    did_descend = true;
                    break;
                }
            } else if visible {
                visible_child_index += 1;
            } else {
                visible_child_index += visible_child_count;
            }
        }
        if !did_descend {
            break;
        }
    }

    (*self_).stack.size = initial_size;
    -1
}

unsafe fn ts_tree_cursor_goto_sibling_internal(
    _self: *mut TSTreeCursor,
    advance: unsafe fn(&mut CursorChildIterator, *mut TreeCursorEntry, *mut bool) -> bool,
) -> TreeCursorStep {
    let self_ = _self as *mut TreeCursor;
    let initial_size = (*self_).stack.size;

    while (*self_).stack.size > 1 {
        let entry = array_pop(&mut (*self_).stack);
        let mut iterator = ts_tree_cursor_iterate_children(&*self_);
        iterator.child_index = entry.child_index;
        iterator.structural_child_index = entry.structural_child_index;
        iterator.position = entry.position;
        iterator.descendant_index = entry.descendant_index;

        let mut visible = false;
        let mut entry = std::mem::zeroed::<TreeCursorEntry>();
        advance(&mut iterator, &mut entry, &mut visible);
        if visible && (*self_).stack.size + 1 < initial_size {
            break;
        }

        while advance(&mut iterator, &mut entry, &mut visible) {
            if visible {
                array_push(&mut (*self_).stack, entry);
                return TreeCursorStep::TreeCursorStepVisible;
            }

            if ts_subtree_visible_child_count(*entry.subtree) > 0 {
                array_push(&mut (*self_).stack, entry);
                return TreeCursorStep::TreeCursorStepHidden;
            }
        }
    }

    (*self_).stack.size = initial_size;
    TreeCursorStep::TreeCursorStepNone
}

// ---------------------------------------------------------------------------
// Inline from header: ts_tree_cursor_current_subtree
// ---------------------------------------------------------------------------

#[inline]
pub unsafe fn ts_tree_cursor_current_subtree(_self: *const TSTreeCursor) -> Subtree {
    let self_ = _self as *const TreeCursor;
    let last_entry = &*array_back(&(*self_).stack);
    *last_entry.subtree
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
    ts_tree_cursor_init(&mut self_ as *mut TSTreeCursor as *mut TreeCursor, node);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_reset(_self: *mut TSTreeCursor, node: TSNode) {
    ts_tree_cursor_init(_self as *mut TreeCursor, node);
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_init(self_: *mut TreeCursor, node: TSNode) {
    (*self_).tree = node.tree as *const TSTree;
    (*self_).root_alias_symbol = node.context[3] as TSSymbol;
    array_clear(&mut (*self_).stack);
    array_push(&mut (*self_).stack, TreeCursorEntry {
        subtree: node.id as *const Subtree,
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
pub unsafe extern "C" fn ts_tree_cursor_delete(_self: *mut TSTreeCursor) {
    let self_ = _self as *mut TreeCursor;
    array_delete(&mut (*self_).stack);
}

// ---------------------------------------------------------------------------
// Navigation: children
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_internal(
    _self: *mut TSTreeCursor,
) -> TreeCursorStep {
    let self_ = _self as *mut TreeCursor;
    let mut visible = false;
    let mut entry = std::mem::zeroed::<TreeCursorEntry>();
    let mut iterator = ts_tree_cursor_iterate_children(&*self_);
    while ts_tree_cursor_child_iterator_next(&mut iterator, &mut entry, &mut visible) {
        if visible {
            array_push(&mut (*self_).stack, entry);
            return TreeCursorStep::TreeCursorStepVisible;
        }
        if ts_subtree_visible_child_count(*entry.subtree) > 0 {
            array_push(&mut (*self_).stack, entry);
            return TreeCursorStep::TreeCursorStepHidden;
        }
    }
    TreeCursorStep::TreeCursorStepNone
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child(
    _self: *mut TSTreeCursor,
) -> bool {
    loop {
        match ts_tree_cursor_goto_first_child_internal(_self) {
            TreeCursorStep::TreeCursorStepHidden => continue,
            TreeCursorStep::TreeCursorStepVisible => return true,
            _ => return false,
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_last_child_internal(
    _self: *mut TSTreeCursor,
) -> TreeCursorStep {
    let self_ = _self as *mut TreeCursor;
    let mut visible = false;
    let mut entry = std::mem::zeroed::<TreeCursorEntry>();
    let mut iterator = ts_tree_cursor_iterate_children(&*self_);
    if iterator.parent.ptr.is_null() || (*iterator.parent.ptr).child_count == 0 {
        return TreeCursorStep::TreeCursorStepNone;
    }

    let mut last_entry = std::mem::zeroed::<TreeCursorEntry>();
    let mut last_step = TreeCursorStep::TreeCursorStepNone;
    while ts_tree_cursor_child_iterator_next(&mut iterator, &mut entry, &mut visible) {
        if visible {
            last_entry = entry;
            last_step = TreeCursorStep::TreeCursorStepVisible;
        } else if ts_subtree_visible_child_count(*entry.subtree) > 0 {
            last_entry = entry;
            last_step = TreeCursorStep::TreeCursorStepHidden;
        }
    }
    if !last_entry.subtree.is_null() {
        array_push(&mut (*self_).stack, last_entry);
        return last_step;
    }

    TreeCursorStep::TreeCursorStepNone
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_last_child(
    _self: *mut TSTreeCursor,
) -> bool {
    loop {
        match ts_tree_cursor_goto_last_child_internal(_self) {
            TreeCursorStep::TreeCursorStepHidden => continue,
            TreeCursorStep::TreeCursorStepVisible => return true,
            _ => return false,
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_byte(
    _self: *mut TSTreeCursor,
    goal_byte: u32,
) -> i64 {
    ts_tree_cursor_goto_first_child_for_byte_and_point(_self, goal_byte, POINT_ZERO)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_point(
    _self: *mut TSTreeCursor,
    goal_point: TSPoint,
) -> i64 {
    ts_tree_cursor_goto_first_child_for_byte_and_point(_self, 0, goal_point)
}

// ---------------------------------------------------------------------------
// Navigation: siblings, parent, descendant
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling_internal(
    _self: *mut TSTreeCursor,
) -> TreeCursorStep {
    ts_tree_cursor_goto_sibling_internal(_self, ts_tree_cursor_child_iterator_next)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling(
    _self: *mut TSTreeCursor,
) -> bool {
    match ts_tree_cursor_goto_next_sibling_internal(_self) {
        TreeCursorStep::TreeCursorStepHidden => {
            ts_tree_cursor_goto_first_child(_self);
            true
        }
        TreeCursorStep::TreeCursorStepVisible => true,
        _ => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_previous_sibling_internal(
    _self: *mut TSTreeCursor,
) -> TreeCursorStep {
    let self_ = _self as *mut TreeCursor;

    let step = ts_tree_cursor_goto_sibling_internal(
        _self, ts_tree_cursor_child_iterator_previous,
    );
    if step == TreeCursorStep::TreeCursorStepNone {
        return step;
    }

    // if length is already valid, there's no need to recompute it
    if !length_is_undefined((*array_back(&(*self_).stack)).position) {
        return step;
    }

    // restore position from the parent node
    let parent = &*array_get(&(*self_).stack, (*self_).stack.size - 2);
    let mut position = parent.position;
    let child_index = (*array_back(&(*self_).stack)).child_index;
    let children = ts_subtree_children(*parent.subtree);

    if child_index > 0 {
        // skip first child padding since its position should match the position of the parent
        position = length_add(position, ts_subtree_size(*children.add(0)));
        for i in 1..child_index {
            position = length_add(position, ts_subtree_total_size(*children.add(i as usize)));
        }
        position = length_add(position, ts_subtree_padding(*children.add(child_index as usize)));
    }

    (*array_back(&(*self_).stack)).position = position;

    step
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_previous_sibling(
    _self: *mut TSTreeCursor,
) -> bool {
    match ts_tree_cursor_goto_previous_sibling_internal(_self) {
        TreeCursorStep::TreeCursorStepHidden => {
            ts_tree_cursor_goto_last_child(_self);
            true
        }
        TreeCursorStep::TreeCursorStepVisible => true,
        _ => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_parent(
    _self: *mut TSTreeCursor,
) -> bool {
    let self_ = _self as *mut TreeCursor;
    let mut i = (*self_).stack.size as i32 - 2;
    while i + 1 > 0 {
        if ts_tree_cursor_is_entry_visible(&*self_, i as u32) {
            (*self_).stack.size = i as u32 + 1;
            return true;
        }
        i -= 1;
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_descendant(
    _self: *mut TSTreeCursor,
    goal_descendant_index: u32,
) {
    let self_ = _self as *mut TreeCursor;

    // Ascend to the lowest ancestor that contains the goal node.
    loop {
        let i = (*self_).stack.size - 1;
        let entry = &*array_get(&(*self_).stack, i);
        let next_descendant_index =
            entry.descendant_index
            + (if ts_tree_cursor_is_entry_visible(&*self_, i) { 1 } else { 0 })
            + ts_subtree_visible_descendant_count(*entry.subtree);
        if entry.descendant_index <= goal_descendant_index
            && next_descendant_index > goal_descendant_index
        {
            break;
        } else if (*self_).stack.size <= 1 {
            return;
        } else {
            (*self_).stack.size -= 1;
        }
    }

    // Descend to the goal node.
    let mut did_descend = true;
    while did_descend {
        did_descend = false;
        let mut visible = false;
        let mut entry = std::mem::zeroed::<TreeCursorEntry>();
        let mut iterator = ts_tree_cursor_iterate_children(&*self_);
        if iterator.descendant_index > goal_descendant_index {
            return;
        }

        while ts_tree_cursor_child_iterator_next(&mut iterator, &mut entry, &mut visible) {
            if iterator.descendant_index > goal_descendant_index {
                array_push(&mut (*self_).stack, entry);
                if visible && entry.descendant_index == goal_descendant_index {
                    return;
                } else {
                    did_descend = true;
                    break;
                }
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_descendant_index(
    _self: *const TSTreeCursor,
) -> u32 {
    let self_ = _self as *const TreeCursor;
    let last_entry = &*array_back(&(*self_).stack);
    last_entry.descendant_index
}

// ---------------------------------------------------------------------------
// Node info & copy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_node(
    _self: *const TSTreeCursor,
) -> TSNode {
    let self_ = _self as *const TreeCursor;
    let last_entry = &*array_back(&(*self_).stack);
    let is_extra = ts_subtree_extra(*last_entry.subtree);
    let mut alias_symbol: TSSymbol = if is_extra { 0 } else { (*self_).root_alias_symbol };
    if (*self_).stack.size > 1 && !is_extra {
        let parent_entry = &*array_get(&(*self_).stack, (*self_).stack.size - 2);
        alias_symbol = ts_language_alias_at(
            (*(*self_).tree).language,
            (*(*parent_entry.subtree).ptr).data.children.production_id as u32,
            last_entry.structural_child_index,
        );
    }
    ts_node_new(
        (*self_).tree,
        last_entry.subtree,
        last_entry.position,
        alias_symbol,
    )
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_status(
    _self: *const TSTreeCursor,
    field_id: *mut TSFieldId,
    has_later_siblings: *mut bool,
    has_later_named_siblings: *mut bool,
    can_have_later_siblings_with_this_field: *mut bool,
    supertypes: *mut TSSymbol,
    supertype_count: *mut u32,
) {
    let self_ = _self as *const TreeCursor;
    let max_supertypes = *supertype_count;
    *field_id = 0;
    *supertype_count = 0;
    *has_later_siblings = false;
    *has_later_named_siblings = false;
    *can_have_later_siblings_with_this_field = false;

    // Walk up the tree, visiting the current node and its invisible ancestors
    let mut i = (*self_).stack.size - 1;
    while i > 0 {
        let entry = &*array_get(&(*self_).stack, i);
        let parent_entry = &*array_get(&(*self_).stack, i - 1);

        let alias_sequence = ts_language_alias_sequence(
            (*(*self_).tree).language,
            (*(*parent_entry.subtree).ptr).data.children.production_id as u32,
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
            (*(*self_).tree).language,
            entry_symbol,
        );
        if i != (*self_).stack.size - 1 && entry_metadata.visible {
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
                let sibling = *ts_subtree_children(*parent_entry.subtree).add(j as usize);
                let sibling_metadata = ts_language_symbol_metadata(
                    (*(*self_).tree).language,
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
                (*(*self_).tree).language,
                (*(*parent_entry.subtree).ptr).data.children.production_id as u32,
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
                        && (*map).child_index as u32 > entry.structural_child_index
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
    _self: *const TSTreeCursor,
) -> u32 {
    let self_ = _self as *const TreeCursor;
    let mut depth: u32 = 0;
    for i in 1..(*self_).stack.size {
        if ts_tree_cursor_is_entry_visible(&*self_, i) {
            depth += 1;
        }
    }
    depth
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_parent_node(
    _self: *const TSTreeCursor,
) -> TSNode {
    let self_ = _self as *const TreeCursor;
    let mut i = (*self_).stack.size as i32 - 2;
    while i >= 0 {
        let entry = &*array_get(&(*self_).stack, i as u32);
        let mut is_visible = true;
        let mut alias_symbol: TSSymbol = 0;
        if i > 0 {
            let parent_entry = &*array_get(&(*self_).stack, i as u32 - 1);
            alias_symbol = ts_language_alias_at(
                (*(*self_).tree).language,
                (*(*parent_entry.subtree).ptr).data.children.production_id as u32,
                entry.structural_child_index,
            );
            is_visible = alias_symbol != 0 || ts_subtree_visible(*entry.subtree);
        }
        if is_visible {
            return ts_node_new(
                (*self_).tree,
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
    _self: *const TSTreeCursor,
) -> TSFieldId {
    let self_ = _self as *const TreeCursor;

    // Walk up the tree, visiting the current node and its invisible ancestors.
    let mut i = (*self_).stack.size - 1;
    while i > 0 {
        let entry = &*array_get(&(*self_).stack, i);
        let parent_entry = &*array_get(&(*self_).stack, i - 1);

        // Stop walking up when another visible node is found.
        if i != (*self_).stack.size - 1
            && ts_tree_cursor_is_entry_visible(&*self_, i)
        {
            break;
        }

        if ts_subtree_extra(*entry.subtree) {
            break;
        }

        let mut field_map: *const TSFieldMapEntry = ptr::null();
        let mut field_map_end: *const TSFieldMapEntry = ptr::null();
        ts_language_field_map(
            (*(*self_).tree).language,
            (*(*parent_entry.subtree).ptr).data.children.production_id as u32,
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
    _self: *const TSTreeCursor,
) -> *const i8 {
    let id = ts_tree_cursor_current_field_id(_self);
    if id != 0 {
        let self_ = _self as *const TreeCursor;
        let lang = (*(*self_).tree).language as *const TSLanguageFull;
        return *(*lang).field_names.add(id as usize);
    }
    ptr::null()
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_copy(
    _cursor: *const TSTreeCursor,
) -> TSTreeCursor {
    let cursor = _cursor as *const TreeCursor;
    let mut res = TSTreeCursor {
        tree: ptr::null(),
        id: ptr::null(),
        context: [0, 0, 0],
    };
    let copy = &mut res as *mut TSTreeCursor as *mut TreeCursor;
    (*copy).tree = (*cursor).tree;
    (*copy).root_alias_symbol = (*cursor).root_alias_symbol;
    array_init(&mut (*copy).stack);
    array_push_all(&mut (*copy).stack, &(*cursor).stack);
    res
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_reset_to(
    _dst: *mut TSTreeCursor,
    _src: *const TSTreeCursor,
) {
    let cursor = _src as *const TreeCursor;
    let copy = _dst as *mut TreeCursor;
    (*copy).tree = (*cursor).tree;
    (*copy).root_alias_symbol = (*cursor).root_alias_symbol;
    array_clear(&mut (*copy).stack);
    array_push_all(&mut (*copy).stack, &(*cursor).stack);
}
