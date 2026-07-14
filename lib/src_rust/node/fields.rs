use super::{
    language_field_map_slice, language_full, node_child_iterator_next, node_is_relevant,
    node_iterate_children, node_language, node_null, node_relevant_child_count, node_subtree, ptr,
    subtree_extra, TSNode,
};

/// Look up the direct field attached to a structural child.
#[inline]
unsafe fn node_field_name_from_language(node: TSNode, structural_child_index: u32) -> *const i8 {
    let field_map = language_field_map_slice(
        node_language(node),
        u32::from(node_subtree(node).heap_data().children().production_id),
    );
    let language = language_full(node_language(node));
    for entry in field_map {
        if !entry.inherited && entry.child_index == structural_child_index as u8 {
            return *language.field_names.add(entry.field_id as usize);
        }
    }
    ptr::null()
}

/// Find a visible child's field name, carrying fields through hidden nodes.
///
/// `include_anonymous` selects the public child index space: all visible
/// children when true, named children only when false.
unsafe fn node_field_name_for_child(
    mut node: TSNode,
    mut child_index: u32,
    include_anonymous: bool,
) -> *const i8 {
    let mut inherited_field_name: *const i8 = ptr::null();

    loop {
        let mut did_descend = false;
        let mut child = node_null();
        let mut visible_index = 0;
        let mut iterator = node_iterate_children(&node);

        while node_child_iterator_next(&mut iterator, &mut child) {
            if node_is_relevant(child, include_anonymous) {
                if visible_index == child_index {
                    if subtree_extra(node_subtree(child)) {
                        return ptr::null();
                    }
                    let field_name =
                        node_field_name_from_language(node, iterator.structural_child_index - 1);
                    return if field_name.is_null() {
                        inherited_field_name
                    } else {
                        field_name
                    };
                }
                visible_index += 1;
                continue;
            }

            let descendant_index = child_index - visible_index;
            let descendant_count = node_relevant_child_count(child, include_anonymous);
            if descendant_index < descendant_count {
                let field_name =
                    node_field_name_from_language(node, iterator.structural_child_index - 1);
                if !field_name.is_null() {
                    inherited_field_name = field_name;
                }
                node = child;
                child_index = descendant_index;
                did_descend = true;
                break;
            }
            visible_index += descendant_count;
        }

        if !did_descend {
            return ptr::null();
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_field_name_for_child(node: TSNode, child_index: u32) -> *const i8 {
    node_field_name_for_child(node, child_index, true)
}

#[no_mangle]
pub unsafe extern "C" fn ts_node_field_name_for_named_child(
    node: TSNode,
    named_child_index: u32,
) -> *const i8 {
    node_field_name_for_child(node, named_child_index, false)
}
