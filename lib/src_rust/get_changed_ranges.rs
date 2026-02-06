#![allow(dead_code)]
#![allow(non_snake_case)]

use core::ffi::c_void;
use std::ptr;

use crate::ffi::{TSInputEdit, TSLanguage, TSPoint, TSRange, TSSymbol};

use super::alloc::{ts_calloc, ts_free, ts_realloc};
use super::error_costs::ERROR_STATE;
use super::length::{length_add, length_min, length_zero, Length, LENGTH_MAX};
use super::point::{point_add, point_sub, POINT_MAX};
use super::subtree::{
    ts_builtin_sym_error, ts_subtree_child_count, ts_subtree_children,
    ts_subtree_error_cost, ts_subtree_extra, ts_subtree_has_changes,
    ts_subtree_has_external_tokens, ts_subtree_padding,
    ts_subtree_parse_state, ts_subtree_size, ts_subtree_symbol,
    ts_subtree_total_size, ts_subtree_visible, Subtree, NULL_SUBTREE,
    TS_TREE_STATE_NONE,
};
use super::language::ts_language_alias_at;
use super::tree_cursor::{TreeCursor, TreeCursorEntry, TreeCursorEntryArray};

// ---------------------------------------------------------------------------
// Extern C functions (exported from other Rust modules)
// ---------------------------------------------------------------------------

extern "C" {
    fn ts_subtree_last_external_token(self_: Subtree) -> Subtree;
    fn ts_subtree_external_scanner_state_eq(
        a: Subtree,
        b: Subtree,
    ) -> bool;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// TSRangeArray — Array(TSRange)
#[repr(C)]
pub struct TSRangeArray {
    pub contents: *mut TSRange,
    pub size: u32,
    pub capacity: u32,
}

/// Iterator — internal state for tree diffing
struct Iterator {
    cursor: TreeCursor,
    language: *const TSLanguage,
    visible_depth: u32,
    in_padding: bool,
    prev_external_token: Subtree,
}

/// IteratorComparison — result of comparing two iterators
#[derive(PartialEq, Eq)]
enum IteratorComparison {
    IteratorDiffers,
    IteratorMayDiffer,
    IteratorMatches,
}

// ---------------------------------------------------------------------------
// Array helpers for TSRangeArray
// ---------------------------------------------------------------------------

#[inline]
unsafe fn array_back_range(arr: &TSRangeArray) -> *mut TSRange {
    arr.contents.add(arr.size as usize - 1)
}

#[inline]
unsafe fn array_get_range(arr: &TSRangeArray, index: u32) -> *mut TSRange {
    arr.contents.add(index as usize)
}

unsafe fn array_grow_range(arr: &mut TSRangeArray, count: u32) {
    let new_size = arr.size + count;
    if new_size > arr.capacity {
        let mut new_capacity = if arr.capacity > 0 { arr.capacity } else { 8 };
        while new_capacity < new_size {
            new_capacity *= 2;
        }
        if arr.contents.is_null() {
            arr.contents = ts_calloc(new_capacity as usize, std::mem::size_of::<TSRange>()) as *mut TSRange;
        } else {
            arr.contents = ts_realloc(
                arr.contents as *mut c_void,
                new_capacity as usize * std::mem::size_of::<TSRange>(),
            ) as *mut TSRange;
        }
        arr.capacity = new_capacity;
    }
}

unsafe fn array_push_range(arr: &mut TSRangeArray, range: TSRange) {
    array_grow_range(arr, 1);
    ptr::write(arr.contents.add(arr.size as usize), range);
    arr.size += 1;
}

// ---------------------------------------------------------------------------
// Array helpers for TreeCursorEntry stack (mirrors tree_cursor.rs helpers)
// ---------------------------------------------------------------------------

#[inline]
unsafe fn stack_back(arr: &TreeCursorEntryArray) -> *mut TreeCursorEntry {
    arr.contents.add(arr.size as usize - 1)
}

#[inline]
unsafe fn stack_get(arr: &TreeCursorEntryArray, index: u32) -> *mut TreeCursorEntry {
    arr.contents.add(index as usize)
}

#[inline]
unsafe fn stack_clear(arr: &mut TreeCursorEntryArray) {
    arr.size = 0;
}

unsafe fn stack_grow(arr: &mut TreeCursorEntryArray, count: u32) {
    let new_size = arr.size + count;
    if new_size > arr.capacity {
        let mut new_capacity = if arr.capacity > 0 { arr.capacity } else { 8 };
        while new_capacity < new_size {
            new_capacity *= 2;
        }
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

unsafe fn stack_push(arr: &mut TreeCursorEntryArray, entry: TreeCursorEntry) {
    stack_grow(arr, 1);
    ptr::write(arr.contents.add(arr.size as usize), entry);
    arr.size += 1;
}

unsafe fn stack_pop(arr: &mut TreeCursorEntryArray) -> TreeCursorEntry {
    arr.size -= 1;
    ptr::read(arr.contents.add(arr.size as usize))
}

// ---------------------------------------------------------------------------
// Internal helpers — skeletons
// ---------------------------------------------------------------------------

unsafe fn ts_range_array_add(self_: &mut TSRangeArray, start: Length, end: Length) {
    if self_.size > 0 {
        let last_range = array_back_range(self_);
        if start.bytes <= (*last_range).end_byte {
            (*last_range).end_byte = end.bytes;
            (*last_range).end_point = end.extent;
            return;
        }
    }

    if start.bytes < end.bytes {
        let range = TSRange {
            start_point: start.extent,
            end_point: end.extent,
            start_byte: start.bytes,
            end_byte: end.bytes,
        };
        array_push_range(self_, range);
    }
}

unsafe fn iterator_new(
    cursor: *mut TreeCursor,
    tree: *const Subtree,
    language: *const TSLanguage,
) -> Iterator {
    stack_clear(&mut (*cursor).stack);
    stack_push(&mut (*cursor).stack, TreeCursorEntry {
        subtree: tree,
        position: length_zero(),
        child_index: 0,
        structural_child_index: 0,
        descendant_index: 0,
    });
    Iterator {
        cursor: ptr::read(cursor),
        language,
        visible_depth: 1,
        in_padding: false,
        prev_external_token: NULL_SUBTREE,
    }
}

#[inline]
unsafe fn iterator_done(self_: &Iterator) -> bool {
    self_.cursor.stack.size == 0
}

unsafe fn iterator_start_position(self_: &Iterator) -> Length {
    let entry = *stack_back(&self_.cursor.stack);
    if self_.in_padding {
        entry.position
    } else {
        length_add(entry.position, ts_subtree_padding(*entry.subtree))
    }
}

unsafe fn iterator_end_position(self_: &Iterator) -> Length {
    let entry = *stack_back(&self_.cursor.stack);
    let result = length_add(entry.position, ts_subtree_padding(*entry.subtree));
    if self_.in_padding {
        result
    } else {
        length_add(result, ts_subtree_size(*entry.subtree))
    }
}

unsafe fn iterator_tree_is_visible(self_: &Iterator) -> bool {
    let entry = *stack_back(&self_.cursor.stack);
    if ts_subtree_visible(*entry.subtree) {
        return true;
    }
    if self_.cursor.stack.size > 1 {
        let parent = *(*stack_get(
            &self_.cursor.stack,
            self_.cursor.stack.size - 2,
        )).subtree;
        return ts_language_alias_at(
            self_.language,
            (*parent.ptr).data.children.production_id as u32,
            entry.structural_child_index,
        ) != 0;
    }
    false
}

unsafe fn iterator_get_visible_state(
    self_: &Iterator,
    tree: *mut Subtree,
    alias_symbol: *mut TSSymbol,
    start_byte: *mut u32,
) {
    let mut i = self_.cursor.stack.size - 1;

    if self_.in_padding {
        if i == 0 {
            return;
        }
        i -= 1;
    }

    loop {
        let entry = *stack_get(&self_.cursor.stack, i);

        if i > 0 {
            let parent = (*stack_get(&self_.cursor.stack, i - 1)).subtree;
            *alias_symbol = ts_language_alias_at(
                self_.language,
                (*(*parent).ptr).data.children.production_id as u32,
                entry.structural_child_index,
            );
        }

        if ts_subtree_visible(*entry.subtree) || *alias_symbol != 0 {
            *tree = *entry.subtree;
            *start_byte = entry.position.bytes;
            break;
        }

        if i == 0 {
            break;
        }
        i -= 1;
    }
}

unsafe fn iterator_ascend(self_: &mut Iterator) {
    if iterator_done(self_) {
        return;
    }
    if iterator_tree_is_visible(self_) && !self_.in_padding {
        self_.visible_depth -= 1;
    }
    if (*stack_back(&self_.cursor.stack)).child_index > 0 {
        self_.in_padding = false;
    }
    self_.cursor.stack.size -= 1;
}

unsafe fn iterator_descend(self_: &mut Iterator, goal_position: u32) -> bool {
    if self_.in_padding {
        return false;
    }

    let mut did_descend;
    loop {
        did_descend = false;
        let entry = *stack_back(&self_.cursor.stack);
        let mut position = entry.position;
        let mut structural_child_index: u32 = 0;
        let n = ts_subtree_child_count(*entry.subtree);
        for i in 0..n {
            let child = &*ts_subtree_children(*entry.subtree).add(i as usize);
            let child_left = length_add(position, ts_subtree_padding(*child));
            let child_right = length_add(child_left, ts_subtree_size(*child));

            if child_right.bytes > goal_position {
                stack_push(&mut self_.cursor.stack, TreeCursorEntry {
                    subtree: child as *const Subtree,
                    position,
                    child_index: i,
                    structural_child_index,
                    descendant_index: 0,
                });

                if iterator_tree_is_visible(self_) {
                    if child_left.bytes > goal_position {
                        self_.in_padding = true;
                    } else {
                        self_.visible_depth += 1;
                    }
                    return true;
                }

                did_descend = true;
                break;
            }

            position = child_right;
            if !ts_subtree_extra(*child) {
                structural_child_index += 1;
            }
            let last_external_token = ts_subtree_last_external_token(*child);
            if !last_external_token.ptr.is_null() {
                self_.prev_external_token = last_external_token;
            }
        }
        if !did_descend {
            break;
        }
    }

    false
}

unsafe fn iterator_advance(self_: &mut Iterator) {
    if self_.in_padding {
        self_.in_padding = false;
        if iterator_tree_is_visible(self_) {
            self_.visible_depth += 1;
        } else {
            iterator_descend(self_, 0);
        }
        return;
    }

    loop {
        if iterator_tree_is_visible(self_) {
            self_.visible_depth -= 1;
        }
        let entry = stack_pop(&mut self_.cursor.stack);
        if iterator_done(self_) {
            return;
        }

        let parent = (*stack_back(&self_.cursor.stack)).subtree;
        let child_index = entry.child_index + 1;
        let last_external_token = ts_subtree_last_external_token(*entry.subtree);
        if !last_external_token.ptr.is_null() {
            self_.prev_external_token = last_external_token;
        }
        if ts_subtree_child_count(*parent) > child_index {
            let position = length_add(entry.position, ts_subtree_total_size(*entry.subtree));
            let mut structural_child_index = entry.structural_child_index;
            if !ts_subtree_extra(*entry.subtree) {
                structural_child_index += 1;
            }
            let next_child = &*ts_subtree_children(*parent).add(child_index as usize);

            stack_push(&mut self_.cursor.stack, TreeCursorEntry {
                subtree: next_child as *const Subtree,
                position,
                child_index,
                structural_child_index,
                descendant_index: 0,
            });

            if iterator_tree_is_visible(self_) {
                if ts_subtree_padding(*next_child).bytes > 0 {
                    self_.in_padding = true;
                } else {
                    self_.visible_depth += 1;
                }
            } else {
                iterator_descend(self_, 0);
            }
            break;
        }
    }
}

unsafe fn iterator_compare(
    old_iter: &Iterator,
    new_iter: &Iterator,
) -> IteratorComparison {
    let mut old_tree = NULL_SUBTREE;
    let mut new_tree = NULL_SUBTREE;
    let mut old_start: u32 = 0;
    let mut new_start: u32 = 0;
    let mut old_alias_symbol: TSSymbol = 0;
    let mut new_alias_symbol: TSSymbol = 0;
    iterator_get_visible_state(old_iter, &mut old_tree, &mut old_alias_symbol, &mut old_start);
    iterator_get_visible_state(new_iter, &mut new_tree, &mut new_alias_symbol, &mut new_start);
    let old_symbol = ts_subtree_symbol(old_tree);
    let new_symbol = ts_subtree_symbol(new_tree);

    if old_tree.ptr.is_null() && new_tree.ptr.is_null() {
        return IteratorComparison::IteratorMatches;
    }
    if old_tree.ptr.is_null() || new_tree.ptr.is_null() {
        return IteratorComparison::IteratorDiffers;
    }
    if old_alias_symbol != new_alias_symbol || old_symbol != new_symbol {
        return IteratorComparison::IteratorDiffers;
    }

    let old_size = ts_subtree_size(old_tree).bytes;
    let new_size = ts_subtree_size(new_tree).bytes;
    let old_state = ts_subtree_parse_state(old_tree);
    let new_state = ts_subtree_parse_state(new_tree);
    let old_has_external_tokens = ts_subtree_has_external_tokens(old_tree);
    let new_has_external_tokens = ts_subtree_has_external_tokens(new_tree);
    let old_error_cost = ts_subtree_error_cost(old_tree);
    let new_error_cost = ts_subtree_error_cost(new_tree);

    if old_start != new_start
        || old_symbol == ts_builtin_sym_error
        || old_size != new_size
        || old_state == TS_TREE_STATE_NONE
        || new_state == TS_TREE_STATE_NONE
        || (old_state == ERROR_STATE) != (new_state == ERROR_STATE)
        || old_error_cost != new_error_cost
        || old_has_external_tokens != new_has_external_tokens
        || ts_subtree_has_changes(old_tree)
        || (old_has_external_tokens
            && !ts_subtree_external_scanner_state_eq(
                old_iter.prev_external_token,
                new_iter.prev_external_token,
            ))
    {
        return IteratorComparison::IteratorMayDiffer;
    }

    IteratorComparison::IteratorMatches
}

// ---------------------------------------------------------------------------
// Exported functions — skeletons
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_range_array_intersects(
    self_: *const TSRangeArray,
    start_index: u32,
    start_byte: u32,
    end_byte: u32,
) -> bool {
    for i in start_index..(*self_).size {
        let range = array_get_range(&*self_, i);
        if (*range).end_byte > start_byte {
            if (*range).start_byte >= end_byte {
                break;
            }
            return true;
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn ts_range_array_get_changed_ranges(
    old_ranges: *const TSRange,
    old_range_count: u32,
    new_ranges: *const TSRange,
    new_range_count: u32,
    differences: *mut TSRangeArray,
) {
    let mut new_index: u32 = 0;
    let mut old_index: u32 = 0;
    let mut current_position = length_zero();
    let mut in_old_range = false;
    let mut in_new_range = false;

    while old_index < old_range_count || new_index < new_range_count {
        let old_range = old_ranges.add(old_index as usize);
        let new_range = new_ranges.add(new_index as usize);

        let next_old_position = if in_old_range {
            Length { bytes: (*old_range).end_byte, extent: (*old_range).end_point }
        } else if old_index < old_range_count {
            Length { bytes: (*old_range).start_byte, extent: (*old_range).start_point }
        } else {
            LENGTH_MAX
        };

        let next_new_position = if in_new_range {
            Length { bytes: (*new_range).end_byte, extent: (*new_range).end_point }
        } else if new_index < new_range_count {
            Length { bytes: (*new_range).start_byte, extent: (*new_range).start_point }
        } else {
            LENGTH_MAX
        };

        if next_old_position.bytes < next_new_position.bytes {
            if in_old_range != in_new_range {
                ts_range_array_add(&mut *differences, current_position, next_old_position);
            }
            if in_old_range { old_index += 1; }
            current_position = next_old_position;
            in_old_range = !in_old_range;
        } else if next_new_position.bytes < next_old_position.bytes {
            if in_old_range != in_new_range {
                ts_range_array_add(&mut *differences, current_position, next_new_position);
            }
            if in_new_range { new_index += 1; }
            current_position = next_new_position;
            in_new_range = !in_new_range;
        } else {
            if in_old_range != in_new_range {
                ts_range_array_add(&mut *differences, current_position, next_new_position);
            }
            if in_old_range { old_index += 1; }
            if in_new_range { new_index += 1; }
            in_old_range = !in_old_range;
            in_new_range = !in_new_range;
            current_position = next_new_position;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_range_edit(
    range: *mut TSRange,
    edit: *const TSInputEdit,
) {
    if (*range).end_byte >= (*edit).old_end_byte {
        if (*range).end_byte != u32::MAX {
            (*range).end_byte = (*edit).new_end_byte + ((*range).end_byte - (*edit).old_end_byte);
            (*range).end_point = point_add(
                (*edit).new_end_point,
                point_sub((*range).end_point, (*edit).old_end_point),
            );
            if (*range).end_byte < (*edit).new_end_byte {
                (*range).end_byte = u32::MAX;
                (*range).end_point = POINT_MAX;
            }
        }
    } else if (*range).end_byte > (*edit).start_byte {
        (*range).end_byte = (*edit).start_byte;
        (*range).end_point = (*edit).start_point;
    }

    if (*range).start_byte >= (*edit).old_end_byte {
        (*range).start_byte = (*edit).new_end_byte + ((*range).start_byte - (*edit).old_end_byte);
        (*range).start_point = point_add(
            (*edit).new_end_point,
            point_sub((*range).start_point, (*edit).old_end_point),
        );
        if (*range).start_byte < (*edit).new_end_byte {
            (*range).start_byte = u32::MAX;
            (*range).start_point = POINT_MAX;
        }
    } else if (*range).start_byte > (*edit).start_byte {
        (*range).start_byte = (*edit).start_byte;
        (*range).start_point = (*edit).start_point;
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_subtree_get_changed_ranges(
    old_tree: *const Subtree,
    new_tree: *const Subtree,
    cursor1: *mut TreeCursor,
    cursor2: *mut TreeCursor,
    language: *const TSLanguage,
    included_range_differences: *const TSRangeArray,
    ranges: *mut *mut TSRange,
) -> u32 {
    let mut results = TSRangeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    };

    let mut old_iter = iterator_new(cursor1, old_tree, language);
    let mut new_iter = iterator_new(cursor2, new_tree, language);

    let mut included_range_difference_index: u32 = 0;

    let mut position = iterator_start_position(&old_iter);
    let mut next_position = iterator_start_position(&new_iter);
    if position.bytes < next_position.bytes {
        ts_range_array_add(&mut results, position, next_position);
        position = next_position;
    } else if position.bytes > next_position.bytes {
        ts_range_array_add(&mut results, next_position, position);
        next_position = position;
    }

    loop {
        // Compare the old and new subtrees.
        let mut comparison = iterator_compare(&old_iter, &new_iter);

        // Even if the two subtrees appear to be identical, they could differ
        // internally if they contain a range of text that was previously
        // excluded from the parse, and is now included, or vice-versa.
        if comparison == IteratorComparison::IteratorMatches
            && ts_range_array_intersects(
                included_range_differences,
                included_range_difference_index,
                position.bytes,
                iterator_end_position(&old_iter).bytes,
            )
        {
            comparison = IteratorComparison::IteratorMayDiffer;
        }

        let mut is_changed = false;
        match comparison {
            // If the subtrees are definitely identical, move to the end
            // of both subtrees.
            IteratorComparison::IteratorMatches => {
                next_position = iterator_end_position(&old_iter);
            }

            // If the subtrees might differ internally, descend into both
            // subtrees, finding the first child that spans the current position.
            IteratorComparison::IteratorMayDiffer => {
                if iterator_descend(&mut old_iter, position.bytes) {
                    if !iterator_descend(&mut new_iter, position.bytes) {
                        is_changed = true;
                        next_position = iterator_end_position(&old_iter);
                    }
                } else if iterator_descend(&mut new_iter, position.bytes) {
                    is_changed = true;
                    next_position = iterator_end_position(&new_iter);
                } else {
                    next_position = length_min(
                        iterator_end_position(&old_iter),
                        iterator_end_position(&new_iter),
                    );
                }
            }

            // If the subtrees are different, record a change and then move
            // to the end of both subtrees.
            IteratorComparison::IteratorDiffers => {
                is_changed = true;
                next_position = length_min(
                    iterator_end_position(&old_iter),
                    iterator_end_position(&new_iter),
                );
            }
        }

        // Ensure that both iterators are caught up to the current position.
        while !iterator_done(&old_iter)
            && iterator_end_position(&old_iter).bytes <= next_position.bytes
        {
            iterator_advance(&mut old_iter);
        }
        while !iterator_done(&new_iter)
            && iterator_end_position(&new_iter).bytes <= next_position.bytes
        {
            iterator_advance(&mut new_iter);
        }

        // Ensure that both iterators are at the same depth in the tree.
        while old_iter.visible_depth > new_iter.visible_depth {
            iterator_ascend(&mut old_iter);
        }
        while new_iter.visible_depth > old_iter.visible_depth {
            iterator_ascend(&mut new_iter);
        }

        if is_changed {
            ts_range_array_add(&mut results, position, next_position);
        }

        position = next_position;

        // Keep track of the current position in the included range differences
        // array in order to avoid scanning the entire array on each iteration.
        while included_range_difference_index < (*included_range_differences).size {
            let range = array_get_range(
                &*included_range_differences,
                included_range_difference_index,
            );
            if (*range).end_byte <= position.bytes {
                included_range_difference_index += 1;
            } else {
                break;
            }
        }

        if iterator_done(&old_iter) || iterator_done(&new_iter) {
            break;
        }
    }

    let old_size = ts_subtree_total_size(*old_tree);
    let new_size = ts_subtree_total_size(*new_tree);
    if old_size.bytes < new_size.bytes {
        ts_range_array_add(&mut results, old_size, new_size);
    } else if new_size.bytes < old_size.bytes {
        ts_range_array_add(&mut results, new_size, old_size);
    }

    *cursor1 = old_iter.cursor;
    *cursor2 = new_iter.cursor;
    *ranges = results.contents;
    results.size
}
