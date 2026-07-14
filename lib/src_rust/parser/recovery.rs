use super::{
    language_actions, language_full, language_has_actions, language_has_reduce_action,
    language_table_entry, length_sub, lexer_mark_end, lexer_reset, parser_accept,
    parser_better_version_exists, parser_log, parser_log_stack, parser_new_node, parser_reduce,
    parser_symbol_name, parser_version_status, ptr, ptr_mut, ptr_ref, stack_copy_version,
    stack_get_summary, stack_merge, stack_pop_count, stack_pop_error, stack_push,
    stack_record_summary, stack_remove_version, stack_renumber_version, subtree_array_delete,
    subtree_array_remove_trailing_extras, subtree_new_error_node, subtree_new_missing_leaf,
    ts_language_next_state, DisplayCStr, Length, ReduceAction, StackVersion, Subtree, SubtreeArray,
    TSParser, TSStateId, TSSymbol, TableEntry, Write, ERROR_COST_PER_SKIPPED_CHAR,
    ERROR_COST_PER_SKIPPED_LINE, ERROR_COST_PER_SKIPPED_TREE, ERROR_STATE, MAX_SUMMARY_DEPTH,
    MAX_VERSION_COUNT, NULL_SUBTREE, STACK_VERSION_NONE, TSPARSE_ACTION_TYPE_RECOVER,
    TSPARSE_ACTION_TYPE_REDUCE, TSPARSE_ACTION_TYPE_SHIFT, TS_BUILTIN_SYM_ERROR_REPEAT,
};

// ---------------------------------------------------------------------------
// Internal helpers — error recovery
// ---------------------------------------------------------------------------

unsafe fn parser_do_all_potential_reductions(
    self_: &mut TSParser,
    starting_version: StackVersion,
    lookahead_symbol: TSSymbol,
) -> bool {
    let lang = language_full(self_.language);
    let initial_version_count = ptr_ref(self_.stack).version_count();

    let mut can_shift_lookahead_symbol = false;
    let mut version = starting_version;
    let mut i: u32 = 0;
    loop {
        let version_count = ptr_ref(self_.stack).version_count();
        if version >= version_count {
            break;
        }

        let merged = 'merge: {
            for j in initial_version_count..version {
                if stack_merge(ptr_mut(self_.stack), j, version) {
                    break 'merge true;
                }
            }
            false
        };
        if merged {
            i += 1;
            continue;
        }

        let state = ptr_ref(self_.stack).state(version);
        let mut has_shift_action = false;
        self_.reduce_actions.clear();

        let (first_symbol, end_symbol): (TSSymbol, TSSymbol) = if lookahead_symbol != 0 {
            (lookahead_symbol, lookahead_symbol + 1)
        } else {
            (1, lang.token_count as TSSymbol)
        };

        let mut symbol = first_symbol;
        while symbol < end_symbol {
            let mut entry = TableEntry::empty();
            language_table_entry(self_.language, state, symbol, &mut entry);
            for j in 0..entry.action_count {
                let action = *entry.actions.add(j as usize);
                match action.type_ {
                    TSPARSE_ACTION_TYPE_SHIFT | TSPARSE_ACTION_TYPE_RECOVER
                        if !action.shift.extra && !action.shift.repetition =>
                    {
                        has_shift_action = true;
                    }
                    TSPARSE_ACTION_TYPE_REDUCE if action.reduce.child_count > 0 => {
                        self_.reduce_actions.add(ReduceAction {
                            symbol: action.reduce.symbol,
                            count: u32::from(action.reduce.child_count),
                            dynamic_precedence: i32::from(action.reduce.dynamic_precedence),
                            production_id: action.reduce.production_id,
                        });
                    }
                    _ => {}
                }
            }
            symbol += 1;
        }

        let mut reduction_version = STACK_VERSION_NONE;
        for j in 0..self_.reduce_actions.size {
            let action = self_.reduce_actions.as_slice()[j as usize];
            reduction_version = parser_reduce(self_, version, action, true, false);
        }

        if has_shift_action {
            can_shift_lookahead_symbol = true;
        } else if reduction_version != STACK_VERSION_NONE && i < MAX_VERSION_COUNT {
            stack_renumber_version(ptr_mut(self_.stack), reduction_version, version);
            i += 1;
            continue;
        } else if lookahead_symbol != 0 {
            stack_remove_version(ptr_mut(self_.stack), version);
        }

        if version == starting_version {
            version = version_count;
        } else {
            version += 1;
        }
        i += 1;
    }

    can_shift_lookahead_symbol
}

unsafe fn parser_recover_to_state(
    self_: &mut TSParser,
    version: StackVersion,
    depth: u32,
    goal_state: TSStateId,
) -> bool {
    let stack = ptr_mut(self_.stack);
    let mut pop = stack_pop_count(stack, version, depth);
    let mut previous_version = STACK_VERSION_NONE;

    let mut i: u32 = 0;
    while i < pop.size {
        let mut slice = ptr::read(pop.get_unchecked(i));

        if slice.version == previous_version {
            subtree_array_delete(&mut self_.tree_pool, &mut slice.subtrees);
            pop.erase(i);
            continue;
        }

        if stack.state(slice.version) != goal_state {
            stack.halt(slice.version);
            subtree_array_delete(&mut self_.tree_pool, &mut slice.subtrees);
            pop.erase(i);
            continue;
        }

        let mut error_trees = stack_pop_error(stack, slice.version);
        if let Some(&error_tree) = error_trees.as_slice().first() {
            debug_assert_eq!(error_trees.len(), 1);
            let error_child_count = error_tree.child_count();
            if error_child_count > 0 {
                let error_children = error_tree.children();
                slice
                    .subtrees
                    .splice(0, 0, error_child_count, error_children.as_ptr());
                for child in error_children {
                    (*child).retain();
                }
            }
            subtree_array_delete(&mut self_.tree_pool, &mut error_trees);
        }

        subtree_array_remove_trailing_extras(&mut slice.subtrees, &mut self_.trailing_extras);

        if !slice.subtrees.is_empty() {
            let error = subtree_new_error_node(slice.subtrees, true, self_.language);
            stack_push(stack, slice.version, error, goal_state);
        } else {
            slice.subtrees.delete();
        }

        for &tree in self_.trailing_extras.as_slice() {
            stack_push(stack, slice.version, tree, goal_state);
        }

        previous_version = slice.version;
        i += 1;
    }

    previous_version != STACK_VERSION_NONE
}

/// Recover a paused version, preferring an earlier valid state before skipping.
///
/// Successful earlier-state recovery keeps the original version and may fork a
/// new one. Remaining inactive forks are removed, EOF is finalized immediately,
/// and only then is the current token wrapped in an error repetition.
pub(super) unsafe fn parser_recover(
    self_: &mut TSParser,
    version: StackVersion,
    mut lookahead: Subtree,
) {
    let mut did_recover = false;
    let stack = ptr_mut(self_.stack);
    let previous_version_count = stack.version_count();
    let position = stack.position(version);
    let node_count_since_error = stack.node_count_since_error(version);
    let current_error_cost = stack.error_cost(version);
    let summary = stack_get_summary(stack, version);

    // Strategy 1: Find a previous state where the lookahead is valid.
    if let Some(summary) = summary.filter(|_| !lookahead.is_error()) {
        for &entry in summary.as_slice() {
            if entry.state == ERROR_STATE {
                continue;
            }
            if entry.position.bytes == position.bytes {
                continue;
            }
            let mut depth = entry.depth;
            if node_count_since_error > 0 {
                depth += 1;
            }

            // Check for redundant versions
            let would_merge = 'merge: {
                for j in 0..previous_version_count {
                    if stack.state(j) == entry.state && stack.position(j).bytes == position.bytes {
                        break 'merge true;
                    }
                }
                false
            };
            if would_merge {
                continue;
            }

            let new_cost = current_error_cost
                + entry.depth * ERROR_COST_PER_SKIPPED_TREE
                + (position.bytes - entry.position.bytes) * ERROR_COST_PER_SKIPPED_CHAR
                + (position.extent.row - entry.position.extent.row) * ERROR_COST_PER_SKIPPED_LINE;
            if parser_better_version_exists(self_, version, false, new_cost) {
                break;
            }

            if language_has_actions(self_.language, entry.state, lookahead.symbol())
                && parser_recover_to_state(self_, version, depth, entry.state)
            {
                did_recover = true;
                parser_log(self_, |_, log| {
                    write!(
                        log,
                        "recover_to_previous state:{}, depth:{depth}",
                        u32::from(entry.state)
                    )
                });
                parser_log_stack(self_);
                break;
            }
        }
    }

    // Remove halted versions
    let mut i = previous_version_count;
    while i < stack.version_count() {
        if !stack.is_active(i) {
            parser_log(self_, |_, log| write!(log, "removed paused version:{i}"));
            stack_remove_version(stack, i);
            parser_log_stack(self_);
        } else {
            i += 1;
        }
    }

    // EOF: wrap everything and terminate
    if lookahead.is_eof() {
        parser_log(self_, |_, log| log.write_str("recover_eof"));
        let children = SubtreeArray::new();
        let parent = subtree_new_error_node(children, false, self_.language);
        stack_push(stack, version, parent, 1);
        parser_accept(self_, version, lookahead);
        return;
    }

    // Strategy 2: skip the current token
    let new_cost = current_error_cost
        + ERROR_COST_PER_SKIPPED_TREE
        + lookahead.total_bytes() * ERROR_COST_PER_SKIPPED_CHAR
        + lookahead.total_size().extent.row * ERROR_COST_PER_SKIPPED_LINE;
    let cannot_skip_after_recovery = did_recover
        && (stack.version_count() > MAX_VERSION_COUNT
            || lookahead.has_external_scanner_state_change());
    if cannot_skip_after_recovery || parser_better_version_exists(self_, version, false, new_cost) {
        stack.halt(version);
        lookahead.release(&mut self_.tree_pool);
        return;
    }

    // Mark extra tokens
    let mut n: u32 = 0;
    let actions = language_actions(self_.language, 1, lookahead.symbol(), &mut n);
    let marks_extra = if n == 0 {
        false
    } else {
        let action = ptr_ref(actions.add(n as usize - 1));
        action.type_ == TSPARSE_ACTION_TYPE_SHIFT && action.shift.extra
    };
    if marks_extra {
        let mut mutable_lookahead = lookahead.make_mut(&mut self_.tree_pool);
        mutable_lookahead.set_extra(true);
        lookahead = mutable_lookahead.into_immutable();
    }

    // Wrap the lookahead in an ERROR
    parser_log(self_, |context, log| {
        write!(
            log,
            "skip_token symbol:{}",
            DisplayCStr(parser_symbol_name(context.language, lookahead.symbol()))
        )
    });
    let mut children = SubtreeArray::new();
    children.reserve(1);
    children.push(lookahead);
    let mut error_repeat = parser_new_node(self_, TS_BUILTIN_SYM_ERROR_REPEAT, children, 0);

    // Merge with existing error on top of stack
    if node_count_since_error > 0 {
        let mut pop = stack_pop_count(stack, version, 1);

        if pop.size > 1 {
            for pi in 1..pop.size {
                subtree_array_delete(
                    &mut self_.tree_pool,
                    &mut pop.get_unchecked_mut(pi).subtrees,
                );
            }
            while stack.version_count() > pop.get_unchecked(0).version + 1 {
                stack_remove_version(stack, pop.get_unchecked(0).version + 1);
            }
        }

        stack_renumber_version(stack, pop.get_unchecked(0).version, version);
        let slot = &mut pop.get_unchecked_mut(0).subtrees;
        slot.push(error_repeat.into_immutable());
        let children = core::mem::replace(slot, SubtreeArray::new());
        error_repeat = parser_new_node(self_, TS_BUILTIN_SYM_ERROR_REPEAT, children, 0);
    }

    // Push the ERROR
    stack_push(stack, version, error_repeat.into_immutable(), ERROR_STATE);
    if lookahead.has_external_tokens() {
        stack.set_last_external_token(version, lookahead.last_external_token());
    }

    let mut has_error = true;
    for vi in 0..stack.version_count() {
        let status = parser_version_status(self_, vi);
        if !status.is_in_error {
            has_error = false;
            break;
        }
    }
    self_.has_error = has_error;
}

/// Try one missing terminal before committing a version to error recovery.
unsafe fn parser_try_insert_missing_token(
    self_: &mut TSParser,
    version: StackVersion,
    lookahead: Subtree,
    position: Length,
) -> bool {
    let state = ptr_ref(self_.stack).state(version);
    let language = language_full(self_.language);
    let mut missing_symbol: TSSymbol = 1;
    while u32::from(missing_symbol) < language.token_count {
        let state_after_missing_symbol =
            ts_language_next_state(self_.language, state, missing_symbol);
        if state_after_missing_symbol == 0 || state_after_missing_symbol == state {
            missing_symbol += 1;
            continue;
        }
        if !language_has_reduce_action(
            self_.language,
            state_after_missing_symbol,
            lookahead.symbol(),
        ) {
            missing_symbol += 1;
            continue;
        }

        // The lexer may snap to the next included range, so use its marked end
        // to place the missing token at the visible input position.
        lexer_reset(&mut self_.lexer, position);
        lexer_mark_end(&mut self_.lexer);
        let padding = length_sub(self_.lexer.token_end_position, position);
        let lookahead_bytes = lookahead.total_bytes() + lookahead.lookahead_bytes();
        let candidate = stack_copy_version(ptr_mut(self_.stack), version);
        let missing_tree = subtree_new_missing_leaf(
            &mut self_.tree_pool,
            missing_symbol,
            padding,
            lookahead_bytes,
            self_.language,
        );
        stack_push(
            ptr_mut(self_.stack),
            candidate,
            missing_tree,
            state_after_missing_symbol,
        );

        if parser_do_all_potential_reductions(self_, candidate, lookahead.symbol()) {
            parser_log(self_, |context, log| {
                write!(
                    log,
                    "recover_with_missing symbol:{}, state:{}",
                    DisplayCStr(parser_symbol_name(context.language, missing_symbol)),
                    u32::from(ptr_ref(context.stack).state(candidate))
                )
            });
            return true;
        }
        missing_symbol += 1;
    }
    false
}

/// Start recovery after all ordinary parse actions for a version have failed.
pub(super) unsafe fn parser_handle_error(
    self_: &mut TSParser,
    version: StackVersion,
    lookahead: Subtree,
) {
    let previous_version_count = ptr_ref(self_.stack).version_count();

    // Perform any reductions that can happen in this state, regardless of the lookahead. After
    // skipping one or more invalid tokens, the parser might find a token that would have allowed
    // a reduction to take place.
    parser_do_all_potential_reductions(self_, version, 0);
    let version_count = ptr_ref(self_.stack).version_count();
    let position = ptr_ref(self_.stack).position(version);

    // Push a discontinuity onto the stack. Merge all of the stack versions that
    // were created in the previous step.
    let mut did_insert_missing_token = false;
    let mut v = version;
    while v < version_count {
        if !did_insert_missing_token {
            did_insert_missing_token =
                parser_try_insert_missing_token(self_, v, lookahead, position);
        }

        stack_push(ptr_mut(self_.stack), v, NULL_SUBTREE, ERROR_STATE);
        v = if v == version {
            previous_version_count
        } else {
            v + 1
        };
    }

    for _i in previous_version_count..version_count {
        let did_merge = stack_merge(ptr_mut(self_.stack), version, previous_version_count);
        debug_assert!(did_merge);
    }

    stack_record_summary(ptr_mut(self_.stack), version, MAX_SUMMARY_DEPTH);

    // Begin recovery with the current lookahead node, rather than waiting for the
    // next turn of the parse loop. This ensures that the tree accounts for the
    // current lookahead token's "lookahead bytes" value, which describes how far
    // the lexer needed to look ahead beyond the content of the token in order to
    // recognize it.
    parser_recover(self_, version, lookahead);

    parser_log_stack(self_);
}

// ---------------------------------------------------------------------------
