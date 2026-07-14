//! DOT rendering for the graph-structured parse stack.
//!
//! The graph shows every live stack head, shared node, predecessor link, parse
//! state, error cost, and external-scanner state. It is diagnostic-only and is
//! kept separate from stack mutation and traversal.

use core::ptr::NonNull;

use super::{
    c_void, fprintf, language_write_symbol_as_dot_string, ptr, stack_head, stderr_file, Array,
    Stack, StackIterator, StackNode, StackStatus, TSLanguage, ERROR_STATE,
};

/// Print the stack as a DOT graph for debugging.
pub unsafe fn stack_print_dot_graph(
    stack: &mut Stack,
    language: *const TSLanguage,
    mut f: *mut c_void,
) -> bool {
    stack.iterators.reserve(32);
    if f.is_null() {
        f = stderr_file();
    }

    fprintf(f, c"digraph stack {\n".as_ptr().cast::<i8>());
    fprintf(f, c"rankdir=\"RL\";\n".as_ptr().cast::<i8>());
    fprintf(f, c"edge [arrowhead=none]\n".as_ptr().cast::<i8>());

    let mut visited_nodes: Array<NonNull<StackNode>> = Array::new();

    stack.iterators.clear();
    for i in 0..stack.heads.size {
        if stack_head(stack, i).status == StackStatus::Halted {
            continue;
        }
        let node_count_since_error = stack.node_count_since_error(i);
        let error_cost = stack.error_cost(i);
        let head = stack_head(stack, i);

        fprintf(
            f,
            c"node_head_%u [shape=none, label=\"\"]\n"
                .as_ptr()
                .cast::<i8>(),
            i,
        );
        fprintf(
            f,
            c"node_head_%u -> node_%p [".as_ptr().cast::<i8>(),
            i,
            head.node.as_ptr().cast::<c_void>(),
        );

        if head.status == StackStatus::Paused {
            fprintf(f, c"color=red ".as_ptr().cast::<i8>());
        }
        fprintf(
            f,
            c"label=%u, fontcolor=blue, weight=10000, labeltooltip=\"node_count: %u\nerror_cost: %u".as_ptr().cast::<i8>(),
            i,
            node_count_since_error,
            error_cost,
        );

        if let Some(summary) = head.summary {
            fprintf(f, c"\nsummary:".as_ptr().cast::<i8>());
            let summary = summary.as_ref();
            for entry in summary.as_slice() {
                fprintf(f, c" %u".as_ptr().cast::<i8>(), u32::from(entry.state));
            }
        }

        if !head.last_external_token.is_null() {
            let state = head.last_external_token.external_scanner_state();
            fprintf(f, c"\nexternal_scanner_state:".as_ptr().cast::<i8>());
            for &byte in state.as_bytes() {
                fprintf(f, c" %2X".as_ptr().cast::<i8>(), u32::from(byte));
            }
        }

        fprintf(f, c"\"]\n".as_ptr().cast::<i8>());

        let iter = StackIterator {
            node: head.node,
            subtrees: Array::new(),
            subtree_count: 0,
        };
        stack.iterators.push(iter);
    }

    loop {
        let mut all_iterators_done = true;

        for i in 0..stack.iterators.size {
            let iterator = ptr::read(stack.iterators.get_unchecked(i));
            let node = iterator.node;

            if visited_nodes.as_slice().contains(&node) {
                continue;
            }
            all_iterators_done = false;
            let node_ref = node.as_ref();

            fprintf(
                f,
                c"node_%p [".as_ptr().cast::<i8>(),
                node.as_ptr().cast::<c_void>(),
            );
            if node_ref.state == ERROR_STATE {
                fprintf(f, c"label=\"?\"".as_ptr().cast::<i8>());
            } else if node_ref.link_count == 1
                && !node_ref.links[0].subtree.is_null()
                && node_ref.links[0].subtree.extra()
            {
                fprintf(f, c"shape=point margin=0 label=\"\"".as_ptr().cast::<i8>());
            } else {
                fprintf(
                    f,
                    c"label=\"%d\"".as_ptr().cast::<i8>(),
                    i32::from(node_ref.state),
                );
            }

            fprintf(
                f,
                c" tooltip=\"position: %u,%u\nnode_count:%u\nerror_cost: %u\ndynamic_precedence: %d\"];\n".as_ptr().cast::<i8>(),
                node_ref.position.extent.row + 1,
                node_ref.position.extent.column,
                node_ref.node_count,
                node_ref.error_cost,
                node_ref.dynamic_precedence,
            );

            for j in 0..node_ref.link_count as usize {
                let link = node_ref.links[j];
                fprintf(
                    f,
                    c"node_%p -> node_%p [".as_ptr().cast::<i8>(),
                    node.as_ptr().cast::<c_void>(),
                    link.node.as_ptr().cast::<c_void>(),
                );
                let subtree = link.subtree;
                if !subtree.is_null() && subtree.extra() {
                    fprintf(f, c"fontcolor=gray ".as_ptr().cast::<i8>());
                }

                if subtree.is_null() {
                    fprintf(f, c"color=red".as_ptr().cast::<i8>());
                } else {
                    fprintf(f, c"label=\"".as_ptr().cast::<i8>());
                    let quoted = subtree.visible() && !subtree.named();
                    if quoted {
                        fprintf(f, c"'".as_ptr().cast::<i8>());
                    }
                    language_write_symbol_as_dot_string(language, f, subtree.symbol());
                    if quoted {
                        fprintf(f, c"'".as_ptr().cast::<i8>());
                    }
                    fprintf(f, c"\"".as_ptr().cast::<i8>());
                    fprintf(
                        f,
                        c"labeltooltip=\"error_cost: %u\ndynamic_precedence: %d\""
                            .as_ptr()
                            .cast::<i8>(),
                        subtree.error_cost(),
                        subtree.dynamic_precedence(),
                    );
                }

                fprintf(f, c"];\n".as_ptr().cast::<i8>());

                let next_iterator = if j == 0 {
                    stack.iterators.get_unchecked_mut(i)
                } else {
                    stack.iterators.push(ptr::read(&iterator));
                    stack.iterators.last_unchecked_mut()
                };
                next_iterator.node = link.node;
            }

            visited_nodes.push(node);
        }
        if all_iterators_done {
            break;
        }
    }

    fprintf(f, c"}\n".as_ptr().cast::<i8>());

    visited_nodes.delete();
    true
}
