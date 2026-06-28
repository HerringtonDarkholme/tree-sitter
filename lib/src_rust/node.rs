#![allow(non_snake_case)]

use core::ptr;

use crate::ffi::{TSFieldId, TSInputEdit, TSLanguage, TSNode, TSPoint, TSStateId, TSSymbol};

use super::language::{
    language_alias_sequence, language_field_map, language_full, language_public_symbol,
    ts_language_field_id_for_name, ts_language_next_state, ts_language_symbol_metadata,
    ts_language_symbol_name,
};
use super::length::{length_add, length_zero, Length};
use super::point::{point_add, point_edit, point_eq, point_gt, point_lt, point_lte};
use super::subtree::subtree_parse_state;
use super::subtree::{
    subtree_child, subtree_child_count, subtree_error_cost, subtree_extra, subtree_has_changes,
    subtree_missing, subtree_named, subtree_padding, subtree_size, subtree_string, subtree_symbol,
    subtree_total_bytes, subtree_visible, subtree_visible_descendant_count, ts_builtin_sym_error,
    Subtree, TSFieldMapEntry, NULL_SUBTREE, TS_TREE_STATE_NONE,
};
use super::tree::{tree_root_node_ref, TSTree};
use super::utils::{ptr_mut, ptr_ref};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Internal child iterator for public `TSNode` navigation.
///
/// This iterator walks raw subtree children while tracking source position,
/// structural child index, and aliases. Public node APIs then decide whether to
/// expose each child or descend through hidden nodes.
struct NodeChildIterator {
    /// Parent subtree whose children are being scanned.
    parent: Subtree,
    /// Owning tree for language metadata and node construction.
    tree: *const TSTree,
    /// Start position of the next child.
    position: Length,
    /// Raw child index, including hidden and extra children.
    child_index: u32,
    /// Index among non-extra children, used for fields and aliases.
    structural_child_index: u32,
    /// Alias symbols for the parent production.
    alias_sequence: *const TSSymbol,
}

// ---------------------------------------------------------------------------
// Internal helpers — inline accessors
// ---------------------------------------------------------------------------

#[inline]
fn node__null() -> TSNode {
    node_new(ptr::null(), ptr::null(), length_zero(), 0)
}

#[inline]
const fn node__alias(self_: &TSNode) -> u32 {
    self_.context[3]
}

#[inline]
const unsafe fn node__subtree(self_: TSNode) -> Subtree {
    *self_.id.cast::<Subtree>()
}

#[inline]
const fn node_tree(self_: TSNode) -> *const TSTree {
    self_.tree.cast::<TSTree>()
}

#[inline]
const unsafe fn node_language(self_: TSNode) -> *const TSLanguage {
    (*node_tree(self_)).language
}

#[inline]
fn node_is_null(self_: TSNode) -> bool {
    self_.id.is_null()
}

#[inline]
const unsafe fn node_child_count(self_: TSNode) -> u32 {
    let tree = node__subtree(self_);
    if subtree_child_count(tree) > 0 {
        (*tree.ptr).data.children.visible_child_count
    } else {
        0
    }
}

#[inline]
const unsafe fn node_named_child_count(self_: TSNode) -> u32 {
    let tree = node__subtree(self_);
    if subtree_child_count(tree) > 0 {
        (*tree.ptr).data.children.named_child_count
    } else {
        0
    }
}

#[inline]
const fn node_start_byte(self_: TSNode) -> u32 {
    self_.context[0]
}

#[inline]
const fn node_start_point(self_: TSNode) -> TSPoint {
    TSPoint {
        row: self_.context[1],
        column: self_.context[2],
    }
}

#[inline]
unsafe fn node_end_byte(self_: TSNode) -> u32 {
    node_start_byte(self_) + subtree_size(node__subtree(self_)).bytes
}

#[inline]
unsafe fn node_end_point(self_: TSNode) -> TSPoint {
    point_add(
        node_start_point(self_),
        subtree_size(node__subtree(self_)).extent,
    )
}

#[inline]
unsafe fn node_symbol(self_: TSNode) -> TSSymbol {
    let mut symbol = node__alias(&self_) as TSSymbol;
    if symbol == 0 {
        symbol = subtree_symbol(node__subtree(self_));
    }
    language_public_symbol(node_language(self_), symbol)
}

#[inline]
unsafe fn node_type(self_: TSNode) -> *const i8 {
    let mut symbol = node__alias(&self_) as TSSymbol;
    if symbol == 0 {
        symbol = subtree_symbol(node__subtree(self_));
    }
    ts_language_symbol_name(node_language(self_), symbol)
}

// ---------------------------------------------------------------------------
// Internal helpers — child iteration
// ---------------------------------------------------------------------------

#[inline]
unsafe fn node_iterate_children(node: &TSNode) -> NodeChildIterator {
    let subtree = node__subtree(*node);
    if subtree_child_count(subtree) == 0 {
        return NodeChildIterator {
            parent: NULL_SUBTREE,
            tree: node_tree(*node),
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            alias_sequence: ptr::null(),
        };
    }
    let alias_sequence = language_alias_sequence(
        node_language(*node),
        u32::from((*subtree.ptr).data.children.production_id),
    );
    NodeChildIterator {
        parent: subtree,
        tree: node_tree(*node),
        position: Length {
            bytes: node_start_byte(*node),
            extent: node_start_point(*node),
        },
        child_index: 0,
        structural_child_index: 0,
        alias_sequence,
    }
}

/// Advance the child iterator and construct a `TSNode` for the raw child.
///
/// The iterator applies padding before each non-first child, resolves aliases
/// from the production's alias sequence, and leaves `position` at the child's
/// end after returning.
unsafe fn node_child_iterator_next(self_: &mut NodeChildIterator, result: &mut TSNode) -> bool {
    if self_.parent.ptr.is_null() || self_.child_index == (*self_.parent.ptr).child_count {
        return false;
    }
    let child = subtree_child(self_.parent, self_.child_index);
    let mut alias_symbol: TSSymbol = 0;
    if !subtree_extra(*child) {
        if !self_.alias_sequence.is_null() {
            alias_symbol = *self_
                .alias_sequence
                .add(self_.structural_child_index as usize);
        }
        self_.structural_child_index += 1;
    }
    if self_.child_index > 0 {
        self_.position = length_add(self_.position, subtree_padding(*child));
    }
    *result = node_new(
        self_.tree,
        core::ptr::from_ref::<Subtree>(child),
        self_.position,
        alias_symbol,
    );
    self_.position = length_add(self_.position, subtree_size(*child));
    self_.child_index += 1;
    true
}

// ---------------------------------------------------------------------------
// Internal helpers — relevance & child count
// ---------------------------------------------------------------------------

#[inline]
const unsafe fn node__is_relevant(self_: TSNode, include_anonymous: bool) -> bool {
    let tree = node__subtree(self_);
    if include_anonymous {
        subtree_visible(tree) || node__alias(&self_) != 0
    } else {
        let alias = node__alias(&self_) as TSSymbol;
        if alias != 0 {
            ts_language_symbol_metadata(node_language(self_), alias).named
        } else {
            subtree_visible(tree) && subtree_named(tree)
        }
    }
}

#[inline]
const unsafe fn node__relevant_child_count(self_: TSNode, include_anonymous: bool) -> u32 {
    let tree = node__subtree(self_);
    if subtree_child_count(tree) > 0 {
        if include_anonymous {
            (*tree.ptr).data.children.visible_child_count
        } else {
            (*tree.ptr).data.children.named_child_count
        }
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — navigation
// ---------------------------------------------------------------------------

unsafe fn node__child(self_: TSNode, mut child_index: u32, include_anonymous: bool) -> TSNode {
    let mut result = self_;

    loop {
        let mut did_descend = false;

        let mut child = node__null();
        let mut index: u32 = 0;
        let mut iterator = node_iterate_children(&result);
        while node_child_iterator_next(&mut iterator, &mut child) {
            if node__is_relevant(child, include_anonymous) {
                if index == child_index {
                    return child;
                }
                index += 1;
            } else {
                let grandchild_index = child_index - index;
                let grandchild_count = node__relevant_child_count(child, include_anonymous);
                if grandchild_index < grandchild_count {
                    did_descend = true;
                    result = child;
                    child_index = grandchild_index;
                    break;
                }
                index += grandchild_count;
            }
        }
        if !did_descend {
            break;
        }
    }

    node__null()
}

/// Check whether an empty descendant at the end of a subtree aliases `other`.
///
/// Empty nodes make sibling navigation ambiguous because multiple nodes can end
/// at the same byte. This recursive check lets previous-sibling logic decide
/// whether an equal end byte means "inside this child" or "before this child".
unsafe fn subtree_has_trailing_empty_descendant(self_: Subtree, other: Subtree) -> bool {
    let count = subtree_child_count(self_);
    if count == 0 {
        return false;
    }
    let mut i = count - 1;
    loop {
        let child = *subtree_child(self_, i);
        if subtree_total_bytes(child) > 0 {
            break;
        }
        if child.ptr == other.ptr || subtree_has_trailing_empty_descendant(child, other) {
            return true;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    false
}

/// Find the previous visible/named sibling.
///
/// The search walks upward through parents and keeps the nearest earlier
/// relevant candidate. Hidden nodes with relevant descendants are entered so
/// sibling APIs skip implementation-only nodes while preserving source order.
unsafe fn node__prev_sibling(self_: TSNode, include_anonymous: bool) -> TSNode {
    let self_subtree = node__subtree(self_);
    let self_is_empty = subtree_total_bytes(self_subtree) == 0;
    let target_end_byte = node_end_byte(self_);

    let mut node = ts_node_parent(self_);
    let mut earlier_node = node__null();
    let mut earlier_node_is_relevant = false;

    while !node_is_null(node) {
        let mut earlier_child = node__null();
        let mut earlier_child_is_relevant = false;
        let mut found_child_containing_target = false;

        let mut child = node__null();
        let mut iterator = node_iterate_children(&node);
        while node_child_iterator_next(&mut iterator, &mut child) {
            if child.id == self_.id {
                break;
            }
            if iterator.position.bytes > target_end_byte {
                found_child_containing_target = true;
                break;
            }

            if iterator.position.bytes == target_end_byte
                && (!self_is_empty
                    || subtree_has_trailing_empty_descendant(node__subtree(child), self_subtree))
            {
                found_child_containing_target = true;
                break;
            }

            if node__is_relevant(child, include_anonymous) {
                earlier_child = child;
                earlier_child_is_relevant = true;
            } else if node__relevant_child_count(child, include_anonymous) > 0 {
                earlier_child = child;
                earlier_child_is_relevant = false;
            }
        }

        if found_child_containing_target {
            if !node_is_null(earlier_child) {
                earlier_node = earlier_child;
                earlier_node_is_relevant = earlier_child_is_relevant;
            }
            node = child;
        } else if earlier_child_is_relevant {
            return earlier_child;
        } else if !node_is_null(earlier_child) {
            node = earlier_child;
        } else if earlier_node_is_relevant {
            return earlier_node;
        } else {
            node = earlier_node;
            earlier_node = node__null();
            earlier_node_is_relevant = false;
        }
    }

    node__null()
}

/// Find the next visible/named sibling.
///
/// This mirrors `node__prev_sibling`, but tracks the nearest later candidate
/// while walking through hidden nodes that contain the original target.
unsafe fn node__next_sibling(self_: TSNode, include_anonymous: bool) -> TSNode {
    let target_end_byte = node_end_byte(self_);

    let mut node = ts_node_parent(self_);
    let mut later_node = node__null();
    let mut later_node_is_relevant = false;

    while !node_is_null(node) {
        let mut later_child = node__null();
        let mut later_child_is_relevant = false;
        let mut child_containing_target = node__null();

        let mut child = node__null();
        let mut iterator = node_iterate_children(&node);
        while node_child_iterator_next(&mut iterator, &mut child) {
            if iterator.position.bytes <= target_end_byte {
                continue;
            }
            let start_byte = node_start_byte(self_);
            let child_start_byte = node_start_byte(child);

            let is_empty = start_byte == target_end_byte;
            let contains_target = if is_empty {
                child_start_byte < start_byte
            } else {
                child_start_byte <= start_byte
            };

            if contains_target {
                if node__subtree(child).ptr != node__subtree(self_).ptr {
                    child_containing_target = child;
                }
            } else if node__is_relevant(child, include_anonymous) {
                later_child = child;
                later_child_is_relevant = true;
                break;
            } else if node__relevant_child_count(child, include_anonymous) > 0 {
                later_child = child;
                later_child_is_relevant = false;
                break;
            }
        }

        if !node_is_null(child_containing_target) {
            if !node_is_null(later_child) {
                later_node = later_child;
                later_node_is_relevant = later_child_is_relevant;
            }
            node = child_containing_target;
        } else if later_child_is_relevant {
            return later_child;
        } else if !node_is_null(later_child) {
            node = later_child;
        } else if later_node_is_relevant {
            return later_node;
        } else {
            node = later_node;
        }
    }

    node__null()
}

/// Find the first visible/named child whose end byte is after `goal`.
///
/// Hidden children are searched recursively. A saved iterator lets the search
/// resume at the original depth after exploring a hidden child that did not
/// produce a match.
unsafe fn node__first_child_for_byte(self_: TSNode, goal: u32, include_anonymous: bool) -> TSNode {
    let mut node = self_;

    let mut resume_iterator: Option<NodeChildIterator> = None;

    loop {
        let mut did_descend = false;

        let mut child = node__null();
        let mut iterator = node_iterate_children(&node);
        'resume_sibling_scan: loop {
            while node_child_iterator_next(&mut iterator, &mut child) {
                if node_end_byte(child) > goal {
                    if node__is_relevant(child, include_anonymous) {
                        return child;
                    } else if node_child_count(child) > 0 {
                        if iterator.child_index < subtree_child_count(node__subtree(child)) {
                            resume_iterator = Some(iterator);
                        }
                        did_descend = true;
                        node = child;
                        break;
                    }
                }
            }

            if !did_descend {
                if let Some(saved) = resume_iterator.take() {
                    iterator = saved;
                    continue 'resume_sibling_scan;
                }
            }
            break;
        }
        if !did_descend {
            break;
        }
    }

    node__null()
}

/// Find the smallest visible/named descendant covering a byte range.
///
/// The search descends while a child fully contains the target range and keeps
/// the last relevant node seen, so hidden implementation nodes are skipped in
/// the returned result.
unsafe fn node__descendant_for_byte_range(
    self_: TSNode,
    range_start: u32,
    range_end: u32,
    include_anonymous: bool,
) -> TSNode {
    if range_start > range_end {
        return node__null();
    }
    let mut node = self_;
    let mut last_visible_node = self_;

    loop {
        let mut did_descend = false;

        let mut child = node__null();
        let mut iterator = node_iterate_children(&node);
        while node_child_iterator_next(&mut iterator, &mut child) {
            let node_end = iterator.position.bytes;

            if node_end < range_end {
                continue;
            }

            let is_empty = node_start_byte(child) == node_end;
            if if is_empty {
                node_end < range_start
            } else {
                node_end <= range_start
            } {
                continue;
            }

            if range_start < node_start_byte(child) {
                break;
            }

            node = child;
            if node__is_relevant(node, include_anonymous) {
                last_visible_node = node;
            }
            did_descend = true;
            break;
        }
        if !did_descend {
            break;
        }
    }

    last_visible_node
}

/// Point-coordinate variant of `node__descendant_for_byte_range`.
unsafe fn node__descendant_for_point_range(
    self_: TSNode,
    range_start: TSPoint,
    range_end: TSPoint,
    include_anonymous: bool,
) -> TSNode {
    if point_gt(range_start, range_end) {
        return node__null();
    }
    let mut node = self_;
    let mut last_visible_node = self_;

    loop {
        let mut did_descend = false;

        let mut child = node__null();
        let mut iterator = node_iterate_children(&node);
        while node_child_iterator_next(&mut iterator, &mut child) {
            let node_end = iterator.position.extent;

            if point_lt(node_end, range_end) {
                continue;
            }

            let is_empty = point_eq(node_start_point(child), node_end);
            if if is_empty {
                point_lt(node_end, range_start)
            } else {
                point_lte(node_end, range_start)
            } {
                continue;
            }

            if point_lt(range_start, node_start_point(child)) {
                break;
            }

            node = child;
            if node__is_relevant(node, include_anonymous) {
                last_visible_node = node;
            }
            did_descend = true;
            break;
        }
        if !did_descend {
            break;
        }
    }

    last_visible_node
}

#[inline]
unsafe fn node__field_name_from_language(self_: TSNode, structural_child_index: u32) -> *const i8 {
    let mut field_map: *const TSFieldMapEntry = ptr::null();
    let mut field_map_end: *const TSFieldMapEntry = ptr::null();
    language_field_map(
        node_language(self_),
        u32::from((*node__subtree(self_).ptr).data.children.production_id),
        &mut field_map,
        &mut field_map_end,
    );
    let lang = language_full(node_language(self_));
    while field_map != field_map_end {
        if !(*field_map).inherited && (*field_map).child_index == structural_child_index as u8 {
            return *lang.field_names.add((*field_map).field_id as usize);
        }
        field_map = field_map.add(1);
    }
    ptr::null()
}

// ---------------------------------------------------------------------------
// Internal constructors
// ---------------------------------------------------------------------------

pub fn node_new(
    tree: *const TSTree,
    subtree: *const Subtree,
    position: Length,
    alias: TSSymbol,
) -> TSNode {
    TSNode {
        context: [
            position.bytes,
            position.extent.row,
            position.extent.column,
            u32::from(alias),
        ],
        id: subtree.cast::<core::ffi::c_void>(),
        tree: tree.cast::<crate::ffi::TSTree>(),
    }
}

// ---------------------------------------------------------------------------
// Exported functions — simple accessors
// ---------------------------------------------------------------------------

#[no_mangle]
pub const unsafe extern "C" fn ts_node_start_byte(self_: TSNode) -> u32 {
    node_start_byte(self_)
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_start_point(self_: TSNode) -> TSPoint {
    node_start_point(self_)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_end_byte(self_: TSNode) -> u32 {
    node_end_byte(self_)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_end_point(self_: TSNode) -> TSPoint {
    node_end_point(self_)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_symbol(self_: TSNode) -> TSSymbol {
    node_symbol(self_)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_type(self_: TSNode) -> *const i8 {
    node_type(self_)
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_language(self_: TSNode) -> *const TSLanguage {
    node_language(self_)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_grammar_symbol(self_: TSNode) -> TSSymbol {
    subtree_symbol(node__subtree(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_grammar_type(self_: TSNode) -> *const i8 {
    ts_language_symbol_name(node_language(self_), subtree_symbol(node__subtree(self_)))
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_string(self_: TSNode) -> *mut i8 {
    let alias_symbol = node__alias(&self_) as TSSymbol;
    let language = node_language(self_);
    subtree_string(
        node__subtree(self_),
        alias_symbol,
        ts_language_symbol_metadata(language, alias_symbol).visible,
        language,
        false,
    )
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_eq(self_: TSNode, other: TSNode) -> bool {
    self_.tree == other.tree && self_.id == other.id
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_is_null(self_: TSNode) -> bool {
    node_is_null(self_)
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_is_extra(self_: TSNode) -> bool {
    subtree_extra(node__subtree(self_))
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_is_named(self_: TSNode) -> bool {
    let alias = node__alias(&self_) as TSSymbol;
    if alias != 0 {
        ts_language_symbol_metadata(node_language(self_), alias).named
    } else {
        subtree_named(node__subtree(self_))
    }
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_is_missing(self_: TSNode) -> bool {
    subtree_missing(node__subtree(self_))
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_has_changes(self_: TSNode) -> bool {
    subtree_has_changes(node__subtree(self_))
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_has_error(self_: TSNode) -> bool {
    subtree_error_cost(node__subtree(self_)) > 0
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_is_error(self_: TSNode) -> bool {
    node_symbol(self_) == ts_builtin_sym_error
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_descendant_count(self_: TSNode) -> u32 {
    subtree_visible_descendant_count(node__subtree(self_)) + 1
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_parse_state(self_: TSNode) -> TSStateId {
    subtree_parse_state(node__subtree(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_next_parse_state(self_: TSNode) -> TSStateId {
    let subtree = node__subtree(self_);
    let state = subtree_parse_state(subtree);
    if state == TS_TREE_STATE_NONE {
        return TS_TREE_STATE_NONE;
    }
    let symbol = subtree_symbol(subtree);
    ts_language_next_state(node_language(self_), state, symbol)
}

// ---------------------------------------------------------------------------
// Exported functions — child count
// ---------------------------------------------------------------------------

#[no_mangle]
pub const unsafe extern "C" fn ts_node_child_count(self_: TSNode) -> u32 {
    node_child_count(self_)
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_named_child_count(self_: TSNode) -> u32 {
    node_named_child_count(self_)
}

// ---------------------------------------------------------------------------
// Exported functions — navigation
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_node_parent(self_: TSNode) -> TSNode {
    let tree = node_tree(self_);
    let mut node = tree_root_node_ref(tree, ptr_ref(tree));
    if node.id == self_.id {
        return node__null();
    }

    loop {
        let next_node = ts_node_child_with_descendant(node, self_);
        if next_node.id == self_.id || node_is_null(next_node) {
            break;
        }
        node = next_node;
    }

    node
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child_with_descendant(
    mut self_: TSNode,
    descendant: TSNode,
) -> TSNode {
    let start_byte = node_start_byte(descendant);
    let end_byte = node_end_byte(descendant);
    let is_empty = start_byte == end_byte;

    loop {
        let mut iter = node_iterate_children(&self_);
        loop {
            if !node_child_iterator_next(&mut iter, &mut self_)
                || node_start_byte(self_) > start_byte
            {
                return node__null();
            }
            if self_.id == descendant.id {
                return self_;
            }

            // If the descendant is empty, and the end byte is within `self`,
            // we check whether `self` contains it or not.
            if is_empty && iter.position.bytes >= end_byte && node_child_count(self_) > 0 {
                let child = ts_node_child_with_descendant(self_, descendant);
                if !node_is_null(child) {
                    return if node__is_relevant(self_, true) {
                        self_
                    } else {
                        child
                    };
                }
            }

            if !((if is_empty {
                iter.position.bytes <= end_byte
            } else {
                iter.position.bytes < end_byte
            }) || node_child_count(self_) == 0)
            {
                break;
            }
        }
        if node__is_relevant(self_, true) {
            break;
        }
    }

    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child(self_: TSNode, child_index: u32) -> TSNode {
    node__child(self_, child_index, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_child(self_: TSNode, child_index: u32) -> TSNode {
    node__child(self_, child_index, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child_by_field_id(
    mut self_: TSNode,
    field_id: TSFieldId,
) -> TSNode {
    // Loop replaces C's "goto recur" tail-call pattern
    'recur: loop {
        if field_id == 0 || node_child_count(self_) == 0 {
            return node__null();
        }

        let mut field_map: *const TSFieldMapEntry = ptr::null();
        let mut field_map_end: *const TSFieldMapEntry = ptr::null();
        language_field_map(
            node_language(self_),
            u32::from((*node__subtree(self_).ptr).data.children.production_id),
            &mut field_map,
            &mut field_map_end,
        );
        if field_map == field_map_end {
            return node__null();
        }

        // Scan to find mappings for the given field id
        while (*field_map).field_id < field_id {
            field_map = field_map.add(1);
            if field_map == field_map_end {
                return node__null();
            }
        }
        while (*field_map_end.sub(1)).field_id > field_id {
            field_map_end = field_map_end.sub(1);
            if field_map == field_map_end {
                return node__null();
            }
        }

        let mut child = node__null();
        let mut iterator = node_iterate_children(&self_);
        while node_child_iterator_next(&mut iterator, &mut child) {
            if !subtree_extra(node__subtree(child)) {
                let index = iterator.structural_child_index - 1;
                if (index as u8) < (*field_map).child_index {
                    continue;
                }

                if (*field_map).inherited {
                    // If this is the *last* possible child node for this field,
                    // then perform a tail call (loop iteration)
                    if field_map.add(1) == field_map_end {
                        self_ = child;
                        continue 'recur;
                    }
                    let result = ts_node_child_by_field_id(child, field_id);
                    if !result.id.is_null() {
                        return result;
                    }
                    field_map = field_map.add(1);
                    if field_map == field_map_end {
                        return node__null();
                    }
                } else if node__is_relevant(child, true) {
                    return child;
                } else if node_child_count(child) > 0 {
                    return node__child(child, 0, true);
                }
                field_map = field_map.add(1);
                if field_map == field_map_end {
                    return node__null();
                }
            }
        }

        return node__null();
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child_by_field_name(
    self_: TSNode,
    name: *const i8,
    name_length: u32,
) -> TSNode {
    let field_id = ts_language_field_id_for_name(node_language(self_), name, name_length);
    ts_node_child_by_field_id(self_, field_id)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_next_sibling(self_: TSNode) -> TSNode {
    node__next_sibling(self_, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_next_named_sibling(self_: TSNode) -> TSNode {
    node__next_sibling(self_, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_prev_sibling(self_: TSNode) -> TSNode {
    node__prev_sibling(self_, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_prev_named_sibling(self_: TSNode) -> TSNode {
    node__prev_sibling(self_, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_first_child_for_byte(self_: TSNode, byte: u32) -> TSNode {
    node__first_child_for_byte(self_, byte, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_first_named_child_for_byte(self_: TSNode, byte: u32) -> TSNode {
    node__first_child_for_byte(self_, byte, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_descendant_for_byte_range(
    self_: TSNode,
    start: u32,
    end: u32,
) -> TSNode {
    node__descendant_for_byte_range(self_, start, end, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_descendant_for_byte_range(
    self_: TSNode,
    start: u32,
    end: u32,
) -> TSNode {
    node__descendant_for_byte_range(self_, start, end, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_descendant_for_point_range(
    self_: TSNode,
    start: TSPoint,
    end: TSPoint,
) -> TSNode {
    node__descendant_for_point_range(self_, start, end, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_descendant_for_point_range(
    self_: TSNode,
    start: TSPoint,
    end: TSPoint,
) -> TSNode {
    node__descendant_for_point_range(self_, start, end, false)
}

// ---------------------------------------------------------------------------
// Exported functions — field name accessors
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_node_field_name_for_child(
    self_: TSNode,
    mut child_index: u32,
) -> *const i8 {
    let mut result = self_;
    let mut inherited_field_name: *const i8 = ptr::null();

    loop {
        let mut did_descend = false;

        let mut child = node__null();
        let mut index: u32 = 0;
        let mut iterator = node_iterate_children(&result);
        while node_child_iterator_next(&mut iterator, &mut child) {
            if node__is_relevant(child, true) {
                if index == child_index {
                    if subtree_extra(node__subtree(child)) {
                        return ptr::null();
                    }
                    let field_name =
                        node__field_name_from_language(result, iterator.structural_child_index - 1);
                    if !field_name.is_null() {
                        return field_name;
                    }
                    return inherited_field_name;
                }
                index += 1;
            } else {
                let grandchild_index = child_index - index;
                let grandchild_count = node__relevant_child_count(child, true);
                if grandchild_index < grandchild_count {
                    let field_name =
                        node__field_name_from_language(result, iterator.structural_child_index - 1);
                    if !field_name.is_null() {
                        inherited_field_name = field_name;
                    }
                    did_descend = true;
                    result = child;
                    child_index = grandchild_index;
                    break;
                }
                index += grandchild_count;
            }
        }
        if !did_descend {
            break;
        }
    }

    ptr::null()
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_field_name_for_named_child(
    self_: TSNode,
    mut named_child_index: u32,
) -> *const i8 {
    let mut result = self_;
    let mut inherited_field_name: *const i8 = ptr::null();

    loop {
        let mut did_descend = false;

        let mut child = node__null();
        let mut index: u32 = 0;
        let mut iterator = node_iterate_children(&result);
        while node_child_iterator_next(&mut iterator, &mut child) {
            if node__is_relevant(child, false) {
                if index == named_child_index {
                    if subtree_extra(node__subtree(child)) {
                        return ptr::null();
                    }
                    let field_name =
                        node__field_name_from_language(result, iterator.structural_child_index - 1);
                    if !field_name.is_null() {
                        return field_name;
                    }
                    return inherited_field_name;
                }
                index += 1;
            } else {
                let named_grandchild_index = named_child_index - index;
                let grandchild_count = node__relevant_child_count(child, false);
                if named_grandchild_index < grandchild_count {
                    let field_name =
                        node__field_name_from_language(result, iterator.structural_child_index - 1);
                    if !field_name.is_null() {
                        inherited_field_name = field_name;
                    }
                    did_descend = true;
                    result = child;
                    named_child_index = named_grandchild_index;
                    break;
                }
                index += grandchild_count;
            }
        }
        if !did_descend {
            break;
        }
    }

    ptr::null()
}

// ---------------------------------------------------------------------------
// Exported functions — mutation
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_node_edit(self_: *mut TSNode, edit: *const TSInputEdit) {
    let self_ = ptr_mut(self_);
    let edit = ptr_ref(edit);
    let mut start_byte = node_start_byte(*self_);
    let mut start_point = node_start_point(*self_);

    point_edit(&mut start_point, &mut start_byte, edit);

    self_.context[0] = start_byte;
    self_.context[1] = start_point.row;
    self_.context[2] = start_point.column;
}
