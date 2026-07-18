//! Apply an input edit to an existing subtree hierarchy.
//!
//! Editing updates byte and point geometry, marks affected ancestors as
//! changed, and descends only into children whose ranges overlap the edit.
//! Inline leaves are promoted to heap nodes if their edited measurements no
//! longer fit the packed representation. Shared nodes use the same copy-on-
//! write ownership rules as other subtree mutation.

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use core::{cell::Cell, ptr::NonNull, sync::atomic::AtomicBool};

use crate::ffi::{TSInputEdit, TSSymbol};

use super::super::length::{length_add, length_saturating_sub, length_sub, length_zero, Length};
use super::data::{SubtreeHeapData, SubtreeHeapDataContent};
use super::handle::{MutableSubtree, Subtree};
use super::storage::subtree_pool_allocate;
use super::{
    subtree_can_inline, subtree_set_has_changes, Edit, EditEntry, SubtreeArena, SubtreePool,
};

/// Calculate the edited padding and content size for one subtree.
///
/// `None` means the edit is strictly after the subtree, including its
/// lookahead, so neither this subtree nor its children need to be visited.
unsafe fn subtree_edited_size(
    tree: Subtree,
    arena: *mut SubtreeArena,
    edit: Edit,
) -> Option<(Length, Length, u32)> {
    let is_noop = edit.old_end.bytes == edit.start.bytes && edit.new_end.bytes == edit.start.bytes;
    let is_pure_insertion = edit.old_end.bytes == edit.start.bytes;
    let mut size = tree.size(arena);
    let mut padding = tree.padding(arena);
    let total_size = length_add(padding, size);
    let lookahead_bytes = tree.lookahead_bytes(arena);
    let end_byte = total_size.bytes + lookahead_bytes;
    if edit.start.bytes > end_byte || (is_noop && edit.start.bytes == end_byte) {
        return None;
    }

    if edit.old_end.bytes <= padding.bytes {
        // The edit is entirely within the space before this subtree.
        padding = length_add(edit.new_end, length_sub(padding, edit.old_end));
    } else if edit.start.bytes < padding.bytes {
        // The edit starts before and extends into this subtree.
        size = length_saturating_sub(size, length_sub(edit.old_end, padding));
        padding = edit.new_end;
    } else if edit.start.bytes < total_size.bytes
        || (edit.start.bytes == total_size.bytes && is_pure_insertion)
    {
        // The edit is within this subtree.
        size = length_add(
            length_sub(edit.new_end, padding),
            length_saturating_sub(total_size, edit.old_end),
        );
    }

    Some((padding, size, lookahead_bytes))
}

/// Apply edited geometry, promoting an inline subtree when it no longer fits.
unsafe fn subtree_apply_edit_size(
    pool: &mut SubtreePool,
    tree: Subtree,
    padding: Length,
    size: Length,
    lookahead_bytes: u32,
) -> Subtree {
    let mut arena = pool.arena();
    let mut result = tree.make_mut(pool);

    if result.is_inline() {
        if subtree_can_inline(padding, size, lookahead_bytes) {
            let data = result.inline_data_mut(arena).unwrap();
            data.padding_bytes = padding.bytes as u8;
            data.set_padding_rows(padding.extent.row as u8);
            data.padding_columns = padding.extent.column as u8;
            data.size_bytes = size.bytes as u8;
        } else {
            let inline = result.inline_data(arena).unwrap();
            let data = subtree_pool_allocate(pool);
            arena = pool.arena();
            let mut heap_data = SubtreeHeapData {
                parser_shared: Cell::new(false),
                published_shared: AtomicBool::new(false),
                parser_visited: Cell::new(false),
                padding,
                size,
                lookahead_bytes,
                error_cost: 0,
                child_count: 0,
                symbol: TSSymbol::from(inline.symbol),
                parse_state: inline.parse_state,
                flags: 0,
                data: SubtreeHeapDataContent::LookaheadChar(0),
            };
            heap_data.set_visible(inline.visible());
            heap_data.set_named(inline.named());
            heap_data.set_extra(inline.extra());
            heap_data.set_is_missing(inline.is_missing());
            heap_data.set_is_keyword(inline.is_keyword());
            data.as_ptr().write(heap_data);
            result = MutableSubtree::from_heap(arena, data);
        }
    } else {
        result.heap_data_mut(arena).padding = padding;
        result.heap_data_mut(arena).size = size;
    }

    subtree_set_has_changes(&mut result, arena);
    result.into_immutable()
}

/// Translate an edit into each affected child's coordinate space.
unsafe fn subtree_schedule_edited_children(
    tree: NonNull<Subtree>,
    arena: *mut SubtreeArena,
    mut edit: Edit,
    stack: &mut Vec<EditEntry>,
) {
    let tree = tree.as_ptr();
    let is_pure_insertion = edit.old_end.bytes == edit.start.bytes;
    let parent_depends_on_column = (*tree).depends_on_column(arena);
    let column_shifted = edit.new_end.extent.column != edit.old_end.extent.column;
    let padding = (*tree).padding(arena);
    let mut child_right = length_zero();
    let children = (*tree).children(arena);

    for (i, child) in children.iter().enumerate() {
        let child_size = (*child).total_size(arena);
        let child_left = child_right;
        child_right = length_add(child_left, child_size);

        if child_right.bytes + (*child).lookahead_bytes(arena) < edit.start.bytes {
            continue;
        }

        if ((child_left.bytes > edit.old_end.bytes)
            || (child_left.bytes == edit.old_end.bytes && child_size.bytes > 0 && i > 0))
            && (!parent_depends_on_column || child_left.extent.row > padding.extent.row)
            && (!(*child).depends_on_column(arena)
                || !column_shifted
                || child_left.extent.row > edit.old_end.extent.row)
        {
            break;
        }

        let mut child_edit = Edit {
            start: length_saturating_sub(edit.start, child_left),
            old_end: length_saturating_sub(edit.old_end, child_left),
            new_end: length_saturating_sub(edit.new_end, child_left),
        };

        if child_right.bytes > edit.start.bytes
            || (child_right.bytes == edit.start.bytes && is_pure_insertion)
        {
            edit.new_end = edit.start;
        } else {
            child_edit.old_end = child_edit.start;
            child_edit.new_end = child_edit.start;
        }

        stack.push(EditEntry {
            tree: NonNull::from(child),
            edit: child_edit,
        });
    }
}

pub unsafe fn subtree_edit(
    mut self_: Subtree,
    input_edit: &TSInputEdit,
    pool: &mut SubtreePool,
) -> Subtree {
    let mut arena = pool.arena();
    let mut stack: Vec<EditEntry> = Vec::new();
    stack.push(EditEntry {
        tree: NonNull::from(&mut self_),
        edit: Edit {
            start: Length {
                bytes: input_edit.start_byte,
                extent: input_edit.start_point,
            },
            old_end: Length {
                bytes: input_edit.old_end_byte,
                extent: input_edit.old_end_point,
            },
            new_end: Length {
                bytes: input_edit.new_end_byte,
                extent: input_edit.new_end_point,
            },
        },
    });

    while let Some(entry) = stack.pop() {
        let tree = entry.tree.as_ptr();
        let Some((padding, size, lookahead_bytes)) = subtree_edited_size(*tree, arena, entry.edit)
        else {
            continue;
        };

        *tree = subtree_apply_edit_size(pool, *tree, padding, size, lookahead_bytes);
        arena = pool.arena();
        subtree_schedule_edited_children(entry.tree, arena, entry.edit, &mut stack);
    }

    self_
}
