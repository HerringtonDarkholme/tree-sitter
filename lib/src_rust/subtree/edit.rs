#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use super::{
    length_add, length_saturating_sub, length_sub, length_zero, ptr, subtree_can_inline,
    subtree_children_slice, subtree_depends_on_column, subtree_from_mut, subtree_lookahead_bytes,
    subtree_make_mut, subtree_padding, subtree_pool_allocate, subtree_set_has_changes,
    subtree_size, subtree_total_size, Edit, EditEntry, Length, Subtree, SubtreeHeapData,
    SubtreeHeapDataContent, SubtreePool, TSInputEdit, TSSymbol,
};

/// Calculate the edited padding and content size for one subtree.
///
/// `None` means the edit is strictly after the subtree, including its
/// lookahead, so neither this subtree nor its children need to be visited.
unsafe fn subtree_edited_size(tree: Subtree, edit: Edit) -> Option<(Length, Length, u32)> {
    let is_noop = edit.old_end.bytes == edit.start.bytes && edit.new_end.bytes == edit.start.bytes;
    let is_pure_insertion = edit.old_end.bytes == edit.start.bytes;
    let mut size = subtree_size(tree);
    let mut padding = subtree_padding(tree);
    let total_size = length_add(padding, size);
    let lookahead_bytes = subtree_lookahead_bytes(tree);
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
    let mut result = subtree_make_mut(pool, tree);

    if result.data.is_inline() {
        if subtree_can_inline(padding, size, lookahead_bytes) {
            result.data.padding_bytes = padding.bytes as u8;
            result.data.set_padding_rows(padding.extent.row as u8);
            result.data.padding_columns = padding.extent.column as u8;
            result.data.size_bytes = size.bytes as u8;
        } else {
            let data = subtree_pool_allocate(pool);
            *data = SubtreeHeapData {
                ref_count: 1,
                padding,
                size,
                lookahead_bytes,
                error_cost: 0,
                child_count: 0,
                symbol: TSSymbol::from(result.data.symbol),
                parse_state: result.data.parse_state,
                flags: SubtreeHeapData::make_flags(
                    result.data.visible(),
                    result.data.named(),
                    result.data.extra(),
                    false,
                    false,
                    false,
                    false,
                    result.data.is_missing(),
                    result.data.is_keyword(),
                ),
                data: SubtreeHeapDataContent { lookahead_char: 0 },
            };
            result.ptr = data;
        }
    } else {
        (*result.ptr).padding = padding;
        (*result.ptr).size = size;
    }

    subtree_set_has_changes(&mut result);
    subtree_from_mut(result)
}

/// Translate an edit into each affected child's coordinate space.
unsafe fn subtree_schedule_edited_children(
    tree: *mut Subtree,
    mut edit: Edit,
    stack: &mut Vec<EditEntry>,
) {
    let is_pure_insertion = edit.old_end.bytes == edit.start.bytes;
    let parent_depends_on_column = subtree_depends_on_column(*tree);
    let column_shifted = edit.new_end.extent.column != edit.old_end.extent.column;
    let padding = subtree_padding(*tree);
    let mut child_right = length_zero();
    let children = subtree_children_slice(*tree);

    for (i, child) in children.iter().enumerate() {
        let child_size = subtree_total_size(*child);
        let child_left = child_right;
        child_right = length_add(child_left, child_size);

        if child_right.bytes + subtree_lookahead_bytes(*child) < edit.start.bytes {
            continue;
        }

        if ((child_left.bytes > edit.old_end.bytes)
            || (child_left.bytes == edit.old_end.bytes && child_size.bytes > 0 && i > 0))
            && (!parent_depends_on_column || child_left.extent.row > padding.extent.row)
            && (!subtree_depends_on_column(*child)
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
            tree: ptr::from_ref(child).cast_mut(),
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
        tree: core::ptr::addr_of_mut!(self_),
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
        let Some((padding, size, lookahead_bytes)) = subtree_edited_size(*entry.tree, entry.edit)
        else {
            continue;
        };

        *entry.tree = subtree_apply_edit_size(pool, *entry.tree, padding, size, lookahead_bytes);
        subtree_schedule_edited_children(entry.tree, entry.edit, &mut stack);
    }

    self_
}
