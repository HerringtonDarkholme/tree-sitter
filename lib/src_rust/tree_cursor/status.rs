use core::ptr;

use crate::ffi::{TSFieldId, TSLanguage, TSNode, TSSymbol, TSTreeCursor};

use super::super::language::{language_field_map, language_full};
use super::super::subtree::TSFieldMapEntry;
use super::{
    cursor_ref, language_alias_at, node_new, out_param_mut, subtree_child, subtree_extra,
    subtree_symbol, subtree_visible_child_count, tree_cursor_entry_slice,
    tree_cursor_is_entry_visible, ts_language_symbol_metadata, Subtree, TreeCursorEntry,
};

// ---------------------------------------------------------------------------
// Node info & copy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_node(self_: *const TSTreeCursor) -> TSNode {
    let cursor = cursor_ref(self_);
    let entries = tree_cursor_entry_slice(&cursor.stack);
    let last_entry = entries.last().unwrap_unchecked();
    let is_extra = subtree_extra(*last_entry.subtree);
    let alias_symbol = if is_extra {
        0
    } else if cursor.stack.size > 1 {
        let parent_entry = entries.get_unchecked(cursor.stack.size as usize - 2);
        language_alias_at(
            (*cursor.tree).language,
            u32::from((*(*parent_entry.subtree).ptr).data.children.production_id),
            last_entry.structural_child_index,
        )
    } else {
        cursor.root_alias_symbol
    };
    node_new(
        cursor.tree,
        last_entry.subtree,
        last_entry.position,
        alias_symbol,
    )
}

/// Resolve a child's alias-aware symbol within its parent production.
unsafe fn tree_cursor_child_symbol(
    language: *const TSLanguage,
    parent: Subtree,
    child: Subtree,
    structural_child_index: u32,
) -> TSSymbol {
    if !subtree_extra(child) {
        let alias = language_alias_at(
            language,
            u32::from((*parent.ptr).data.children.production_id),
            structural_child_index,
        );
        if alias != 0 {
            return alias;
        }
    }
    subtree_symbol(child)
}

/// Record whether an entry has later visible or named siblings.
unsafe fn tree_cursor_record_later_siblings(
    language: *const TSLanguage,
    parent: &TreeCursorEntry,
    entry: &TreeCursorEntry,
    has_later_siblings: &mut bool,
    has_later_named_siblings: &mut bool,
) {
    if *has_later_siblings {
        return;
    }

    let parent_subtree = *parent.subtree;
    let sibling_count = (*parent_subtree.ptr).child_count;
    let mut structural_child_index = entry.structural_child_index;
    if !subtree_extra(*entry.subtree) {
        structural_child_index += 1;
    }
    for child_index in entry.child_index + 1..sibling_count {
        let sibling = subtree_child(parent_subtree, child_index);
        let metadata = ts_language_symbol_metadata(
            language,
            tree_cursor_child_symbol(language, parent_subtree, *sibling, structural_child_index),
        );
        if metadata.visible {
            *has_later_siblings = true;
            *has_later_named_siblings |= metadata.named;
        } else if subtree_visible_child_count(*sibling) > 0 {
            *has_later_siblings = true;
            *has_later_named_siblings |= (*sibling.ptr).data.children.named_child_count > 0;
        }
        if *has_later_named_siblings {
            return;
        }
        if !subtree_extra(*sibling) {
            structural_child_index += 1;
        }
    }
}

/// Update the current field and whether it can occur again later.
unsafe fn tree_cursor_update_field_status(
    language: *const TSLanguage,
    parent: &TreeCursorEntry,
    entry: &TreeCursorEntry,
    field_id: &mut TSFieldId,
    can_have_later_siblings_with_this_field: &mut bool,
) {
    if subtree_extra(*entry.subtree) {
        return;
    }

    let mut field_map: *const TSFieldMapEntry = ptr::null();
    let mut field_map_end: *const TSFieldMapEntry = ptr::null();
    language_field_map(
        language,
        u32::from((*(*parent.subtree).ptr).data.children.production_id),
        &mut field_map,
        &mut field_map_end,
    );

    let mut map = field_map;
    while map < field_map_end {
        if *field_id == 0
            && !(*map).inherited
            && (*map).child_index == entry.structural_child_index as u8
        {
            *field_id = (*map).field_id;
        }
        if *field_id != 0
            && (*map).field_id == *field_id
            && u32::from((*map).child_index) > entry.structural_child_index
        {
            *can_have_later_siblings_with_this_field = true;
        }
        map = map.add(1);
    }
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
    let cursor = cursor_ref(self_);
    let language = (*cursor.tree).language;
    let field_id = out_param_mut(field_id);
    let has_later_siblings = out_param_mut(has_later_siblings);
    let has_later_named_siblings = out_param_mut(has_later_named_siblings);
    let can_have_later_siblings_with_this_field =
        out_param_mut(can_have_later_siblings_with_this_field);
    let supertype_count = out_param_mut(supertype_count);
    let max_supertypes = *supertype_count;
    *field_id = 0;
    *supertype_count = 0;
    *has_later_siblings = false;
    *has_later_named_siblings = false;
    *can_have_later_siblings_with_this_field = false;

    let entries = tree_cursor_entry_slice(&cursor.stack);
    let mut i = cursor.stack.size - 1;
    while i > 0 {
        let entry = entries.get_unchecked(i as usize);
        let parent = entries.get_unchecked((i - 1) as usize);
        let entry_symbol = tree_cursor_child_symbol(
            language,
            *parent.subtree,
            *entry.subtree,
            entry.structural_child_index,
        );
        let entry_metadata = ts_language_symbol_metadata(language, entry_symbol);

        if i != cursor.stack.size - 1 && entry_metadata.visible {
            break;
        }
        if entry_metadata.supertype && *supertype_count < max_supertypes {
            *supertypes.add(*supertype_count as usize) = entry_symbol;
            *supertype_count += 1;
        }

        tree_cursor_record_later_siblings(
            language,
            parent,
            entry,
            has_later_siblings,
            has_later_named_siblings,
        );
        tree_cursor_update_field_status(
            language,
            parent,
            entry,
            field_id,
            can_have_later_siblings_with_this_field,
        );
        i -= 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_field_id(self_: *const TSTreeCursor) -> TSFieldId {
    let cursor = cursor_ref(self_);

    // Walk up the tree, visiting the current node and its invisible ancestors.
    let entries = tree_cursor_entry_slice(&cursor.stack);
    let mut i = cursor.stack.size - 1;
    while i > 0 {
        let entry = entries.get_unchecked(i as usize);
        let parent_entry = entries.get_unchecked((i - 1) as usize);

        // Stop walking up when another visible node is found.
        if i != cursor.stack.size - 1 && tree_cursor_is_entry_visible(cursor, i) {
            break;
        }

        if subtree_extra(*entry.subtree) {
            break;
        }

        let mut field_id = 0;
        let mut can_repeat = false;
        tree_cursor_update_field_status(
            (*cursor.tree).language,
            parent_entry,
            entry,
            &mut field_id,
            &mut can_repeat,
        );
        if field_id != 0 {
            return field_id;
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
        let cursor = cursor_ref(self_);
        let lang = language_full((*cursor.tree).language);
        return *lang.field_names.add(id as usize);
    }
    ptr::null()
}
