use super::{
    array_swap, language_full, language_lookup, parser_log, parser_symbol_name, ptr, ptr_mut,
    ptr_ref, stack_merge, stack_pop_all, stack_pop_count, stack_push, stack_remove_version,
    subtree_array_clear, subtree_array_delete, subtree_array_remove_trailing_extras,
    subtree_compare, subtree_from_mut, subtree_last_external_token, subtree_make_mut,
    subtree_new_node, subtree_release, subtree_retain, subtree_set_extra, ts_language_next_state,
    DisplayCStr, MutableSubtree, ReduceAction, StackVersion, Subtree, SubtreeArray, TSParser,
    TSStateId, TSSymbol, Write, MAX_VERSION_COUNT, MAX_VERSION_COUNT_OVERFLOW, NULL_SUBTREE,
    STACK_VERSION_NONE, TS_BUILTIN_SYM_ERROR, TS_BUILTIN_SYM_ERROR_REPEAT, TS_TREE_STATE_NONE,
};

pub(super) unsafe fn parser_select_tree(
    parser: &mut TSParser,
    left: Subtree,
    right: Subtree,
) -> bool {
    if left.is_null() {
        return true;
    }
    if right.is_null() {
        return false;
    }

    let left_error_cost = left.error_cost();
    let right_error_cost = right.error_cost();
    if right_error_cost < left_error_cost {
        parser_log(parser, |context, log| {
            write!(
                log,
                "select_smaller_error symbol:{}, over_symbol:{}",
                DisplayCStr(parser_symbol_name(context.language, right.symbol())),
                DisplayCStr(parser_symbol_name(context.language, left.symbol()))
            )
        });
        return true;
    }
    if left_error_cost < right_error_cost {
        parser_log(parser, |context, log| {
            write!(
                log,
                "select_smaller_error symbol:{}, over_symbol:{}",
                DisplayCStr(parser_symbol_name(context.language, left.symbol())),
                DisplayCStr(parser_symbol_name(context.language, right.symbol()))
            )
        });
        return false;
    }

    let left_precedence = left.dynamic_precedence();
    let right_precedence = right.dynamic_precedence();
    if right_precedence != left_precedence {
        let select_right = right_precedence > left_precedence;
        let (selected, rejected, precedence, other_precedence) = if select_right {
            (right, left, right_precedence, left_precedence)
        } else {
            (left, right, left_precedence, right_precedence)
        };
        parser_log(parser, |context, log| {
            write!(
                log,
                "select_higher_precedence symbol:{}, prec:{precedence}, over_symbol:{}, other_prec:{other_precedence}",
                DisplayCStr(parser_symbol_name(context.language, selected.symbol())),
                DisplayCStr(parser_symbol_name(context.language, rejected.symbol()))
            )
        });
        return select_right;
    }

    if left_error_cost > 0 {
        return true;
    }

    match subtree_compare(left, right, &mut parser.tree_pool) {
        -1 => {
            parser_log(parser, |context, log| {
                write!(
                    log,
                    "select_earlier symbol:{}, over_symbol:{}",
                    DisplayCStr(parser_symbol_name(context.language, left.symbol())),
                    DisplayCStr(parser_symbol_name(context.language, right.symbol()))
                )
            });
            false
        }
        1 => {
            parser_log(parser, |context, log| {
                write!(
                    log,
                    "select_earlier symbol:{}, over_symbol:{}",
                    DisplayCStr(parser_symbol_name(context.language, right.symbol())),
                    DisplayCStr(parser_symbol_name(context.language, left.symbol()))
                )
            });
            true
        }
        _ => {
            parser_log(parser, |context, log| {
                write!(
                    log,
                    "select_existing symbol:{}, over_symbol:{}",
                    DisplayCStr(parser_symbol_name(context.language, left.symbol())),
                    DisplayCStr(parser_symbol_name(context.language, right.symbol()))
                )
            });
            false
        }
    }
}

unsafe fn parser_select_children(
    parser: &mut TSParser,
    left: Subtree,
    children: &SubtreeArray,
) -> bool {
    parser.scratch_trees.assign(children);
    let scratch_tree =
        subtree_new_node(left.symbol(), &mut parser.scratch_trees, 0, parser.language);
    parser_select_tree(parser, left, subtree_from_mut(scratch_tree))
}

pub(super) unsafe fn parser_new_node(
    parser: &mut TSParser,
    symbol: TSSymbol,
    children: &mut SubtreeArray,
    production_id: u32,
) -> MutableSubtree {
    subtree_new_node(symbol, children, production_id, parser.language)
}

pub(super) unsafe fn parser_shift(
    parser: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    lookahead: Subtree,
    extra: bool,
) {
    let is_leaf = lookahead.child_count() == 0;
    let subtree_to_push = if extra != lookahead.extra() && is_leaf {
        let mut result = subtree_make_mut(&mut parser.tree_pool, lookahead);
        subtree_set_extra(&mut result, extra);
        subtree_from_mut(result)
    } else {
        lookahead
    };

    let stack = ptr_mut(parser.stack);
    stack_push(stack, version, subtree_to_push, state);
    if subtree_to_push.has_external_tokens() {
        stack.set_last_external_token(version, subtree_last_external_token(subtree_to_push));
    }
}

unsafe fn parser_finish_reduction(
    parser: &mut TSParser,
    version: StackVersion,
    parent: MutableSubtree,
    state: TSStateId,
    action: ReduceAction,
    end_of_non_terminal_extra: bool,
    parse_state: TSStateId,
) {
    let next_state = if action.symbol != TS_BUILTIN_SYM_ERROR
        && action.symbol != TS_BUILTIN_SYM_ERROR_REPEAT
        && u32::from(action.symbol) >= language_full(parser.language).token_count
    {
        language_lookup(parser.language, state, action.symbol)
    } else {
        ts_language_next_state(parser.language, state, action.symbol)
    };

    if end_of_non_terminal_extra && next_state == state {
        parent.heap_data_mut().set_extra(true);
    }
    parent.heap_data_mut().parse_state = parse_state;
    parent.heap_data_mut().children_mut().dynamic_precedence += action.dynamic_precedence;

    let stack = ptr_mut(parser.stack);
    stack_push(stack, version, subtree_from_mut(parent), next_state);
    for &extra in parser.trailing_extras.as_slice() {
        stack_push(stack, version, extra, next_state);
    }
}

/// Build and push one parent for every distinct path produced by a GLR pop.
///
/// Equivalent child lists are resolved before the parent is pushed, and the
/// resulting version is merged back into an earlier compatible version.
pub(super) unsafe fn parser_reduce(
    parser: &mut TSParser,
    version: StackVersion,
    action: ReduceAction,
    invalidate_parse_state: bool,
    end_of_non_terminal_extra: bool,
) -> StackVersion {
    let initial_version_count = ptr_ref(parser.stack).version_count();
    let pop = stack_pop_count(ptr_mut(parser.stack), version, action.count);
    let stack = ptr_mut(parser.stack);
    let halted_version_count = stack.halted_version_count();
    let mut removed_version_count = 0;
    let mut i = 0;
    while i < pop.size {
        let mut slice = ptr::read(pop.get_unchecked(i));
        let slice_version = slice.version - removed_version_count;

        if slice_version > MAX_VERSION_COUNT + MAX_VERSION_COUNT_OVERFLOW + halted_version_count {
            stack_remove_version(stack, slice_version);
            subtree_array_delete(&mut parser.tree_pool, &mut slice.subtrees);
            removed_version_count += 1;
            while i + 1 < pop.size {
                parser_log(parser, |_, log| {
                    log.write_str("aborting reduce with too many versions")
                });
                let mut next_slice = ptr::read(pop.get_unchecked(i + 1));
                if next_slice.version != slice.version {
                    break;
                }
                subtree_array_delete(&mut parser.tree_pool, &mut next_slice.subtrees);
                i += 1;
            }
            i += 1;
            continue;
        }

        let mut children = slice.subtrees;
        subtree_array_remove_trailing_extras(&mut children, &mut parser.trailing_extras);
        let mut parent = parser_new_node(
            parser,
            action.symbol,
            &mut children,
            u32::from(action.production_id),
        );

        while i + 1 < pop.size {
            let next_slice = ptr::read(pop.get_unchecked(i + 1));
            if next_slice.version != slice.version {
                break;
            }
            i += 1;
            let mut next_children = next_slice.subtrees;
            subtree_array_remove_trailing_extras(&mut next_children, &mut parser.trailing_extras2);

            if parser_select_children(parser, subtree_from_mut(parent), &next_children) {
                subtree_array_clear(&mut parser.tree_pool, &mut parser.trailing_extras);
                subtree_release(&mut parser.tree_pool, subtree_from_mut(parent));
                array_swap(&mut parser.trailing_extras, &mut parser.trailing_extras2);
                parent = parser_new_node(
                    parser,
                    action.symbol,
                    &mut next_children,
                    u32::from(action.production_id),
                );
            } else {
                subtree_array_clear(&mut parser.tree_pool, &mut parser.trailing_extras2);
                subtree_array_delete(&mut parser.tree_pool, &mut next_children);
            }
        }

        let state = stack.state(slice_version);
        let parse_state = if invalidate_parse_state || pop.size > 1 || initial_version_count > 1 {
            TS_TREE_STATE_NONE
        } else {
            state
        };
        parser_finish_reduction(
            parser,
            slice_version,
            parent,
            state,
            action,
            end_of_non_terminal_extra,
            parse_state,
        );

        for candidate in 0..slice_version {
            if candidate != version && stack_merge(stack, candidate, slice_version) {
                removed_version_count += 1;
                break;
            }
        }
        i += 1;
    }

    if stack.version_count() > initial_version_count {
        initial_version_count
    } else {
        STACK_VERSION_NONE
    }
}

/// Finish a successful version and select its best complete syntax tree.
pub(super) unsafe fn parser_accept(
    parser: &mut TSParser,
    version: StackVersion,
    lookahead: Subtree,
) {
    debug_assert!(lookahead.is_eof());
    let stack = ptr_mut(parser.stack);
    stack_push(stack, version, lookahead, 1);

    let pop = stack_pop_all(stack, version);
    for slice in pop.as_slice() {
        let mut trees = ptr::read(&slice.subtrees);
        let mut root = NULL_SUBTREE;
        let mut index = trees.len();
        while index > 0 {
            index -= 1;
            let tree = trees.as_slice()[index];
            if !tree.extra() {
                debug_assert!(!tree.data.is_inline());
                let children = tree.children();
                for &child in children {
                    subtree_retain(child);
                }
                trees.splice(index as u32, 1, children.len() as u32, children.as_ptr());
                root = subtree_from_mut(parser_new_node(
                    parser,
                    tree.symbol(),
                    &mut trees,
                    u32::from(tree.heap_data().children().production_id),
                ));
                subtree_release(&mut parser.tree_pool, tree);
                break;
            }
        }

        debug_assert!(!root.is_null());
        parser.accept_count += 1;
        if parser.finished_tree.is_null() || parser_select_tree(parser, parser.finished_tree, root)
        {
            if !parser.finished_tree.is_null() {
                subtree_release(&mut parser.tree_pool, parser.finished_tree);
            }
            parser.finished_tree = root;
        } else {
            subtree_release(&mut parser.tree_pool, root);
        }
    }

    stack_remove_version(stack, pop.as_slice()[0].version);
    stack.halt(version);
}
