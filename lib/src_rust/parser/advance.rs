//! Parse-table action interpreter and GLR version selection.
//!
//! [`parser_advance`] advances one stack version until it shifts a token,
//! accepts a tree, pauses at an error, or is merged into another version.
//! Reductions keep the same lookahead and return to the table in a new state;
//! conflicts can therefore create several versions before any input is
//! consumed. After all active versions have had a chance to move,
//! [`parser_condense_stack`] merges compatible versions and prunes the ones
//! that are clearly worse.

use core::fmt::Write;

use crate::ffi::TSStateId;

use super::super::error_costs::{ERROR_COST_PER_SKIPPED_TREE, ERROR_STATE};
use super::super::language::{
    language_full, language_is_reserved_word, TSLexerMode, TSParseAction, TableEntry,
    TSPARSE_ACTION_TYPE_ACCEPT, TSPARSE_ACTION_TYPE_RECOVER, TSPARSE_ACTION_TYPE_REDUCE,
    TSPARSE_ACTION_TYPE_SHIFT,
};
use super::super::reduce_action::ReduceAction;
use super::super::stack::{
    stack_can_merge, stack_materialize, stack_merge, stack_remove_version, stack_renumber_version,
    stack_swap_versions, stack_try_enable_window, StackVersion, STACK_VERSION_NONE,
};
use super::super::subtree::Subtree;
use super::super::utils::{ptr_mut, ptr_ref};
use super::actions::{parser_accept, parser_reduce, parser_shift};
use super::lexing::{parser_get_initial_lookahead, parser_lex_lookahead};
use super::logging::{
    parser_log, parser_log_stack, parser_symbol_name, parser_tree_name, DisplayCStr,
};
use super::recovery::{parser_handle_error, parser_recover};
use super::{
    parser_table_entry, ErrorComparison, ErrorStatus, TSParser, MAX_COST_DIFFERENCE,
    MAX_VERSION_COUNT, OP_COUNT_PER_PARSER_CALLBACK_CHECK,
};

// ---------------------------------------------------------------------------
// Internal helpers — version comparison
// ---------------------------------------------------------------------------

const fn parser_compare_versions(a: ErrorStatus, b: ErrorStatus) -> ErrorComparison {
    if !a.is_in_error && b.is_in_error {
        if a.cost < b.cost {
            return ErrorComparison::TakeLeft;
        }
        return ErrorComparison::PreferLeft;
    }

    if a.is_in_error && !b.is_in_error {
        if b.cost < a.cost {
            return ErrorComparison::TakeRight;
        }
        return ErrorComparison::PreferRight;
    }

    if a.cost < b.cost {
        if (b.cost - a.cost) * (1 + a.node_count) > MAX_COST_DIFFERENCE {
            return ErrorComparison::TakeLeft;
        }
        return ErrorComparison::PreferLeft;
    }

    if b.cost < a.cost {
        if (a.cost - b.cost) * (1 + b.node_count) > MAX_COST_DIFFERENCE {
            return ErrorComparison::TakeRight;
        }
        return ErrorComparison::PreferRight;
    }

    if a.dynamic_precedence > b.dynamic_precedence {
        return ErrorComparison::PreferLeft;
    }
    if b.dynamic_precedence > a.dynamic_precedence {
        return ErrorComparison::PreferRight;
    }
    ErrorComparison::None
}

pub(super) unsafe fn parser_version_status(
    self_: &mut TSParser,
    version: StackVersion,
) -> ErrorStatus {
    let stack = ptr_mut(self_.stack);
    let mut cost = stack.error_cost(version);
    let head = stack.head(version);
    let is_paused = head.is_paused();
    let dynamic_precedence = stack.dynamic_precedence(version);
    let state = head.state();
    if is_paused {
        cost += ERROR_COST_PER_SKIPPED_TREE;
    }
    ErrorStatus {
        cost,
        node_count: stack.node_count_since_error(version),
        dynamic_precedence,
        is_in_error: is_paused || state == ERROR_STATE,
    }
}

pub(super) unsafe fn parser_better_version_exists(
    self_: &mut TSParser,
    version: StackVersion,
    is_in_error: bool,
    cost: u32,
) -> bool {
    if !self_.finished_tree.is_null()
        && self_.finished_tree.error_cost(self_.tree_pool.arena()) <= cost
    {
        return true;
    }

    let stack = ptr_mut(self_.stack);
    let head = stack.head(version);
    let position = head.position();
    let dynamic_precedence = stack.dynamic_precedence(version);
    let status = ErrorStatus {
        cost,
        is_in_error,
        dynamic_precedence,
        node_count: stack.node_count_since_error(version),
    };

    let n = stack.version_count();
    for i in 0..n {
        if i == version
            || !stack.head(i).is_active()
            || stack.head(i).position().bytes < position.bytes
        {
            continue;
        }
        let status_i = parser_version_status(self_, i);
        match parser_compare_versions(status, status_i) {
            ErrorComparison::TakeRight => return true,
            ErrorComparison::PreferRight if stack_can_merge(ptr_ref(self_.stack), i, version) => {
                return true;
            }
            _ => {}
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Internal helpers — lexing
// ---------------------------------------------------------------------------

pub(super) unsafe fn parser_call_main_lex_fn(self_: &mut TSParser, lex_mode: TSLexerMode) -> bool {
    (language_full(self_.language).lex_fn.unwrap())(&mut self_.lexer.data, lex_mode.lex_state)
}

pub(super) unsafe fn parser_call_keyword_lex_fn(self_: &mut TSParser) -> bool {
    (language_full(self_.language).keyword_lex_fn.unwrap())(&mut self_.lexer.data, 0)
}

// Internal helpers — advance & condense
// ---------------------------------------------------------------------------

enum ParseActionsResult {
    Done,
    Reductions {
        did_reduce: bool,
        last_reduction_version: StackVersion,
    },
}

pub(super) unsafe fn parser_check_progress(
    self_: &mut TSParser,
    lookahead: Option<&mut Subtree>,
    position: Option<u32>,
    operations: u32,
) -> bool {
    self_.operation_count += operations;
    if self_.operation_count >= OP_COUNT_PER_PARSER_CALLBACK_CHECK {
        self_.operation_count = 0;
    }
    if self_.parse_options.progress_callback.is_none() {
        return true;
    }
    if let Some(position) = position {
        self_.parse_state.current_byte_offset = position;
        self_.parse_state.has_error = self_.has_error;
    }
    if self_.operation_count == 0
        && self_.parse_options.progress_callback.unwrap()(&mut self_.parse_state)
    {
        if let Some(lookahead) = lookahead {
            if !lookahead.is_null() {
                (*lookahead).release(&mut self_.tree_pool);
            }
        }
        return false;
    }
    true
}

unsafe fn parser_shift_for_action(
    self_: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    lookahead: &mut Subtree,
    action: TSParseAction,
) {
    let shift = action.shift;
    let next_state = if shift.extra {
        parser_log(self_, |_, log| log.write_str("shift_extra"));
        state
    } else {
        parser_log(self_, |_, log| {
            write!(log, "shift state:{}", u32::from(shift.state))
        });
        shift.state
    };

    parser_shift(self_, version, next_state, *lookahead, shift.extra);
}

unsafe fn parser_apply_parse_actions(
    self_: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    lookahead: &mut Subtree,
    table_entry: &TableEntry,
) -> ParseActionsResult {
    if table_entry.action_count == 1 {
        return parser_apply_single_parse_action(
            self_,
            version,
            state,
            lookahead,
            *table_entry.actions,
        );
    }

    parser_apply_multiple_parse_actions(self_, version, state, lookahead, table_entry)
}

unsafe fn parser_reduce_for_action(
    self_: &mut TSParser,
    version: StackVersion,
    lookahead: Subtree,
    action: TSParseAction,
    invalidate_parse_state: bool,
) -> StackVersion {
    let reduce = action.reduce;
    let reduce_action = ReduceAction {
        symbol: reduce.symbol,
        count: u32::from(reduce.child_count),
        dynamic_precedence: i32::from(reduce.dynamic_precedence),
        production_id: reduce.production_id,
    };
    parser_log(self_, |context, log| {
        write!(
            log,
            "reduce sym:{}, child_count:{}",
            DisplayCStr(parser_symbol_name(context.language, reduce.symbol)),
            u32::from(reduce.child_count)
        )
    });
    parser_reduce(
        self_,
        version,
        reduce_action,
        invalidate_parse_state,
        lookahead.is_null(),
    )
}

unsafe fn parser_apply_single_parse_action(
    self_: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    lookahead: &mut Subtree,
    action: TSParseAction,
) -> ParseActionsResult {
    match action.type_ {
        TSPARSE_ACTION_TYPE_SHIFT if !action.shift.repetition => {
            parser_shift_for_action(self_, version, state, lookahead, action);
            ParseActionsResult::Done
        }
        TSPARSE_ACTION_TYPE_REDUCE => {
            let reduction_version =
                parser_reduce_for_action(self_, version, *lookahead, action, false);
            ParseActionsResult::Reductions {
                did_reduce: true,
                last_reduction_version: reduction_version,
            }
        }
        TSPARSE_ACTION_TYPE_ACCEPT => {
            parser_log(self_, |_, log| log.write_str("accept"));
            parser_accept(self_, version, *lookahead);
            ParseActionsResult::Done
        }
        TSPARSE_ACTION_TYPE_RECOVER => {
            parser_recover(self_, version, *lookahead);
            ParseActionsResult::Done
        }
        _ => ParseActionsResult::Reductions {
            did_reduce: false,
            last_reduction_version: STACK_VERSION_NONE,
        },
    }
}

#[inline(never)]
unsafe fn parser_apply_multiple_parse_actions(
    self_: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    lookahead: &mut Subtree,
    table_entry: &TableEntry,
) -> ParseActionsResult {
    if table_entry.action_count > 1 {
        stack_materialize(ptr_mut(self_.stack));
    }
    let mut did_reduce = false;
    let mut last_reduction_version = STACK_VERSION_NONE;

    for i in 0..table_entry.action_count {
        let action = *table_entry.actions.add(i as usize);

        match action.type_ {
            TSPARSE_ACTION_TYPE_SHIFT => {
                if action.shift.repetition {
                    break;
                }
                parser_shift_for_action(self_, version, state, lookahead, action);
                return ParseActionsResult::Done;
            }

            TSPARSE_ACTION_TYPE_REDUCE => {
                let reduction_version =
                    parser_reduce_for_action(self_, version, *lookahead, action, true);
                did_reduce = true;
                if reduction_version != STACK_VERSION_NONE {
                    last_reduction_version = reduction_version;
                }
            }

            TSPARSE_ACTION_TYPE_ACCEPT => {
                parser_log(self_, |_, log| log.write_str("accept"));
                parser_accept(self_, version, *lookahead);
                return ParseActionsResult::Done;
            }

            TSPARSE_ACTION_TYPE_RECOVER => {
                parser_recover(self_, version, *lookahead);
                return ParseActionsResult::Done;
            }

            _ => {}
        }
    }

    ParseActionsResult::Reductions {
        did_reduce,
        last_reduction_version,
    }
}

unsafe fn parser_continue_after_reduction(
    self_: &mut TSParser,
    version: StackVersion,
    last_reduction_version: StackVersion,
    state: &mut TSStateId,
    lookahead: Subtree,
    table_entry: &mut TableEntry,
) -> bool {
    stack_renumber_version(ptr_mut(self_.stack), last_reduction_version, version);
    parser_log_stack(self_);
    *state = ptr_ref(self_.stack).head(version).state();

    // At the end of a non-terminal extra rule, the lexer will return a null
    // subtree, because the parser needs to perform a fixed reduction regardless
    // of the lookahead node. After that reduction, run the lexer again from the
    // current parse state.
    if lookahead.is_null() {
        true
    } else {
        parser_table_entry(
            self_,
            *state,
            lookahead.symbol(self_.tree_pool.arena()),
            table_entry,
        );
        false
    }
}

unsafe fn parser_halt_after_merged_reduction(
    self_: &mut TSParser,
    version: StackVersion,
    lookahead: Subtree,
) {
    if !lookahead.is_null() {
        lookahead.release(&mut self_.tree_pool);
    }
    ptr_mut(self_.stack).halt(version);
}

unsafe fn parser_try_keyword_fallback(
    self_: &mut TSParser,
    state: TSStateId,
    lookahead: &mut Subtree,
    table_entry: &mut TableEntry,
) -> bool {
    let keyword_capture_token = language_full(self_.language).keyword_capture_token;
    let mut arena = self_.tree_pool.arena();
    if !(*lookahead).is_keyword(arena)
        || (*lookahead).symbol(arena) == keyword_capture_token
        || language_is_reserved_word(self_.language, state, (*lookahead).symbol(arena))
    {
        return false;
    }

    parser_table_entry(self_, state, keyword_capture_token, table_entry);
    if table_entry.action_count == 0 {
        return false;
    }

    parser_log(self_, |context, log| {
        write!(
            log,
            "switch from_keyword:{}, to_word_token:{}",
            DisplayCStr(parser_tree_name(context.language, arena, *lookahead)),
            DisplayCStr(parser_symbol_name(context.language, keyword_capture_token))
        )
    });

    let mut mutable_lookahead = (*lookahead).make_mut(&mut self_.tree_pool);
    arena = self_.tree_pool.arena();
    mutable_lookahead.set_symbol(arena, keyword_capture_token, self_.language);
    *lookahead = mutable_lookahead.into_immutable();
    true
}

unsafe fn parser_pause_with_error(self_: &mut TSParser, version: StackVersion, lookahead: Subtree) {
    let arena = self_.tree_pool.arena();
    parser_log(self_, |context, log| {
        write!(
            log,
            "detect_error lookahead:{}",
            DisplayCStr(parser_tree_name(context.language, arena, lookahead,))
        )
    });
    ptr_mut(self_.stack).pause(version, lookahead);
}

/// Advance one stack version until it shifts, accepts, recovers, pauses, or halts.
///
/// This is the parser action interpreter. It first obtains a lookahead from the
/// token cache or lexer. Then it repeatedly reads the parse-table
/// entry for `(state, lookahead)` and executes its actions. Reductions keep the
/// same lookahead and continue in the new goto state; shifts consume the
/// lookahead and return to the outer parse loop.
pub(super) unsafe fn parser_advance(self_: &mut TSParser, version: StackVersion) -> bool {
    if self_.lexer.logger.log.is_some() || !self_.dot_graph_file.is_null() {
        stack_materialize(ptr_mut(self_.stack));
    }
    let stack = ptr_ref(self_.stack);
    let head = stack.head(version);
    let mut state = head.state();
    let position = head.position().bytes;
    let last_external_token = head.last_external_token();

    let (mut lookahead, mut table_entry, mut needs_lex) =
        parser_get_initial_lookahead(self_, state, position, last_external_token);

    loop {
        if needs_lex {
            needs_lex = false;
            parser_lex_lookahead(
                self_,
                version,
                state,
                position,
                last_external_token,
                &mut lookahead,
                &mut table_entry,
            );
        }

        // If a progress callback was provided, then check every
        // time a fixed number of parse actions has been processed.
        if !parser_check_progress(self_, Some(&mut lookahead), Some(position), 1) {
            return false;
        }

        let ParseActionsResult::Reductions {
            did_reduce,
            last_reduction_version,
        } = parser_apply_parse_actions(self_, version, state, &mut lookahead, &table_entry)
        else {
            return true;
        };

        // If a reduction was performed, then replace the current stack version
        // with one of the stack versions created by a reduction, and continue
        // processing this version of the stack with the same lookahead symbol.
        if last_reduction_version != STACK_VERSION_NONE {
            needs_lex = parser_continue_after_reduction(
                self_,
                version,
                last_reduction_version,
                &mut state,
                lookahead,
                &mut table_entry,
            );
            continue;
        }

        // A reduction was performed, but was merged into an existing stack version.
        // This version can be discarded.
        if did_reduce {
            parser_halt_after_merged_reduction(self_, version, lookahead);
            return true;
        }

        // If the current lookahead token is a keyword that is not valid, but the
        // default word token *is* valid, then treat the lookahead token as the word
        // token instead.
        if parser_try_keyword_fallback(self_, state, &mut lookahead, &mut table_entry) {
            continue;
        }

        // Otherwise, there is definitely an error in this version of the parse stack.
        // Mark this version as paused and continue processing any other stack
        // versions that exist. If some other version advances successfully, then
        // this version can simply be removed. But if all versions end up paused,
        // then error recovery is needed.
        parser_pause_with_error(self_, version, lookahead);
        return true;
    }
}

pub(super) unsafe fn parser_condense_stack(self_: &mut TSParser) -> u32 {
    let mut made_changes = false;
    let mut min_error_cost = u32::MAX;
    let mut i: StackVersion = 0;
    while i < ptr_ref(self_.stack).version_count() {
        // Prune any versions that have been marked for removal.
        if ptr_ref(self_.stack).head(i).is_halted() {
            stack_remove_version(ptr_mut(self_.stack), i);
            continue;
        }

        // Keep track of the minimum error cost of any stack version so
        // that it can be returned.
        let status_i = parser_version_status(self_, i);
        if !status_i.is_in_error && status_i.cost < min_error_cost {
            min_error_cost = status_i.cost;
        }

        // Examine each pair of stack versions, removing any versions that
        // are clearly worse than another version. Ensure that the versions
        // are ordered from most promising to least promising.
        let mut j: StackVersion = 0;
        while j < i {
            let status_j = parser_version_status(self_, j);

            match parser_compare_versions(status_j, status_i) {
                ErrorComparison::TakeLeft => {
                    made_changes = true;
                    stack_remove_version(ptr_mut(self_.stack), i);
                    i -= 1;
                    break;
                }

                ErrorComparison::PreferLeft | ErrorComparison::None => {
                    if stack_merge(ptr_mut(self_.stack), j, i) {
                        made_changes = true;
                        i -= 1;
                        break;
                    }
                }

                ErrorComparison::PreferRight => {
                    made_changes = true;
                    if stack_merge(ptr_mut(self_.stack), j, i) {
                        i -= 1;
                        break;
                    }
                    stack_swap_versions(ptr_mut(self_.stack), i, j);
                }

                ErrorComparison::TakeRight => {
                    made_changes = true;
                    stack_remove_version(ptr_mut(self_.stack), j);
                    i -= 1;
                    j = j.wrapping_sub(1);
                }
            }
            j = j.wrapping_add(1);
        }
        i = i.wrapping_add(1);
    }

    // Enforce a hard upper bound on the number of stack versions by
    // discarding the least promising versions.
    while ptr_ref(self_.stack).version_count() > MAX_VERSION_COUNT {
        stack_remove_version(ptr_mut(self_.stack), MAX_VERSION_COUNT);
        made_changes = true;
    }

    // If the best-performing stack version is currently paused, or all
    // versions are paused, then resume the best paused version and begin
    // the error recovery process. Otherwise, remove the paused versions.
    if ptr_ref(self_.stack).version_count() > 0 {
        let mut has_unpaused_version = false;
        let mut i: StackVersion = 0;
        let mut n = ptr_ref(self_.stack).version_count();
        while i < n {
            if ptr_ref(self_.stack).head(i).is_paused() {
                if !has_unpaused_version && self_.accept_count < MAX_VERSION_COUNT {
                    parser_log(self_, |_, log| write!(log, "resume version:{i}"));
                    min_error_cost = ptr_ref(self_.stack).error_cost(i);
                    let lookahead = ptr_mut(self_.stack).resume(i);
                    parser_handle_error(self_, i, lookahead);
                    has_unpaused_version = true;
                } else {
                    stack_remove_version(ptr_mut(self_.stack), i);
                    made_changes = true;
                    n -= 1;
                    continue;
                }
            } else {
                has_unpaused_version = true;
            }
            i += 1;
        }
    }

    if made_changes {
        parser_log(self_, |_, log| log.write_str("condense"));
        parser_log_stack(self_);
    }

    if self_.lexer.logger.log.is_none() && self_.dot_graph_file.is_null() {
        stack_try_enable_window(ptr_mut(self_.stack));
    }

    min_error_cost
}
