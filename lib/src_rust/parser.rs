use core::ffi::{c_char, c_void, CStr};
use core::fmt::{self, Write};
use core::ptr;

use crate::ffi::{
    TSInput, TSInputEncoding, TSInputEncodingUTF8, TSLanguage, TSLogTypeParse, TSLogger,
    TSParseOptions, TSParseState, TSPoint, TSRange, TSStateId, TSSymbol,
};

use super::alloc::{free, malloc};
use super::error_costs::{
    ERROR_COST_PER_SKIPPED_CHAR, ERROR_COST_PER_SKIPPED_LINE, ERROR_COST_PER_SKIPPED_TREE,
    ERROR_STATE,
};
use super::language::{
    language_actions, language_enabled_external_tokens, language_full, language_has_actions,
    language_has_reduce_action, language_is_reserved_word, language_lex_mode_for_state,
    language_lookup, language_table_entry, ts_language_next_state, ts_language_symbol_name,
    TSLexerMode, TSParseAction, TableEntry, TSPARSE_ACTION_TYPE_ACCEPT,
    TSPARSE_ACTION_TYPE_RECOVER, TSPARSE_ACTION_TYPE_REDUCE, TSPARSE_ACTION_TYPE_SHIFT,
};
use super::length::{length_sub, length_zero, Length};
use super::lexer::{
    lexer_advance, lexer_delete, lexer_finish, lexer_included_ranges, lexer_is_eof, lexer_mark_end,
    lexer_new, lexer_reset, lexer_set_included_ranges, lexer_set_input, lexer_start, Lexer,
};
use super::reduce_action::{reduce_action_set_add, ReduceAction, ReduceActionSet};
use super::stack::{
    // Stack functions (now Rust-only)
    stack_can_merge,
    stack_clear,
    stack_copy_version,
    stack_delete,
    stack_dynamic_precedence,
    stack_error_cost,
    stack_get_summary,
    stack_halt,
    stack_halted_version_count,
    stack_has_advanced_since_error,
    stack_is_active,
    stack_is_halted,
    stack_is_paused,
    stack_last_external_token,
    stack_merge,
    stack_new,
    stack_node_count_since_error,
    stack_pause,
    stack_pop_all,
    stack_pop_builder_delete,
    stack_pop_builder_new,
    stack_pop_count,
    stack_pop_count_into,
    stack_pop_count_linear_in_place,
    stack_pop_error,
    stack_position,
    stack_print_dot_graph,
    stack_push,
    stack_record_summary,
    stack_remove_version,
    stack_renumber_version,
    stack_resume,
    stack_set_last_external_token,
    stack_state,
    stack_swap_versions,
    stack_version_count,
    Stack,
    StackPopBuilder,
    StackSliceSpan,
    StackVersion,
    STACK_VERSION_NONE,
};
use super::subtree::{
    // Subtree functions (now Rust-only)
    external_scanner_state_data,
    external_scanner_state_eq,
    external_scanner_state_init,
    subtree_array_clear,
    subtree_array_delete,
    subtree_array_remove_trailing_extras,
    subtree_child,
    subtree_child_count,
    subtree_children_slice,
    subtree_compare,
    subtree_compress,
    subtree_dynamic_precedence,
    subtree_error_cost,
    subtree_external_scanner_state,
    subtree_external_scanner_state_eq,
    subtree_extra,
    subtree_from_mut,
    subtree_has_external_scanner_state_change,
    subtree_has_external_tokens,
    subtree_is_eof,
    subtree_is_error,
    subtree_is_keyword,
    subtree_last_external_token,
    subtree_lookahead_bytes,
    subtree_make_mut,
    subtree_new_error,
    subtree_new_error_node,
    subtree_new_leaf,
    subtree_new_missing_leaf,
    subtree_new_node,
    subtree_new_node_in_arena,
    subtree_parse_state,
    subtree_pool_delete,
    subtree_pool_new,
    subtree_print_dot_graph,
    subtree_release,
    subtree_repeat_depth,
    subtree_retain,
    subtree_set_extra,
    subtree_set_symbol,
    subtree_size,
    subtree_symbol,
    subtree_to_mut_unsafe,
    subtree_total_bytes,
    subtree_total_size,
    tree_arena_new,
    tree_arena_release,
    ExternalScannerState,
    MutableSubtree,
    Subtree,
    SubtreeArray,
    SubtreePool,
    TreeArena,
    NULL_SUBTREE,
    TS_BUILTIN_SYM_END,
    TS_BUILTIN_SYM_ERROR,
    TS_BUILTIN_SYM_ERROR_REPEAT,
    TS_TREE_STATE_NONE,
};
use super::tree::{tree_new_with_arena, TSTree};
use super::utils::{
    array_assign, array_back_ref, array_clear, array_delete, array_erase, array_get_mut,
    array_get_ref, array_new, array_pop, array_push, array_reserve, array_splice, array_swap,
};
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
const TREE_SITTER_SERIALIZATION_BUFFER_SIZE: usize = 1024;
const TREE_SITTER_LANGUAGE_VERSION: u32 = 15;
const TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION: u32 = 13;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One-token cache shared by stack versions at the same byte offset.
///
/// GLR versions often ask the lexer for the same position and external scanner
/// state. The cache stores the concrete token plus the last external token that
/// determined scanner state, so another version can reuse it only when scanner
/// state is equivalent.
#[repr(C)]
struct TokenCache {
    /// Retained lookahead token.
    token: Subtree,
    /// Retained token carrying the external scanner state used for `token`.
    last_external_token: Subtree,
    /// Byte offset where `token` was lexed.
    byte_index: u32,
}

/// Summary used to compare and prune stack versions.
#[repr(C)]
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
#[repr(C)]
struct TSStringInput {
    string: *const c_char,
    length: u32,
}

/// Main parser runtime state.
///
/// One `TSParser` owns all mutable state for a parse: lexer callbacks, GLR
/// stack versions, parser scratch arrays, external scanner state, and the final
/// accepted tree. The public C API treats this as opaque.
#[repr(C)]
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
    /// Reusable pop-result builder for reductions.
    reduce_builder: StackPopBuilder,
    /// Scratch arrays for stripping and comparing trailing extras.
    trailing_extras: SubtreeArray,
    trailing_extras2: SubtreeArray,
    /// Scratch child array used for subtree comparisons.
    scratch_trees: SubtreeArray,
    /// Cached lexer result for repeated same-position lookups.
    token_cache: TokenCache,
    deterministic_reduction_count: u32,
    /// Arena that owns internal nodes in the returned tree.
    tree_arena: *mut TreeArena,
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

// ---------------------------------------------------------------------------
// Internal helpers — logging & breakdown
// ---------------------------------------------------------------------------

struct ParserLogBuffer<'a> {
    bytes: &'a mut [u8],
    len: usize,
}

impl ParserLogBuffer<'_> {
    fn write_bytes(&mut self, bytes: &[u8]) {
        let available = self.bytes.len().saturating_sub(self.len + 1);
        let count = available.min(bytes.len());
        self.bytes[self.len..self.len + count].copy_from_slice(&bytes[..count]);
        self.len += count;
    }
}

impl Write for ParserLogBuffer<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.write_bytes(value.as_bytes());
        Ok(())
    }
}

struct DisplayCStr(*const c_char);

impl fmt::Display for DisplayCStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut bytes = unsafe { CStr::from_ptr(self.0) }.to_bytes();
        while !bytes.is_empty() {
            match core::str::from_utf8(bytes) {
                Ok(value) => return formatter.write_str(value),
                Err(error) => {
                    let valid = error.valid_up_to();
                    formatter
                        .write_str(unsafe { core::str::from_utf8_unchecked(&bytes[..valid]) })?;
                    formatter.write_char(char::REPLACEMENT_CHARACTER)?;
                    bytes = &bytes[valid + error.error_len().unwrap_or(1)..];
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct ParserLogContext {
    language: *const TSLanguage,
    stack: *mut Stack,
}

unsafe fn parser_log(
    self_: &mut TSParser,
    write_message: impl FnOnce(ParserLogContext, &mut ParserLogBuffer<'_>) -> fmt::Result,
) {
    if self_.lexer.logger.log.is_none() && self_.dot_graph_file.is_null() {
        return;
    }

    {
        let context = ParserLogContext {
            language: self_.language,
            stack: self_.stack,
        };
        let mut buffer = ParserLogBuffer {
            bytes: &mut self_.lexer.debug_buffer,
            len: 0,
        };
        let _ = write_message(context, &mut buffer);
        buffer.bytes[buffer.len] = 0;
    }

    parser_emit_log(self_);
}

unsafe fn parser_log_stack(self_: &TSParser) {
    if !self_.dot_graph_file.is_null() {
        stack_print_dot_graph(ptr_mut(self_.stack), self_.language, self_.dot_graph_file);
        fputs(c"\n\n".as_ptr().cast::<i8>(), self_.dot_graph_file);
    }
}

unsafe fn parser_log_tree(self_: &TSParser, tree: Subtree) {
    if !self_.dot_graph_file.is_null() {
        subtree_print_dot_graph(tree, self_.language, self_.dot_graph_file);
        fputs(c"\n".as_ptr().cast::<i8>(), self_.dot_graph_file);
    }
}

unsafe fn parser_symbol_name(language: *const TSLanguage, symbol: TSSymbol) -> *const c_char {
    ts_language_symbol_name(language, symbol)
}

unsafe fn parser_tree_name(language: *const TSLanguage, tree: Subtree) -> *const c_char {
    parser_symbol_name(language, subtree_symbol(tree))
}

unsafe fn parser_log_lookahead(self_: &mut TSParser, symbol: *const c_char, size: u32) {
    parser_log(self_, |_, buffer| {
        buffer.write_str("lexed_lookahead sym:")?;
        for byte in CStr::from_ptr(symbol).to_bytes() {
            match *byte {
                b'\t' => buffer.write_str("\\t")?,
                b'\n' => buffer.write_str("\\n")?,
                0x0b => buffer.write_str("\\v")?,
                0x0c => buffer.write_str("\\f")?,
                b'\r' => buffer.write_str("\\r")?,
                b'\\' => buffer.write_str("\\\\")?,
                _ => buffer.write_bytes(core::slice::from_ref(byte)),
            }
        }
        write!(buffer, ", size:{size}")
    });
}

unsafe fn parser_emit_log(self_: &mut TSParser) {
    if let Some(log_fn) = self_.lexer.logger.log {
        log_fn(
            self_.lexer.logger.payload,
            TSLogTypeParse,
            self_.lexer.debug_buffer.as_ptr().cast::<c_char>(),
        );
    }

    if !self_.dot_graph_file.is_null() {
        fprintf(
            self_.dot_graph_file,
            c"graph {\nlabel=\"".as_ptr().cast::<i8>(),
        );
        let mut chr = self_.lexer.debug_buffer.as_ptr();
        while *chr != 0 {
            if *chr == b'"' || *chr == b'\\' {
                fputc(i32::from(b'\\'), self_.dot_graph_file);
            }
            fputc(i32::from(*chr), self_.dot_graph_file);
            chr = chr.add(1);
        }
        fprintf(self_.dot_graph_file, c"\"\n}\n\n".as_ptr().cast::<i8>());
    }
}

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
    if !self_.finished_tree.ptr.is_null() && subtree_error_cost(self_.finished_tree) <= cost {
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

// ---------------------------------------------------------------------------
// Internal helpers — external scanner
// ---------------------------------------------------------------------------

unsafe fn parser_external_scanner_create(self_: &mut TSParser) {
    if !self_.language.is_null() {
        let lang = language_full(self_.language);
        if lang.external_scanner.states.is_null() {
            return;
        }

        if let Some(create_fn) = lang.external_scanner.create {
            self_.external_scanner_payload = create_fn();
        }
    }
}

unsafe fn parser_external_scanner_destroy(self_: &mut TSParser) {
    if !self_.language.is_null() && !self_.external_scanner_payload.is_null() {
        let lang = language_full(self_.language);
        if let Some(destroy_fn) = lang.external_scanner.destroy {
            destroy_fn(self_.external_scanner_payload);
        }
    }
    self_.external_scanner_payload = ptr::null_mut();
}

unsafe fn parser_external_scanner_serialize(self_: &mut TSParser) -> u32 {
    let length = (language_full(self_.language)
        .external_scanner
        .serialize
        .unwrap())(
        self_.external_scanner_payload,
        self_.lexer.debug_buffer.as_mut_ptr().cast::<i8>(),
    );
    debug_assert!(length as usize <= TREE_SITTER_SERIALIZATION_BUFFER_SIZE);
    length
}

unsafe fn parser_external_scanner_deserialize(self_: &mut TSParser, external_token: Subtree) {
    let (data, length) = if !external_token.ptr.is_null() {
        let state = subtree_external_scanner_state(&external_token);
        (external_scanner_state_data(state), state.length)
    } else {
        (ptr::null(), 0)
    };

    (language_full(self_.language)
        .external_scanner
        .deserialize
        .unwrap())(self_.external_scanner_payload, data.cast::<i8>(), length);
}

unsafe fn parser_external_scanner_scan(
    self_: &mut TSParser,
    external_lex_state: TSStateId,
) -> bool {
    let lang = language_full(self_.language);
    let valid_external_tokens =
        language_enabled_external_tokens(self_.language, u32::from(external_lex_state));
    (lang.external_scanner.scan.unwrap())(
        self_.external_scanner_payload,
        &mut self_.lexer.data,
        valid_external_tokens,
    )
}

// ---------------------------------------------------------------------------
// Internal helpers — token reuse & lexing
// ---------------------------------------------------------------------------

unsafe fn parser_can_reuse_token(
    self_: &TSParser,
    state: TSStateId,
    token: Subtree,
    table_entry: &TableEntry,
) -> bool {
    debug_assert_eq!(subtree_child_count(token), 0);
    let token_symbol = subtree_symbol(token);
    let current_lex_mode = language_lex_mode_for_state(self_.language, state);

    // At the end of a non-terminal extra node, the lexer normally returns
    // NULL, which indicates that the parser should look for a reduce action
    // at symbol `0`. Avoid reusing tokens in this situation.
    if current_lex_mode.lex_state == u16::MAX {
        return false;
    }

    // If the token was created in a state with the same set of lookaheads, it is reusable.
    if table_entry.action_count > 0 {
        let token_state = subtree_parse_state(token);
        let token_lex_mode = language_lex_mode_for_state(self_.language, token_state);
        if token_lex_mode.lex_state == current_lex_mode.lex_state
            && token_lex_mode.external_lex_state == current_lex_mode.external_lex_state
            && token_lex_mode.reserved_word_set_id == current_lex_mode.reserved_word_set_id
        {
            let lang = language_full(self_.language);
            if token_symbol != lang.keyword_capture_token
                || (!subtree_is_keyword(token) && subtree_parse_state(token) == state)
            {
                return true;
            }
        }
    }

    // Empty tokens are not reusable in states with different lookaheads.
    if subtree_size(token).bytes == 0 && token_symbol != TS_BUILTIN_SYM_END {
        return false;
    }

    // If the current state allows external tokens or other tokens that conflict with this
    // token, this token is not reusable.
    current_lex_mode.external_lex_state == 0 && table_entry.is_reusable
}

/// Build the error token produced after skipping unrecognized input.
unsafe fn parser_new_error_lookahead(
    self_: &mut TSParser,
    parse_state: TSStateId,
    start_position: Length,
    error_start_position: Length,
    error_end_position: Length,
    lookahead_end_byte: u32,
    first_error_character: i32,
) -> Subtree {
    let padding = length_sub(error_start_position, start_position);
    let size = length_sub(error_end_position, error_start_position);
    let lookahead_bytes = lookahead_end_byte - error_end_position.bytes;
    subtree_new_error(
        &mut self_.tree_pool,
        first_error_character,
        padding,
        size,
        lookahead_bytes,
        parse_state,
        self_.language,
    )
}

/// Resolve the public symbol for a token found by internal or external lexing.
///
/// External scanners return an index into their symbol map. Internal lexing may
/// return the grammar's word token, in which case the keyword lexer gets one
/// chance to refine it to a reserved word that is valid in the current state.
unsafe fn parser_resolve_lexed_symbol(
    self_: &mut TSParser,
    parse_state: TSStateId,
    found_external_token: bool,
) -> (TSSymbol, bool) {
    let lang = language_full(self_.language);
    let mut symbol = self_.lexer.data.result_symbol;
    let mut is_keyword = false;

    if found_external_token {
        symbol = *lang.external_scanner.symbol_map.add(symbol as usize);
    } else if symbol == lang.keyword_capture_token && symbol != 0 {
        let end_byte = self_.lexer.token_end_position.bytes;
        let token_start_position = self_.lexer.token_start_position;
        lexer_reset(&mut self_.lexer, token_start_position);
        lexer_start(&mut self_.lexer);

        is_keyword = parser_call_keyword_lex_fn(self_);

        if is_keyword
            && self_.lexer.token_end_position.bytes == end_byte
            && (language_has_actions(self_.language, parse_state, self_.lexer.data.result_symbol)
                || language_is_reserved_word(
                    self_.language,
                    parse_state,
                    self_.lexer.data.result_symbol,
                ))
        {
            symbol = self_.lexer.data.result_symbol;
        }
    }

    (symbol, is_keyword)
}

/// Build the concrete leaf token after the lexing loop succeeds.
#[allow(clippy::too_many_arguments)]
unsafe fn parser_new_leaf_lookahead(
    self_: &mut TSParser,
    parse_state: TSStateId,
    start_position: Length,
    lookahead_end_byte: u32,
    found_external_token: bool,
    called_get_column: bool,
    external_scanner_state_len: u32,
    external_scanner_state_changed: bool,
) -> Subtree {
    let padding = length_sub(self_.lexer.token_start_position, start_position);
    let size = length_sub(
        self_.lexer.token_end_position,
        self_.lexer.token_start_position,
    );
    let lookahead_bytes = lookahead_end_byte - self_.lexer.token_end_position.bytes;
    let (symbol, is_keyword) =
        parser_resolve_lexed_symbol(self_, parse_state, found_external_token);

    let result = subtree_new_leaf(
        &mut self_.tree_pool,
        symbol,
        padding,
        size,
        lookahead_bytes,
        parse_state,
        found_external_token,
        called_get_column,
        is_keyword,
        self_.language,
    );

    if found_external_token {
        let mut_result = subtree_to_mut_unsafe(result);
        let external_scanner_state =
            ptr::addr_of_mut!((*mut_result.ptr).data.external_scanner_state)
                .cast::<ExternalScannerState>()
                .as_mut()
                .unwrap_unchecked();
        external_scanner_state_init(
            external_scanner_state,
            self_.lexer.debug_buffer.as_ptr(),
            external_scanner_state_len,
        );
        (*mut_result.ptr).set_has_external_scanner_state_change(external_scanner_state_changed);
    }

    result
}

/// Scan from the current stack position and return one lookahead subtree.
///
/// The scanner first gives an external scanner a chance when the parse state
/// enables one, then falls back to the generated lexer. If normal lexing fails,
/// it switches to the error lex mode and consumes bytes until it can produce an
/// error token or EOF.
unsafe fn parser_lex(
    self_: &mut TSParser,
    version: StackVersion,
    parse_state: TSStateId,
) -> Subtree {
    let lang = language_full(self_.language);
    let mut lex_mode = language_lex_mode_for_state(self_.language, parse_state);
    if lex_mode.lex_state == u16::MAX {
        parser_log(self_, |_, log| {
            log.write_str("no_lookahead_after_non_terminal_extra")
        });
        return NULL_SUBTREE;
    }

    let stack = ptr_ref(self_.stack);
    let start_position = stack_position(stack, version);
    let external_token = stack_last_external_token(stack, version);

    let mut found_external_token = false;
    let mut error_mode = parse_state == ERROR_STATE;
    let mut skipped_error = false;
    let mut called_get_column = false;
    let mut first_error_character: i32 = 0;
    let mut error_start_position = length_zero();
    let mut error_end_position = length_zero();
    let mut lookahead_end_byte: u32 = 0;
    let mut external_scanner_state_len: u32 = 0;
    let mut external_scanner_state_changed = false;
    lexer_reset(&mut self_.lexer, start_position);

    loop {
        let mut found_token;
        let current_position = self_.lexer.current_position;
        let column_data = self_.lexer.column_data;

        if lex_mode.external_lex_state != 0 {
            parser_log(self_, |_, log| {
                write!(
                    log,
                    "lex_external state:{}, row:{}, column:{}",
                    i32::from(lex_mode.external_lex_state),
                    current_position.extent.row,
                    current_position.extent.column
                )
            });
            lexer_start(&mut self_.lexer);
            parser_external_scanner_deserialize(self_, external_token);
            found_token = parser_external_scanner_scan(self_, lex_mode.external_lex_state);
            lexer_finish(&mut self_.lexer, &mut lookahead_end_byte);

            if found_token {
                external_scanner_state_len = parser_external_scanner_serialize(self_);
                let external_scanner_state = subtree_external_scanner_state(&external_token);
                external_scanner_state_changed = !external_scanner_state_eq(
                    external_scanner_state,
                    self_.lexer.debug_buffer.as_ptr(),
                    external_scanner_state_len,
                );

                if self_.lexer.token_end_position.bytes <= current_position.bytes
                    && !external_scanner_state_changed
                {
                    let symbol = *lang
                        .external_scanner
                        .symbol_map
                        .add(self_.lexer.data.result_symbol as usize);
                    let next_parse_state =
                        ts_language_next_state(self_.language, parse_state, symbol);
                    let token_is_extra = next_parse_state == parse_state;
                    if error_mode
                        || !stack_has_advanced_since_error(ptr_ref(self_.stack), version)
                        || token_is_extra
                    {
                        parser_log(self_, |context, log| {
                            write!(
                                log,
                                "ignore_empty_external_token symbol:{}",
                                DisplayCStr(parser_symbol_name(context.language, symbol))
                            )
                        });
                        found_token = false;
                    }
                }
            }

            if found_token {
                found_external_token = true;
                called_get_column = self_.lexer.did_get_column;
                break;
            }

            lexer_reset(&mut self_.lexer, current_position);
            self_.lexer.column_data = column_data;
        }

        parser_log(self_, |_, log| {
            write!(
                log,
                "lex_internal state:{}, row:{}, column:{}",
                i32::from(lex_mode.lex_state),
                current_position.extent.row,
                current_position.extent.column
            )
        });
        lexer_start(&mut self_.lexer);
        found_token = parser_call_main_lex_fn(self_, lex_mode);
        lexer_finish(&mut self_.lexer, &mut lookahead_end_byte);
        if found_token {
            break;
        }

        if !error_mode {
            error_mode = true;
            lex_mode = language_lex_mode_for_state(self_.language, ERROR_STATE);
            lexer_reset(&mut self_.lexer, start_position);
            continue;
        }

        if !skipped_error {
            parser_log(self_, |_, log| log.write_str("skip_unrecognized_character"));
            skipped_error = true;
            error_start_position = self_.lexer.token_start_position;
            error_end_position = self_.lexer.token_start_position;
            first_error_character = self_.lexer.data.lookahead;
        }

        if self_.lexer.current_position.bytes == error_end_position.bytes {
            if lexer_is_eof(&self_.lexer) {
                self_.lexer.data.result_symbol = TS_BUILTIN_SYM_ERROR;
                break;
            }
            lexer_advance(&mut self_.lexer, false);
        }

        error_end_position = self_.lexer.current_position;
    }

    let result = if skipped_error {
        parser_new_error_lookahead(
            self_,
            parse_state,
            start_position,
            error_start_position,
            error_end_position,
            lookahead_end_byte,
            first_error_character,
        )
    } else {
        parser_new_leaf_lookahead(
            self_,
            parse_state,
            start_position,
            lookahead_end_byte,
            found_external_token,
            called_get_column,
            external_scanner_state_len,
            external_scanner_state_changed,
        )
    };

    parser_log_lookahead(
        self_,
        parser_symbol_name(self_.language, subtree_symbol(result)),
        subtree_total_size(result).bytes,
    );
    result
}

unsafe fn parser_get_cached_token(
    self_: &TSParser,
    state: TSStateId,
    position: usize,
    last_external_token: Subtree,
) -> Option<(Subtree, TableEntry)> {
    let cache = &self_.token_cache;
    if !cache.token.ptr.is_null()
        && cache.byte_index == position as u32
        && subtree_external_scanner_state_eq(&cache.last_external_token, &last_external_token)
    {
        let mut table_entry = TableEntry::empty();
        language_table_entry(
            self_.language,
            state,
            subtree_symbol(cache.token),
            &mut table_entry,
        );
        if parser_can_reuse_token(self_, state, cache.token, &table_entry) {
            subtree_retain(cache.token);
            return Some((cache.token, table_entry));
        }
    }
    None
}

unsafe fn parser_set_cached_token(
    self_: &mut TSParser,
    byte_index: u32,
    last_external_token: Subtree,
    token: Subtree,
) {
    let cache = &mut self_.token_cache;
    if !token.ptr.is_null() {
        subtree_retain(token);
    }
    if !last_external_token.ptr.is_null() {
        subtree_retain(last_external_token);
    }
    if !cache.token.ptr.is_null() {
        subtree_release(&mut self_.tree_pool, cache.token);
    }
    if !cache.last_external_token.ptr.is_null() {
        subtree_release(&mut self_.tree_pool, cache.last_external_token);
    }
    cache.token = token;
    cache.byte_index = byte_index;
    cache.last_external_token = last_external_token;
}

/// Find the initial lookahead for one stack version.
///
/// The parser tries sources in cheapest-to-most-expensive order:
///
/// 1. Reuse the parser's one-token cache for another version at this position.
/// 2. Ask the lexer to scan a fresh token.
///
/// The returned `needs_lex` flag tells `parser_advance` whether step 2 is
/// still required.
unsafe fn parser_get_initial_lookahead(
    self_: &mut TSParser,
    state: TSStateId,
    position: u32,
    last_external_token: Subtree,
) -> (Subtree, TableEntry, bool) {
    let (lookahead, table_entry) =
        parser_get_cached_token(self_, state, position as usize, last_external_token)
            .unwrap_or((NULL_SUBTREE, TableEntry::empty()));

    let needs_lex = lookahead.ptr.is_null();
    (lookahead, table_entry, needs_lex)
}

/// Lex a token for the current stack version and prepare its parse-table entry.
///
/// A null lookahead is meaningful when parsing a non-terminal extra: it asks the
/// parser to consult the EOF entry for a forced reduction, after which lexing
/// resumes from the new parse state.
unsafe fn parser_lex_lookahead(
    self_: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    position: u32,
    last_external_token: Subtree,
    lookahead: &mut Subtree,
    table_entry: &mut TableEntry,
) {
    *lookahead = parser_lex(self_, version, state);

    if !lookahead.ptr.is_null() {
        parser_set_cached_token(self_, position, last_external_token, *lookahead);
        language_table_entry(
            self_.language,
            state,
            subtree_symbol(*lookahead),
            table_entry,
        );
    } else {
        language_table_entry(self_.language, state, TS_BUILTIN_SYM_END, table_entry);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — tree selection
// ---------------------------------------------------------------------------

unsafe fn parser_select_tree(self_: &mut TSParser, left: Subtree, right: Subtree) -> bool {
    if left.ptr.is_null() {
        return true;
    }
    if right.ptr.is_null() {
        return false;
    }

    let left_error_cost = subtree_error_cost(left);
    let right_error_cost = subtree_error_cost(right);
    if right_error_cost < left_error_cost {
        parser_log(self_, |context, log| {
            write!(
                log,
                "select_smaller_error symbol:{}, over_symbol:{}",
                DisplayCStr(parser_symbol_name(context.language, subtree_symbol(right))),
                DisplayCStr(parser_symbol_name(context.language, subtree_symbol(left)))
            )
        });
        return true;
    }

    if left_error_cost < right_error_cost {
        parser_log(self_, |context, log| {
            write!(
                log,
                "select_smaller_error symbol:{}, over_symbol:{}",
                DisplayCStr(parser_symbol_name(context.language, subtree_symbol(left))),
                DisplayCStr(parser_symbol_name(context.language, subtree_symbol(right)))
            )
        });
        return false;
    }

    let left_dynamic_precedence = subtree_dynamic_precedence(left);
    let right_dynamic_precedence = subtree_dynamic_precedence(right);
    if right_dynamic_precedence > left_dynamic_precedence {
        parser_log(self_, |context, log| {
            write!(
                log,
                "select_higher_precedence symbol:{}, prec:{right_dynamic_precedence}, over_symbol:{}, other_prec:{left_dynamic_precedence}",
                DisplayCStr(parser_symbol_name(context.language, subtree_symbol(right))),
                DisplayCStr(parser_symbol_name(context.language, subtree_symbol(left)))
            )
        });
        return true;
    }

    if left_dynamic_precedence > right_dynamic_precedence {
        parser_log(self_, |context, log| {
            write!(
                log,
                "select_higher_precedence symbol:{}, prec:{left_dynamic_precedence}, over_symbol:{}, other_prec:{right_dynamic_precedence}",
                DisplayCStr(parser_symbol_name(context.language, subtree_symbol(left))),
                DisplayCStr(parser_symbol_name(context.language, subtree_symbol(right)))
            )
        });
        return false;
    }

    if left_error_cost > 0 {
        return true;
    }

    let comparison = subtree_compare(left, right, &mut self_.tree_pool);
    match comparison {
        -1 => {
            parser_log(self_, |context, log| {
                write!(
                    log,
                    "select_earlier symbol:{}, over_symbol:{}",
                    DisplayCStr(parser_symbol_name(context.language, subtree_symbol(left))),
                    DisplayCStr(parser_symbol_name(context.language, subtree_symbol(right)))
                )
            });
            false
        }
        1 => {
            parser_log(self_, |context, log| {
                write!(
                    log,
                    "select_earlier symbol:{}, over_symbol:{}",
                    DisplayCStr(parser_symbol_name(context.language, subtree_symbol(right))),
                    DisplayCStr(parser_symbol_name(context.language, subtree_symbol(left)))
                )
            });
            true
        }
        _ => {
            parser_log(self_, |context, log| {
                write!(
                    log,
                    "select_existing symbol:{}, over_symbol:{}",
                    DisplayCStr(parser_symbol_name(context.language, subtree_symbol(left))),
                    DisplayCStr(parser_symbol_name(context.language, subtree_symbol(right)))
                )
            });
            false
        }
    }
}

unsafe fn parser_select_children(
    self_: &mut TSParser,
    left: Subtree,
    children: &SubtreeArray,
) -> bool {
    let scratch_trees = &mut self_.scratch_trees;
    array_assign(scratch_trees, children);

    let scratch_tree = subtree_new_node(
        subtree_symbol(left),
        &mut self_.scratch_trees,
        0,
        self_.language,
    );

    parser_select_tree(self_, left, subtree_from_mut(scratch_tree))
}

unsafe fn parser_new_node(
    self_: &mut TSParser,
    symbol: TSSymbol,
    children: &mut SubtreeArray,
    production_id: u32,
) -> MutableSubtree {
    if self_.tree_arena.is_null() {
        subtree_new_node(symbol, children, production_id, self_.language)
    } else {
        let result = subtree_new_node_in_arena(
            self_.tree_arena,
            symbol,
            children.contents,
            children.size,
            production_id,
            self_.language,
        );
        array_delete(children);
        result
    }
}

const unsafe fn parser_builder_span_subtrees(
    builder: &StackPopBuilder,
    span: StackSliceSpan,
) -> SubtreeArray {
    SubtreeArray {
        contents: if span.size > 0 {
            builder.subtrees.contents.add(span.start as usize)
        } else {
            ptr::null_mut()
        },
        size: span.size,
        capacity: span.size,
    }
}

unsafe fn parser_new_node_from_builder_span(
    self_: &mut TSParser,
    symbol: TSSymbol,
    children: &SubtreeArray,
    production_id: u32,
) -> MutableSubtree {
    if self_.tree_arena.is_null() {
        let mut owned_children = array_new();
        array_reserve(&mut owned_children, children.size);
        if children.size > 0 {
            ptr::copy_nonoverlapping(
                children.contents,
                owned_children.contents,
                children.size as usize,
            );
        }
        owned_children.size = children.size;
        subtree_new_node(symbol, &mut owned_children, production_id, self_.language)
    } else {
        subtree_new_node_in_arena(
            self_.tree_arena,
            symbol,
            children.contents,
            children.size,
            production_id,
            self_.language,
        )
    }
}

unsafe fn parser_release_builder_span(self_: &mut TSParser, span: StackSliceSpan) {
    if span.size == 0 {
        return;
    }
    let contents = self_
        .reduce_builder
        .subtrees
        .contents
        .add(span.start as usize);
    for i in 0..span.size {
        subtree_release(&mut self_.tree_pool, *contents.add(i as usize));
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — shift/reduce/accept
// ---------------------------------------------------------------------------

unsafe fn parser_shift(
    self_: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    lookahead: Subtree,
    extra: bool,
) {
    let is_leaf = subtree_child_count(lookahead) == 0;
    let subtree_to_push = if extra != subtree_extra(lookahead) && is_leaf {
        let mut result = subtree_make_mut(&mut self_.tree_pool, lookahead);
        subtree_set_extra(&mut result, extra);
        subtree_from_mut(result)
    } else {
        lookahead
    };

    stack_push(ptr_mut(self_.stack), version, subtree_to_push, state);
    if subtree_has_external_tokens(subtree_to_push) {
        stack_set_last_external_token(
            ptr_mut(self_.stack),
            version,
            subtree_last_external_token(subtree_to_push),
        );
    }
}

const IN_PLACE_REDUCTION_WARMUP: u32 = 5_000;

unsafe fn parser_reduce_in_place_after_warmup(
    self_: &mut TSParser,
    version: StackVersion,
    symbol: TSSymbol,
    count: u32,
    dynamic_precedence: i32,
    production_id: u16,
    end_of_non_terminal_extra: bool,
) -> bool {
    if stack_version_count(ptr_ref(self_.stack)) != 1
        || self_.deterministic_reduction_count < IN_PLACE_REDUCTION_WARMUP
        || count == 0
    {
        return false;
    }

    let stack = ptr_mut(self_.stack);
    if !stack_pop_count_linear_in_place(stack, version, count, &mut self_.reduce_builder) {
        return false;
    }

    let mut children = SubtreeArray {
        contents: self_.reduce_builder.subtrees.contents,
        size: self_.reduce_builder.subtrees.size,
        capacity: self_.reduce_builder.subtrees.capacity,
    };
    subtree_array_remove_trailing_extras(&mut children, &mut self_.trailing_extras);

    let parent =
        parser_new_node_from_builder_span(self_, symbol, &children, u32::from(production_id));
    let state = stack_state(stack, version);
    let next_state = if symbol != TS_BUILTIN_SYM_ERROR
        && symbol != TS_BUILTIN_SYM_ERROR_REPEAT
        && u32::from(symbol) >= language_full(self_.language).token_count
    {
        language_lookup(self_.language, state, symbol)
    } else {
        ts_language_next_state(self_.language, state, symbol)
    };
    if end_of_non_terminal_extra && next_state == state {
        (*parent.ptr).set_extra(true);
    }
    (*parent.ptr).parse_state = state;
    (*parent.ptr).data.children.dynamic_precedence += dynamic_precedence;

    stack_push(stack, version, subtree_from_mut(parent), next_state);
    for j in 0..self_.trailing_extras.size {
        stack_push(
            stack,
            version,
            *array_get_ref(&self_.trailing_extras, j),
            next_state,
        );
    }

    self_.reduce_builder.subtrees.size = 0;
    true
}

#[allow(clippy::too_many_arguments)]
/// Apply one reduce action to a stack version.
///
/// Algorithm:
/// - Pop `count` payloads from the target version. A GLR node can have multiple
///   predecessor links, so one reduce can produce several child slices.
/// - For slices that came from the same version, choose the best child list and
///   release the others.
/// - Build the parent subtree, compute the goto state, push the parent and any
///   stripped trailing extras.
/// - Try to merge the resulting stack version back into earlier versions.
///
/// Pop results are written into `reduce_builder`, avoiding a temporary
/// `StackSliceArray` allocation on each reduction.
unsafe fn parser_reduce(
    self_: &mut TSParser,
    version: StackVersion,
    symbol: TSSymbol,
    count: u32,
    dynamic_precedence: i32,
    production_id: u16,
    invalidate_parse_state: bool,
    end_of_non_terminal_extra: bool,
) -> StackVersion {
    let initial_version_count = stack_version_count(ptr_ref(self_.stack));

    stack_pop_count_into(
        ptr_mut(self_.stack),
        version,
        count,
        &mut self_.reduce_builder,
    );
    let mut removed_version_count: u32 = 0;
    let stack = ptr_mut(self_.stack);
    let halted_version_count = stack_halted_version_count(stack);
    let mut i: u32 = 0;
    let pop_size = self_.reduce_builder.slices.size;
    while i < pop_size {
        let span = *array_get_ref(&self_.reduce_builder.slices, i);
        let slice_version = span.version - removed_version_count;

        // Limit max versions
        if slice_version > MAX_VERSION_COUNT + MAX_VERSION_COUNT_OVERFLOW + halted_version_count {
            stack_remove_version(stack, slice_version);
            parser_release_builder_span(self_, span);
            removed_version_count += 1;
            while i + 1 < pop_size {
                parser_log(self_, |_, log| {
                    log.write_str("aborting reduce with too many versions")
                });
                let next_span = *array_get_ref(&self_.reduce_builder.slices, i + 1);
                if next_span.version != span.version {
                    break;
                }
                parser_release_builder_span(self_, next_span);
                i += 1;
            }
            i += 1;
            continue;
        }

        // Remove trailing extras from children
        let mut children = parser_builder_span_subtrees(&self_.reduce_builder, span);
        subtree_array_remove_trailing_extras(&mut children, &mut self_.trailing_extras);

        let mut parent =
            parser_new_node_from_builder_span(self_, symbol, &children, u32::from(production_id));

        // Handle merged stack versions
        while i + 1 < pop_size {
            let next_span = *array_get_ref(&self_.reduce_builder.slices, i + 1);
            if next_span.version != span.version {
                break;
            }
            i += 1;

            let mut next_slice_children =
                parser_builder_span_subtrees(&self_.reduce_builder, next_span);
            subtree_array_remove_trailing_extras(
                &mut next_slice_children,
                &mut self_.trailing_extras2,
            );

            if parser_select_children(self_, subtree_from_mut(parent), &next_slice_children) {
                subtree_array_clear(&mut self_.tree_pool, &mut self_.trailing_extras);
                subtree_release(&mut self_.tree_pool, subtree_from_mut(parent));
                array_swap(&mut self_.trailing_extras, &mut self_.trailing_extras2);
                parent = parser_new_node_from_builder_span(
                    self_,
                    symbol,
                    &next_slice_children,
                    u32::from(production_id),
                );
            } else {
                array_clear(&mut self_.trailing_extras2);
                parser_release_builder_span(self_, next_span);
            }
        }

        let state = stack_state(stack, slice_version);
        let next_state = if symbol != TS_BUILTIN_SYM_ERROR
            && symbol != TS_BUILTIN_SYM_ERROR_REPEAT
            && u32::from(symbol) >= language_full(self_.language).token_count
        {
            language_lookup(self_.language, state, symbol)
        } else {
            ts_language_next_state(self_.language, state, symbol)
        };
        if end_of_non_terminal_extra && next_state == state {
            (*parent.ptr).set_extra(true);
        }
        (*parent.ptr).parse_state =
            if invalidate_parse_state || pop_size > 1 || initial_version_count > 1 {
                TS_TREE_STATE_NONE
            } else {
                state
            };
        (*parent.ptr).data.children.dynamic_precedence += dynamic_precedence;

        // Push the parent node and trailing extras
        stack_push(stack, slice_version, subtree_from_mut(parent), next_state);
        for j in 0..self_.trailing_extras.size {
            stack_push(
                stack,
                slice_version,
                *array_get_ref(&self_.trailing_extras, j),
                next_state,
            );
        }

        for j in 0..slice_version {
            if j == version {
                continue;
            }
            if stack_merge(stack, j, slice_version) {
                removed_version_count += 1;
                break;
            }
        }

        i += 1;
    }
    self_.reduce_builder.slices.size = 0;
    self_.reduce_builder.subtrees.size = 0;

    if stack_version_count(stack) > initial_version_count {
        initial_version_count
    } else {
        STACK_VERSION_NONE
    }
}

unsafe fn parser_accept(self_: &mut TSParser, version: StackVersion, lookahead: Subtree) {
    debug_assert!(subtree_is_eof(lookahead));
    let stack = ptr_mut(self_.stack);
    stack_push(stack, version, lookahead, 1);

    let pop = stack_pop_all(stack, version);
    for i in 0..pop.size {
        let mut trees = ptr::read(&array_get_ref(&pop, i).subtrees);

        let mut root = NULL_SUBTREE;
        let mut j = i64::from(trees.size) - 1;
        while j >= 0 {
            let tree = *array_get_ref(&trees, j as u32);
            if !subtree_extra(tree) {
                debug_assert!(!tree.data.is_inline());
                let child_count = subtree_child_count(tree);
                let children = subtree_children_slice(tree);
                for child in children {
                    subtree_retain(*child);
                }
                array_splice(&mut trees, j as u32, 1, child_count, children.as_ptr());
                root = subtree_from_mut(parser_new_node(
                    self_,
                    subtree_symbol(tree),
                    &mut trees,
                    u32::from((*tree.ptr).data.children.production_id),
                ));
                subtree_release(&mut self_.tree_pool, tree);
                break;
            }
            j -= 1;
        }

        debug_assert!(!root.ptr.is_null());
        self_.accept_count += 1;

        if !self_.finished_tree.ptr.is_null() {
            if parser_select_tree(self_, self_.finished_tree, root) {
                subtree_release(&mut self_.tree_pool, self_.finished_tree);
                self_.finished_tree = root;
            } else {
                subtree_release(&mut self_.tree_pool, root);
            }
        } else {
            self_.finished_tree = root;
        }
    }

    stack_remove_version(stack, array_get_ref(&pop, 0).version);
    stack_halt(stack, version);
}

// ---------------------------------------------------------------------------
// Internal helpers — error recovery
// ---------------------------------------------------------------------------

unsafe fn parser_do_all_potential_reductions(
    self_: &mut TSParser,
    starting_version: StackVersion,
    lookahead_symbol: TSSymbol,
) -> bool {
    let lang = language_full(self_.language);
    let initial_version_count = stack_version_count(ptr_ref(self_.stack));

    let mut can_shift_lookahead_symbol = false;
    let mut version = starting_version;
    let mut i: u32 = 0;
    loop {
        let version_count = stack_version_count(ptr_ref(self_.stack));
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

        let state = stack_state(ptr_ref(self_.stack), version);
        let mut has_shift_action = false;
        array_clear(&mut self_.reduce_actions);

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
                        reduce_action_set_add(
                            &mut self_.reduce_actions,
                            ReduceAction {
                                symbol: action.reduce.symbol,
                                count: u32::from(action.reduce.child_count),
                                dynamic_precedence: i32::from(action.reduce.dynamic_precedence),
                                production_id: action.reduce.production_id,
                            },
                        );
                    }
                    _ => {}
                }
            }
            symbol += 1;
        }

        let mut reduction_version = STACK_VERSION_NONE;
        for j in 0..self_.reduce_actions.size {
            let action = array_get_ref(&self_.reduce_actions, j);
            reduction_version = parser_reduce(
                self_,
                version,
                action.symbol,
                action.count,
                action.dynamic_precedence,
                action.production_id,
                true,
                false,
            );
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
        let mut slice = ptr::read(array_get_ref(&pop, i));

        if slice.version == previous_version {
            subtree_array_delete(&mut self_.tree_pool, &mut slice.subtrees);
            array_erase(&mut pop, i);
            continue;
        }

        if stack_state(stack, slice.version) != goal_state {
            stack_halt(stack, slice.version);
            subtree_array_delete(&mut self_.tree_pool, &mut slice.subtrees);
            array_erase(&mut pop, i);
            continue;
        }

        let mut error_trees = stack_pop_error(stack, slice.version);
        if error_trees.size > 0 {
            debug_assert_eq!(error_trees.size, 1);
            let error_tree = *error_trees.contents;
            let error_child_count = subtree_child_count(error_tree);
            if error_child_count > 0 {
                let error_children = subtree_children_slice(error_tree);
                array_splice(
                    &mut slice.subtrees,
                    0,
                    0,
                    error_child_count,
                    error_children.as_ptr(),
                );
                for child in error_children {
                    subtree_retain(*child);
                }
            }
            subtree_array_delete(&mut self_.tree_pool, &mut error_trees);
        }

        subtree_array_remove_trailing_extras(&mut slice.subtrees, &mut self_.trailing_extras);

        if slice.subtrees.size > 0 {
            let error = subtree_new_error_node(&mut slice.subtrees, true, self_.language);
            stack_push(stack, slice.version, error, goal_state);
        } else {
            array_delete(&mut slice.subtrees);
        }

        for j in 0..self_.trailing_extras.size {
            let tree = *array_get_ref(&self_.trailing_extras, j);
            stack_push(stack, slice.version, tree, goal_state);
        }

        previous_version = slice.version;
        i += 1;
    }

    previous_version != STACK_VERSION_NONE
}

unsafe fn parser_recover(self_: &mut TSParser, version: StackVersion, mut lookahead: Subtree) {
    let mut did_recover = false;
    let stack = ptr_mut(self_.stack);
    let previous_version_count = stack_version_count(stack);
    let position = stack_position(stack, version);
    let summary = stack_get_summary(stack, version);
    let node_count_since_error = stack_node_count_since_error(stack, version);
    let current_error_cost = stack_error_cost(stack, version);

    // Strategy 1: Find a previous state where the lookahead is valid.
    if !summary.is_null() && !subtree_is_error(lookahead) {
        let summary = ptr_ref(summary);
        for i in 0..summary.size {
            let entry = *array_get_ref(summary, i);

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
                    if stack_state(stack, j) == entry.state
                        && stack_position(stack, j).bytes == position.bytes
                    {
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

            if language_has_actions(self_.language, entry.state, subtree_symbol(lookahead))
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
    while i < stack_version_count(stack) {
        if !stack_is_active(stack, i) {
            parser_log(self_, |_, log| write!(log, "removed paused version:{i}"));
            stack_remove_version(stack, i);
            parser_log_stack(self_);
        } else {
            i += 1;
        }
    }

    // EOF: wrap everything and terminate
    if subtree_is_eof(lookahead) {
        parser_log(self_, |_, log| log.write_str("recover_eof"));
        let mut children: SubtreeArray = array_new();
        let parent = subtree_new_error_node(&mut children, false, self_.language);
        stack_push(stack, version, parent, 1);
        parser_accept(self_, version, lookahead);
        return;
    }

    // Strategy 2: skip the current token
    if did_recover && stack_version_count(stack) > MAX_VERSION_COUNT {
        stack_halt(stack, version);
        subtree_release(&mut self_.tree_pool, lookahead);
        return;
    }

    if did_recover && subtree_has_external_scanner_state_change(lookahead) {
        stack_halt(stack, version);
        subtree_release(&mut self_.tree_pool, lookahead);
        return;
    }

    let new_cost = current_error_cost
        + ERROR_COST_PER_SKIPPED_TREE
        + subtree_total_bytes(lookahead) * ERROR_COST_PER_SKIPPED_CHAR
        + subtree_total_size(lookahead).extent.row * ERROR_COST_PER_SKIPPED_LINE;
    if parser_better_version_exists(self_, version, false, new_cost) {
        stack_halt(stack, version);
        subtree_release(&mut self_.tree_pool, lookahead);
        return;
    }

    // Mark extra tokens
    let mut n: u32 = 0;
    let actions = language_actions(self_.language, 1, subtree_symbol(lookahead), &mut n);
    if n > 0
        && (*actions.add(n as usize - 1)).type_ == TSPARSE_ACTION_TYPE_SHIFT
        && (*actions.add(n as usize - 1)).shift.extra
    {
        let mut mutable_lookahead = subtree_make_mut(&mut self_.tree_pool, lookahead);
        subtree_set_extra(&mut mutable_lookahead, true);
        lookahead = subtree_from_mut(mutable_lookahead);
    }

    // Wrap the lookahead in an ERROR
    parser_log(self_, |context, log| {
        write!(
            log,
            "skip_token symbol:{}",
            DisplayCStr(parser_symbol_name(
                context.language,
                subtree_symbol(lookahead)
            ))
        )
    });
    let mut children: SubtreeArray = array_new();
    array_reserve(&mut children, 1);
    array_push(&mut children, lookahead);
    let mut error_repeat = parser_new_node(self_, TS_BUILTIN_SYM_ERROR_REPEAT, &mut children, 0);

    // Merge with existing error on top of stack
    if node_count_since_error > 0 {
        let mut pop = stack_pop_count(stack, version, 1);

        if pop.size > 1 {
            for pi in 1..pop.size {
                subtree_array_delete(
                    &mut self_.tree_pool,
                    &mut array_get_mut(&mut pop, pi).subtrees,
                );
            }
            while stack_version_count(stack) > array_get_ref(&pop, 0).version + 1 {
                stack_remove_version(stack, array_get_ref(&pop, 0).version + 1);
            }
        }

        stack_renumber_version(stack, array_get_ref(&pop, 0).version, version);
        let slot = &mut array_get_mut(&mut pop, 0).subtrees;
        array_push(slot, subtree_from_mut(error_repeat));
        error_repeat = parser_new_node(self_, TS_BUILTIN_SYM_ERROR_REPEAT, slot, 0);
    }

    // Push the ERROR
    stack_push(stack, version, subtree_from_mut(error_repeat), ERROR_STATE);
    if subtree_has_external_tokens(lookahead) {
        stack_set_last_external_token(stack, version, subtree_last_external_token(lookahead));
    }

    let mut has_error = true;
    for vi in 0..stack_version_count(stack) {
        let status = parser_version_status(self_, vi);
        if !status.is_in_error {
            has_error = false;
            break;
        }
    }
    self_.has_error = has_error;
}

unsafe fn parser_handle_error(self_: &mut TSParser, version: StackVersion, lookahead: Subtree) {
    let previous_version_count = stack_version_count(ptr_ref(self_.stack));

    // Perform any reductions that can happen in this state, regardless of the lookahead. After
    // skipping one or more invalid tokens, the parser might find a token that would have allowed
    // a reduction to take place.
    parser_do_all_potential_reductions(self_, version, 0);
    let version_count = stack_version_count(ptr_ref(self_.stack));
    let position = stack_position(ptr_ref(self_.stack), version);

    // Push a discontinuity onto the stack. Merge all of the stack versions that
    // were created in the previous step.
    let mut did_insert_missing_token = false;
    let mut v = version;
    while v < version_count {
        if !did_insert_missing_token {
            let state = stack_state(ptr_ref(self_.stack), v);
            let language = language_full(self_.language);
            let mut missing_symbol: TSSymbol = 1;
            while u32::from(missing_symbol) < language.token_count {
                let state_after_missing_symbol =
                    ts_language_next_state(self_.language, state, missing_symbol);
                if state_after_missing_symbol == 0 || state_after_missing_symbol == state {
                    missing_symbol += 1;
                    continue;
                }

                if language_has_reduce_action(
                    self_.language,
                    state_after_missing_symbol,
                    subtree_symbol(lookahead),
                ) {
                    // In case the parser is currently outside of any included range, the lexer will
                    // snap to the beginning of the next included range. The missing token's padding
                    // must be assigned to position it within the next included range.
                    lexer_reset(&mut self_.lexer, position);
                    lexer_mark_end(&mut self_.lexer);
                    let padding = length_sub(self_.lexer.token_end_position, position);
                    let lookahead_bytes =
                        subtree_total_bytes(lookahead) + subtree_lookahead_bytes(lookahead);

                    let version_with_missing_tree = stack_copy_version(ptr_mut(self_.stack), v);
                    let missing_tree = subtree_new_missing_leaf(
                        &mut self_.tree_pool,
                        missing_symbol,
                        padding,
                        lookahead_bytes,
                        self_.language,
                    );
                    stack_push(
                        ptr_mut(self_.stack),
                        version_with_missing_tree,
                        missing_tree,
                        state_after_missing_symbol,
                    );

                    if parser_do_all_potential_reductions(
                        self_,
                        version_with_missing_tree,
                        subtree_symbol(lookahead),
                    ) {
                        parser_log(self_, |context, log| {
                            write!(
                                log,
                                "recover_with_missing symbol:{}, state:{}",
                                DisplayCStr(parser_symbol_name(context.language, missing_symbol)),
                                u32::from(stack_state(
                                    ptr_ref(context.stack),
                                    version_with_missing_tree,
                                ))
                            )
                        });
                        did_insert_missing_token = true;
                        break;
                    }
                }
                missing_symbol += 1;
            }
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
            if !lookahead.ptr.is_null() {
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

unsafe fn parser_recover_for_action(
    self_: &mut TSParser,
    version: StackVersion,
    lookahead: &mut Subtree,
) {
    parser_recover(self_, version, *lookahead);
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
                let invalidate_parse_state = table_entry.action_count > 1;
                let end_of_non_terminal_extra = lookahead.ptr.is_null();
                if table_entry.action_count == 1 && stack_version_count(ptr_ref(self_.stack)) == 1 {
                    self_.deterministic_reduction_count =
                        self_.deterministic_reduction_count.saturating_add(1);
                }
                parser_log(self_, |context, log| {
                    write!(
                        log,
                        "reduce sym:{}, child_count:{}",
                        DisplayCStr(parser_symbol_name(context.language, reduce.symbol)),
                        u32::from(reduce.child_count)
                    )
                });
                let reduction_version = if table_entry.action_count == 1
                    && parser_reduce_in_place_after_warmup(
                        self_,
                        version,
                        reduce.symbol,
                        u32::from(reduce.child_count),
                        i32::from(reduce.dynamic_precedence),
                        reduce.production_id,
                        end_of_non_terminal_extra,
                    ) {
                    version
                } else {
                    parser_reduce(
                        self_,
                        version,
                        reduce.symbol,
                        u32::from(reduce.child_count),
                        i32::from(reduce.dynamic_precedence),
                        reduce.production_id,
                        invalidate_parse_state,
                        end_of_non_terminal_extra,
                    )
                };
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
                parser_recover_for_action(self_, version, lookahead);
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
    if lookahead.ptr.is_null() {
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
    if !lookahead.ptr.is_null() {
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

unsafe fn parser_balance_subtree(self_: &mut TSParser) -> bool {
    let finished_tree = self_.finished_tree;

    // If we haven't canceled balancing in progress before, then we want to clear the tree stack and
    // push the initial finished tree onto it. Otherwise, if we're resuming balancing after a
    // cancellation, we don't want to clear the tree stack.
    if !self_.canceled_balancing {
        array_clear(&mut self_.tree_pool.tree_stack);
        if subtree_child_count(finished_tree) > 0 && (*finished_tree.ptr).ref_count == 1 {
            array_push(
                &mut self_.tree_pool.tree_stack,
                subtree_to_mut_unsafe(finished_tree),
            );
        }
    }

    while self_.tree_pool.tree_stack.size > 0 {
        if !parser_check_progress(self_, None, None, 1) {
            return false;
        }

        let tree = *array_back_ref(&self_.tree_pool.tree_stack);

        if (*tree.ptr).data.children.repeat_depth > 0 {
            let tree_subtree = subtree_from_mut(tree);
            let children = subtree_children_slice(tree_subtree);
            let child1 = *children.get_unchecked(0);
            let child2 = *children.get_unchecked((*tree.ptr).child_count as usize - 1);
            let repeat_delta =
                i64::from(subtree_repeat_depth(child1)) - i64::from(subtree_repeat_depth(child2));
            if repeat_delta > 0 {
                let n = repeat_delta as u32;

                let mut i = n / 2;
                while i > 0 {
                    subtree_compress(tree, i, self_.language, &mut self_.tree_pool.tree_stack);

                    // We scale the operation count increment in `parser_check_progress` proportionately to the compression
                    // size since larger values of i take longer to process. Shifting by 4 empirically provides good check
                    // intervals (e.g. 193 operations when i=3100) to prevent blocking during large compressions.
                    let operations = if i >> 4 > 0 { i >> 4 } else { 1 };
                    if !parser_check_progress(self_, None, None, operations) {
                        return false;
                    }
                    i /= 2;
                }
            }
        }

        array_pop(&mut self_.tree_pool.tree_stack);

        for i in 0..(*tree.ptr).child_count {
            let tree_subtree = subtree_from_mut(tree);
            let child = *subtree_child(tree_subtree, i);
            if subtree_child_count(child) > 0 && (*child.ptr).ref_count == 1 {
                array_push(
                    &mut self_.tree_pool.tree_stack,
                    subtree_to_mut_unsafe(child),
                );
            }
        }
    }

    true
}

unsafe fn parser_has_outstanding_parse(self_: &TSParser) -> bool {
    self_.canceled_balancing
        || !self_.external_scanner_payload.is_null()
        || stack_state(ptr_ref(self_.stack), 0) != 1
        || stack_node_count_since_error(ptr_mut(self_.stack), 0) != 0
}

unsafe fn parser_take_finished_tree(self_: &mut TSParser) -> *mut TSTree {
    let arena = self_.tree_arena;
    self_.tree_arena = ptr::null_mut();
    let result = tree_new_with_arena(
        self_.finished_tree,
        self_.language,
        self_.lexer.included_ranges,
        self_.lexer.included_range_count,
        arena,
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
            reduce_actions: array_new(),
            finished_tree: NULL_SUBTREE,
            reduce_builder: stack_pop_builder_new(),
            trailing_extras: array_new(),
            trailing_extras2: array_new(),
            scratch_trees: array_new(),
            token_cache: TokenCache {
                token: NULL_SUBTREE,
                last_external_token: NULL_SUBTREE,
                byte_index: 0,
            },
            deterministic_reduction_count: 0,
            tree_arena: ptr::null_mut(),
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
    array_reserve(&mut parser.reduce_actions, 4);
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
        array_delete(&mut parser.reduce_actions);
    }
    if !parser.tree_arena.is_null() {
        tree_arena_release(parser.tree_arena);
        parser.tree_arena = ptr::null_mut();
    }
    lexer_delete(&mut parser.lexer);
    parser_set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    subtree_pool_delete(&mut parser.tree_pool);
    stack_pop_builder_delete(&mut parser.reduce_builder);
    array_delete(&mut parser.trailing_extras);
    array_delete(&mut parser.trailing_extras2);
    array_delete(&mut parser.scratch_trees);
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

    parser.deterministic_reduction_count = 0;
    lexer_reset(&mut parser.lexer, length_zero());
    stack_clear(ptr_mut(parser.stack));
    parser_set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    if !parser.finished_tree.ptr.is_null() {
        subtree_release(&mut parser.tree_pool, parser.finished_tree);
        parser.finished_tree = NULL_SUBTREE;
    }
    if !parser.tree_arena.is_null() {
        tree_arena_release(parser.tree_arena);
        parser.tree_arena = ptr::null_mut();
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
/// - initialize lexer, external scanner, and tree arena;
/// - process every active stack version until none can advance normally;
/// - condense/merge/prune stack versions;
/// - recover when all versions are paused at errors;
/// - balance the accepted tree and transfer arena ownership into `TSTree`.
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
            // goto balance
            debug_assert!(!parser.finished_tree.ptr.is_null());
            if !parser_balance_subtree(parser) {
                parser.canceled_balancing = true;
                return ptr::null_mut();
            }
            parser.canceled_balancing = false;
            parser_log(parser, |_, log| log.write_str("done"));
            parser_log_tree(parser, parser.finished_tree);

            let result = parser_take_finished_tree(parser);

            // goto exit
            ts_parser_reset(self_);
            return result;
        }
    } else {
        parser_external_scanner_create(parser);
        parser.tree_arena = tree_arena_new();
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
        if !parser.finished_tree.ptr.is_null()
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
    debug_assert!(!parser.finished_tree.ptr.is_null());
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
