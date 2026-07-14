//! Backward traversal and popping of the graph-structured stack.
//!
//! A linear stack pop has one result, but a GLR node can have several
//! predecessors. [`stack_iter`] walks every relevant backward path and creates
//! a stack version for each distinct result. The public helpers specialize
//! that traversal for reducing a fixed child count, removing an error, popping
//! the whole stack, and recording state summaries for recovery.

use core::ptr::{self, NonNull};

use super::super::subtree::{
    subtree_alloc_size, subtree_array_copy, subtree_array_delete, subtree_array_reverse, Subtree,
    SubtreeArray, NULL_SUBTREE,
};
use super::super::utils::{ptr_mut, Array};
use super::stack_node::stack_node_retain;
use super::{
    stack_head, Stack, StackHead, StackIterationAction, StackIterator, StackLink, StackNode,
    StackSlice, StackSliceArray, StackStatus, StackSummary, StackSummaryEntry, StackVersion,
    MAX_ITERATOR_COUNT,
};

/// Add a new version to the stack, cloning metadata from an existing version.
unsafe fn stack_add_version(
    self_: &mut Stack,
    original_version: StackVersion,
    node: NonNull<StackNode>,
) -> StackVersion {
    let original_head = stack_head(self_, original_version);
    let head = StackHead {
        node,
        node_count_at_last_error: original_head.node_count_at_last_error,
        last_external_token: original_head.last_external_token,
        status: StackStatus::Active,
        lookahead_when_paused: NULL_SUBTREE,
        summary: None,
    };
    self_.heads.push(head);
    stack_node_retain(node);
    let head = self_.heads.last_unchecked();
    if !head.last_external_token.is_null() {
        head.last_external_token.retain();
    }
    self_.heads.size - 1
}

/// Add a slice to the stack's slice array, finding or creating a version.
unsafe fn stack_add_slice(
    self_: &mut Stack,
    original_version: StackVersion,
    node: NonNull<StackNode>,
    subtrees: &SubtreeArray,
) {
    for (i, slice) in self_.slices.as_slice().iter().enumerate().rev() {
        let version = slice.version;
        if stack_head(self_, version).node == node {
            let slice = StackSlice {
                subtrees: ptr::read(subtrees),
                version,
            };
            self_.slices.insert(i as u32 + 1, slice);
            return;
        }
    }

    let version = stack_add_version(self_, original_version, node);
    let slice = StackSlice {
        subtrees: ptr::read(subtrees),
        version,
    };
    self_.slices.push(slice);
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
    stack.slices.clear();
    stack.iterators.clear();

    let head = stack_head(stack, version);
    let mut new_iterator = StackIterator {
        node: head.node,
        subtrees: Array::new(),
        subtree_count: 0,
    };

    if let Some(goal_subtree_count) = goal_subtree_count {
        let reserve_count =
            subtree_alloc_size(goal_subtree_count) / core::mem::size_of::<Subtree>();
        let subtrees = &mut new_iterator.subtrees;
        subtrees.reserve(u32::try_from(reserve_count).unwrap());
    }
    let include_subtrees = goal_subtree_count.is_some();

    stack.iterators.push(new_iterator);

    while !stack.iterators.is_empty() {
        let mut i: u32 = 0;
        let mut active_iterator_count = stack.iterators.size;
        while i < active_iterator_count {
            let iterator = stack.iterators.get_unchecked(i);
            let node = iterator.node;

            let (should_pop, should_stop) = match action_for(iterator) {
                StackIterationAction::Continue => (false, node.as_ref().link_count == 0),
                StackIterationAction::Stop => (false, true),
                StackIterationAction::Pop => (true, node.as_ref().link_count == 0),
                StackIterationAction::PopAndStop => (true, true),
            };

            if should_pop {
                let mut subtrees = ptr::read(&stack.iterators.get_unchecked(i).subtrees);
                if !should_stop {
                    let source_subtrees = ptr::read(&subtrees);
                    subtree_array_copy(&source_subtrees, &mut subtrees);
                }
                subtree_array_reverse(&mut subtrees);
                stack_add_slice(stack, version, node, &subtrees);
            }

            if should_stop {
                if !should_pop {
                    let iter = stack.iterators.get_unchecked_mut(i);
                    subtree_array_delete(ptr_mut(stack.subtree_pool), &mut iter.subtrees);
                }
                stack.iterators.erase(i);
                active_iterator_count -= 1;
                continue;
            }

            // Copy all alternate branches, then reuse the current iterator for
            // link 0 so the common path avoids an extra subtree-array clone.
            let link_count = u32::from(node.as_ref().link_count);
            for branch_index in 1..=link_count {
                let next_iterator: &mut StackIterator;
                let link: StackLink;
                if branch_index == link_count {
                    link = node.as_ref().links[0];
                    next_iterator = stack.iterators.get_unchecked_mut(i);
                } else {
                    if stack.iterators.size >= MAX_ITERATOR_COUNT {
                        continue;
                    }
                    link = node.as_ref().links[branch_index as usize];
                    let current_iterator = ptr::read(stack.iterators.get_unchecked(i));
                    stack.iterators.push(current_iterator);
                    next_iterator = stack.iterators.last_unchecked_mut();
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
                        subtrees.push(subtree);
                        subtree.retain();
                    }

                    if !subtree.extra() {
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
    if let Some(&first_subtree) = iterator.subtrees.as_slice().first() {
        if !*found_error && first_subtree.is_error() {
            *found_error = true;
            StackIterationAction::PopAndStop
        } else {
            StackIterationAction::Stop
        }
    } else {
        StackIterationAction::Continue
    }
}

pub(super) const unsafe fn pop_all_action(iterator: &StackIterator) -> StackIterationAction {
    let node = iterator.node.as_ref();
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
    let node = iterator.node.as_ref();
    let state = node.state;
    let depth = iterator.subtree_count;
    if depth > max_depth {
        return StackIterationAction::Stop;
    }
    for entry in summary.as_slice().iter().rev() {
        if entry.depth < depth {
            break;
        }
        if entry.depth == depth && entry.state == state {
            return StackIterationAction::Continue;
        }
    }
    summary.push(StackSummaryEntry {
        position: node.position,
        depth,
        state,
    });
    StackIterationAction::Continue
}
