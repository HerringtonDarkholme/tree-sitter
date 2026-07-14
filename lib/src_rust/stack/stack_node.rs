use core::ffi::c_void;
use core::ptr::{self, NonNull};

use super::{
    free, length_add, length_zero, malloc, subtree_child_count, subtree_dynamic_precedence,
    subtree_error_cost, subtree_external_scanner_state_eq, subtree_extra, subtree_padding,
    subtree_release, subtree_retain, subtree_size, subtree_symbol, subtree_total_size,
    subtree_visible, subtree_visible_descendant_count, Length, StackHead, StackLink,
    StackNodeArray, Subtree, SubtreePool, TSStateId, MAX_LINK_COUNT, MAX_NODE_POOL_SIZE,
    NULL_SUBTREE, TS_BUILTIN_SYM_ERROR_REPEAT,
};

/// Node in the persistent GLR stack graph.
///
/// A parser version points at one node head. Pushing creates a new node linked
/// to the previous head; popping walks backward through links and may fork when
/// a node has multiple predecessors.
pub(super) struct StackNode {
    pub(super) state: TSStateId,
    pub(super) position: Length,
    pub(super) links: [StackLink; MAX_LINK_COUNT],
    pub(super) link_count: u16,
    pub(super) ref_count: u32,
    pub(super) error_cost: u32,
    pub(super) node_count: u32,
    pub(super) dynamic_precedence: i32,
}

/// Retain a stack node referenced by a head or successor link.
pub(super) unsafe fn stack_node_retain(mut node: NonNull<StackNode>) {
    let node = node.as_mut();
    debug_assert!(node.ref_count > 0);
    node.ref_count += 1;
    debug_assert!(node.ref_count != 0);
}

/// Release a stack node and recursively release predecessors that become unused.
pub(super) unsafe fn stack_node_release(
    mut node_ptr: NonNull<StackNode>,
    pool: &mut StackNodeArray,
    subtree_pool: &mut SubtreePool,
) {
    loop {
        let node = node_ptr.as_mut();
        debug_assert!(node.ref_count != 0);
        node.ref_count -= 1;
        if node.ref_count > 0 {
            return;
        }

        let first_predecessor = if node.link_count > 0 {
            for i in (1..usize::from(node.link_count)).rev() {
                let link = node.links[i];
                if !link.subtree.is_null() {
                    subtree_release(subtree_pool, link.subtree);
                }
                stack_node_release(link.node, pool, subtree_pool);
            }
            let link = node.links[0];
            if !link.subtree.is_null() {
                subtree_release(subtree_pool, link.subtree);
            }
            Some(link.node)
        } else {
            None
        };

        if pool.size < MAX_NODE_POOL_SIZE {
            pool.push(node_ptr);
        } else {
            free(node_ptr.as_ptr().cast::<c_void>());
        }

        if let Some(predecessor) = first_predecessor {
            node_ptr = predecessor;
            continue;
        }
        return;
    }
}

unsafe fn stack_subtree_node_count(subtree: Subtree) -> u32 {
    let mut count = subtree_visible_descendant_count(subtree);
    if subtree_visible(subtree) {
        count += 1;
    }
    if subtree_symbol(subtree) == TS_BUILTIN_SYM_ERROR_REPEAT {
        count += 1;
    }
    count
}

/// Allocate a stack node, reusing a recently released node when possible.
pub(super) unsafe fn stack_node_new(
    previous_node: Option<NonNull<StackNode>>,
    subtree: Subtree,
    state: TSStateId,
    pool: &mut StackNodeArray,
) -> NonNull<StackNode> {
    let mut node = if pool.size > 0 {
        pool.pop()
    } else {
        NonNull::new_unchecked(malloc(core::mem::size_of::<StackNode>()).cast::<StackNode>())
    };

    ptr::write(
        node.as_ptr(),
        StackNode {
            state,
            position: length_zero(),
            links: [StackLink {
                node: NonNull::dangling(),
                subtree: NULL_SUBTREE,
            }; MAX_LINK_COUNT],
            link_count: 0,
            ref_count: 1,
            error_cost: 0,
            node_count: 0,
            dynamic_precedence: 0,
        },
    );

    if let Some(previous_node) = previous_node {
        let previous = previous_node.as_ref();
        let node_data = node.as_mut();
        node_data.link_count = 1;
        node_data.links[0] = StackLink {
            node: previous_node,
            subtree,
        };

        node_data.position = previous.position;
        node_data.error_cost = previous.error_cost;
        node_data.dynamic_precedence = previous.dynamic_precedence;
        node_data.node_count = previous.node_count;

        if !subtree.is_null() {
            node_data.error_cost += subtree_error_cost(subtree);
            node_data.position = length_add(node_data.position, subtree_total_size(subtree));
            node_data.node_count += stack_subtree_node_count(subtree);
            node_data.dynamic_precedence += subtree_dynamic_precedence(subtree);
        }
    }

    node
}

unsafe fn stack_subtree_is_equivalent(left: Subtree, right: Subtree) -> bool {
    if left == right {
        return true;
    }
    if left.is_null() || right.is_null() {
        return false;
    }

    let left_symbol = subtree_symbol(left);
    let right_symbol = subtree_symbol(right);
    if left_symbol != right_symbol {
        return false;
    }

    let left_error_cost = subtree_error_cost(left);
    let right_error_cost = subtree_error_cost(right);
    if left_error_cost > 0 && right_error_cost > 0 {
        return true;
    }

    let left_child_count = subtree_child_count(left);
    let right_child_count = subtree_child_count(right);
    subtree_padding(left).bytes == subtree_padding(right).bytes
        && subtree_size(left).bytes == subtree_size(right).bytes
        && left_child_count == right_child_count
        && subtree_extra(left) == subtree_extra(right)
        && subtree_external_scanner_state_eq(left, right)
}

/// Add a predecessor edge, merging equivalent paths to keep the graph shallow.
pub(super) unsafe fn stack_node_add_link(
    node: &mut StackNode,
    link: StackLink,
    subtree_pool: &mut SubtreePool,
) {
    let node_ptr = NonNull::from(&mut *node);
    if link.node == node_ptr {
        return;
    }

    for i in 0..node.link_count as usize {
        let existing_link = &mut node.links[i];
        if stack_subtree_is_equivalent(existing_link.subtree, link.subtree) {
            if existing_link.node == link.node {
                if subtree_dynamic_precedence(link.subtree)
                    > subtree_dynamic_precedence(existing_link.subtree)
                {
                    subtree_retain(link.subtree);
                    subtree_release(subtree_pool, existing_link.subtree);
                    existing_link.subtree = link.subtree;
                    node.dynamic_precedence = link.node.as_ref().dynamic_precedence
                        + subtree_dynamic_precedence(link.subtree);
                }
                return;
            }

            let existing_node = existing_link.node.as_ref();
            let link_node = link.node.as_ref();
            if existing_node.state == link_node.state
                && existing_node.position.bytes == link_node.position.bytes
                && existing_node.error_cost == link_node.error_cost
            {
                for j in 0..link_node.link_count as usize {
                    stack_node_add_link(
                        existing_link.node.as_mut(),
                        link_node.links[j],
                        subtree_pool,
                    );
                }
                let mut dynamic_precedence = link_node.dynamic_precedence;
                if !link.subtree.is_null() {
                    dynamic_precedence += subtree_dynamic_precedence(link.subtree);
                }
                if dynamic_precedence > node.dynamic_precedence {
                    node.dynamic_precedence = dynamic_precedence;
                }
                return;
            }
        }
    }

    if node.link_count as usize == MAX_LINK_COUNT {
        return;
    }

    stack_node_retain(link.node);
    let link_node = link.node.as_ref();
    let mut node_count = link_node.node_count;
    let mut dynamic_precedence = link_node.dynamic_precedence;
    node.links[node.link_count as usize] = link;
    node.link_count += 1;

    if !link.subtree.is_null() {
        subtree_retain(link.subtree);
        node_count += stack_subtree_node_count(link.subtree);
        dynamic_precedence += subtree_dynamic_precedence(link.subtree);
    }

    if node_count > node.node_count {
        node.node_count = node_count;
    }
    if dynamic_precedence > node.dynamic_precedence {
        node.dynamic_precedence = dynamic_precedence;
    }
}

/// Delete a stack head and release everything it owns.
pub(super) unsafe fn stack_head_delete(
    head: &mut StackHead,
    pool: &mut StackNodeArray,
    subtree_pool: &mut SubtreePool,
) {
    if !head.last_external_token.is_null() {
        subtree_release(subtree_pool, head.last_external_token);
    }
    if !head.lookahead_when_paused.is_null() {
        subtree_release(subtree_pool, head.lookahead_when_paused);
    }
    if let Some(mut summary) = head.summary.take() {
        summary.as_mut().delete();
        free(summary.as_ptr().cast::<c_void>());
    }
    stack_node_release(head.node, pool, subtree_pool);
}
