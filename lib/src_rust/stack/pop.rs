use super::{
    array_back_mut, array_back_ref, array_clear, array_erase, array_get_mut, array_get_ref,
    array_insert, array_new, array_push, array_reserve, ptr, ptr_mut, ptr_ref, stack_head,
    stack_node_retain, subtree_alloc_size, subtree_array_copy, subtree_array_delete,
    subtree_array_reverse, subtree_extra, subtree_is_error, subtree_retain, Stack, StackHead,
    StackIterationAction, StackIterator, StackLink, StackNode, StackSlice, StackSliceArray,
    StackStatus, StackSummary, StackSummaryEntry, StackVersion, Subtree, SubtreeArray,
    MAX_ITERATOR_COUNT, NULL_SUBTREE,
};

/// Add a new version to the stack, cloning metadata from an existing version.
unsafe fn stack_add_version(
    self_: &mut Stack,
    original_version: StackVersion,
    node: &mut StackNode,
) -> StackVersion {
    let node_ptr = ptr::from_mut(node);
    let original_head = stack_head(self_, original_version);
    let head = StackHead {
        node: node_ptr,
        node_count_at_last_error: original_head.node_count_at_last_error,
        last_external_token: original_head.last_external_token,
        status: StackStatus::Active,
        lookahead_when_paused: NULL_SUBTREE,
        summary: None,
    };
    array_push(&mut self_.heads, head);
    stack_node_retain(node);
    let head = array_back_ref(&self_.heads);
    if !head.last_external_token.is_null() {
        subtree_retain(head.last_external_token);
    }
    self_.heads.size - 1
}

/// Add a slice to the stack's slice array, finding or creating a version.
unsafe fn stack_add_slice(
    self_: &mut Stack,
    original_version: StackVersion,
    node: &mut StackNode,
    subtrees: &SubtreeArray,
) {
    let node_ptr = ptr::from_mut(node);
    for i in (0..self_.slices.size).rev() {
        let version = array_get_ref(&self_.slices, i).version;
        if stack_head(self_, version).node == node_ptr {
            let slice = StackSlice {
                subtrees: ptr::read(subtrees),
                version,
            };
            array_insert(&mut self_.slices, i + 1, slice);
            return;
        }
    }

    let version = stack_add_version(self_, original_version, node);
    let slice = StackSlice {
        subtrees: ptr::read(subtrees),
        version,
    };
    array_push(&mut self_.slices, slice);
}

/// Core iteration function for walking the stack graph.
pub(super) unsafe fn stack_iter<F>(
    stack: &mut Stack,
    version: StackVersion,
    mut action_for: F,
    goal_subtree_count: Option<u32>,
) -> StackSliceArray
where
    F: FnMut(&StackIterator) -> StackIterationAction,
{
    array_clear(&mut stack.slices);
    array_clear(&mut stack.iterators);

    let head = stack_head(stack, version);
    let mut new_iterator = StackIterator {
        node: head.node,
        subtrees: array_new(),
        subtree_count: 0,
    };

    if let Some(goal_subtree_count) = goal_subtree_count {
        let reserve_count =
            subtree_alloc_size(goal_subtree_count) / core::mem::size_of::<Subtree>();
        let subtrees = &mut new_iterator.subtrees;
        array_reserve(subtrees, u32::try_from(reserve_count).unwrap());
    }
    let include_subtrees = goal_subtree_count.is_some();

    array_push(&mut stack.iterators, new_iterator);

    while stack.iterators.size > 0 {
        let mut i: u32 = 0;
        let mut active_iterator_count = stack.iterators.size;
        while i < active_iterator_count {
            let iterator = array_get_ref(&stack.iterators, i);
            let node = iterator.node;

            let (should_pop, should_stop) = match action_for(iterator) {
                StackIterationAction::Continue => (false, (*node).link_count == 0),
                StackIterationAction::Stop => (false, true),
                StackIterationAction::Pop => (true, (*node).link_count == 0),
                StackIterationAction::PopAndStop => (true, true),
            };

            if should_pop {
                let mut subtrees = ptr::read(&array_get_ref(&stack.iterators, i).subtrees);
                if !should_stop {
                    let source_subtrees = ptr::read(&subtrees);
                    subtree_array_copy(&source_subtrees, &mut subtrees);
                }
                subtree_array_reverse(&mut subtrees);
                stack_add_slice(stack, version, ptr_mut(node), &subtrees);
            }

            if should_stop {
                if !should_pop {
                    let iter = array_get_mut(&mut stack.iterators, i);
                    subtree_array_delete(ptr_mut(stack.subtree_pool), &mut iter.subtrees);
                }
                array_erase(&mut stack.iterators, i);
                active_iterator_count -= 1;
                continue;
            }

            // Copy all alternate branches, then reuse the current iterator for
            // link 0 so the common path avoids an extra subtree-array clone.
            let link_count = u32::from((*node).link_count);
            for branch_index in 1..=link_count {
                let next_iterator: &mut StackIterator;
                let link: StackLink;
                if branch_index == link_count {
                    link = (*node).links[0];
                    next_iterator = array_get_mut(&mut stack.iterators, i);
                } else {
                    if stack.iterators.size >= MAX_ITERATOR_COUNT {
                        continue;
                    }
                    link = (*node).links[branch_index as usize];
                    let current_iterator = ptr::read(array_get_ref(&stack.iterators, i));
                    array_push(&mut stack.iterators, current_iterator);
                    next_iterator = array_back_mut(&mut stack.iterators);
                    let source_subtrees = ptr::read(&next_iterator.subtrees);
                    subtree_array_copy(&source_subtrees, &mut next_iterator.subtrees);
                }

                next_iterator.node = link.node;
                let subtree = link.subtree;
                if subtree.is_null() {
                    next_iterator.subtree_count += 1;
                } else {
                    if include_subtrees {
                        let subtrees = &mut next_iterator.subtrees;
                        array_push(subtrees, subtree);
                        subtree_retain(subtree);
                    }

                    if !subtree_extra(subtree) {
                        next_iterator.subtree_count += 1;
                    }
                }
            }
            i = i.wrapping_add(1);
        }
    }

    ptr::read(&stack.slices)
}

pub(super) const fn pop_count_action(
    iterator: &StackIterator,
    goal_subtree_count: u32,
) -> StackIterationAction {
    if iterator.subtree_count == goal_subtree_count {
        StackIterationAction::PopAndStop
    } else {
        StackIterationAction::Continue
    }
}

pub(super) unsafe fn pop_error_action(
    iterator: &StackIterator,
    found_error: &mut bool,
) -> StackIterationAction {
    if iterator.subtrees.size > 0 {
        if !*found_error && subtree_is_error(*array_get_ref(&iterator.subtrees, 0)) {
            *found_error = true;
            StackIterationAction::PopAndStop
        } else {
            StackIterationAction::Stop
        }
    } else {
        StackIterationAction::Continue
    }
}

pub(super) unsafe fn pop_all_action(iterator: &StackIterator) -> StackIterationAction {
    let node = ptr_ref(iterator.node);
    if node.link_count == 0 {
        StackIterationAction::Pop
    } else {
        StackIterationAction::Continue
    }
}

pub(super) unsafe fn summarize_stack_action(
    iterator: &StackIterator,
    summary: &mut StackSummary,
    max_depth: u32,
) -> StackIterationAction {
    let node = ptr_ref(iterator.node);
    let state = node.state;
    let depth = iterator.subtree_count;
    if depth > max_depth {
        return StackIterationAction::Stop;
    }
    for i in (0..summary.size).rev() {
        let entry = array_get_ref(summary, i);
        if entry.depth < depth {
            break;
        }
        if entry.depth == depth && entry.state == state {
            return StackIterationAction::Continue;
        }
    }
    array_push(
        summary,
        StackSummaryEntry {
            position: node.position,
            depth,
            state,
        },
    );
    StackIterationAction::Continue
}
