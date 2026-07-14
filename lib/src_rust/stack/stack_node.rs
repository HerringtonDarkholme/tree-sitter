use core::ffi::c_void;
use core::ptr::{self, NonNull};

use super::{
    free, length_add, length_zero, malloc, Length, StackHead, StackLink, StackNodeArray, Subtree,
    SubtreePool, TSStateId, MAX_LINK_COUNT, MAX_NODE_POOL_SIZE, NULL_SUBTREE,
    TS_BUILTIN_SYM_ERROR_REPEAT,
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
                    link.subtree.release(subtree_pool);
                }
                stack_node_release(link.node, pool, subtree_pool);
            }
            let link = node.links[0];
            if !link.subtree.is_null() {
                link.subtree.release(subtree_pool);
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
    let mut count = subtree.visible_descendant_count();
    if subtree.visible() {
        count += 1;
    }
    if subtree.symbol() == TS_BUILTIN_SYM_ERROR_REPEAT {
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
            node_data.error_cost += subtree.error_cost();
            node_data.position = length_add(node_data.position, subtree.total_size());
            node_data.node_count += stack_subtree_node_count(subtree);
            node_data.dynamic_precedence += subtree.dynamic_precedence();
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

    let left_symbol = left.symbol();
    let right_symbol = right.symbol();
    if left_symbol != right_symbol {
        return false;
    }

    let left_error_cost = left.error_cost();
    let right_error_cost = right.error_cost();
    if left_error_cost > 0 && right_error_cost > 0 {
        return true;
    }

    let left_child_count = left.child_count();
    let right_child_count = right.child_count();
    left.padding().bytes == right.padding().bytes
        && left.size().bytes == right.size().bytes
        && left_child_count == right_child_count
        && left.extra() == right.extra()
        && left.has_same_external_scanner_state(right)
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
                if link.subtree.dynamic_precedence() > existing_link.subtree.dynamic_precedence() {
                    link.subtree.retain();
                    existing_link.subtree.release(subtree_pool);
                    existing_link.subtree = link.subtree;
                    node.dynamic_precedence =
                        link.node.as_ref().dynamic_precedence + link.subtree.dynamic_precedence();
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
                    dynamic_precedence += link.subtree.dynamic_precedence();
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
        link.subtree.retain();
        node_count += stack_subtree_node_count(link.subtree);
        dynamic_precedence += link.subtree.dynamic_precedence();
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
        head.last_external_token.release(subtree_pool);
    }
    if !head.lookahead_when_paused.is_null() {
        head.lookahead_when_paused.release(subtree_pool);
    }
    if let Some(mut summary) = head.summary.take() {
        summary.as_mut().delete();
        free(summary.as_ptr().cast::<c_void>());
    }
    stack_node_release(head.node, pool, subtree_pool);
}
