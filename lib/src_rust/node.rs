//! Stateless inspection and navigation for public syntax nodes.
//!
//! A [`TSNode`] is a small borrowed view: it identifies one internal subtree,
//! its owning [`TSTree`], its start position, and any alias applied by the
//! parent production. It does not own or reference-count the subtree, so it is
//! valid only while its tree remains alive.
//!
//! Public children do not map one-to-one to stored children. Hidden grammar
//! nodes are flattened, extras may be handled specially, and aliases and field
//! names come from generated language metadata. The `navigation` child module
//! performs that flattening for child, sibling, and descendant searches;
//! `fields` resolves fields through hidden nodes. This module contains the
//! common representation helpers and exported query functions.

use core::ptr;

use crate::ffi::{TSFieldId, TSInputEdit, TSLanguage, TSNode, TSPoint, TSStateId, TSSymbol};

use super::language::{
    language_alias_sequence_slice, language_field_map_slice, language_full, language_public_symbol,
    ts_language_field_id_for_name, ts_language_next_state, ts_language_symbol_metadata,
    ts_language_symbol_name,
};
use super::length::{length_add, length_zero, Length};
use super::point::{point_add, point_edit, point_eq, point_gt, point_lt, point_lte};
use super::subtree::{
    subtree_string, Subtree, NULL_SUBTREE, TS_BUILTIN_SYM_ERROR, TS_TREE_STATE_NONE,
};
use super::tree::TSTree;
use super::utils::{ptr_mut, ptr_ref};

mod fields;
mod navigation;
use navigation::{
    node_child, node_descendant_for_byte_range, node_descendant_for_point_range,
    node_first_child_for_byte, node_next_sibling, node_prev_sibling,
};

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
    alias_sequence: &'static [TSSymbol],
}

// ---------------------------------------------------------------------------
// Internal helpers — inline accessors
// ---------------------------------------------------------------------------

#[inline]
fn node_null() -> TSNode {
    node_new(ptr::null(), ptr::null(), length_zero(), 0)
}

#[inline]
const fn node_alias(self_: &TSNode) -> u32 {
    self_.context[3]
}

#[inline]
const unsafe fn node_subtree(self_: TSNode) -> Subtree {
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
    let tree = node_subtree(self_);
    if tree.child_count() > 0 {
        tree.heap_data().children().visible_child_count
    } else {
        0
    }
}

#[inline]
const unsafe fn node_named_child_count(self_: TSNode) -> u32 {
    let tree = node_subtree(self_);
    if tree.child_count() > 0 {
        tree.heap_data().children().named_child_count
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
    node_start_byte(self_) + node_subtree(self_).size().bytes
}

#[inline]
unsafe fn node_end_point(self_: TSNode) -> TSPoint {
    point_add(node_start_point(self_), node_subtree(self_).size().extent)
}

#[inline]
unsafe fn node_symbol(self_: TSNode) -> TSSymbol {
    let mut symbol = node_alias(&self_) as TSSymbol;
    if symbol == 0 {
        symbol = node_subtree(self_).symbol();
    }
    language_public_symbol(node_language(self_), symbol)
}

#[inline]
unsafe fn node_type(self_: TSNode) -> *const i8 {
    let mut symbol = node_alias(&self_) as TSSymbol;
    if symbol == 0 {
        symbol = node_subtree(self_).symbol();
    }
    ts_language_symbol_name(node_language(self_), symbol)
}

// ---------------------------------------------------------------------------
// Internal helpers — child iteration
// ---------------------------------------------------------------------------

#[inline]
unsafe fn node_iterate_children(node: &TSNode) -> NodeChildIterator {
    let subtree = node_subtree(*node);
    if subtree.child_count() == 0 {
        return NodeChildIterator {
            parent: NULL_SUBTREE,
            tree: node_tree(*node),
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            alias_sequence: &[],
        };
    }
    let alias_sequence = language_alias_sequence_slice(
        node_language(*node),
        u32::from(subtree.heap_data().children().production_id),
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
    if self_.parent.is_null() || self_.child_index == self_.parent.heap_data().child_count {
        return false;
    }
    let child = (self_.parent).child(self_.child_index);
    let mut alias_symbol: TSSymbol = 0;
    if !(*child).extra() {
        alias_symbol = self_
            .alias_sequence
            .get(self_.structural_child_index as usize)
            .copied()
            .unwrap_or(0);
        self_.structural_child_index += 1;
    }
    if self_.child_index > 0 {
        self_.position = length_add(self_.position, (*child).padding());
    }
    *result = node_new(
        self_.tree,
        core::ptr::from_ref::<Subtree>(child),
        self_.position,
        alias_symbol,
    );
    self_.position = length_add(self_.position, (*child).size());
    self_.child_index += 1;
    true
}

// ---------------------------------------------------------------------------
// Internal helpers — relevance & child count
// ---------------------------------------------------------------------------

#[inline]
const unsafe fn node_is_relevant(self_: TSNode, include_anonymous: bool) -> bool {
    let tree = node_subtree(self_);
    if include_anonymous {
        tree.visible() || node_alias(&self_) != 0
    } else {
        let alias = node_alias(&self_) as TSSymbol;
        if alias != 0 {
            ts_language_symbol_metadata(node_language(self_), alias).named
        } else {
            tree.visible() && tree.named()
        }
    }
}

#[inline]
const unsafe fn node_relevant_child_count(self_: TSNode, include_anonymous: bool) -> u32 {
    let tree = node_subtree(self_);
    if tree.child_count() > 0 {
        if include_anonymous {
            tree.heap_data().children().visible_child_count
        } else {
            tree.heap_data().children().named_child_count
        }
    } else {
        0
    }
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
    node_subtree(self_).symbol()
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_grammar_type(self_: TSNode) -> *const i8 {
    ts_language_symbol_name(node_language(self_), node_subtree(self_).symbol())
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_string(self_: TSNode) -> *mut i8 {
    let alias_symbol = node_alias(&self_) as TSSymbol;
    let language = node_language(self_);
    subtree_string(
        node_subtree(self_),
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
    node_subtree(self_).extra()
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_is_named(self_: TSNode) -> bool {
    let alias = node_alias(&self_) as TSSymbol;
    if alias != 0 {
        ts_language_symbol_metadata(node_language(self_), alias).named
    } else {
        node_subtree(self_).named()
    }
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_is_missing(self_: TSNode) -> bool {
    node_subtree(self_).missing()
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_has_changes(self_: TSNode) -> bool {
    node_subtree(self_).has_changes()
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_has_error(self_: TSNode) -> bool {
    node_subtree(self_).error_cost() > 0
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_is_error(self_: TSNode) -> bool {
    node_symbol(self_) == TS_BUILTIN_SYM_ERROR
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_descendant_count(self_: TSNode) -> u32 {
    node_subtree(self_).visible_descendant_count() + 1
}

#[no_mangle]
pub const unsafe extern "C" fn ts_node_parse_state(self_: TSNode) -> TSStateId {
    node_subtree(self_).parse_state()
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_next_parse_state(self_: TSNode) -> TSStateId {
    let subtree = node_subtree(self_);
    let state = subtree.parse_state();
    if state == TS_TREE_STATE_NONE {
        return TS_TREE_STATE_NONE;
    }
    let symbol = subtree.symbol();
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
    let mut node = ptr_ref(tree).root_node(tree);
    if node.id == self_.id {
        return node_null();
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
                return node_null();
            }
            if self_.id == descendant.id {
                return self_;
            }

            // If the descendant is empty, and the end byte is within `self`,
            // we check whether `self` contains it or not.
            if is_empty && iter.position.bytes >= end_byte && node_child_count(self_) > 0 {
                let child = ts_node_child_with_descendant(self_, descendant);
                if !node_is_null(child) {
                    return if node_is_relevant(self_, true) {
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
        if node_is_relevant(self_, true) {
            break;
        }
    }

    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child(self_: TSNode, child_index: u32) -> TSNode {
    node_child(self_, child_index, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_child(self_: TSNode, child_index: u32) -> TSNode {
    node_child(self_, child_index, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child_by_field_id(
    mut self_: TSNode,
    field_id: TSFieldId,
) -> TSNode {
    // Loop replaces C's "goto recur" tail-call pattern
    'recur: loop {
        if field_id == 0 || node_child_count(self_) == 0 {
            return node_null();
        }

        let field_map = language_field_map_slice(
            node_language(self_),
            u32::from(node_subtree(self_).heap_data().children().production_id),
        );
        if field_map.is_empty() {
            return node_null();
        }

        // Scan to find mappings for the given field id
        let mut field_map_start = 0;
        while field_map_start < field_map.len() && field_map[field_map_start].field_id < field_id {
            field_map_start += 1;
        }
        let mut field_map_end = field_map.len();
        while field_map_end > field_map_start && field_map[field_map_end - 1].field_id > field_id {
            field_map_end -= 1;
        }
        if field_map_start == field_map_end {
            return node_null();
        }

        let mut child = node_null();
        let mut iterator = node_iterate_children(&self_);
        while node_child_iterator_next(&mut iterator, &mut child) {
            if !node_subtree(child).extra() {
                let index = iterator.structural_child_index - 1;
                if (index as u8) < field_map[field_map_start].child_index {
                    continue;
                }

                if field_map[field_map_start].inherited {
                    // If this is the *last* possible child node for this field,
                    // then perform a tail call (loop iteration)
                    if field_map_start + 1 == field_map_end {
                        self_ = child;
                        continue 'recur;
                    }
                    let result = ts_node_child_by_field_id(child, field_id);
                    if !result.id.is_null() {
                        return result;
                    }
                    field_map_start += 1;
                    if field_map_start == field_map_end {
                        return node_null();
                    }
                } else if node_is_relevant(child, true) {
                    return child;
                } else if node_child_count(child) > 0 {
                    return node_child(child, 0, true);
                }
                field_map_start += 1;
                if field_map_start == field_map_end {
                    return node_null();
                }
            }
        }

        return node_null();
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
    node_next_sibling(self_, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_next_named_sibling(self_: TSNode) -> TSNode {
    node_next_sibling(self_, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_prev_sibling(self_: TSNode) -> TSNode {
    node_prev_sibling(self_, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_prev_named_sibling(self_: TSNode) -> TSNode {
    node_prev_sibling(self_, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_first_child_for_byte(self_: TSNode, byte: u32) -> TSNode {
    node_first_child_for_byte(self_, byte, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_first_named_child_for_byte(self_: TSNode, byte: u32) -> TSNode {
    node_first_child_for_byte(self_, byte, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_descendant_for_byte_range(
    self_: TSNode,
    start: u32,
    end: u32,
) -> TSNode {
    node_descendant_for_byte_range(self_, start, end, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_descendant_for_byte_range(
    self_: TSNode,
    start: u32,
    end: u32,
) -> TSNode {
    node_descendant_for_byte_range(self_, start, end, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_descendant_for_point_range(
    self_: TSNode,
    start: TSPoint,
    end: TSPoint,
) -> TSNode {
    node_descendant_for_point_range(self_, start, end, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_descendant_for_point_range(
    self_: TSNode,
    start: TSPoint,
    end: TSPoint,
) -> TSNode {
    node_descendant_for_point_range(self_, start, end, false)
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
