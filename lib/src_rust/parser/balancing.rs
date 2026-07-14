use super::{parser_check_progress, subtree_compress, TSParser};

/// Incrementally rebalance the accepted tree, preserving work across cancellation.
pub(super) unsafe fn parser_balance_subtree(parser: &mut TSParser) -> bool {
    let finished_tree = parser.finished_tree;

    if !parser.canceled_balancing {
        parser.tree_pool.tree_stack.clear();
        if finished_tree.child_count() > 0 && finished_tree.heap_data().ref_count() == 1 {
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

        if tree.heap_data().children().repeat_depth > 0 {
            let tree_subtree = tree.into_immutable();
            let children = tree_subtree.children();
            let first_depth = children.get_unchecked(0).repeat_depth();
            let last_depth = children
                .get_unchecked(tree.heap_data().child_count as usize - 1)
                .repeat_depth();
            let repeat_delta = i64::from(first_depth) - i64::from(last_depth);
            if repeat_delta > 0 {
                let mut compression = repeat_delta as u32 / 2;
                while compression > 0 {
                    subtree_compress(
                        tree,
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

        for child_index in 0..tree.heap_data().child_count {
            let child = *(tree.into_immutable()).child(child_index);
            if child.child_count() > 0 && child.heap_data().ref_count() == 1 {
                parser.tree_pool.tree_stack.push(child.into_mut());
            }
        }
    }

    true
}
