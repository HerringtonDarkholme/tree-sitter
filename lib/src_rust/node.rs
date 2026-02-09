#![allow(dead_code)]
#![allow(non_snake_case)]

use std::ptr;

use crate::ffi::{
    TSFieldId, TSInputEdit, TSLanguage, TSNode, TSPoint, TSStateId, TSSymbol,
};

use super::length::{length_add, length_zero, Length};
use super::point::{point_add, point_eq, point_gt, point_lt, point_lte};
use super::subtree::{
    ts_builtin_sym_error, ts_subtree_child_count, ts_subtree_children,
    ts_subtree_error_cost, ts_subtree_extra, ts_subtree_has_changes,
    ts_subtree_missing, ts_subtree_named, ts_subtree_padding,
    ts_subtree_size, ts_subtree_string, ts_subtree_symbol, ts_subtree_total_bytes,
    ts_subtree_visible, ts_subtree_visible_descendant_count,
    Subtree, NULL_SUBTREE, TS_TREE_STATE_NONE,
    TSFieldMapEntry, TSSymbolMetadata,
};
use super::subtree::ts_subtree_parse_state;
use super::language::{ts_language_alias_sequence, ts_language_field_map, TSLanguageFull};
use super::tree::TSTree;

// ---------------------------------------------------------------------------
// Extern C functions (exported from other Rust modules)
// ---------------------------------------------------------------------------

extern "C" {
    // language.rs (exported)
    fn ts_language_symbol_metadata(
        self_: *const TSLanguage,
        symbol: TSSymbol,
    ) -> TSSymbolMetadata;
    fn ts_language_public_symbol(
        self_: *const TSLanguage,
        symbol: TSSymbol,
    ) -> TSSymbol;
    fn ts_language_symbol_name(
        self_: *const TSLanguage,
        symbol: TSSymbol,
    ) -> *const i8;
    fn ts_language_next_state(
        self_: *const TSLanguage,
        state: TSStateId,
        symbol: TSSymbol,
    ) -> TSStateId;
    fn ts_language_field_id_for_name(
        self_: *const TSLanguage,
        name: *const i8,
        name_length: u32,
    ) -> TSFieldId;

    // tree.rs (exported)
    fn ts_tree_root_node(self_: *const crate::ffi::TSTree) -> TSNode;

    // point.rs (exported)
    fn ts_point_edit(
        point: *mut TSPoint,
        byte: *mut u32,
        edit: *const TSInputEdit,
    );
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// NodeChildIterator — internal iterator for walking children
struct NodeChildIterator {
    parent: Subtree,
    tree: *const TSTree,
    position: Length,
    child_index: u32,
    structural_child_index: u32,
    alias_sequence: *const TSSymbol,
}

// ---------------------------------------------------------------------------
// Internal helpers — inline accessors
// ---------------------------------------------------------------------------

#[inline]
unsafe fn ts_node__null() -> TSNode {
    ts_node_new(ptr::null(), ptr::null(), length_zero(), 0)
}

#[inline]
unsafe fn ts_node__alias(self_: &TSNode) -> u32 {
    self_.context[3]
}

#[inline]
unsafe fn ts_node__subtree(self_: TSNode) -> Subtree {
    *(self_.id as *const Subtree)
}

// ---------------------------------------------------------------------------
// Internal helpers — child iteration
// ---------------------------------------------------------------------------

#[inline]
unsafe fn ts_node_iterate_children(node: &TSNode) -> NodeChildIterator {
    let subtree = ts_node__subtree(*node);
    if ts_subtree_child_count(subtree) == 0 {
        return NodeChildIterator {
            parent: NULL_SUBTREE,
            tree: node.tree as *const TSTree,
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            alias_sequence: ptr::null(),
        };
    }
    let tree = node.tree as *const TSTree;
    let alias_sequence = ts_language_alias_sequence(
        (*tree).language,
        (*subtree.ptr).data.children.production_id as u32,
    );
    NodeChildIterator {
        parent: subtree,
        tree: tree,
        position: Length {
            bytes: ts_node_start_byte(*node),
            extent: ts_node_start_point(*node),
        },
        child_index: 0,
        structural_child_index: 0,
        alias_sequence,
    }
}

#[inline]
unsafe fn ts_node_child_iterator_done(self_: &NodeChildIterator) -> bool {
    self_.child_index == (*self_.parent.ptr).child_count
}

unsafe fn ts_node_child_iterator_next(
    self_: &mut NodeChildIterator,
    result: *mut TSNode,
) -> bool {
    if self_.parent.ptr.is_null() || ts_node_child_iterator_done(self_) {
        return false;
    }
    let child = &*ts_subtree_children(self_.parent).add(self_.child_index as usize);
    let mut alias_symbol: TSSymbol = 0;
    if !ts_subtree_extra(*child) {
        if !self_.alias_sequence.is_null() {
            alias_symbol = *self_.alias_sequence.add(self_.structural_child_index as usize);
        }
        self_.structural_child_index += 1;
    }
    if self_.child_index > 0 {
        self_.position = length_add(self_.position, ts_subtree_padding(*child));
    }
    *result = ts_node_new(
        self_.tree,
        child as *const Subtree,
        self_.position,
        alias_symbol,
    );
    self_.position = length_add(self_.position, ts_subtree_size(*child));
    self_.child_index += 1;
    true
}

// ---------------------------------------------------------------------------
// Internal helpers — relevance & child count
// ---------------------------------------------------------------------------

#[inline]
unsafe fn ts_node__is_relevant(self_: TSNode, include_anonymous: bool) -> bool {
    let tree = ts_node__subtree(self_);
    if include_anonymous {
        ts_subtree_visible(tree) || ts_node__alias(&self_) != 0
    } else {
        let alias = ts_node__alias(&self_) as TSSymbol;
        if alias != 0 {
            let t = self_.tree as *const TSTree;
            ts_language_symbol_metadata((*t).language, alias).named
        } else {
            ts_subtree_visible(tree) && ts_subtree_named(tree)
        }
    }
}

#[inline]
unsafe fn ts_node__relevant_child_count(
    self_: TSNode,
    include_anonymous: bool,
) -> u32 {
    let tree = ts_node__subtree(self_);
    if ts_subtree_child_count(tree) > 0 {
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

unsafe fn ts_node__child(
    mut self_: TSNode,
    mut child_index: u32,
    include_anonymous: bool,
) -> TSNode {
    let mut result = self_;
    let mut did_descend = true;

    while did_descend {
        did_descend = false;

        let mut child: TSNode = std::mem::zeroed();
        let mut index: u32 = 0;
        let mut iterator = ts_node_iterate_children(&result);
        while ts_node_child_iterator_next(&mut iterator, &mut child) {
            if ts_node__is_relevant(child, include_anonymous) {
                if index == child_index {
                    return child;
                }
                index += 1;
            } else {
                let grandchild_index = child_index - index;
                let grandchild_count = ts_node__relevant_child_count(child, include_anonymous);
                if grandchild_index < grandchild_count {
                    did_descend = true;
                    result = child;
                    child_index = grandchild_index;
                    break;
                }
                index += grandchild_count;
            }
        }
    }

    ts_node__null()
}

unsafe fn ts_subtree_has_trailing_empty_descendant(
    self_: Subtree,
    other: Subtree,
) -> bool {
    let count = ts_subtree_child_count(self_);
    if count == 0 { return false; }
    let mut i = count - 1;
    loop {
        let child = *ts_subtree_children(self_).add(i as usize);
        if ts_subtree_total_bytes(child) > 0 { break; }
        if child.ptr == other.ptr || ts_subtree_has_trailing_empty_descendant(child, other) {
            return true;
        }
        if i == 0 { break; }
        i -= 1;
    }
    false
}

unsafe fn ts_node__prev_sibling(self_: TSNode, include_anonymous: bool) -> TSNode {
    let self_subtree = ts_node__subtree(self_);
    let self_is_empty = ts_subtree_total_bytes(self_subtree) == 0;
    let target_end_byte = ts_node_end_byte(self_);

    let mut node = ts_node_parent(self_);
    let mut earlier_node = ts_node__null();
    let mut earlier_node_is_relevant = false;

    while !ts_node_is_null(node) {
        let mut earlier_child = ts_node__null();
        let mut earlier_child_is_relevant = false;
        let mut found_child_containing_target = false;

        let mut child: TSNode = std::mem::zeroed();
        let mut iterator = ts_node_iterate_children(&node);
        while ts_node_child_iterator_next(&mut iterator, &mut child) {
            if child.id == self_.id { break; }
            if iterator.position.bytes > target_end_byte {
                found_child_containing_target = true;
                break;
            }

            if iterator.position.bytes == target_end_byte
                && (!self_is_empty
                    || ts_subtree_has_trailing_empty_descendant(
                        ts_node__subtree(child),
                        self_subtree,
                    ))
            {
                found_child_containing_target = true;
                break;
            }

            if ts_node__is_relevant(child, include_anonymous) {
                earlier_child = child;
                earlier_child_is_relevant = true;
            } else if ts_node__relevant_child_count(child, include_anonymous) > 0 {
                earlier_child = child;
                earlier_child_is_relevant = false;
            }
        }

        if found_child_containing_target {
            if !ts_node_is_null(earlier_child) {
                earlier_node = earlier_child;
                earlier_node_is_relevant = earlier_child_is_relevant;
            }
            node = child;
        } else if earlier_child_is_relevant {
            return earlier_child;
        } else if !ts_node_is_null(earlier_child) {
            node = earlier_child;
        } else if earlier_node_is_relevant {
            return earlier_node;
        } else {
            node = earlier_node;
            earlier_node = ts_node__null();
            earlier_node_is_relevant = false;
        }
    }

    ts_node__null()
}

unsafe fn ts_node__next_sibling(self_: TSNode, include_anonymous: bool) -> TSNode {
    let target_end_byte = ts_node_end_byte(self_);

    let mut node = ts_node_parent(self_);
    let mut later_node = ts_node__null();
    let mut later_node_is_relevant = false;

    while !ts_node_is_null(node) {
        let mut later_child = ts_node__null();
        let mut later_child_is_relevant = false;
        let mut child_containing_target = ts_node__null();

        let mut child: TSNode = std::mem::zeroed();
        let mut iterator = ts_node_iterate_children(&node);
        while ts_node_child_iterator_next(&mut iterator, &mut child) {
            if iterator.position.bytes <= target_end_byte { continue; }
            let start_byte = ts_node_start_byte(self_);
            let child_start_byte = ts_node_start_byte(child);

            let is_empty = start_byte == target_end_byte;
            let contains_target = if is_empty {
                child_start_byte < start_byte
            } else {
                child_start_byte <= start_byte
            };

            if contains_target {
                if ts_node__subtree(child).ptr != ts_node__subtree(self_).ptr {
                    child_containing_target = child;
                }
            } else if ts_node__is_relevant(child, include_anonymous) {
                later_child = child;
                later_child_is_relevant = true;
                break;
            } else if ts_node__relevant_child_count(child, include_anonymous) > 0 {
                later_child = child;
                later_child_is_relevant = false;
                break;
            }
        }

        if !ts_node_is_null(child_containing_target) {
            if !ts_node_is_null(later_child) {
                later_node = later_child;
                later_node_is_relevant = later_child_is_relevant;
            }
            node = child_containing_target;
        } else if later_child_is_relevant {
            return later_child;
        } else if !ts_node_is_null(later_child) {
            node = later_child;
        } else if later_node_is_relevant {
            return later_node;
        } else {
            node = later_node;
        }
    }

    ts_node__null()
}

unsafe fn ts_node__first_child_for_byte(
    self_: TSNode,
    goal: u32,
    include_anonymous: bool,
) -> TSNode {
    let mut node = self_;
    let mut did_descend = true;

    let mut last_iterator: Option<NodeChildIterator> = None;

    while did_descend {
        did_descend = false;

        let mut child: TSNode = std::mem::zeroed();
        let mut iterator = ts_node_iterate_children(&node);
        // labeled loop replaces C's "goto loop"
        'outer: loop {
            while ts_node_child_iterator_next(&mut iterator, &mut child) {
                if ts_node_end_byte(child) > goal {
                    if ts_node__is_relevant(child, include_anonymous) {
                        return child;
                    } else if ts_node_child_count(child) > 0 {
                        if iterator.child_index
                            < ts_subtree_child_count(ts_node__subtree(child))
                        {
                            last_iterator = Some(NodeChildIterator {
                                parent: iterator.parent,
                                tree: iterator.tree,
                                position: iterator.position,
                                child_index: iterator.child_index,
                                structural_child_index: iterator.structural_child_index,
                                alias_sequence: iterator.alias_sequence,
                            });
                        }
                        did_descend = true;
                        node = child;
                        break;
                    }
                }
            }

            if !did_descend {
                if let Some(saved) = last_iterator.take() {
                    iterator = saved;
                    continue 'outer;
                }
            }
            break;
        }
    }

    ts_node__null()
}

unsafe fn ts_node__descendant_for_byte_range(
    self_: TSNode,
    range_start: u32,
    range_end: u32,
    include_anonymous: bool,
) -> TSNode {
    if range_start > range_end {
        return ts_node__null();
    }
    let mut node = self_;
    let mut last_visible_node = self_;

    let mut did_descend = true;
    while did_descend {
        did_descend = false;

        let mut child: TSNode = std::mem::zeroed();
        let mut iterator = ts_node_iterate_children(&node);
        while ts_node_child_iterator_next(&mut iterator, &mut child) {
            let node_end = iterator.position.bytes;

            if node_end < range_end { continue; }

            let is_empty = ts_node_start_byte(child) == node_end;
            if if is_empty { node_end < range_start } else { node_end <= range_start } {
                continue;
            }

            if range_start < ts_node_start_byte(child) { break; }

            node = child;
            if ts_node__is_relevant(node, include_anonymous) {
                last_visible_node = node;
            }
            did_descend = true;
            break;
        }
    }

    last_visible_node
}

unsafe fn ts_node__descendant_for_point_range(
    self_: TSNode,
    range_start: TSPoint,
    range_end: TSPoint,
    include_anonymous: bool,
) -> TSNode {
    if point_gt(range_start, range_end) {
        return ts_node__null();
    }
    let mut node = self_;
    let mut last_visible_node = self_;

    let mut did_descend = true;
    while did_descend {
        did_descend = false;

        let mut child: TSNode = std::mem::zeroed();
        let mut iterator = ts_node_iterate_children(&node);
        while ts_node_child_iterator_next(&mut iterator, &mut child) {
            let node_end = iterator.position.extent;

            if point_lt(node_end, range_end) { continue; }

            let is_empty = point_eq(ts_node_start_point(child), node_end);
            if if is_empty {
                point_lt(node_end, range_start)
            } else {
                point_lte(node_end, range_start)
            } {
                continue;
            }

            if point_lt(range_start, ts_node_start_point(child)) { break; }

            node = child;
            if ts_node__is_relevant(node, include_anonymous) {
                last_visible_node = node;
            }
            did_descend = true;
            break;
        }
    }

    last_visible_node
}

#[inline]
unsafe fn ts_node__field_name_from_language(
    self_: TSNode,
    structural_child_index: u32,
) -> *const i8 {
    let tree = self_.tree as *const TSTree;
    let mut field_map: *const TSFieldMapEntry = ptr::null();
    let mut field_map_end: *const TSFieldMapEntry = ptr::null();
    ts_language_field_map(
        (*tree).language,
        (*ts_node__subtree(self_).ptr).data.children.production_id as u32,
        &mut field_map,
        &mut field_map_end,
    );
    let lang = (*tree).language as *const TSLanguageFull;
    while field_map != field_map_end {
        if !(*field_map).inherited && (*field_map).child_index == structural_child_index as u8 {
            return *(*lang).field_names.add((*field_map).field_id as usize);
        }
        field_map = field_map.add(1);
    }
    ptr::null()
}

// ---------------------------------------------------------------------------
// Exported functions — constructors
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_node_new(
    tree: *const TSTree,
    subtree: *const Subtree,
    position: Length,
    alias: TSSymbol,
) -> TSNode {
    TSNode {
        context: [position.bytes, position.extent.row, position.extent.column, alias as u32],
        id: subtree as *const core::ffi::c_void,
        tree: tree as *const crate::ffi::TSTree,
    }
}

// ---------------------------------------------------------------------------
// Exported functions — simple accessors
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_node_start_byte(self_: TSNode) -> u32 {
    self_.context[0]
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_start_point(self_: TSNode) -> TSPoint {
    TSPoint { row: self_.context[1], column: self_.context[2] }
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_end_byte(self_: TSNode) -> u32 {
    ts_node_start_byte(self_) + ts_subtree_size(ts_node__subtree(self_)).bytes
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_end_point(self_: TSNode) -> TSPoint {
    point_add(ts_node_start_point(self_), ts_subtree_size(ts_node__subtree(self_)).extent)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_symbol(self_: TSNode) -> TSSymbol {
    let mut symbol = ts_node__alias(&self_) as TSSymbol;
    if symbol == 0 {
        symbol = ts_subtree_symbol(ts_node__subtree(self_));
    }
    let tree = self_.tree as *const TSTree;
    ts_language_public_symbol((*tree).language, symbol)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_type(self_: TSNode) -> *const i8 {
    let mut symbol = ts_node__alias(&self_) as TSSymbol;
    if symbol == 0 {
        symbol = ts_subtree_symbol(ts_node__subtree(self_));
    }
    let tree = self_.tree as *const TSTree;
    ts_language_symbol_name((*tree).language, symbol)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_language(self_: TSNode) -> *const TSLanguage {
    let tree = self_.tree as *const TSTree;
    (*tree).language
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_grammar_symbol(self_: TSNode) -> TSSymbol {
    ts_subtree_symbol(ts_node__subtree(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_grammar_type(self_: TSNode) -> *const i8 {
    let tree = self_.tree as *const TSTree;
    let symbol = ts_subtree_symbol(ts_node__subtree(self_));
    ts_language_symbol_name((*tree).language, symbol)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_string(self_: TSNode) -> *mut i8 {
    let tree = self_.tree as *const TSTree;
    let alias_symbol = ts_node__alias(&self_) as TSSymbol;
    ts_subtree_string(
        ts_node__subtree(self_),
        alias_symbol,
        ts_language_symbol_metadata((*tree).language, alias_symbol).visible,
        (*tree).language,
        false,
    )
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_eq(self_: TSNode, other: TSNode) -> bool {
    self_.tree == other.tree && self_.id == other.id
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_is_null(self_: TSNode) -> bool {
    self_.id.is_null()
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_is_extra(self_: TSNode) -> bool {
    ts_subtree_extra(ts_node__subtree(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_is_named(self_: TSNode) -> bool {
    let alias = ts_node__alias(&self_) as TSSymbol;
    if alias != 0 {
        let tree = self_.tree as *const TSTree;
        ts_language_symbol_metadata((*tree).language, alias).named
    } else {
        ts_subtree_named(ts_node__subtree(self_))
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_is_missing(self_: TSNode) -> bool {
    ts_subtree_missing(ts_node__subtree(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_has_changes(self_: TSNode) -> bool {
    ts_subtree_has_changes(ts_node__subtree(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_has_error(self_: TSNode) -> bool {
    ts_subtree_error_cost(ts_node__subtree(self_)) > 0
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_is_error(self_: TSNode) -> bool {
    ts_node_symbol(self_) == ts_builtin_sym_error
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_descendant_count(self_: TSNode) -> u32 {
    ts_subtree_visible_descendant_count(ts_node__subtree(self_)) + 1
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_parse_state(self_: TSNode) -> TSStateId {
    ts_subtree_parse_state(ts_node__subtree(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_next_parse_state(self_: TSNode) -> TSStateId {
    let tree = self_.tree as *const TSTree;
    let language = (*tree).language;
    let state = ts_node_parse_state(self_);
    if state == TS_TREE_STATE_NONE {
        return TS_TREE_STATE_NONE;
    }
    let symbol = ts_node_grammar_symbol(self_);
    ts_language_next_state(language, state, symbol)
}

// ---------------------------------------------------------------------------
// Exported functions — child count
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_node_child_count(self_: TSNode) -> u32 {
    let tree = ts_node__subtree(self_);
    if ts_subtree_child_count(tree) > 0 {
        (*tree.ptr).data.children.visible_child_count
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_child_count(self_: TSNode) -> u32 {
    let tree = ts_node__subtree(self_);
    if ts_subtree_child_count(tree) > 0 {
        (*tree.ptr).data.children.named_child_count
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Exported functions — navigation
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_node_parent(self_: TSNode) -> TSNode {
    let tree = self_.tree as *const TSTree;
    let mut node = ts_tree_root_node(tree as *const crate::ffi::TSTree);
    if node.id == self_.id { return ts_node__null(); }

    loop {
        let next_node = ts_node_child_with_descendant(node, self_);
        if next_node.id == self_.id || ts_node_is_null(next_node) { break; }
        node = next_node;
    }

    node
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child_with_descendant(
    mut self_: TSNode,
    descendant: TSNode,
) -> TSNode {
    let start_byte = ts_node_start_byte(descendant);
    let end_byte = ts_node_end_byte(descendant);
    let is_empty = start_byte == end_byte;

    loop {
        let mut iter = ts_node_iterate_children(&self_);
        loop {
            if !ts_node_child_iterator_next(&mut iter, &mut self_)
                || ts_node_start_byte(self_) > start_byte
            {
                return ts_node__null();
            }
            if self_.id == descendant.id {
                return self_;
            }

            // If the descendant is empty, and the end byte is within `self`,
            // we check whether `self` contains it or not.
            if is_empty && iter.position.bytes >= end_byte && ts_node_child_count(self_) > 0 {
                let child = ts_node_child_with_descendant(self_, descendant);
                if !ts_node_is_null(child) {
                    return if ts_node__is_relevant(self_, true) { self_ } else { child };
                }
            }

            if !((if is_empty {
                iter.position.bytes <= end_byte
            } else {
                iter.position.bytes < end_byte
            }) || ts_node_child_count(self_) == 0) {
                break;
            }
        }
        if ts_node__is_relevant(self_, true) {
            break;
        }
    }

    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child(self_: TSNode, child_index: u32) -> TSNode {
    ts_node__child(self_, child_index, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_child(self_: TSNode, child_index: u32) -> TSNode {
    ts_node__child(self_, child_index, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child_by_field_id(
    mut self_: TSNode,
    field_id: TSFieldId,
) -> TSNode {
    // Loop replaces C's "goto recur" tail-call pattern
    'recur: loop {
        if field_id == 0 || ts_node_child_count(self_) == 0 {
            return ts_node__null();
        }

        let tree = self_.tree as *const TSTree;
        let mut field_map: *const TSFieldMapEntry = ptr::null();
        let mut field_map_end: *const TSFieldMapEntry = ptr::null();
        ts_language_field_map(
            (*tree).language,
            (*ts_node__subtree(self_).ptr).data.children.production_id as u32,
            &mut field_map,
            &mut field_map_end,
        );
        if field_map == field_map_end {
            return ts_node__null();
        }

        // Scan to find mappings for the given field id
        while (*field_map).field_id < field_id {
            field_map = field_map.add(1);
            if field_map == field_map_end {
                return ts_node__null();
            }
        }
        while (*field_map_end.sub(1)).field_id > field_id {
            field_map_end = field_map_end.sub(1);
            if field_map == field_map_end {
                return ts_node__null();
            }
        }

        let mut child: TSNode = std::mem::zeroed();
        let mut iterator = ts_node_iterate_children(&self_);
        while ts_node_child_iterator_next(&mut iterator, &mut child) {
            if !ts_subtree_extra(ts_node__subtree(child)) {
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
                    } else {
                        let result = ts_node_child_by_field_id(child, field_id);
                        if !result.id.is_null() {
                            return result;
                        }
                        field_map = field_map.add(1);
                        if field_map == field_map_end {
                            return ts_node__null();
                        }
                    }
                } else if ts_node__is_relevant(child, true) {
                    return child;
                } else if ts_node_child_count(child) > 0 {
                    return ts_node_child(child, 0);
                } else {
                    field_map = field_map.add(1);
                    if field_map == field_map_end {
                        return ts_node__null();
                    }
                }
            }
        }

        return ts_node__null();
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_child_by_field_name(
    self_: TSNode,
    name: *const i8,
    name_length: u32,
) -> TSNode {
    let tree = self_.tree as *const TSTree;
    let field_id = ts_language_field_id_for_name((*tree).language, name, name_length);
    ts_node_child_by_field_id(self_, field_id)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_next_sibling(self_: TSNode) -> TSNode {
    ts_node__next_sibling(self_, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_next_named_sibling(self_: TSNode) -> TSNode {
    ts_node__next_sibling(self_, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_prev_sibling(self_: TSNode) -> TSNode {
    ts_node__prev_sibling(self_, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_prev_named_sibling(self_: TSNode) -> TSNode {
    ts_node__prev_sibling(self_, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_first_child_for_byte(self_: TSNode, byte: u32) -> TSNode {
    ts_node__first_child_for_byte(self_, byte, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_first_named_child_for_byte(self_: TSNode, byte: u32) -> TSNode {
    ts_node__first_child_for_byte(self_, byte, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_descendant_for_byte_range(
    self_: TSNode,
    start: u32,
    end: u32,
) -> TSNode {
    ts_node__descendant_for_byte_range(self_, start, end, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_descendant_for_byte_range(
    self_: TSNode,
    start: u32,
    end: u32,
) -> TSNode {
    ts_node__descendant_for_byte_range(self_, start, end, false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_descendant_for_point_range(
    self_: TSNode,
    start: TSPoint,
    end: TSPoint,
) -> TSNode {
    ts_node__descendant_for_point_range(self_, start, end, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_named_descendant_for_point_range(
    self_: TSNode,
    start: TSPoint,
    end: TSPoint,
) -> TSNode {
    ts_node__descendant_for_point_range(self_, start, end, false)
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
    let mut did_descend = true;
    let mut inherited_field_name: *const i8 = ptr::null();

    while did_descend {
        did_descend = false;

        let mut child: TSNode = std::mem::zeroed();
        let mut index: u32 = 0;
        let mut iterator = ts_node_iterate_children(&result);
        while ts_node_child_iterator_next(&mut iterator, &mut child) {
            if ts_node__is_relevant(child, true) {
                if index == child_index {
                    if ts_node_is_extra(child) {
                        return ptr::null();
                    }
                    let field_name = ts_node__field_name_from_language(
                        result,
                        iterator.structural_child_index - 1,
                    );
                    if !field_name.is_null() {
                        return field_name;
                    }
                    return inherited_field_name;
                }
                index += 1;
            } else {
                let grandchild_index = child_index - index;
                let grandchild_count = ts_node__relevant_child_count(child, true);
                if grandchild_index < grandchild_count {
                    let field_name = ts_node__field_name_from_language(
                        result,
                        iterator.structural_child_index - 1,
                    );
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
    }

    ptr::null()
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_field_name_for_named_child(
    self_: TSNode,
    mut named_child_index: u32,
) -> *const i8 {
    let mut result = self_;
    let mut did_descend = true;
    let mut inherited_field_name: *const i8 = ptr::null();

    while did_descend {
        did_descend = false;

        let mut child: TSNode = std::mem::zeroed();
        let mut index: u32 = 0;
        let mut iterator = ts_node_iterate_children(&result);
        while ts_node_child_iterator_next(&mut iterator, &mut child) {
            if ts_node__is_relevant(child, false) {
                if index == named_child_index {
                    if ts_node_is_extra(child) {
                        return ptr::null();
                    }
                    let field_name = ts_node__field_name_from_language(
                        result,
                        iterator.structural_child_index - 1,
                    );
                    if !field_name.is_null() {
                        return field_name;
                    }
                    return inherited_field_name;
                }
                index += 1;
            } else {
                let named_grandchild_index = named_child_index - index;
                let grandchild_count = ts_node__relevant_child_count(child, false);
                if named_grandchild_index < grandchild_count {
                    let field_name = ts_node__field_name_from_language(
                        result,
                        iterator.structural_child_index - 1,
                    );
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
    }

    ptr::null()
}

// ---------------------------------------------------------------------------
// Exported functions — mutation
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_node_edit(
    self_: *mut TSNode,
    edit: *const TSInputEdit,
) {
    let mut start_byte = ts_node_start_byte(*self_);
    let mut start_point = ts_node_start_point(*self_);

    ts_point_edit(&mut start_point, &mut start_byte, edit);

    (*self_).context[0] = start_byte;
    (*self_).context[1] = start_point.row;
    (*self_).context[2] = start_point.column;
}
