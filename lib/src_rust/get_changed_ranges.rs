#![allow(dead_code)]
#![allow(non_snake_case)]

use core::{cmp::Ordering, ffi::c_void};
use std::ptr;

use crate::ffi::{TSInputEdit, TSLanguage, TSRange, TSSymbol};

use super::alloc::{calloc, realloc};
use super::error_costs::ERROR_STATE;
use super::language::language_alias_at;
use super::length::{length_add, length_min, length_zero, Length, LENGTH_MAX};
use super::point::{point_add, point_sub, POINT_MAX};
use super::subtree::{
    subtree_child_count, subtree_children, subtree_error_cost, subtree_external_scanner_state_eq,
    subtree_extra, subtree_has_changes, subtree_has_external_tokens, subtree_last_external_token,
    subtree_padding, subtree_parse_state, subtree_size, subtree_symbol, subtree_total_size,
    subtree_visible, ts_builtin_sym_error, Subtree, NULL_SUBTREE, TS_TREE_STATE_NONE,
};
use super::tree_cursor::{TreeCursor, TreeCursorEntry, TreeCursorEntryArray};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Growable array of changed ranges.
#[repr(C)]
pub struct TSRangeArray {
    /// Backing range storage.
    pub contents: *mut TSRange,
    /// Number of initialized ranges.
    pub size: u32,
    /// Allocated range capacity.
    pub capacity: u32,
}

/// Cursor used when diffing two syntax trees.
///
/// The iterator walks visible syntax ranges in source order. It can also stop
/// on a node's padding so edits in leading whitespace are reported separately
/// from edits in the node's content.
struct Iterator {
    /// Cursor stack pointing at the current subtree.
    cursor: TreeCursor,
    /// Language metadata used for alias visibility.
    language: *const TSLanguage,
    /// Number of visible ancestors currently on the stack.
    visible_depth: u32,
    /// Whether the current iterator item is leading padding, not node content.
    in_padding: bool,
    /// Last external token seen before the current iterator position.
    prev_external_token: Subtree,
}

/// Result of comparing old and new iterator positions.
#[derive(PartialEq, Eq)]
enum IteratorComparison {
    /// The visible nodes are definitely different.
    Differs,
    /// The visible nodes match at this level, but children may differ.
    MayDiffer,
    /// The visible nodes and reuse-sensitive metadata match.
    Matches,
}

/// Visible node state used for comparing old and new iterators.
struct VisibleState {
    /// Nearest visible or aliased subtree.
    tree: Subtree,
    /// Alias symbol that makes a hidden node visible, or zero.
    alias_symbol: TSSymbol,
    /// Start byte of `tree`.
    start_byte: u32,
}

// ---------------------------------------------------------------------------
// Array helpers for TSRangeArray
// ---------------------------------------------------------------------------

#[inline]
const unsafe fn range_array_slice(arr: &TSRangeArray) -> &[TSRange] {
    std::slice::from_raw_parts(arr.contents, arr.size as usize)
}

#[inline]
pub const unsafe fn range_slice<'a>(ranges: *const TSRange, count: u32) -> &'a [TSRange] {
    if count == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ranges, count as usize)
    }
}

unsafe fn array_grow_range(arr: &mut TSRangeArray, count: u32) {
    let new_size = arr.size + count;
    if new_size > arr.capacity {
        let mut new_capacity = if arr.capacity > 0 { arr.capacity } else { 8 };
        while new_capacity < new_size {
            new_capacity *= 2;
        }
        if arr.contents.is_null() {
            arr.contents =
                calloc(new_capacity as usize, std::mem::size_of::<TSRange>()).cast::<TSRange>();
        } else {
            arr.contents = realloc(
                arr.contents.cast::<c_void>(),
                new_capacity as usize * std::mem::size_of::<TSRange>(),
            )
            .cast::<TSRange>();
        }
        arr.capacity = new_capacity;
    }
}

unsafe fn array_push_range(arr: &mut TSRangeArray, range: TSRange) {
    array_grow_range(arr, 1);
    ptr::write(arr.contents.add(arr.size as usize), range);
    arr.size += 1;
}

pub fn range_edit_ref(range: &mut TSRange, edit: &TSInputEdit) {
    if range.end_byte >= edit.old_end_byte {
        if range.end_byte != u32::MAX {
            range.end_byte = edit.new_end_byte + (range.end_byte - edit.old_end_byte);
            range.end_point = point_add(
                edit.new_end_point,
                point_sub(range.end_point, edit.old_end_point),
            );
            if range.end_byte < edit.new_end_byte {
                range.end_byte = u32::MAX;
                range.end_point = POINT_MAX;
            }
        }
    } else if range.end_byte > edit.start_byte {
        range.end_byte = edit.start_byte;
        range.end_point = edit.start_point;
    }

    if range.start_byte >= edit.old_end_byte {
        range.start_byte = edit.new_end_byte + (range.start_byte - edit.old_end_byte);
        range.start_point = point_add(
            edit.new_end_point,
            point_sub(range.start_point, edit.old_end_point),
        );
        if range.start_byte < edit.new_end_byte {
            range.start_byte = u32::MAX;
            range.start_point = POINT_MAX;
        }
    } else if range.start_byte > edit.start_byte {
        range.start_byte = edit.start_byte;
        range.start_point = edit.start_point;
    }
}

pub unsafe fn range_array_intersects_ref(
    ranges: &TSRangeArray,
    start_index: u32,
    start_byte: u32,
    end_byte: u32,
) -> bool {
    for i in start_index..ranges.size {
        let range = range_array_slice(ranges).get_unchecked(i as usize);
        if range.end_byte > start_byte {
            if range.start_byte >= end_byte {
                break;
            }
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Array helpers for TreeCursorEntry stack (mirrors tree_cursor.rs helpers)
// ---------------------------------------------------------------------------

#[inline]
const unsafe fn stack_slice(arr: &TreeCursorEntryArray) -> &[TreeCursorEntry] {
    std::slice::from_raw_parts(arr.contents, arr.size as usize)
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
            arr.contents = calloc(
                new_capacity as usize,
                std::mem::size_of::<TreeCursorEntry>(),
            )
            .cast::<TreeCursorEntry>();
        } else {
            arr.contents = realloc(
                arr.contents.cast::<c_void>(),
                new_capacity as usize * std::mem::size_of::<TreeCursorEntry>(),
            )
            .cast::<TreeCursorEntry>();
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

#[inline]
unsafe fn subtree_child<'a>(parent: Subtree, index: u32) -> &'a Subtree {
    subtree_children_slice(parent).get_unchecked(index as usize)
}

#[inline]
const unsafe fn subtree_children_slice<'a>(parent: Subtree) -> &'a [Subtree] {
    std::slice::from_raw_parts(
        subtree_children(parent),
        subtree_child_count(parent) as usize,
    )
}

// ---------------------------------------------------------------------------
// Internal helpers — skeletons
// ---------------------------------------------------------------------------

unsafe fn range_array_add(self_: &mut TSRangeArray, start: Length, end: Length) {
    if self_.size > 0 {
        let last_range = self_
            .contents
            .add(self_.size as usize - 1)
            .as_mut()
            .unwrap_unchecked();
        if start.bytes <= last_range.end_byte {
            last_range.end_byte = end.bytes;
            last_range.end_point = end.extent;
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

/// Create a diff iterator rooted at a subtree.
unsafe fn iterator_new(
    cursor: &mut TreeCursor,
    tree: &Subtree,
    language: *const TSLanguage,
) -> Iterator {
    stack_clear(&mut cursor.stack);
    stack_push(
        &mut cursor.stack,
        TreeCursorEntry {
            subtree: tree,
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
        },
    );
    Iterator {
        cursor: ptr::read(cursor),
        language,
        visible_depth: 1,
        in_padding: false,
        prev_external_token: NULL_SUBTREE,
    }
}

#[inline]
const unsafe fn iterator_done(self_: &Iterator) -> bool {
    self_.cursor.stack.size == 0
}

/// Return the current item's start position.
///
/// For padding items this is the parent entry position. For node-content items,
/// it is the position after leading padding.
unsafe fn iterator_start_position(self_: &Iterator) -> Length {
    let entry = stack_slice(&self_.cursor.stack).last().unwrap_unchecked();
    if self_.in_padding {
        entry.position
    } else {
        length_add(entry.position, subtree_padding(*entry.subtree))
    }
}

/// Return the current item's end position.
unsafe fn iterator_end_position(self_: &Iterator) -> Length {
    let entry = stack_slice(&self_.cursor.stack).last().unwrap_unchecked();
    let result = length_add(entry.position, subtree_padding(*entry.subtree));
    if self_.in_padding {
        result
    } else {
        length_add(result, subtree_size(*entry.subtree))
    }
}

/// Determine whether the current cursor entry is publicly visible.
///
/// Hidden grammar nodes can still be visible through aliases, so this must check
/// the parent production's alias sequence in addition to subtree visibility.
unsafe fn iterator_tree_is_visible(self_: &Iterator) -> bool {
    let entries = stack_slice(&self_.cursor.stack);
    let entry = entries.last().unwrap_unchecked();
    if subtree_visible(*entry.subtree) {
        return true;
    }
    if self_.cursor.stack.size > 1 {
        let parent_entry = entries.get_unchecked(self_.cursor.stack.size as usize - 2);
        let parent = *parent_entry.subtree;
        return language_alias_at(
            self_.language,
            u32::from((*parent.ptr).data.children.production_id),
            entry.structural_child_index,
        ) != 0;
    }
    false
}

/// Find the nearest visible state at or above the iterator position.
unsafe fn iterator_get_visible_state(self_: &Iterator) -> VisibleState {
    let mut result = VisibleState {
        tree: NULL_SUBTREE,
        alias_symbol: 0,
        start_byte: 0,
    };
    let mut i = self_.cursor.stack.size - 1;

    if self_.in_padding {
        if i == 0 {
            return result;
        }
        i -= 1;
    }

    let entries = stack_slice(&self_.cursor.stack);
    loop {
        let entry = entries.get_unchecked(i as usize);

        if i > 0 {
            let parent = entries.get_unchecked((i - 1) as usize).subtree;
            result.alias_symbol = language_alias_at(
                self_.language,
                u32::from((*(*parent).ptr).data.children.production_id),
                entry.structural_child_index,
            );
        }

        if subtree_visible(*entry.subtree) || result.alias_symbol != 0 {
            result.tree = *entry.subtree;
            result.start_byte = entry.position.bytes;
            break;
        }

        if i == 0 {
            break;
        }
        i -= 1;
    }
    result
}

/// Move one level up in the diff cursor.
unsafe fn iterator_ascend(self_: &mut Iterator) {
    if iterator_done(self_) {
        return;
    }
    if iterator_tree_is_visible(self_) && !self_.in_padding {
        self_.visible_depth -= 1;
    }
    if stack_slice(&self_.cursor.stack)
        .last()
        .unwrap_unchecked()
        .child_index
        > 0
    {
        self_.in_padding = false;
    }
    self_.cursor.stack.size -= 1;
}

/// Descend to the child that spans `goal_position`.
///
/// If the child is visible and its padding starts after the goal, the iterator
/// stops in padding. Otherwise it stops on the child content or keeps descending
/// through hidden children.
unsafe fn iterator_descend(self_: &mut Iterator, goal_position: u32) -> bool {
    if self_.in_padding {
        return false;
    }

    let mut did_descend;
    loop {
        did_descend = false;
        let entry = *stack_slice(&self_.cursor.stack).last().unwrap_unchecked();
        let mut position = entry.position;
        let mut structural_child_index: u32 = 0;
        let n = subtree_child_count(*entry.subtree);
        for i in 0..n {
            let child = subtree_child(*entry.subtree, i);
            let child_left = length_add(position, subtree_padding(*child));
            let child_right = length_add(child_left, subtree_size(*child));

            if child_right.bytes > goal_position {
                stack_push(
                    &mut self_.cursor.stack,
                    TreeCursorEntry {
                        subtree: std::ptr::from_ref::<Subtree>(child),
                        position,
                        child_index: i,
                        structural_child_index,
                        descendant_index: 0,
                    },
                );

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
            if !subtree_extra(*child) {
                structural_child_index += 1;
            }
            let last_external_token = subtree_last_external_token(*child);
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

/// Advance to the next visible range or padding range in source order.
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

        let parent = stack_slice(&self_.cursor.stack)
            .last()
            .unwrap_unchecked()
            .subtree;
        let child_index = entry.child_index + 1;
        let last_external_token = subtree_last_external_token(*entry.subtree);
        if !last_external_token.ptr.is_null() {
            self_.prev_external_token = last_external_token;
        }
        if subtree_child_count(*parent) > child_index {
            let position = length_add(entry.position, subtree_total_size(*entry.subtree));
            let mut structural_child_index = entry.structural_child_index;
            if !subtree_extra(*entry.subtree) {
                structural_child_index += 1;
            }
            let next_child = subtree_child(*parent, child_index);

            stack_push(
                &mut self_.cursor.stack,
                TreeCursorEntry {
                    subtree: std::ptr::from_ref::<Subtree>(next_child),
                    position,
                    child_index,
                    structural_child_index,
                    descendant_index: 0,
                },
            );

            if iterator_tree_is_visible(self_) {
                if subtree_padding(*next_child).bytes > 0 {
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

/// Compare the visible old/new states at the current iterator positions.
///
/// Definite differences can be reported immediately. "May differ" asks the diff
/// loop to descend because external scanner state, parse states, edit flags, or
/// error metadata prevent treating the whole subtree as identical.
unsafe fn iterator_compare(old_iter: &Iterator, new_iter: &Iterator) -> IteratorComparison {
    let old_visible = iterator_get_visible_state(old_iter);
    let new_visible = iterator_get_visible_state(new_iter);
    let old_tree = old_visible.tree;
    let new_tree = new_visible.tree;
    let old_symbol = subtree_symbol(old_tree);
    let new_symbol = subtree_symbol(new_tree);

    if old_tree.ptr.is_null() && new_tree.ptr.is_null() {
        return IteratorComparison::Matches;
    }
    if old_tree.ptr.is_null() || new_tree.ptr.is_null() {
        return IteratorComparison::Differs;
    }
    if old_visible.alias_symbol != new_visible.alias_symbol || old_symbol != new_symbol {
        return IteratorComparison::Differs;
    }

    let old_size = subtree_size(old_tree).bytes;
    let new_size = subtree_size(new_tree).bytes;
    let old_state = subtree_parse_state(old_tree);
    let new_state = subtree_parse_state(new_tree);
    let old_has_external_tokens = subtree_has_external_tokens(old_tree);
    let new_has_external_tokens = subtree_has_external_tokens(new_tree);
    let old_error_cost = subtree_error_cost(old_tree);
    let new_error_cost = subtree_error_cost(new_tree);

    if old_visible.start_byte != new_visible.start_byte
        || old_symbol == ts_builtin_sym_error
        || old_size != new_size
        || old_state == TS_TREE_STATE_NONE
        || new_state == TS_TREE_STATE_NONE
        || (old_state == ERROR_STATE) != (new_state == ERROR_STATE)
        || old_error_cost != new_error_cost
        || old_has_external_tokens != new_has_external_tokens
        || subtree_has_changes(old_tree)
        || (old_has_external_tokens
            && !subtree_external_scanner_state_eq(
                &old_iter.prev_external_token,
                &new_iter.prev_external_token,
            ))
    {
        return IteratorComparison::MayDiffer;
    }

    IteratorComparison::Matches
}

// ---------------------------------------------------------------------------
// Exported functions — skeletons
// ---------------------------------------------------------------------------

pub unsafe fn range_array_get_changed_ranges_ref(
    old_ranges: &[TSRange],
    new_ranges: &[TSRange],
    differences: &mut TSRangeArray,
) {
    // Sweep the two sorted included-range lists and record the symmetric
    // difference: spans that were visible in exactly one tree.
    let mut new_index = 0;
    let mut old_index = 0;
    let mut current_position = length_zero();
    let mut in_old_range = false;
    let mut in_new_range = false;

    while old_index < old_ranges.len() || new_index < new_ranges.len() {
        let next_old_position = if in_old_range {
            let old_range = old_ranges.get_unchecked(old_index);
            Length {
                bytes: old_range.end_byte,
                extent: old_range.end_point,
            }
        } else if old_index < old_ranges.len() {
            let old_range = old_ranges.get_unchecked(old_index);
            Length {
                bytes: old_range.start_byte,
                extent: old_range.start_point,
            }
        } else {
            LENGTH_MAX
        };

        let next_new_position = if in_new_range {
            let new_range = new_ranges.get_unchecked(new_index);
            Length {
                bytes: new_range.end_byte,
                extent: new_range.end_point,
            }
        } else if new_index < new_ranges.len() {
            let new_range = new_ranges.get_unchecked(new_index);
            Length {
                bytes: new_range.start_byte,
                extent: new_range.start_point,
            }
        } else {
            LENGTH_MAX
        };

        match next_old_position.bytes.cmp(&next_new_position.bytes) {
            Ordering::Less => {
                if in_old_range != in_new_range {
                    range_array_add(differences, current_position, next_old_position);
                }
                if in_old_range {
                    old_index += 1;
                }
                current_position = next_old_position;
                in_old_range = !in_old_range;
            }
            Ordering::Greater => {
                if in_old_range != in_new_range {
                    range_array_add(differences, current_position, next_new_position);
                }
                if in_new_range {
                    new_index += 1;
                }
                current_position = next_new_position;
                in_new_range = !in_new_range;
            }
            Ordering::Equal => {
                if in_old_range != in_new_range {
                    range_array_add(differences, current_position, next_new_position);
                }
                if in_old_range {
                    old_index += 1;
                }
                if in_new_range {
                    new_index += 1;
                }
                in_old_range = !in_old_range;
                in_new_range = !in_new_range;
                current_position = next_new_position;
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_range_edit(range: *mut TSRange, edit: *const TSInputEdit) {
    let range = range.as_mut().unwrap_unchecked();
    let edit = edit.as_ref().unwrap_unchecked();

    range_edit_ref(range, edit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::TSPoint;

    fn point(row: u32, column: u32) -> TSPoint {
        TSPoint { row, column }
    }

    fn range(start_byte: u32, end_byte: u32) -> TSRange {
        TSRange {
            start_point: point(0, start_byte),
            end_point: point(0, end_byte),
            start_byte,
            end_byte,
        }
    }

    fn edit() -> TSInputEdit {
        TSInputEdit {
            start_byte: 5,
            old_end_byte: 10,
            new_end_byte: 12,
            start_point: point(0, 5),
            old_end_point: point(0, 10),
            new_end_point: point(1, 2),
        }
    }

    fn assert_range_eq(actual: TSRange, expected: TSRange) {
        assert_eq!(actual.start_byte, expected.start_byte);
        assert_eq!(actual.end_byte, expected.end_byte);
        assert_eq!(actual.start_point.row, expected.start_point.row);
        assert_eq!(actual.start_point.column, expected.start_point.column);
        assert_eq!(actual.end_point.row, expected.end_point.row);
        assert_eq!(actual.end_point.column, expected.end_point.column);
    }

    fn range_array(ranges: &mut [TSRange]) -> TSRangeArray {
        TSRangeArray {
            contents: ranges.as_mut_ptr(),
            size: ranges.len() as u32,
            capacity: ranges.len() as u32,
        }
    }

    #[test]
    fn edit_range_after_changed_range() {
        let mut edited_range = range(14, 18);

        range_edit_ref(&mut edited_range, &edit());

        assert_range_eq(
            edited_range,
            TSRange {
                start_point: point(1, 6),
                end_point: point(1, 10),
                start_byte: 16,
                end_byte: 20,
            },
        );
    }

    #[test]
    fn edit_range_overlapping_changed_range() {
        let mut edited_range = range(7, 14);

        range_edit_ref(&mut edited_range, &edit());

        assert_range_eq(
            edited_range,
            TSRange {
                start_point: point(0, 5),
                end_point: point(1, 6),
                start_byte: 5,
                end_byte: 16,
            },
        );
    }

    #[test]
    fn edit_range_before_changed_range() {
        let mut edited_range = range(1, 4);

        range_edit_ref(&mut edited_range, &edit());

        assert_range_eq(edited_range, range(1, 4));
    }

    #[test]
    fn range_array_intersects_overlapping_range() {
        let mut ranges = [range(2, 4), range(7, 9), range(12, 15)];
        let range_array = range_array(&mut ranges);

        assert!(unsafe { range_array_intersects_ref(&range_array, 0, 8, 11) });
    }

    #[test]
    fn range_array_intersects_respects_start_index() {
        let mut ranges = [range(2, 4), range(7, 9), range(12, 15)];
        let range_array = range_array(&mut ranges);

        assert!(!unsafe { range_array_intersects_ref(&range_array, 2, 8, 11) });
    }
}

pub unsafe fn subtree_get_changed_ranges_ref(
    old_tree: &Subtree,
    new_tree: &Subtree,
    old_cursor: &mut TreeCursor,
    new_cursor: &mut TreeCursor,
    language: *const TSLanguage,
    included_range_differences_array: &TSRangeArray,
    ranges: &mut *mut TSRange,
) -> u32 {
    // Walk both trees in lockstep. Matching subtrees can be skipped wholesale;
    // maybe-matching subtrees are descended into; differing subtrees emit one
    // changed range and advance both iterators past the differing span.
    let mut results = TSRangeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    };

    let mut old_iter = iterator_new(old_cursor, old_tree, language);
    let mut new_iter = iterator_new(new_cursor, new_tree, language);

    let mut included_range_difference_index: u32 = 0;

    let mut position = iterator_start_position(&old_iter);
    let mut next_position = iterator_start_position(&new_iter);
    if position.bytes < next_position.bytes {
        range_array_add(&mut results, position, next_position);
        position = next_position;
    } else if position.bytes > next_position.bytes {
        range_array_add(&mut results, next_position, position);
        next_position = position;
    }

    loop {
        // Compare the old and new subtrees.
        let mut comparison = iterator_compare(&old_iter, &new_iter);

        // Even if the two subtrees appear to be identical, they could differ
        // internally if they contain a range of text that was previously
        // excluded from the parse, and is now included, or vice-versa.
        if comparison == IteratorComparison::Matches
            && range_array_intersects_ref(
                included_range_differences_array,
                included_range_difference_index,
                position.bytes,
                iterator_end_position(&old_iter).bytes,
            )
        {
            comparison = IteratorComparison::MayDiffer;
        }

        let is_changed = match comparison {
            // If the subtrees are definitely identical, move to the end
            // of both subtrees.
            IteratorComparison::Matches => {
                next_position = iterator_end_position(&old_iter);
                false
            }

            // If the subtrees might differ internally, descend into both
            // subtrees, finding the first child that spans the current position.
            IteratorComparison::MayDiffer => {
                if iterator_descend(&mut old_iter, position.bytes) {
                    if !iterator_descend(&mut new_iter, position.bytes) {
                        next_position = iterator_end_position(&old_iter);
                        true
                    } else {
                        false
                    }
                } else if iterator_descend(&mut new_iter, position.bytes) {
                    next_position = iterator_end_position(&new_iter);
                    true
                } else {
                    next_position = length_min(
                        iterator_end_position(&old_iter),
                        iterator_end_position(&new_iter),
                    );
                    false
                }
            }

            // If the subtrees are different, record a change and then move
            // to the end of both subtrees.
            IteratorComparison::Differs => {
                next_position = length_min(
                    iterator_end_position(&old_iter),
                    iterator_end_position(&new_iter),
                );
                true
            }
        };

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
            range_array_add(&mut results, position, next_position);
        }

        position = next_position;

        // Keep track of the current position in the included range differences
        // array in order to avoid scanning the entire array on each iteration.
        while included_range_difference_index < included_range_differences_array.size {
            let range = range_array_slice(included_range_differences_array)
                .get_unchecked(included_range_difference_index as usize);
            if range.end_byte <= position.bytes {
                included_range_difference_index += 1;
            } else {
                break;
            }
        }

        if iterator_done(&old_iter) || iterator_done(&new_iter) {
            break;
        }
    }

    let old_size = subtree_total_size(*old_tree);
    let new_size = subtree_total_size(*new_tree);
    if old_size.bytes < new_size.bytes {
        range_array_add(&mut results, old_size, new_size);
    } else if new_size.bytes < old_size.bytes {
        range_array_add(&mut results, new_size, old_size);
    }

    *old_cursor = old_iter.cursor;
    *new_cursor = new_iter.cursor;
    *ranges = results.contents;
    results.size
}
