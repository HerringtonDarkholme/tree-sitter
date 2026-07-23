//! Final balancing of the accepted subtree.
//!
//! Repeated grammar rules can initially produce deeply nested child arrays.
//! Before exposing the finished tree, this module compresses those nodes into
//! a shallower shape. Work is iterative and resumable so the parser's progress
//! callback can cancel a long balancing pass without discarding completed
//! work.

use super::super::subtree::{subtree_compress, subtree_prepare_for_balancing};
use super::advance::parser_check_progress;
use super::TSParser;

/// Incrementally rebalance the accepted tree, preserving work across cancellation.
pub(super) unsafe fn parser_balance_subtree(parser: &mut TSParser) -> bool {
    let finished_tree = parser.finished_tree;
    let arena = parser.tree_pool.arena();

    if !parser.canceled_balancing {
        subtree_prepare_for_balancing(finished_tree, arena, &mut parser.tree_pool.tree_stack);
        if finished_tree.child_count(arena) > 0 && !finished_tree.shared(arena) {
            parser.tree_pool.tree_stack.push(finished_tree.into_mut());
        }
    }

    while !parser.tree_pool.tree_stack.is_empty() {
        if !parser_check_progress(parser, None, None, 1) {
            return false;
        }

        let tree = *parser
            .tree_pool
            .tree_stack
            .as_slice()
            .last()
            .unwrap_unchecked();

        if tree.heap_data(arena).children().repeat_depth > 0 {
            let tree_subtree = tree.into_immutable();
            let children = tree_subtree.children(arena);
            let first_depth = children.get_unchecked(0).repeat_depth(arena);
            let last_depth = children
                .get_unchecked(tree.heap_data(arena).child_count as usize - 1)
                .repeat_depth(arena);
            let repeat_delta = i64::from(first_depth) - i64::from(last_depth);
            if repeat_delta > 0 {
                let mut compression = repeat_delta as u32 / 2;
                while compression > 0 {
                    subtree_compress(
                        tree,
                        arena,
                        compression,
                        parser.language,
                        &mut parser.tree_pool.tree_stack,
                    );

                    // Larger compressions get a proportionally larger progress
                    // charge so cancellation checks remain responsive.
                    let operations = (compression >> 4).max(1);
                    if !parser_check_progress(parser, None, None, operations) {
                        return false;
                    }
                    compression /= 2;
                }
            }
        }

        parser.tree_pool.tree_stack.pop();

        for child_index in 0..tree.heap_data(arena).child_count {
            let child = *(tree.into_immutable()).child(arena, child_index);
            if child.child_count(arena) > 0 && !child.shared(arena) {
                parser.tree_pool.tree_stack.push(child.into_mut());
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use core::ptr;

    use super::*;
    use crate::core_impl::{
        length::length_zero,
        subtree::{
            subtree_new_error, subtree_new_node, Subtree, SubtreeArray, SubtreePool,
            TS_BUILTIN_SYM_ERROR_REPEAT,
        },
    };

    unsafe fn error_leaf(pool: &mut SubtreePool) -> Subtree {
        subtree_new_error(
            pool,
            b'x' as i32,
            length_zero(),
            length_zero(),
            0,
            0,
            ptr::null(),
        )
    }

    unsafe fn repeat_node(pool: &mut SubtreePool, children: &[Subtree]) -> Subtree {
        let mut child_array = SubtreeArray::new();
        for &child in children {
            child_array.push(child);
        }
        subtree_new_node(
            pool,
            TS_BUILTIN_SYM_ERROR_REPEAT,
            child_array,
            0,
            ptr::null(),
        )
        .into_immutable()
    }

    #[test]
    fn balancing_does_not_mutate_descendants_of_shared_nodes() {
        unsafe {
            let parser_ptr = super::super::ts_parser_new();
            let parser = &mut *parser_ptr;

            // Build a left-heavy repeat subtree whose first edge would be
            // rotated if final balancing visited it.
            let leaf1 = error_leaf(&mut parser.tree_pool);
            let leaf2 = error_leaf(&mut parser.tree_pool);
            let grandchild = repeat_node(&mut parser.tree_pool, &[leaf1, leaf2]);
            let leaf3 = error_leaf(&mut parser.tree_pool);
            let child = repeat_node(&mut parser.tree_pool, &[grandchild, leaf3]);
            let leaf4 = error_leaf(&mut parser.tree_pool);
            let deep_child = repeat_node(&mut parser.tree_pool, &[child, leaf4]);
            let leaf5 = error_leaf(&mut parser.tree_pool);
            let descendant = repeat_node(&mut parser.tree_pool, &[deep_child, leaf5]);
            let arena = parser.tree_pool.arena();
            let original_first_child = *descendant.child(arena, 0);
            assert!(
                original_first_child.repeat_depth(arena)
                    >= descendant.child(arena, 1).repeat_depth(arena) + 2
            );

            // This parent has two incoming edges, but its one physical child
            // edge reaches `descendant` only once. Exact sharing therefore
            // marks the parent shared while leaving the descendant unmarked.
            let shared_parent = repeat_node(&mut parser.tree_pool, &[descendant]);
            shared_parent.retain(arena);
            let root = repeat_node(&mut parser.tree_pool, &[shared_parent, shared_parent]);
            parser.finished_tree = root;

            assert!(parser_balance_subtree(parser));
            assert!(shared_parent.shared(arena));
            assert!(!descendant.shared(arena));
            assert!(*descendant.child(arena, 0) == original_first_child);

            super::super::ts_parser_delete(parser_ptr);
        }
    }
}
