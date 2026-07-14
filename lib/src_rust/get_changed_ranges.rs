use core::cmp::Ordering;
use core::ptr;

use crate::ffi::{TSInputEdit, TSLanguage, TSRange, TSSymbol};

use super::error_costs::ERROR_STATE;
use super::language::language_alias_at;
use super::length::{length_add, length_min, length_zero, Length, LENGTH_MAX};
use super::point::{point_add, point_sub, POINT_MAX};
use super::subtree::{
    subtree_child, subtree_child_count, subtree_error_cost, subtree_external_scanner_state_eq,
    subtree_extra, subtree_has_changes, subtree_has_external_tokens, subtree_last_external_token,
    subtree_padding, subtree_parse_state, subtree_size, subtree_symbol, subtree_total_size,
    subtree_visible, Subtree, NULL_SUBTREE, TS_BUILTIN_SYM_ERROR, TS_TREE_STATE_NONE,
};
use super::tree_cursor::{tree_cursor_entry_slice, TreeCursor, TreeCursorEntry};
use super::utils::{array_new, array_push, Array};
use super::utils::{ptr_mut, ptr_ref};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Growable array of changed ranges.
pub type TSRangeArray = Array<TSRange>;

/// Cursor used when diffing two syntax trees.
///
/// The iterator walks visible syntax ranges in source order. It can also stop
/// on a node's padding so edits in leading whitespace are reported separately
/// from edits in the node's content.
struct DiffIterator {
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
    core::slice::from_raw_parts(arr.contents, arr.size as usize)
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
// Internal range and diff helpers
// ---------------------------------------------------------------------------

unsafe fn range_array_add(arr: &mut TSRangeArray, start: Length, end: Length) {
    if arr.size > 0 {
        let last_range = arr
            .contents
            .add(arr.size as usize - 1)
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
        array_push(arr, range);
    }
}

impl DiffIterator {
    /// Create a diff iterator rooted at a subtree.
    unsafe fn new(cursor: &mut TreeCursor, tree: &Subtree, language: *const TSLanguage) -> Self {
        cursor.stack.clear();
        cursor.stack.push(TreeCursorEntry {
            subtree: tree,
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
        });
        Self {
            cursor: ptr::read(cursor),
            language,
            visible_depth: 1,
            in_padding: false,
            prev_external_token: NULL_SUBTREE,
        }
    }

    #[inline]
    const fn done(&self) -> bool {
        self.cursor.stack.size == 0
    }

    /// Return the current item's start position.
    ///
    /// For padding items this is the parent entry position. For node-content items,
    /// it is the position after leading padding.
    unsafe fn start_position(&self) -> Length {
        let entry = tree_cursor_entry_slice(&self.cursor.stack)
            .last()
            .unwrap_unchecked();
        if self.in_padding {
            entry.position
        } else {
            length_add(entry.position, subtree_padding(*entry.subtree))
        }
    }

    /// Return the current item's end position.
    unsafe fn end_position(&self) -> Length {
        let entry = tree_cursor_entry_slice(&self.cursor.stack)
            .last()
            .unwrap_unchecked();
        let result = length_add(entry.position, subtree_padding(*entry.subtree));
        if self.in_padding {
            result
        } else {
            length_add(result, subtree_size(*entry.subtree))
        }
    }

    /// Determine whether the current cursor entry is publicly visible.
    ///
    /// Hidden grammar nodes can still be visible through aliases, so this must check
    /// the parent production's alias sequence in addition to subtree visibility.
    unsafe fn tree_is_visible(&self) -> bool {
        let entries = tree_cursor_entry_slice(&self.cursor.stack);
        let entry = entries.last().unwrap_unchecked();
        if subtree_visible(*entry.subtree) {
            return true;
        }
        if self.cursor.stack.size > 1 {
            let parent_entry = entries.get_unchecked(self.cursor.stack.size as usize - 2);
            let parent = *parent_entry.subtree;
            return language_alias_at(
                self.language,
                u32::from((*parent.ptr).children().production_id),
                entry.structural_child_index,
            ) != 0;
        }
        false
    }

    /// Find the nearest visible state at or above the iterator position.
    unsafe fn visible_state(&self) -> VisibleState {
        let mut result = VisibleState {
            tree: NULL_SUBTREE,
            alias_symbol: 0,
            start_byte: 0,
        };
        let mut i = self.cursor.stack.size - 1;

        if self.in_padding {
            if i == 0 {
                return result;
            }
            i -= 1;
        }

        let entries = tree_cursor_entry_slice(&self.cursor.stack);
        loop {
            let entry = entries.get_unchecked(i as usize);

            if i > 0 {
                let parent = entries.get_unchecked((i - 1) as usize).subtree;
                result.alias_symbol = language_alias_at(
                    self.language,
                    u32::from((*(*parent).ptr).children().production_id),
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
    unsafe fn ascend(&mut self) {
        if self.done() {
            return;
        }
        if self.tree_is_visible() && !self.in_padding {
            self.visible_depth -= 1;
        }
        if tree_cursor_entry_slice(&self.cursor.stack)
            .last()
            .unwrap_unchecked()
            .child_index
            > 0
        {
            self.in_padding = false;
        }
        self.cursor.stack.size -= 1;
    }

    /// Descend to the child that spans `goal_position`.
    ///
    /// If the child is visible and its padding starts after the goal, the iterator
    /// stops in padding. Otherwise it stops on the child content or keeps descending
    /// through hidden children.
    unsafe fn descend(&mut self, goal_position: u32) -> bool {
        if self.in_padding {
            return false;
        }

        let mut did_descend;
        loop {
            did_descend = false;
            let entry = *tree_cursor_entry_slice(&self.cursor.stack)
                .last()
                .unwrap_unchecked();
            let mut position = entry.position;
            let mut structural_child_index: u32 = 0;
            let n = subtree_child_count(*entry.subtree);
            for i in 0..n {
                let child = subtree_child(*entry.subtree, i);
                let child_left = length_add(position, subtree_padding(*child));
                let child_right = length_add(child_left, subtree_size(*child));

                if child_right.bytes > goal_position {
                    self.cursor.stack.push(TreeCursorEntry {
                        subtree: core::ptr::from_ref::<Subtree>(child),
                        position,
                        child_index: i,
                        structural_child_index,
                        descendant_index: 0,
                    });

                    if self.tree_is_visible() {
                        if child_left.bytes > goal_position {
                            self.in_padding = true;
                        } else {
                            self.visible_depth += 1;
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
                    self.prev_external_token = last_external_token;
                }
            }
            if !did_descend {
                break;
            }
        }

        false
    }

    /// Advance to the next visible range or padding range in source order.
    unsafe fn advance(&mut self) {
        if self.in_padding {
            self.in_padding = false;
            if self.tree_is_visible() {
                self.visible_depth += 1;
            } else {
                self.descend(0);
            }
            return;
        }

        loop {
            if self.tree_is_visible() {
                self.visible_depth -= 1;
            }
            let entry = self.cursor.stack.pop();
            if self.done() {
                return;
            }

            let parent = tree_cursor_entry_slice(&self.cursor.stack)
                .last()
                .unwrap_unchecked()
                .subtree;
            let child_index = entry.child_index + 1;
            let last_external_token = subtree_last_external_token(*entry.subtree);
            if !last_external_token.ptr.is_null() {
                self.prev_external_token = last_external_token;
            }
            if subtree_child_count(*parent) > child_index {
                let position = length_add(entry.position, subtree_total_size(*entry.subtree));
                let mut structural_child_index = entry.structural_child_index;
                if !subtree_extra(*entry.subtree) {
                    structural_child_index += 1;
                }
                let next_child = subtree_child(*parent, child_index);

                self.cursor.stack.push(TreeCursorEntry {
                    subtree: core::ptr::from_ref::<Subtree>(next_child),
                    position,
                    child_index,
                    structural_child_index,
                    descendant_index: 0,
                });

                if self.tree_is_visible() {
                    if subtree_padding(*next_child).bytes > 0 {
                        self.in_padding = true;
                    } else {
                        self.visible_depth += 1;
                    }
                } else {
                    self.descend(0);
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
    unsafe fn compare(&self, new_iter: &Self) -> IteratorComparison {
        let old_visible = self.visible_state();
        let new_visible = new_iter.visible_state();
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
            || old_symbol == TS_BUILTIN_SYM_ERROR
            || old_size != new_size
            || old_state == TS_TREE_STATE_NONE
            || new_state == TS_TREE_STATE_NONE
            || (old_state == ERROR_STATE) != (new_state == ERROR_STATE)
            || old_error_cost != new_error_cost
            || old_has_external_tokens != new_has_external_tokens
            || subtree_has_changes(old_tree)
            || (old_has_external_tokens
                && !subtree_external_scanner_state_eq(
                    &self.prev_external_token,
                    &new_iter.prev_external_token,
                ))
        {
            return IteratorComparison::MayDiffer;
        }

        IteratorComparison::Matches
    }
}

// ---------------------------------------------------------------------------
// Range and changed-tree entry points
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
    let range = ptr_mut(range);
    let edit = ptr_ref(edit);

    range_edit_ref(range, edit);
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
    let mut results = array_new();

    let mut old_iter = DiffIterator::new(old_cursor, old_tree, language);
    let mut new_iter = DiffIterator::new(new_cursor, new_tree, language);

    let mut included_range_difference_index: u32 = 0;

    let mut position = old_iter.start_position();
    let mut next_position = new_iter.start_position();
    if position.bytes < next_position.bytes {
        range_array_add(&mut results, position, next_position);
        position = next_position;
    } else if position.bytes > next_position.bytes {
        range_array_add(&mut results, next_position, position);
        next_position = position;
    }

    loop {
        // Compare the old and new subtrees.
        let mut comparison = old_iter.compare(&new_iter);

        // Even if the two subtrees appear to be identical, they could differ
        // internally if they contain a range of text that was previously
        // excluded from the parse, and is now included, or vice-versa.
        if comparison == IteratorComparison::Matches
            && range_array_intersects_ref(
                included_range_differences_array,
                included_range_difference_index,
                position.bytes,
                old_iter.end_position().bytes,
            )
        {
            comparison = IteratorComparison::MayDiffer;
        }

        let is_changed = match comparison {
            // If the subtrees are definitely identical, move to the end
            // of both subtrees.
            IteratorComparison::Matches => {
                next_position = old_iter.end_position();
                false
            }

            // If the subtrees might differ internally, descend into both
            // subtrees, finding the first child that spans the current position.
            IteratorComparison::MayDiffer => {
                if old_iter.descend(position.bytes) {
                    if !new_iter.descend(position.bytes) {
                        next_position = old_iter.end_position();
                        true
                    } else {
                        false
                    }
                } else if new_iter.descend(position.bytes) {
                    next_position = new_iter.end_position();
                    true
                } else {
                    next_position = length_min(old_iter.end_position(), new_iter.end_position());
                    false
                }
            }

            // If the subtrees are different, record a change and then move
            // to the end of both subtrees.
            IteratorComparison::Differs => {
                next_position = length_min(old_iter.end_position(), new_iter.end_position());
                true
            }
        };

        // Ensure that both iterators are caught up to the current position.
        while !old_iter.done() && old_iter.end_position().bytes <= next_position.bytes {
            old_iter.advance();
        }
        while !new_iter.done() && new_iter.end_position().bytes <= next_position.bytes {
            new_iter.advance();
        }

        // Ensure that both iterators are at the same depth in the tree.
        while old_iter.visible_depth > new_iter.visible_depth {
            old_iter.ascend();
        }
        while new_iter.visible_depth > old_iter.visible_depth {
            new_iter.ascend();
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

        if old_iter.done() || new_iter.done() {
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
