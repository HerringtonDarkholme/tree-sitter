#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use core::{ptr::NonNull, sync::atomic::AtomicU32};

use super::{
    length_add, length_saturating_sub, length_sub, length_zero, subtree_can_inline,
    subtree_pool_allocate, subtree_set_has_changes, Edit, EditEntry, Length, MutableSubtree,
    Subtree, SubtreeHeapData, SubtreeHeapDataContent, SubtreePool, TSInputEdit, TSSymbol,
};

/// Calculate the edited padding and content size for one subtree.
///
/// `None` means the edit is strictly after the subtree, including its
/// lookahead, so neither this subtree nor its children need to be visited.
unsafe fn subtree_edited_size(tree: Subtree, edit: Edit) -> Option<(Length, Length, u32)> {
    let is_noop = edit.old_end.bytes == edit.start.bytes && edit.new_end.bytes == edit.start.bytes;
    let is_pure_insertion = edit.old_end.bytes == edit.start.bytes;
    let mut size = tree.size();
    let mut padding = tree.padding();
    let total_size = length_add(padding, size);
    let lookahead_bytes = tree.lookahead_bytes();
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
    let mut result = tree.make_mut(pool);

    if result.is_inline() {
        if subtree_can_inline(padding, size, lookahead_bytes) {
            let data = result.inline_data_mut().unwrap();
            data.padding_bytes = padding.bytes as u8;
            data.set_padding_rows(padding.extent.row as u8);
            data.padding_columns = padding.extent.column as u8;
            data.size_bytes = size.bytes as u8;
        } else {
            let inline = result.inline_data().unwrap();
            let data = subtree_pool_allocate(pool);
            data.as_ptr().write(SubtreeHeapData {
                ref_count: AtomicU32::new(1),
                padding,
                size,
                lookahead_bytes,
                error_cost: 0,
                child_count: 0,
                symbol: TSSymbol::from(inline.symbol),
                parse_state: inline.parse_state,
                flags: SubtreeHeapData::make_flags(
                    inline.visible(),
                    inline.named(),
                    inline.extra(),
                    false,
                    false,
                    false,
                    false,
                    inline.is_missing(),
                    inline.is_keyword(),
                ),
                data: SubtreeHeapDataContent::LookaheadChar(0),
            });
            result = MutableSubtree::from_heap(data);
        }
    } else {
        result.heap_data_mut().padding = padding;
        result.heap_data_mut().size = size;
    }

    subtree_set_has_changes(&mut result);
    result.into_immutable()
}

/// Translate an edit into each affected child's coordinate space.
unsafe fn subtree_schedule_edited_children(
    tree: NonNull<Subtree>,
    mut edit: Edit,
    stack: &mut Vec<EditEntry>,
) {
    let tree = tree.as_ptr();
    let is_pure_insertion = edit.old_end.bytes == edit.start.bytes;
    let parent_depends_on_column = (*tree).depends_on_column();
    let column_shifted = edit.new_end.extent.column != edit.old_end.extent.column;
    let padding = (*tree).padding();
    let mut child_right = length_zero();
    let children = (*tree).children();

    for (i, child) in children.iter().enumerate() {
        let child_size = (*child).total_size();
        let child_left = child_right;
        child_right = length_add(child_left, child_size);

        if child_right.bytes + (*child).lookahead_bytes() < edit.start.bytes {
            continue;
        }

        if ((child_left.bytes > edit.old_end.bytes)
            || (child_left.bytes == edit.old_end.bytes && child_size.bytes > 0 && i > 0))
            && (!parent_depends_on_column || child_left.extent.row > padding.extent.row)
            && (!(*child).depends_on_column()
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
        let Some((padding, size, lookahead_bytes)) = subtree_edited_size(*tree, entry.edit) else {
            continue;
        };

        *tree = subtree_apply_edit_size(pool, *tree, padding, size, lookahead_bytes);
        subtree_schedule_edited_children(entry.tree, entry.edit, &mut stack);
    }

    self_
}
