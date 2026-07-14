use core::ffi::{c_char, c_void, CStr};
use core::fmt::{self, Write};
use core::ptr;

use crate::ffi::{
    TSInput, TSInputEncoding, TSInputEncodingUTF8, TSLanguage, TSLogTypeParse, TSLogger,
    TSParseOptions, TSParseState, TSPoint, TSRange, TSStateId, TSSymbol,
    TREE_SITTER_LANGUAGE_VERSION, TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION,
};

use super::alloc::{free, malloc};
use super::error_costs::{
    ERROR_COST_PER_SKIPPED_CHAR, ERROR_COST_PER_SKIPPED_LINE, ERROR_COST_PER_SKIPPED_TREE,
    ERROR_STATE,
};
use super::language::{
    language_actions, language_full, language_has_actions, language_has_reduce_action,
    language_is_reserved_word, language_lex_mode_for_state, language_lookup, language_table_entry,
    ts_language_next_state, ts_language_symbol_name, TSLexerMode, TSParseAction, TableEntry,
    TSPARSE_ACTION_TYPE_ACCEPT, TSPARSE_ACTION_TYPE_RECOVER, TSPARSE_ACTION_TYPE_REDUCE,
    TSPARSE_ACTION_TYPE_SHIFT,
};
use super::length::{length_sub, length_zero, Length};
use super::lexer::{
    lexer_advance, lexer_delete, lexer_finish, lexer_included_ranges, lexer_included_ranges_slice,
    lexer_is_eof, lexer_mark_end, lexer_new, lexer_reset, lexer_set_included_ranges,
    lexer_set_input, lexer_start, Lexer,
};
use super::reduce_action::{reduce_action_set_add, ReduceAction, ReduceActionSet};
use super::stack::{
    stack_can_merge, stack_clear, stack_copy_version, stack_delete, stack_dynamic_precedence,
    stack_error_cost, stack_get_summary, stack_halt, stack_halted_version_count,
    stack_has_advanced_since_error, stack_is_active, stack_is_halted, stack_is_paused,
    stack_last_external_token, stack_merge, stack_new, stack_node_count_since_error, stack_pause,
    stack_pop_all, stack_pop_count, stack_pop_error, stack_position, stack_print_dot_graph,
    stack_push, stack_record_summary, stack_remove_version, stack_renumber_version, stack_resume,
    stack_set_last_external_token, stack_state, stack_swap_versions, stack_version_count, Stack,
    StackVersion, STACK_VERSION_NONE,
};
use super::subtree::{
    external_scanner_state_eq, subtree_array_clear, subtree_array_delete,
    subtree_array_remove_trailing_extras, subtree_child, subtree_child_count,
    subtree_children_slice, subtree_compare, subtree_compress, subtree_dynamic_precedence,
    subtree_error_cost, subtree_external_scanner_state_eq, subtree_extra, subtree_from_mut,
    subtree_has_external_scanner_state_change, subtree_has_external_tokens, subtree_is_eof,
    subtree_is_error, subtree_is_keyword, subtree_last_external_token, subtree_lookahead_bytes,
    subtree_make_mut, subtree_new_error, subtree_new_error_node, subtree_new_leaf,
    subtree_new_missing_leaf, subtree_new_node, subtree_parse_state, subtree_pool_delete,
    subtree_pool_new, subtree_print_dot_graph, subtree_release, subtree_repeat_depth,
    subtree_retain, subtree_set_external_scanner_state, subtree_set_extra, subtree_set_symbol,
    subtree_size, subtree_symbol, subtree_to_mut_unsafe, subtree_total_bytes, subtree_total_size,
    MutableSubtree, Subtree, SubtreeArray, SubtreePool, NULL_SUBTREE, TS_BUILTIN_SYM_END,
    TS_BUILTIN_SYM_ERROR, TS_BUILTIN_SYM_ERROR_REPEAT, TS_TREE_STATE_NONE,
};
use super::tree::{tree_new, TSTree};
use super::utils::{array_swap, Array};
use super::utils::{ptr_mut, ptr_ref};

// ---------------------------------------------------------------------------
// Extern C functions
// ---------------------------------------------------------------------------

extern "C" {
    // libc
    fn fprintf(f: *mut c_void, fmt: *const i8, ...) -> i32;
    fn fputs(s: *const i8, f: *mut c_void) -> i32;
    fn fputc(c: i32, f: *mut c_void) -> i32;
    // `fdopen` is spelled `_fdopen` on Windows (declared at the call site);
    // `fclose` keeps its name on all platforms.
    #[cfg(not(target_os = "windows"))]
    fn fdopen(fd: i32, mode: *const i8) -> *mut c_void;
    fn fclose(f: *mut c_void) -> i32;
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_VERSION_COUNT: u32 = 6;
const MAX_VERSION_COUNT_OVERFLOW: u32 = 4;
const MAX_SUMMARY_DEPTH: u32 = 16;
const MAX_COST_DIFFERENCE: u32 = 18 * ERROR_COST_PER_SKIPPED_TREE;
const OP_COUNT_PER_PARSER_CALLBACK_CHECK: u32 = 100;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One-token cache shared by stack versions at the same byte offset.
///
/// GLR versions often ask the lexer for the same position and external scanner
/// state. The cache stores the concrete token plus the last external token that
/// determined scanner state, so another version can reuse it only when scanner
/// state is equivalent.
struct TokenCache {
    /// Retained lookahead token.
    token: Subtree,
    /// Retained token carrying the external scanner state used for `token`.
    last_external_token: Subtree,
    /// Byte offset where `token` was lexed.
    byte_index: u32,
}

/// Summary used to compare and prune stack versions.
#[derive(Clone, Copy)]
struct ErrorStatus {
    /// Accumulated recovery/error cost.
    cost: u32,
    /// Number of visible nodes since the last error.
    node_count: u32,
    /// Dynamic precedence for tie-breaking.
    dynamic_precedence: i32,
    /// Whether the version is currently in error recovery.
    is_in_error: bool,
}

/// `ErrorComparison`
#[derive(PartialEq, Eq)]
enum ErrorComparison {
    TakeLeft,
    PreferLeft,
    None,
    PreferRight,
    TakeRight,
}

/// `TSStringInput` — for string-based parsing
struct TSStringInput {
    string: *const c_char,
    length: u32,
}

/// Main parser runtime state.
///
/// One `TSParser` owns all mutable state for a parse: lexer callbacks, GLR
/// stack versions, parser scratch arrays, external scanner state, and the final
/// accepted tree. The public C API only observes pointers to this opaque type,
/// so its fields deliberately use Rust layout.
pub struct TSParser {
    /// Input adapter and `TSLexer` callback surface.
    lexer: Lexer,
    /// Persistent GLR parse stack.
    stack: *mut Stack,
    /// Free lists used while releasing or mutating subtrees.
    tree_pool: SubtreePool,
    /// Active language tables and callbacks.
    language: *const TSLanguage,
    /// Scratch set of reductions considered during recovery.
    reduce_actions: ReduceActionSet,
    /// Best accepted root found so far.
    finished_tree: Subtree,
    /// Scratch arrays for stripping and comparing trailing extras.
    trailing_extras: SubtreeArray,
    trailing_extras2: SubtreeArray,
    /// Scratch child array used for subtree comparisons.
    scratch_trees: SubtreeArray,
    /// Cached lexer result for repeated same-position lookups.
    token_cache: TokenCache,
    /// Language-owned external scanner payload.
    external_scanner_payload: *mut c_void,
    /// Optional parse debug graph output.
    dot_graph_file: *mut c_void,
    /// Number of accepted trees seen in this parse.
    accept_count: u32,
    /// Progress-callback operation counter.
    operation_count: u32,
    /// Public parse cancellation/progress options.
    parse_options: TSParseOptions,
    /// Mutable status passed to the progress callback.
    parse_state: TSParseState,
    /// Set when balancing was canceled by the progress callback.
    canceled_balancing: bool,
    /// Set once any accepted tree contains an error.
    has_error: bool,
}

#[inline]
fn parse_options_none() -> TSParseOptions {
    TSParseOptions {
        payload: ptr::null_mut(),
        progress_callback: None,
    }
}

#[inline]
const fn parse_state_empty() -> TSParseState {
    TSParseState {
        payload: ptr::null_mut(),
        current_byte_offset: 0,
        has_error: false,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — StringInput
// ---------------------------------------------------------------------------

unsafe extern "C" fn ts_string_input_read(
    payload: *mut c_void,
    byte: u32,
    _point: TSPoint,
    length: *mut u32,
) -> *const c_char {
    let input = ptr_ref(payload.cast::<TSStringInput>());
    if byte >= input.length {
        *length = 0;
        c"".as_ptr()
    } else {
        *length = input.length - byte;
        input.string.add(byte as usize)
    }
}

mod logging;
use logging::{
    parser_log, parser_log_lookahead, parser_log_stack, parser_log_tree, parser_symbol_name,
    parser_tree_name, DisplayCStr,
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

unsafe fn parser_version_status(self_: &mut TSParser, version: StackVersion) -> ErrorStatus {
    let stack = ptr_mut(self_.stack);
    let mut cost = stack_error_cost(stack, version);
    let is_paused = stack_is_paused(stack, version);
    if is_paused {
        cost += ERROR_COST_PER_SKIPPED_TREE;
    }
    ErrorStatus {
        cost,
        node_count: stack_node_count_since_error(stack, version),
        dynamic_precedence: stack_dynamic_precedence(stack, version),
        is_in_error: is_paused || stack_state(stack, version) == ERROR_STATE,
    }
}

unsafe fn parser_better_version_exists(
    self_: &mut TSParser,
    version: StackVersion,
    is_in_error: bool,
    cost: u32,
) -> bool {
    if !self_.finished_tree.is_null() && subtree_error_cost(self_.finished_tree) <= cost {
        return true;
    }

    let stack = ptr_mut(self_.stack);
    let position = stack_position(stack, version);
    let status = ErrorStatus {
        cost,
        is_in_error,
        dynamic_precedence: stack_dynamic_precedence(stack, version),
        node_count: stack_node_count_since_error(stack, version),
    };

    let n = stack_version_count(stack);
    for i in 0..n {
        if i == version
            || !stack_is_active(stack, i)
            || stack_position(stack, i).bytes < position.bytes
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

unsafe fn parser_call_main_lex_fn(self_: &mut TSParser, lex_mode: TSLexerMode) -> bool {
    (language_full(self_.language).lex_fn.unwrap())(&mut self_.lexer.data, lex_mode.lex_state)
}

unsafe fn parser_call_keyword_lex_fn(self_: &mut TSParser) -> bool {
    (language_full(self_.language).keyword_lex_fn.unwrap())(&mut self_.lexer.data, 0)
}

mod external_scanner;
use external_scanner::{
    parser_external_scanner_create, parser_external_scanner_deserialize,
    parser_external_scanner_destroy, parser_external_scanner_scan,
    parser_external_scanner_serialize,
};
mod lexing;
use lexing::{parser_get_initial_lookahead, parser_lex_lookahead, parser_set_cached_token};

mod reduction;
use reduction::{parser_accept, parser_new_node, parser_reduce, parser_shift};
mod recovery;
use recovery::{parser_handle_error, parser_recover};

// Internal helpers — advance & condense
// ---------------------------------------------------------------------------

enum ParseActionsResult {
    Done,
    Reductions {
        did_reduce: bool,
        last_reduction_version: StackVersion,
    },
}

unsafe fn parser_check_progress(
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
                subtree_release(&mut self_.tree_pool, *lookahead);
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
                let reduce = action.reduce;
                let reduce_action = ReduceAction {
                    symbol: reduce.symbol,
                    count: u32::from(reduce.child_count),
                    dynamic_precedence: i32::from(reduce.dynamic_precedence),
                    production_id: reduce.production_id,
                };
                let invalidate_parse_state = table_entry.action_count > 1;
                let end_of_non_terminal_extra = lookahead.is_null();
                parser_log(self_, |context, log| {
                    write!(
                        log,
                        "reduce sym:{}, child_count:{}",
                        DisplayCStr(parser_symbol_name(context.language, reduce.symbol)),
                        u32::from(reduce.child_count)
                    )
                });
                let reduction_version = parser_reduce(
                    self_,
                    version,
                    reduce_action,
                    invalidate_parse_state,
                    end_of_non_terminal_extra,
                );
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
    *state = stack_state(ptr_ref(self_.stack), version);

    // At the end of a non-terminal extra rule, the lexer will return a null
    // subtree, because the parser needs to perform a fixed reduction regardless
    // of the lookahead node. After that reduction, run the lexer again from the
    // current parse state.
    if lookahead.is_null() {
        true
    } else {
        language_table_entry(
            self_.language,
            *state,
            subtree_symbol(lookahead),
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
        subtree_release(&mut self_.tree_pool, lookahead);
    }
    stack_halt(ptr_mut(self_.stack), version);
}

unsafe fn parser_try_keyword_fallback(
    self_: &mut TSParser,
    state: TSStateId,
    lookahead: &mut Subtree,
    table_entry: &mut TableEntry,
) -> bool {
    let keyword_capture_token = language_full(self_.language).keyword_capture_token;
    if !subtree_is_keyword(*lookahead)
        || subtree_symbol(*lookahead) == keyword_capture_token
        || language_is_reserved_word(self_.language, state, subtree_symbol(*lookahead))
    {
        return false;
    }

    language_table_entry(self_.language, state, keyword_capture_token, table_entry);
    if table_entry.action_count == 0 {
        return false;
    }

    parser_log(self_, |context, log| {
        write!(
            log,
            "switch from_keyword:{}, to_word_token:{}",
            DisplayCStr(parser_tree_name(context.language, *lookahead)),
            DisplayCStr(parser_symbol_name(context.language, keyword_capture_token))
        )
    });

    let mut mutable_lookahead = subtree_make_mut(&mut self_.tree_pool, *lookahead);
    subtree_set_symbol(
        &mut mutable_lookahead,
        keyword_capture_token,
        self_.language,
    );
    *lookahead = subtree_from_mut(mutable_lookahead);
    true
}

unsafe fn parser_pause_with_error(self_: &mut TSParser, version: StackVersion, lookahead: Subtree) {
    parser_log(self_, |context, log| {
        write!(
            log,
            "detect_error lookahead:{}",
            DisplayCStr(parser_tree_name(context.language, lookahead))
        )
    });
    stack_pause(ptr_mut(self_.stack), version, lookahead);
}

/// Advance one stack version until it shifts, accepts, recovers, pauses, or halts.
///
/// This is the parser action interpreter. It first obtains a lookahead from the
/// token cache or lexer. Then it repeatedly reads the parse-table
/// entry for `(state, lookahead)` and executes its actions. Reductions keep the
/// same lookahead and continue in the new goto state; shifts consume the
/// lookahead and return to the outer parse loop.
unsafe fn parser_advance(self_: &mut TSParser, version: StackVersion) -> bool {
    let stack = ptr_ref(self_.stack);
    let mut state = stack_state(stack, version);
    let position = stack_position(stack, version).bytes;
    let last_external_token = stack_last_external_token(stack, version);

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

unsafe fn parser_condense_stack(self_: &mut TSParser) -> u32 {
    let mut made_changes = false;
    let mut min_error_cost = u32::MAX;
    let mut i: StackVersion = 0;
    while i < stack_version_count(ptr_ref(self_.stack)) {
        // Prune any versions that have been marked for removal.
        if stack_is_halted(ptr_ref(self_.stack), i) {
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
    while stack_version_count(ptr_ref(self_.stack)) > MAX_VERSION_COUNT {
        stack_remove_version(ptr_mut(self_.stack), MAX_VERSION_COUNT);
        made_changes = true;
    }

    // If the best-performing stack version is currently paused, or all
    // versions are paused, then resume the best paused version and begin
    // the error recovery process. Otherwise, remove the paused versions.
    if stack_version_count(ptr_ref(self_.stack)) > 0 {
        let mut has_unpaused_version = false;
        let mut i: StackVersion = 0;
        let mut n = stack_version_count(ptr_ref(self_.stack));
        while i < n {
            if stack_is_paused(ptr_ref(self_.stack), i) {
                if !has_unpaused_version && self_.accept_count < MAX_VERSION_COUNT {
                    parser_log(self_, |_, log| write!(log, "resume version:{i}"));
                    min_error_cost = stack_error_cost(ptr_ref(self_.stack), i);
                    let lookahead = stack_resume(ptr_mut(self_.stack), i);
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

    min_error_cost
}

mod balancing;
use balancing::parser_balance_subtree;

unsafe fn parser_has_outstanding_parse(self_: &TSParser) -> bool {
    self_.canceled_balancing
        || !self_.external_scanner_payload.is_null()
        || stack_state(ptr_ref(self_.stack), 0) != 1
        || stack_node_count_since_error(ptr_mut(self_.stack), 0) != 0
}

unsafe fn parser_take_finished_tree(self_: &mut TSParser) -> *mut TSTree {
    let result = tree_new(
        self_.finished_tree,
        self_.language,
        lexer_included_ranges_slice(&self_.lexer),
    );
    self_.finished_tree = NULL_SUBTREE;
    result
}

// ---------------------------------------------------------------------------
// Exported functions — lifecycle
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_parser_new() -> *mut TSParser {
    let self_ = malloc(core::mem::size_of::<TSParser>()).cast::<TSParser>();
    ptr::write(
        self_,
        TSParser {
            lexer: lexer_new(),
            stack: ptr::null_mut(),
            tree_pool: subtree_pool_new(32),
            language: ptr::null(),
            reduce_actions: Array::new(),
            finished_tree: NULL_SUBTREE,
            trailing_extras: Array::new(),
            trailing_extras2: Array::new(),
            scratch_trees: Array::new(),
            token_cache: TokenCache {
                token: NULL_SUBTREE,
                last_external_token: NULL_SUBTREE,
                byte_index: 0,
            },
            external_scanner_payload: ptr::null_mut(),
            dot_graph_file: ptr::null_mut(),
            accept_count: 0,
            operation_count: 0,
            parse_options: parse_options_none(),
            parse_state: parse_state_empty(),
            canceled_balancing: false,
            has_error: false,
        },
    );
    let parser = ptr_mut(self_);
    parser.reduce_actions.reserve(4);
    parser.stack = stack_new(&mut parser.tree_pool);
    parser_set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_delete(self_: *mut TSParser) {
    if self_.is_null() {
        return;
    }

    ts_parser_reset(self_);
    let parser = ptr_mut(self_);
    stack_delete(ptr_mut(parser.stack));
    if !parser.reduce_actions.contents.is_null() {
        parser.reduce_actions.delete();
    }
    lexer_delete(&mut parser.lexer);
    parser_set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    subtree_pool_delete(&mut parser.tree_pool);
    parser.trailing_extras.delete();
    parser.trailing_extras2.delete();
    parser.scratch_trees.delete();
    free(self_.cast::<c_void>());
}

// ---------------------------------------------------------------------------
// Exported functions — configuration
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_parser_language(self_: *const TSParser) -> *const TSLanguage {
    let parser = ptr_ref(self_);
    parser.language
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_language(
    self_: *mut TSParser,
    language: *const TSLanguage,
) -> bool {
    ts_parser_reset(self_);
    let parser = ptr_mut(self_);
    parser.language = ptr::null();
    if !language.is_null() {
        let language_data = language_full(language);
        if language_data.abi_version > TREE_SITTER_LANGUAGE_VERSION
            || language_data.abi_version < TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION
        {
            return false;
        }
    }

    parser.language = language;
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_logger(self_: *const TSParser) -> TSLogger {
    let parser = ptr_ref(self_);
    ptr::read(&parser.lexer.logger)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_logger(self_: *mut TSParser, logger: TSLogger) {
    let parser = ptr_mut(self_);
    parser.lexer.logger = logger;
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_print_dot_graphs(self_: *mut TSParser, fd: i32) {
    let parser = ptr_mut(self_);
    if !parser.dot_graph_file.is_null() {
        fclose(parser.dot_graph_file);
    }

    if fd >= 0 {
        #[cfg(target_os = "windows")]
        {
            extern "C" {
                fn _fdopen(fd: i32, mode: *const i8) -> *mut c_void;
            }
            parser.dot_graph_file = _fdopen(fd, c"a".as_ptr().cast::<i8>());
        }
        #[cfg(not(target_os = "windows"))]
        {
            parser.dot_graph_file = fdopen(fd, c"a".as_ptr().cast::<i8>());
        }
    } else {
        parser.dot_graph_file = ptr::null_mut();
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_included_ranges(
    self_: *mut TSParser,
    ranges: *const TSRange,
    count: u32,
) -> bool {
    let parser = ptr_mut(self_);
    lexer_set_included_ranges(&mut parser.lexer, ranges, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_included_ranges(
    self_: *const TSParser,
    count: *mut u32,
) -> *const TSRange {
    let parser = ptr_ref(self_);
    lexer_included_ranges(&parser.lexer, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_reset(self_: *mut TSParser) {
    let parser = ptr_mut(self_);
    parser_external_scanner_destroy(parser);

    lexer_reset(&mut parser.lexer, length_zero());
    stack_clear(ptr_mut(parser.stack));
    parser_set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    if !parser.finished_tree.is_null() {
        subtree_release(&mut parser.tree_pool, parser.finished_tree);
        parser.finished_tree = NULL_SUBTREE;
    }
    parser.accept_count = 0;
    parser.has_error = false;
    parser.canceled_balancing = false;
    parser.parse_options = parse_options_none();
    parser.parse_state = parse_state_empty();
}

// ---------------------------------------------------------------------------
// Exported functions — parsing
// ---------------------------------------------------------------------------

#[no_mangle]
/// Parse one input document and return a new tree.
///
/// The driver owns the outer GLR loop:
/// - initialize the lexer and external scanner;
/// - process every active stack version until none can advance normally;
/// - condense/merge/prune stack versions;
/// - recover when all versions are paused at errors;
/// - balance the accepted tree and transfer its root into `TSTree`.
///
/// Returning null means parsing was canceled. Parser-owned scratch state is
/// reset before returning unless the parse is intentionally resumable.
pub unsafe extern "C-unwind" fn ts_parser_parse(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    input: TSInput,
) -> *mut TSTree {
    let _ = old_tree;
    let parser = ptr_mut(self_);
    if parser.language.is_null() || input.read.is_none() {
        return ptr::null_mut();
    }

    lexer_set_input(&mut parser.lexer, input);
    parser.operation_count = 0;

    if parser_has_outstanding_parse(parser) {
        parser_log(parser, |_, log| log.write_str("resume_parsing"));
        if parser.canceled_balancing {
            debug_assert!(!parser.finished_tree.is_null());
            if !parser_balance_subtree(parser) {
                parser.canceled_balancing = true;
                return ptr::null_mut();
            }
            parser.canceled_balancing = false;
            parser_log(parser, |_, log| log.write_str("done"));
            parser_log_tree(parser, parser.finished_tree);

            let result = parser_take_finished_tree(parser);

            ts_parser_reset(self_);
            return result;
        }
    } else {
        parser_external_scanner_create(parser);
        parser_log(parser, |_, log| log.write_str("new_parse"));
    }

    let mut last_position: u32 = 0;
    let mut version_count: StackVersion;
    loop {
        let mut version: StackVersion = 0;
        loop {
            version_count = stack_version_count(ptr_ref(parser.stack));
            if version >= version_count {
                break;
            }

            while stack_is_active(ptr_ref(parser.stack), version) {
                parser_log(parser, |context, log| {
                    write!(
                        log,
                        "process version:{version}, version_count:{}, state:{}, row:{}, col:{}",
                        stack_version_count(ptr_ref(context.stack)),
                        i32::from(stack_state(ptr_ref(context.stack), version)),
                        stack_position(ptr_ref(context.stack), version).extent.row,
                        stack_position(ptr_ref(context.stack), version)
                            .extent
                            .column
                    )
                });

                if !parser_advance(parser, version) {
                    return ptr::null_mut();
                }

                parser_log_stack(parser);

                let position = stack_position(ptr_ref(parser.stack), version).bytes;
                if position > last_position || (version > 0 && position == last_position) {
                    last_position = position;
                    break;
                }
            }
            version += 1;
        }

        // After advancing each version of the stack, re-sort the versions by their cost,
        // removing any versions that are no longer worth pursuing.
        let min_error_cost = parser_condense_stack(parser);

        // If there's already a finished parse tree that's better than any in-progress version,
        // then terminate parsing. Clear the parse stack to remove any extra references to subtrees
        // within the finished tree, ensuring that these subtrees can be safely mutated in-place
        // for rebalancing.
        if !parser.finished_tree.is_null()
            && subtree_error_cost(parser.finished_tree) < min_error_cost
        {
            stack_clear(ptr_mut(parser.stack));
            break;
        }

        if version_count == 0 {
            break;
        }
    }

    // balance:
    debug_assert!(!parser.finished_tree.is_null());
    if !parser_balance_subtree(parser) {
        parser.canceled_balancing = true;
        return ptr::null_mut();
    }
    parser.canceled_balancing = false;
    parser_log(parser, |_, log| log.write_str("done"));
    parser_log_tree(parser, parser.finished_tree);

    let result = parser_take_finished_tree(parser);

    // exit:
    ts_parser_reset(self_);
    result
}

#[no_mangle]
pub unsafe extern "C-unwind" fn ts_parser_parse_with_options(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    input: TSInput,
    parse_options: TSParseOptions,
) -> *mut TSTree {
    {
        let parser = ptr_mut(self_);
        parser.parse_options = parse_options;
        parser.parse_state.payload = parse_options.payload;
    }
    let result = ts_parser_parse(self_, old_tree, input);
    // Reset parser options before further parse calls.
    let parser = ptr_mut(self_);
    parser.parse_options = parse_options_none();
    result
}

#[no_mangle]
pub unsafe extern "C-unwind" fn ts_parser_parse_string(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    string: *const i8,
    length: u32,
) -> *mut TSTree {
    ts_parser_parse_string_encoding(self_, old_tree, string, length, TSInputEncodingUTF8)
}

#[no_mangle]
pub unsafe extern "C-unwind" fn ts_parser_parse_string_encoding(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    string: *const i8,
    length: u32,
    encoding: TSInputEncoding,
) -> *mut TSTree {
    let input = TSStringInput {
        string: string.cast::<c_char>(),
        length,
    };
    ts_parser_parse(
        self_,
        old_tree,
        TSInput {
            payload: core::ptr::addr_of!(input) as *mut c_void,
            read: Some(ts_string_input_read),
            encoding,
            decode: None,
        },
    )
}
