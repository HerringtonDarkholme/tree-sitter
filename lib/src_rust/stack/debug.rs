use super::{
    array_back_mut, array_clear, array_delete, array_get_mut, array_get_ref, array_new, array_push,
    array_reserve, c_void, external_scanner_state_data, fprintf,
    language_write_symbol_as_dot_string, ptr, ptr_ref, stack_error_cost, stack_head,
    stack_node_count_since_error, stderr_file, subtree_dynamic_precedence, subtree_error_cost,
    subtree_external_scanner_state, subtree_extra, subtree_named, subtree_symbol, subtree_visible,
    Array, Stack, StackIterator, StackNode, StackStatus, TSLanguage, ERROR_STATE,
};

/// Print the stack as a DOT graph for debugging.
pub unsafe fn stack_print_dot_graph(
    stack: &mut Stack,
    language: *const TSLanguage,
    mut f: *mut c_void,
) -> bool {
    array_reserve(&mut stack.iterators, 32);
    if f.is_null() {
        f = stderr_file();
    }

    fprintf(f, c"digraph stack {\n".as_ptr().cast::<i8>());
    fprintf(f, c"rankdir=\"RL\";\n".as_ptr().cast::<i8>());
    fprintf(f, c"edge [arrowhead=none]\n".as_ptr().cast::<i8>());

    let mut visited_nodes: Array<*mut StackNode> = array_new();

    array_clear(&mut stack.iterators);
    for i in 0..stack.heads.size {
        if stack_head(stack, i).status == StackStatus::Halted {
            continue;
        }
        let node_count_since_error = stack_node_count_since_error(stack, i);
        let error_cost = stack_error_cost(stack, i);
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
            head.node as *const c_void,
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
            for j in 0..summary.size {
                let entry = array_get_ref(summary, j);
                fprintf(f, c" %u".as_ptr().cast::<i8>(), u32::from(entry.state));
            }
        }

        if !head.last_external_token.ptr.is_null() {
            let state = subtree_external_scanner_state(&head.last_external_token);
            let data = external_scanner_state_data(state);
            fprintf(f, c"\nexternal_scanner_state:".as_ptr().cast::<i8>());
            for j in 0..state.length {
                fprintf(
                    f,
                    c" %2X".as_ptr().cast::<i8>(),
                    u32::from(*data.add(j as usize)),
                );
            }
        }

        fprintf(f, c"\"]\n".as_ptr().cast::<i8>());

        let iter = StackIterator {
            node: head.node,
            subtrees: array_new(),
            subtree_count: 0,
        };
        array_push(&mut stack.iterators, iter);
    }

    loop {
        let mut all_iterators_done = true;

        for i in 0..stack.iterators.size {
            let iterator = ptr::read(array_get_ref(&stack.iterators, i));
            let mut node = iterator.node;

            for j in 0..visited_nodes.size {
                if *array_get_ref(&visited_nodes, j) == node {
                    node = ptr::null_mut();
                    break;
                }
            }

            if node.is_null() {
                continue;
            }
            all_iterators_done = false;
            let node_ref = ptr_ref(node);

            fprintf(f, c"node_%p [".as_ptr().cast::<i8>(), node as *const c_void);
            if node_ref.state == ERROR_STATE {
                fprintf(f, c"label=\"?\"".as_ptr().cast::<i8>());
            } else if node_ref.link_count == 1
                && !node_ref.links[0].subtree.ptr.is_null()
                && subtree_extra(node_ref.links[0].subtree)
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
                    node as *const c_void,
                    link.node as *const c_void,
                );
                let subtree = link.subtree;
                if !subtree.ptr.is_null() && subtree_extra(subtree) {
                    fprintf(f, c"fontcolor=gray ".as_ptr().cast::<i8>());
                }

                if subtree.ptr.is_null() {
                    fprintf(f, c"color=red".as_ptr().cast::<i8>());
                } else {
                    fprintf(f, c"label=\"".as_ptr().cast::<i8>());
                    let quoted = subtree_visible(subtree) && !subtree_named(subtree);
                    if quoted {
                        fprintf(f, c"'".as_ptr().cast::<i8>());
                    }
                    language_write_symbol_as_dot_string(language, f, subtree_symbol(subtree));
                    if quoted {
                        fprintf(f, c"'".as_ptr().cast::<i8>());
                    }
                    fprintf(f, c"\"".as_ptr().cast::<i8>());
                    fprintf(
                        f,
                        c"labeltooltip=\"error_cost: %u\ndynamic_precedence: %d\""
                            .as_ptr()
                            .cast::<i8>(),
                        subtree_error_cost(subtree),
                        subtree_dynamic_precedence(subtree),
                    );
                }

                fprintf(f, c"];\n".as_ptr().cast::<i8>());

                let next_iterator = if j == 0 {
                    array_get_mut(&mut stack.iterators, i)
                } else {
                    array_push(&mut stack.iterators, ptr::read(&iterator));
                    array_back_mut(&mut stack.iterators)
                };
                next_iterator.node = link.node;
            }

            array_push(&mut visited_nodes, node);
        }
        if all_iterators_done {
            break;
        }
    }

    fprintf(f, c"}\n".as_ptr().cast::<i8>());

    array_delete(&mut visited_nodes);
    true
}
