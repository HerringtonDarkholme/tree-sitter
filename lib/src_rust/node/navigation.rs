use super::{
    node_child_count, node_child_iterator_next, node_end_byte, node_is_null, node_is_relevant,
    node_iterate_children, node_null, node_relevant_child_count, node_start_byte, node_start_point,
    node_subtree, point_eq, point_gt, point_lt, point_lte, subtree_child, subtree_child_count,
    subtree_total_bytes, ts_node_parent, NodeChildIterator, Subtree, TSNode, TSPoint,
};

pub(super) unsafe fn node_child(
    self_: TSNode,
    mut child_index: u32,
    include_anonymous: bool,
) -> TSNode {
    let mut result = self_;

    loop {
        let mut did_descend = false;

        let mut child = node_null();
        let mut index: u32 = 0;
        let mut iterator = node_iterate_children(&result);
        while node_child_iterator_next(&mut iterator, &mut child) {
            if node_is_relevant(child, include_anonymous) {
                if index == child_index {
                    return child;
                }
                index += 1;
            } else {
                let grandchild_index = child_index - index;
                let grandchild_count = node_relevant_child_count(child, include_anonymous);
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

    node_null()
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
        if child.heap_ptr() == other.heap_ptr()
            || subtree_has_trailing_empty_descendant(child, other)
        {
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
pub(super) unsafe fn node_prev_sibling(self_: TSNode, include_anonymous: bool) -> TSNode {
    let self_subtree = node_subtree(self_);
    let self_is_empty = subtree_total_bytes(self_subtree) == 0;
    let target_end_byte = node_end_byte(self_);

    let mut node = ts_node_parent(self_);
    let mut earlier_node = node_null();
    let mut earlier_node_is_relevant = false;

    while !node_is_null(node) {
        let mut earlier_child = node_null();
        let mut earlier_child_is_relevant = false;
        let mut found_child_containing_target = false;

        let mut child = node_null();
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
                    || subtree_has_trailing_empty_descendant(node_subtree(child), self_subtree))
            {
                found_child_containing_target = true;
                break;
            }

            if node_is_relevant(child, include_anonymous) {
                earlier_child = child;
                earlier_child_is_relevant = true;
            } else if node_relevant_child_count(child, include_anonymous) > 0 {
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
            earlier_node = node_null();
            earlier_node_is_relevant = false;
        }
    }

    node_null()
}

/// Find the next visible/named sibling.
///
/// This mirrors `node_prev_sibling`, but tracks the nearest later candidate
/// while walking through hidden nodes that contain the original target.
pub(super) unsafe fn node_next_sibling(self_: TSNode, include_anonymous: bool) -> TSNode {
    let target_end_byte = node_end_byte(self_);

    let mut node = ts_node_parent(self_);
    let mut later_node = node_null();
    let mut later_node_is_relevant = false;

    while !node_is_null(node) {
        let mut later_child = node_null();
        let mut later_child_is_relevant = false;
        let mut child_containing_target = node_null();

        let mut child = node_null();
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
                if node_subtree(child).heap_ptr() != node_subtree(self_).heap_ptr() {
                    child_containing_target = child;
                }
            } else if node_is_relevant(child, include_anonymous) {
                later_child = child;
                later_child_is_relevant = true;
                break;
            } else if node_relevant_child_count(child, include_anonymous) > 0 {
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

    node_null()
}

/// Find the first visible/named child whose end byte is after `goal`.
///
/// Hidden children are searched recursively. A saved iterator lets the search
/// resume at the original depth after exploring a hidden child that did not
/// produce a match.
pub(super) unsafe fn node_first_child_for_byte(
    self_: TSNode,
    goal: u32,
    include_anonymous: bool,
) -> TSNode {
    let mut node = self_;

    let mut resume_iterator: Option<NodeChildIterator> = None;

    loop {
        let mut did_descend = false;

        let mut child = node_null();
        let mut iterator = node_iterate_children(&node);
        'resume_sibling_scan: loop {
            while node_child_iterator_next(&mut iterator, &mut child) {
                if node_end_byte(child) > goal {
                    if node_is_relevant(child, include_anonymous) {
                        return child;
                    } else if node_child_count(child) > 0 {
                        if iterator.child_index < subtree_child_count(node_subtree(child)) {
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

    node_null()
}

/// Find the smallest visible/named descendant covering a byte range.
///
/// The search descends while a child fully contains the target range and keeps
/// the last relevant node seen, so hidden implementation nodes are skipped in
/// the returned result.
pub(super) unsafe fn node_descendant_for_byte_range(
    self_: TSNode,
    range_start: u32,
    range_end: u32,
    include_anonymous: bool,
) -> TSNode {
    if range_start > range_end {
        return node_null();
    }
    let mut node = self_;
    let mut last_visible_node = self_;

    loop {
        let mut did_descend = false;

        let mut child = node_null();
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
            if node_is_relevant(node, include_anonymous) {
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

/// Point-coordinate variant of `node_descendant_for_byte_range`.
pub(super) unsafe fn node_descendant_for_point_range(
    self_: TSNode,
    range_start: TSPoint,
    range_end: TSPoint,
    include_anonymous: bool,
) -> TSNode {
    if point_gt(range_start, range_end) {
        return node_null();
    }
    let mut node = self_;
    let mut last_visible_node = self_;

    loop {
        let mut did_descend = false;

        let mut child = node_null();
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
            if node_is_relevant(node, include_anonymous) {
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
