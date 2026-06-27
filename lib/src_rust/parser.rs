#![allow(dead_code)]
#![allow(non_snake_case)]

use core::ffi::c_void;
use std::ptr;

use crate::ffi::{
    TSInput, TSInputEncoding, TSInputEncodingUTF8, TSLanguage, TSLogTypeParse, TSLogger,
    TSParseOptions, TSParseState, TSPoint, TSRange, TSStateId, TSSymbol, TSWasmStore,
};

use super::alloc::{ts_calloc, ts_free};
use super::error_costs::{
    ERROR_COST_PER_RECOVERY, ERROR_COST_PER_SKIPPED_CHAR, ERROR_COST_PER_SKIPPED_LINE,
    ERROR_COST_PER_SKIPPED_TREE, ERROR_STATE,
};
use super::get_changed_ranges::{
    ts_range_array_get_changed_ranges_ref, ts_range_array_intersects_ref, ts_range_slice,
    TSRangeArray,
};
use super::language::{
    ts_language_actions, ts_language_alias_sequence, ts_language_copy, ts_language_delete,
    ts_language_enabled_external_tokens, ts_language_has_actions, ts_language_has_reduce_action,
    ts_language_is_reserved_word, ts_language_lex_mode_for_state, ts_language_lookup,
    ts_language_next_state, ts_language_symbol_metadata, ts_language_symbol_name,
    ts_language_table_entry, TSLanguageFull, TSLexer, TSLexerMode,
    TSParseActionTypeAccept as TSPARSE_ACTION_TYPE_ACCEPT,
    TSParseActionTypeRecover as TSPARSE_ACTION_TYPE_RECOVER,
    TSParseActionTypeReduce as TSPARSE_ACTION_TYPE_REDUCE,
    TSParseActionTypeShift as TSPARSE_ACTION_TYPE_SHIFT, TableEntry,
};
use super::length::{length_add, length_sub, length_zero, Length};
use super::lexer::{
    ts_lexer_delete, ts_lexer_finish, ts_lexer_included_ranges, ts_lexer_init, ts_lexer_mark_end,
    ts_lexer_reset, ts_lexer_set_included_ranges, ts_lexer_set_input, ts_lexer_start, Lexer,
};
use super::reusable_node::{
    reusable_node_advance, reusable_node_advance_past_leaf, reusable_node_byte_offset,
    reusable_node_clear, reusable_node_delete, reusable_node_descend, reusable_node_new,
    reusable_node_reset, reusable_node_tree, ReusableNode,
};
use super::stack::{
    array_assign,
    array_back_ref,
    array_clear,
    array_delete,
    array_erase,
    array_get_mut,
    array_get_ref,
    array_init,
    array_new,
    array_pop,
    array_push,
    array_reserve,
    array_splice,
    array_swap,
    // Stack functions (now Rust-only)
    ts_stack_can_merge,
    ts_stack_clear,
    ts_stack_copy_version,
    ts_stack_delete,
    ts_stack_dynamic_precedence,
    ts_stack_error_cost,
    ts_stack_get_summary,
    ts_stack_halt,
    ts_stack_halted_version_count,
    ts_stack_has_advanced_since_error,
    ts_stack_is_active,
    ts_stack_is_halted,
    ts_stack_is_paused,
    ts_stack_last_external_token,
    ts_stack_link_payload_is_pending_reduction,
    ts_stack_link_payload_pending_reduction,
    ts_stack_link_payload_release,
    ts_stack_link_payload_retain,
    ts_stack_link_payload_subtree,
    ts_stack_merge,
    ts_stack_new,
    ts_stack_node_count_since_error,
    ts_stack_pause,
    ts_stack_pop_all,
    ts_stack_pop_builder_delete,
    ts_stack_pop_builder_new,
    ts_stack_pop_count,
    ts_stack_pop_count_into,
    ts_stack_pop_error,
    ts_stack_pop_pending,
    ts_stack_position,
    ts_stack_print_dot_graph,
    ts_stack_push,
    ts_stack_record_summary,
    ts_stack_remove_version,
    ts_stack_renumber_version,
    ts_stack_resume,
    ts_stack_set_last_external_token,
    ts_stack_state,
    ts_stack_swap_versions,
    ts_stack_version_count,
    Array,
    PendingReduction,
    Stack,
    StackLinkPayload,
    StackPopBuilder,
    StackSliceSpan,
    StackVersion,
    PENDING_REDUCTION_DEPENDS_ON_COLUMN,
    PENDING_REDUCTION_EXTRA,
    PENDING_REDUCTION_FRAGILE_LEFT,
    PENDING_REDUCTION_FRAGILE_RIGHT,
    PENDING_REDUCTION_HAS_EXTERNAL_SCANNER_STATE_CHANGE,
    PENDING_REDUCTION_HAS_EXTERNAL_TOKENS,
    PENDING_REDUCTION_NAMED,
    PENDING_REDUCTION_VISIBLE,
    STACK_VERSION_NONE,
};
use super::subtree::{
    ts_builtin_sym_end,
    ts_builtin_sym_error,
    ts_builtin_sym_error_repeat,
    // Subtree functions (now Rust-only)
    ts_external_scanner_state_data,
    ts_external_scanner_state_eq,
    ts_external_scanner_state_init,
    ts_subtree_array_clear,
    ts_subtree_array_delete,
    ts_subtree_array_remove_trailing_extras,
    ts_subtree_child_count,
    ts_subtree_children,
    ts_subtree_compare,
    ts_subtree_compress,
    ts_subtree_depends_on_column,
    ts_subtree_dynamic_precedence,
    ts_subtree_error_cost,
    ts_subtree_external_scanner_state,
    ts_subtree_external_scanner_state_eq,
    ts_subtree_extra,
    ts_subtree_fragile_left,
    ts_subtree_fragile_right,
    ts_subtree_from_mut,
    ts_subtree_has_changes,
    ts_subtree_has_external_scanner_state_change,
    ts_subtree_has_external_tokens,
    ts_subtree_is_eof,
    ts_subtree_is_error,
    ts_subtree_is_fragile,
    ts_subtree_is_keyword,
    ts_subtree_last_external_token,
    ts_subtree_leaf_parse_state,
    ts_subtree_leaf_symbol,
    ts_subtree_lookahead_bytes,
    ts_subtree_make_mut,
    ts_subtree_missing,
    ts_subtree_named,
    ts_subtree_named_child_count,
    ts_subtree_new_error,
    ts_subtree_new_error_node,
    ts_subtree_new_leaf,
    ts_subtree_new_missing_leaf,
    ts_subtree_new_node,
    ts_subtree_new_node_in_arena,
    ts_subtree_padding,
    ts_subtree_parse_state,
    ts_subtree_pool_delete,
    ts_subtree_pool_new,
    ts_subtree_print_dot_graph,
    ts_subtree_release,
    ts_subtree_repeat_depth,
    ts_subtree_retain,
    ts_subtree_set_extra,
    ts_subtree_set_symbol,
    ts_subtree_size,
    ts_subtree_symbol,
    ts_subtree_to_mut_unsafe,
    ts_subtree_total_bytes,
    ts_subtree_total_size,
    ts_subtree_visible,
    ts_subtree_visible_child_count,
    ts_subtree_visible_descendant_count,
    ts_tree_arena_new,
    ts_tree_arena_release,
    ExternalScannerState,
    MutableSubtree,
    MutableSubtreeArray,
    Subtree,
    SubtreeArray,
    SubtreePool,
    TreeArena,
    NULL_SUBTREE,
    TS_TREE_STATE_NONE,
};
use super::tree::{ts_tree_new_with_arena, TSTree};

// ---------------------------------------------------------------------------
// Extern C functions
// ---------------------------------------------------------------------------

extern "C" {
    // wasm_store.c (still in C)
    fn ts_language_is_wasm(self_: *const TSLanguage) -> bool;
    fn ts_wasm_store_start(
        self_: *mut TSWasmStore,
        lexer: *mut TSLexer,
        language: *const TSLanguage,
    ) -> bool;
    fn ts_wasm_store_reset(self_: *mut TSWasmStore);
    fn ts_wasm_store_delete(self_: *mut TSWasmStore);
    fn ts_wasm_store_has_error(self_: *const TSWasmStore) -> bool;
    fn ts_wasm_store_call_lex_main(self_: *mut TSWasmStore, state: u16) -> bool;
    fn ts_wasm_store_call_lex_keyword(self_: *mut TSWasmStore, state: u16) -> bool;
    fn ts_wasm_store_call_scanner_create(self_: *mut TSWasmStore) -> u32;
    fn ts_wasm_store_call_scanner_destroy(self_: *mut TSWasmStore, scanner_address: u32);
    fn ts_wasm_store_call_scanner_serialize(
        self_: *mut TSWasmStore,
        scanner_address: u32,
        buffer: *mut i8,
    ) -> u32;
    fn ts_wasm_store_call_scanner_deserialize(
        self_: *mut TSWasmStore,
        scanner_address: u32,
        buffer: *const i8,
        length: u32,
    );
    fn ts_wasm_store_call_scanner_scan(
        self_: *mut TSWasmStore,
        scanner_address: u32,
        valid_tokens_ix: u32,
    ) -> bool;

    // libc
    fn snprintf(buf: *mut i8, size: usize, fmt: *const i8, ...) -> i32;
    fn fprintf(f: *mut c_void, fmt: *const i8, ...) -> i32;
    fn fputs(s: *const i8, f: *mut c_void) -> i32;
    fn fputc(c: i32, f: *mut c_void) -> i32;
    #[cfg(not(target_os = "windows"))]
    fn fdopen(fd: i32, mode: *const i8) -> *mut c_void;
    #[cfg(not(target_os = "windows"))]
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
// Logging macros (equivalent to C LOG, LOG_STACK, LOG_TREE macros)
// ---------------------------------------------------------------------------

macro_rules! LOG {
    ($self_:expr, $($arg:expr),+) => {
        let parser = &mut *$self_;
        if parser.lexer.logger.log.is_some() || !parser.dot_graph_file.is_null() {
            snprintf(
                parser.lexer.debug_buffer.as_mut_ptr().cast::<i8>(),
                TREE_SITTER_SERIALIZATION_BUFFER_SIZE,
                $($arg),+
            );
            ts_parser__log(parser);
        }
    };
}

macro_rules! LOG_STACK {
    ($self_:expr) => {
        if !(*$self_).dot_graph_file.is_null() {
            ts_stack_print_dot_graph(
                parser_stack_mut((*$self_).stack),
                (*$self_).language,
                (*$self_).dot_graph_file,
            );
            fputs(c"\n\n".as_ptr().cast::<i8>(), (*$self_).dot_graph_file);
        }
    };
}

macro_rules! LOG_TREE {
    ($self_:expr, $tree:expr) => {
        if !(*$self_).dot_graph_file.is_null() {
            ts_subtree_print_dot_graph($tree, (*$self_).language, (*$self_).dot_graph_file);
            fputs(c"\n".as_ptr().cast::<i8>(), (*$self_).dot_graph_file);
        }
    };
}

macro_rules! LOG_LOOKAHEAD {
    ($self_:expr, $symbol_name:expr, $size:expr) => {
        if (*$self_).lexer.logger.log.is_some() || !(*$self_).dot_graph_file.is_null() {
            let buf = (*$self_).lexer.debug_buffer.as_mut_ptr().cast::<i8>();
            let symbol = $symbol_name;
            let mut off = snprintf(
                buf,
                TREE_SITTER_SERIALIZATION_BUFFER_SIZE,
                c"lexed_lookahead sym:".as_ptr().cast::<i8>(),
            ) as usize;
            let mut i = 0usize;
            while *symbol.add(i) != 0 && off < TREE_SITTER_SERIALIZATION_BUFFER_SIZE {
                match *symbol.add(i) as u8 {
                    b'\t' => {
                        *buf.add(off) = b'\\' as i8;
                        off += 1;
                        *buf.add(off) = b't' as i8;
                        off += 1;
                    }
                    b'\n' => {
                        *buf.add(off) = b'\\' as i8;
                        off += 1;
                        *buf.add(off) = b'n' as i8;
                        off += 1;
                    }
                    0x0b => {
                        *buf.add(off) = b'\\' as i8;
                        off += 1;
                        *buf.add(off) = b'v' as i8;
                        off += 1;
                    }
                    0x0c => {
                        *buf.add(off) = b'\\' as i8;
                        off += 1;
                        *buf.add(off) = b'f' as i8;
                        off += 1;
                    }
                    b'\r' => {
                        *buf.add(off) = b'\\' as i8;
                        off += 1;
                        *buf.add(off) = b'r' as i8;
                        off += 1;
                    }
                    b'\\' => {
                        *buf.add(off) = b'\\' as i8;
                        off += 1;
                        *buf.add(off) = b'\\' as i8;
                        off += 1;
                    }
                    _ => {
                        *buf.add(off) = *symbol.add(i);
                        off += 1;
                    }
                }
                i += 1;
            }
            snprintf(
                buf.add(off),
                TREE_SITTER_SERIALIZATION_BUFFER_SIZE - off,
                c", size:%u".as_ptr().cast::<i8>(),
                $size,
            );
            ts_parser__log(&mut *$self_);
        }
    };
}

macro_rules! SYM_NAME {
    ($self_:expr, $symbol:expr) => {
        ts_language_symbol_name((*$self_).language, $symbol)
    };
}

macro_rules! TREE_NAME {
    ($self_:expr, $tree:expr) => {
        SYM_NAME!($self_, ts_subtree_symbol($tree))
    };
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Candidate reduction used while searching recovery actions.
///
/// Recovery can scan many lookahead symbols for a parse state. This compact
/// record deduplicates equivalent reduce actions before applying them.
#[repr(C)]
#[derive(Clone, Copy)]
struct ReduceAction {
    /// Number of stack entries consumed by the reduce action.
    count: u32,
    /// Grammar symbol produced by the reduction.
    symbol: TSSymbol,
    /// Dynamic precedence delta for conflict resolution.
    dynamic_precedence: i32,
    /// Production id used for alias/field metadata on the new subtree.
    production_id: u16,
}

/// `ReduceActionSet` — Array(ReduceAction)
type ReduceActionSet = Array<ReduceAction>;

type PendingReductionArray = Array<*mut PendingReduction>;

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
    string: *const i8,
    length: u32,
}

/// Main parser runtime state.
///
/// One `TSParser` owns all mutable state for a parse: lexer callbacks, GLR
/// stack versions, old-tree reuse cursor, parser scratch arrays, external
/// scanner state, and the final accepted tree. The public C API treats this as
/// opaque; the `repr(C)` layout is preserved for parity with the C core.
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
    /// Optional wasm runtime for wasm languages.
    wasm_store: *mut TSWasmStore,
    /// Scratch set of reductions considered during recovery.
    reduce_actions: ReduceActionSet,
    /// Best accepted root found so far.
    finished_tree: Subtree,
    /// Reusable pop-result builder for normal reductions without an old tree.
    reduce_builder: StackPopBuilder,
    /// Parser-owned pending reduction descriptors awaiting cleanup.
    pending_reductions: PendingReductionArray,
    /// Scratch arrays for stripping and comparing trailing extras.
    trailing_extras: SubtreeArray,
    trailing_extras2: SubtreeArray,
    /// Scratch child array used for subtree comparisons.
    scratch_trees: SubtreeArray,
    /// Cached lexer result for repeated same-position lookups.
    token_cache: TokenCache,
    /// Arena that owns internal nodes in the returned tree.
    tree_arena: *mut TreeArena,
    /// Cursor over the old tree for incremental node reuse.
    reusable_node: ReusableNode,
    /// Language-owned external scanner payload.
    external_scanner_payload: *mut c_void,
    /// Optional parse debug graph output.
    dot_graph_file: *mut c_void,
    /// Number of accepted trees seen in this parse.
    accept_count: u32,
    /// Progress-callback operation counter.
    operation_count: u32,
    /// Retained old root while reparsing.
    old_tree: Subtree,
    /// Included-range diffs between old and new parse inputs.
    included_range_differences: TSRangeArray,
    /// Public parse cancellation/progress options.
    parse_options: TSParseOptions,
    /// Mutable status passed to the progress callback.
    parse_state: TSParseState,
    /// Cursor into `included_range_differences`.
    included_range_difference_index: u32,
    /// Set when an external scanner reports an error.
    has_scanner_error: bool,
    /// Set when balancing was canceled by the progress callback.
    canceled_balancing: bool,
    /// Set once any accepted tree contains an error.
    has_error: bool,
}

#[inline]
fn ts_parse_options_none() -> TSParseOptions {
    TSParseOptions {
        payload: ptr::null_mut(),
        progress_callback: None,
    }
}

#[inline]
const fn ts_parse_state_empty() -> TSParseState {
    TSParseState {
        payload: ptr::null_mut(),
        current_byte_offset: 0,
        has_error: false,
    }
}

#[inline]
unsafe fn parser_stack_mut<'a>(stack: *mut Stack) -> &'a mut Stack {
    stack.as_mut().unwrap_unchecked()
}

#[inline]
unsafe fn parser_stack_ref<'a>(stack: *const Stack) -> &'a Stack {
    stack.as_ref().unwrap_unchecked()
}

#[inline]
unsafe fn parser_language_full<'a>(language: *const TSLanguage) -> &'a TSLanguageFull {
    language
        .cast::<TSLanguageFull>()
        .as_ref()
        .unwrap_unchecked()
}

unsafe fn ts_parser__pending_reduction_delete(
    self_: &mut TSParser,
    pending: *mut PendingReduction,
) {
    let pending = pending.as_mut().unwrap_unchecked();
    if !pending.materialized.ptr.is_null() {
        ts_subtree_release(&mut self_.tree_pool, pending.materialized);
        pending.materialized = NULL_SUBTREE;
    } else {
        if !pending.children.contents.is_null() {
            ts_subtree_array_delete(&mut self_.tree_pool, &mut pending.children);
        }
        if !pending.payload_children.contents.is_null() {
            for i in 0..pending.payload_children.size {
                ts_stack_link_payload_release(
                    *array_get_ref(&pending.payload_children, i),
                    &mut self_.tree_pool,
                );
            }
            array_delete(&mut pending.payload_children);
        }
    }
    ts_free(ptr::from_mut(pending).cast::<c_void>());
}

unsafe fn ts_parser__clear_pending_reductions(self_: &mut TSParser) {
    for i in 0..self_.pending_reductions.size {
        ts_parser__pending_reduction_delete(self_, *array_get_ref(&self_.pending_reductions, i));
    }
    array_clear(&mut self_.pending_reductions);
}

unsafe fn ts_parser__pending_reduction_summarize_children(
    pending: &mut PendingReduction,
    language: *const TSLanguage,
) {
    pending.child_count = pending.children.size;
    pending.named_child_count = 0;
    pending.visible_child_count = 0;
    pending.error_cost = 0;
    pending.repeat_depth = 0;
    pending.visible_descendant_count = 0;
    pending.dynamic_precedence = 0;
    pending.node_count = 0;
    pending.padding = length_zero();
    pending.size = length_zero();
    pending.lookahead_bytes = 0;
    pending.flags &= PENDING_REDUCTION_VISIBLE | PENDING_REDUCTION_NAMED | PENDING_REDUCTION_EXTRA;

    let mut structural_index: u32 = 0;
    let alias_sequence = ts_language_alias_sequence(language, u32::from(pending.production_id));
    let mut lookahead_end_byte: u32 = 0;

    for i in 0..pending.children.size {
        let child = *pending.children.contents.add(i as usize);

        if pending.size.extent.row == 0 && ts_subtree_depends_on_column(child) {
            pending.flags |= PENDING_REDUCTION_DEPENDS_ON_COLUMN;
        }

        if ts_subtree_has_external_scanner_state_change(child) {
            pending.flags |= PENDING_REDUCTION_HAS_EXTERNAL_SCANNER_STATE_CHANGE;
        }

        if i == 0 {
            pending.padding = ts_subtree_padding(child);
            pending.size = ts_subtree_size(child);
        } else {
            pending.size = length_add(pending.size, ts_subtree_total_size(child));
        }

        let child_lookahead_end_byte =
            pending.padding.bytes + pending.size.bytes + ts_subtree_lookahead_bytes(child);
        if child_lookahead_end_byte > lookahead_end_byte {
            lookahead_end_byte = child_lookahead_end_byte;
        }

        if ts_subtree_symbol(child) != ts_builtin_sym_error_repeat {
            pending.error_cost += ts_subtree_error_cost(child);
        }

        let grandchild_count = ts_subtree_child_count(child);
        if (pending.symbol == ts_builtin_sym_error || pending.symbol == ts_builtin_sym_error_repeat)
            && !ts_subtree_extra(child)
            && !(ts_subtree_is_error(child) && grandchild_count == 0)
        {
            if ts_subtree_visible(child) {
                pending.error_cost += ERROR_COST_PER_SKIPPED_TREE;
            } else if grandchild_count > 0 {
                pending.error_cost +=
                    ERROR_COST_PER_SKIPPED_TREE * ts_subtree_visible_child_count(child);
            }
        }

        pending.dynamic_precedence += ts_subtree_dynamic_precedence(child);
        pending.visible_descendant_count += ts_subtree_visible_descendant_count(child);

        if !ts_subtree_extra(child)
            && ts_subtree_symbol(child) != 0
            && !alias_sequence.is_null()
            && *alias_sequence.add(structural_index as usize) != 0
        {
            pending.visible_descendant_count += 1;
            pending.visible_child_count += 1;
            if ts_language_symbol_metadata(language, *alias_sequence.add(structural_index as usize))
                .named
            {
                pending.named_child_count += 1;
            }
        } else if ts_subtree_visible(child) {
            pending.visible_descendant_count += 1;
            pending.visible_child_count += 1;
            if ts_subtree_named(child) {
                pending.named_child_count += 1;
            }
        } else if grandchild_count > 0 {
            pending.visible_child_count += ts_subtree_visible_child_count(child);
            pending.named_child_count += ts_subtree_named_child_count(child);
        }

        if ts_subtree_has_external_tokens(child) {
            pending.flags |= PENDING_REDUCTION_HAS_EXTERNAL_TOKENS;
        }

        if ts_subtree_is_error(child) {
            pending.flags |= PENDING_REDUCTION_FRAGILE_LEFT | PENDING_REDUCTION_FRAGILE_RIGHT;
            pending.parse_state = TS_TREE_STATE_NONE;
        }

        if !ts_subtree_extra(child) {
            structural_index += 1;
        }
    }

    pending.lookahead_bytes = lookahead_end_byte - pending.size.bytes - pending.padding.bytes;

    if pending.symbol == ts_builtin_sym_error || pending.symbol == ts_builtin_sym_error_repeat {
        pending.error_cost += ERROR_COST_PER_RECOVERY
            + ERROR_COST_PER_SKIPPED_CHAR * pending.size.bytes
            + ERROR_COST_PER_SKIPPED_LINE * pending.size.extent.row;
        pending.flags |= PENDING_REDUCTION_FRAGILE_LEFT | PENDING_REDUCTION_FRAGILE_RIGHT;
    }

    if pending.child_count > 0 {
        let first_child = *pending.children.contents;
        let last_child = *pending
            .children
            .contents
            .add(pending.child_count as usize - 1);

        pending.first_leaf_symbol = ts_subtree_leaf_symbol(first_child);
        pending.first_leaf_parse_state = ts_subtree_leaf_parse_state(first_child);

        if ts_subtree_fragile_left(first_child) {
            pending.flags |= PENDING_REDUCTION_FRAGILE_LEFT;
        }
        if ts_subtree_fragile_right(last_child) {
            pending.flags |= PENDING_REDUCTION_FRAGILE_RIGHT;
        }

        if pending.child_count >= 2
            && pending.flags & (PENDING_REDUCTION_VISIBLE | PENDING_REDUCTION_NAMED) == 0
            && ts_subtree_symbol(first_child) == pending.symbol
        {
            let first_depth = ts_subtree_repeat_depth(first_child);
            let last_depth = ts_subtree_repeat_depth(last_child);
            pending.repeat_depth = (first_depth.max(last_depth) + 1) as u16;
        }
    }

    pending.node_count = pending.visible_descendant_count;
    if pending.flags & PENDING_REDUCTION_VISIBLE != 0 {
        pending.node_count += 1;
    }
    if pending.symbol == ts_builtin_sym_error_repeat {
        pending.node_count += 1;
    }
}

#[inline]
unsafe fn ts_parser__payload_pending(payload: StackLinkPayload) -> *mut PendingReduction {
    ts_stack_link_payload_pending_reduction(payload)
}

#[inline]
unsafe fn ts_parser__payload_subtree(payload: StackLinkPayload) -> Subtree {
    ts_stack_link_payload_subtree(payload)
}

#[inline]
unsafe fn ts_parser__payload_symbol(payload: StackLinkPayload) -> TSSymbol {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).symbol
    } else {
        ts_subtree_symbol(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_extra(payload: StackLinkPayload) -> bool {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).flags & PENDING_REDUCTION_EXTRA != 0
    } else {
        ts_subtree_extra(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_child_count(payload: StackLinkPayload) -> u32 {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).child_count
    } else {
        ts_subtree_child_count(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_visible(payload: StackLinkPayload) -> bool {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).flags & PENDING_REDUCTION_VISIBLE != 0
    } else {
        ts_subtree_visible(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_named(payload: StackLinkPayload) -> bool {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).flags & PENDING_REDUCTION_NAMED != 0
    } else {
        ts_subtree_named(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_visible_child_count(payload: StackLinkPayload) -> u32 {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).visible_child_count
    } else {
        ts_subtree_visible_child_count(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_named_child_count(payload: StackLinkPayload) -> u32 {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).named_child_count
    } else {
        ts_subtree_named_child_count(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_visible_descendant_count(payload: StackLinkPayload) -> u32 {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).visible_descendant_count
    } else {
        ts_subtree_visible_descendant_count(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_has_external_tokens(payload: StackLinkPayload) -> bool {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).flags & PENDING_REDUCTION_HAS_EXTERNAL_TOKENS != 0
    } else {
        ts_subtree_has_external_tokens(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_has_external_scanner_state_change(payload: StackLinkPayload) -> bool {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).flags
            & PENDING_REDUCTION_HAS_EXTERNAL_SCANNER_STATE_CHANGE
            != 0
    } else {
        ts_subtree_has_external_scanner_state_change(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_depends_on_column(payload: StackLinkPayload) -> bool {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).flags & PENDING_REDUCTION_DEPENDS_ON_COLUMN != 0
    } else {
        ts_subtree_depends_on_column(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_padding(payload: StackLinkPayload) -> Length {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).padding
    } else {
        ts_subtree_padding(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_size(payload: StackLinkPayload) -> Length {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).size
    } else {
        ts_subtree_size(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_total_size(payload: StackLinkPayload) -> Length {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        let pending = ts_parser__payload_pending(payload)
            .as_ref()
            .unwrap_unchecked();
        length_add(pending.padding, pending.size)
    } else {
        ts_subtree_total_size(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_lookahead_bytes(payload: StackLinkPayload) -> u32 {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).lookahead_bytes
    } else {
        ts_subtree_lookahead_bytes(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_error_cost(payload: StackLinkPayload) -> u32 {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).error_cost
    } else {
        ts_subtree_error_cost(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_dynamic_precedence(payload: StackLinkPayload) -> i32 {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).dynamic_precedence
    } else {
        ts_subtree_dynamic_precedence(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_is_error(payload: StackLinkPayload) -> bool {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        let symbol = (*ts_parser__payload_pending(payload)).symbol;
        symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat
    } else {
        ts_subtree_is_error(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_fragile_left(payload: StackLinkPayload) -> bool {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).flags & PENDING_REDUCTION_FRAGILE_LEFT != 0
    } else {
        ts_subtree_fragile_left(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_fragile_right(payload: StackLinkPayload) -> bool {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).flags & PENDING_REDUCTION_FRAGILE_RIGHT != 0
    } else {
        ts_subtree_fragile_right(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_leaf_symbol(payload: StackLinkPayload) -> TSSymbol {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).first_leaf_symbol
    } else {
        ts_subtree_leaf_symbol(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_leaf_parse_state(payload: StackLinkPayload) -> TSStateId {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        (*ts_parser__payload_pending(payload)).first_leaf_parse_state
    } else {
        ts_subtree_leaf_parse_state(ts_parser__payload_subtree(payload))
    }
}

#[inline]
unsafe fn ts_parser__payload_repeat_depth(payload: StackLinkPayload) -> u32 {
    if ts_stack_link_payload_is_pending_reduction(payload) {
        u32::from((*ts_parser__payload_pending(payload)).repeat_depth)
    } else {
        ts_subtree_repeat_depth(ts_parser__payload_subtree(payload))
    }
}

unsafe fn ts_parser__pending_reduction_summarize_payload_children(
    pending: &mut PendingReduction,
    language: *const TSLanguage,
) {
    pending.child_count = pending.payload_children.size;
    pending.named_child_count = 0;
    pending.visible_child_count = 0;
    pending.error_cost = 0;
    pending.repeat_depth = 0;
    pending.visible_descendant_count = 0;
    pending.dynamic_precedence = 0;
    pending.node_count = 0;
    pending.padding = length_zero();
    pending.size = length_zero();
    pending.lookahead_bytes = 0;
    pending.flags &= PENDING_REDUCTION_VISIBLE | PENDING_REDUCTION_NAMED | PENDING_REDUCTION_EXTRA;

    let mut structural_index: u32 = 0;
    let alias_sequence = ts_language_alias_sequence(language, u32::from(pending.production_id));
    let mut lookahead_end_byte: u32 = 0;

    for i in 0..pending.payload_children.size {
        let child = *pending.payload_children.contents.add(i as usize);

        if pending.size.extent.row == 0 && ts_parser__payload_depends_on_column(child) {
            pending.flags |= PENDING_REDUCTION_DEPENDS_ON_COLUMN;
        }

        if ts_parser__payload_has_external_scanner_state_change(child) {
            pending.flags |= PENDING_REDUCTION_HAS_EXTERNAL_SCANNER_STATE_CHANGE;
        }

        if i == 0 {
            pending.padding = ts_parser__payload_padding(child);
            pending.size = ts_parser__payload_size(child);
        } else {
            pending.size = length_add(pending.size, ts_parser__payload_total_size(child));
        }

        let child_lookahead_end_byte =
            pending.padding.bytes + pending.size.bytes + ts_parser__payload_lookahead_bytes(child);
        if child_lookahead_end_byte > lookahead_end_byte {
            lookahead_end_byte = child_lookahead_end_byte;
        }

        if ts_parser__payload_symbol(child) != ts_builtin_sym_error_repeat {
            pending.error_cost += ts_parser__payload_error_cost(child);
        }

        let grandchild_count = ts_parser__payload_child_count(child);
        if (pending.symbol == ts_builtin_sym_error || pending.symbol == ts_builtin_sym_error_repeat)
            && !ts_parser__payload_extra(child)
            && !(ts_parser__payload_is_error(child) && grandchild_count == 0)
        {
            if ts_parser__payload_visible(child) {
                pending.error_cost += ERROR_COST_PER_SKIPPED_TREE;
            } else if grandchild_count > 0 {
                pending.error_cost +=
                    ERROR_COST_PER_SKIPPED_TREE * ts_parser__payload_visible_child_count(child);
            }
        }

        pending.dynamic_precedence += ts_parser__payload_dynamic_precedence(child);
        pending.visible_descendant_count += ts_parser__payload_visible_descendant_count(child);

        if !ts_parser__payload_extra(child)
            && ts_parser__payload_symbol(child) != 0
            && !alias_sequence.is_null()
            && *alias_sequence.add(structural_index as usize) != 0
        {
            pending.visible_descendant_count += 1;
            pending.visible_child_count += 1;
            if ts_language_symbol_metadata(language, *alias_sequence.add(structural_index as usize))
                .named
            {
                pending.named_child_count += 1;
            }
        } else if ts_parser__payload_visible(child) {
            pending.visible_descendant_count += 1;
            pending.visible_child_count += 1;
            if ts_parser__payload_named(child) {
                pending.named_child_count += 1;
            }
        } else if grandchild_count > 0 {
            pending.visible_child_count += ts_parser__payload_visible_child_count(child);
            pending.named_child_count += ts_parser__payload_named_child_count(child);
        }

        if ts_parser__payload_has_external_tokens(child) {
            pending.flags |= PENDING_REDUCTION_HAS_EXTERNAL_TOKENS;
        }

        if ts_parser__payload_is_error(child) {
            pending.flags |= PENDING_REDUCTION_FRAGILE_LEFT | PENDING_REDUCTION_FRAGILE_RIGHT;
            pending.parse_state = TS_TREE_STATE_NONE;
        }

        if !ts_parser__payload_extra(child) {
            structural_index += 1;
        }
    }

    pending.lookahead_bytes = lookahead_end_byte - pending.size.bytes - pending.padding.bytes;

    if pending.symbol == ts_builtin_sym_error || pending.symbol == ts_builtin_sym_error_repeat {
        pending.error_cost += ERROR_COST_PER_RECOVERY
            + ERROR_COST_PER_SKIPPED_CHAR * pending.size.bytes
            + ERROR_COST_PER_SKIPPED_LINE * pending.size.extent.row;
        pending.flags |= PENDING_REDUCTION_FRAGILE_LEFT | PENDING_REDUCTION_FRAGILE_RIGHT;
    }

    if pending.child_count > 0 {
        let first_child = *pending.payload_children.contents;
        let last_child = *pending
            .payload_children
            .contents
            .add(pending.child_count as usize - 1);

        pending.first_leaf_symbol = ts_parser__payload_leaf_symbol(first_child);
        pending.first_leaf_parse_state = ts_parser__payload_leaf_parse_state(first_child);

        if ts_parser__payload_fragile_left(first_child) {
            pending.flags |= PENDING_REDUCTION_FRAGILE_LEFT;
        }
        if ts_parser__payload_fragile_right(last_child) {
            pending.flags |= PENDING_REDUCTION_FRAGILE_RIGHT;
        }

        if pending.child_count >= 2
            && pending.flags & (PENDING_REDUCTION_VISIBLE | PENDING_REDUCTION_NAMED) == 0
            && ts_parser__payload_symbol(first_child) == pending.symbol
        {
            let first_depth = ts_parser__payload_repeat_depth(first_child);
            let last_depth = ts_parser__payload_repeat_depth(last_child);
            pending.repeat_depth = (first_depth.max(last_depth) + 1) as u16;
        }
    }

    pending.node_count = pending.visible_descendant_count;
    if pending.flags & PENDING_REDUCTION_VISIBLE != 0 {
        pending.node_count += 1;
    }
    if pending.symbol == ts_builtin_sym_error_repeat {
        pending.node_count += 1;
    }
}

unsafe fn ts_parser__pending_reduction_new_from_children(
    self_: &mut TSParser,
    symbol: TSSymbol,
    children: &SubtreeArray,
    production_id: u16,
    parse_state: TSStateId,
    extra: bool,
    dynamic_precedence: i32,
) -> *mut PendingReduction {
    let metadata = ts_language_symbol_metadata(self_.language, symbol);
    let fragile = symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat;
    let pending = ts_calloc(1, std::mem::size_of::<PendingReduction>()).cast::<PendingReduction>();
    let pending_ref = pending.as_mut().unwrap_unchecked();
    pending_ref.ref_count = 1;
    pending_ref.symbol = symbol;
    pending_ref.production_id = production_id;
    pending_ref.parse_state = if fragile {
        TS_TREE_STATE_NONE
    } else {
        parse_state
    };
    pending_ref.materialized = NULL_SUBTREE;
    pending_ref.flags = 0;
    if metadata.visible {
        pending_ref.flags |= PENDING_REDUCTION_VISIBLE;
    }
    if metadata.named {
        pending_ref.flags |= PENDING_REDUCTION_NAMED;
    }
    if extra {
        pending_ref.flags |= PENDING_REDUCTION_EXTRA;
    }
    if fragile {
        pending_ref.flags |= PENDING_REDUCTION_FRAGILE_LEFT | PENDING_REDUCTION_FRAGILE_RIGHT;
    }

    array_init(subtree_array_as_array_mut(&mut pending_ref.children));
    array_init(&mut pending_ref.payload_children);
    array_reserve(
        subtree_array_as_array_mut(&mut pending_ref.children),
        children.size,
    );
    for i in 0..children.size {
        let child = *children.contents.add(i as usize);
        ts_subtree_retain(child);
        array_push(subtree_array_as_array_mut(&mut pending_ref.children), child);
    }

    ts_parser__pending_reduction_summarize_children(pending_ref, self_.language);
    pending_ref.dynamic_precedence += dynamic_precedence;
    array_push(&mut self_.pending_reductions, pending);
    pending
}

unsafe fn ts_parser__pending_reduction_new_from_payloads(
    self_: &mut TSParser,
    symbol: TSSymbol,
    children: &Array<StackLinkPayload>,
    production_id: u16,
    parse_state: TSStateId,
    extra: bool,
    dynamic_precedence: i32,
) -> *mut PendingReduction {
    let metadata = ts_language_symbol_metadata(self_.language, symbol);
    let fragile = symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat;
    let pending = ts_calloc(1, std::mem::size_of::<PendingReduction>()).cast::<PendingReduction>();
    let pending_ref = pending.as_mut().unwrap_unchecked();
    pending_ref.ref_count = 1;
    pending_ref.symbol = symbol;
    pending_ref.production_id = production_id;
    pending_ref.parse_state = if fragile {
        TS_TREE_STATE_NONE
    } else {
        parse_state
    };
    pending_ref.materialized = NULL_SUBTREE;
    pending_ref.flags = 0;
    if metadata.visible {
        pending_ref.flags |= PENDING_REDUCTION_VISIBLE;
    }
    if metadata.named {
        pending_ref.flags |= PENDING_REDUCTION_NAMED;
    }
    if extra {
        pending_ref.flags |= PENDING_REDUCTION_EXTRA;
    }
    if fragile {
        pending_ref.flags |= PENDING_REDUCTION_FRAGILE_LEFT | PENDING_REDUCTION_FRAGILE_RIGHT;
    }

    array_init(subtree_array_as_array_mut(&mut pending_ref.children));
    array_init(&mut pending_ref.payload_children);
    array_reserve(&mut pending_ref.payload_children, children.size);
    for i in 0..children.size {
        let child = *children.contents.add(i as usize);
        ts_stack_link_payload_retain(child);
        array_push(&mut pending_ref.payload_children, child);
    }

    ts_parser__pending_reduction_summarize_payload_children(pending_ref, self_.language);
    pending_ref.dynamic_precedence += dynamic_precedence;
    array_push(&mut self_.pending_reductions, pending);
    pending
}

unsafe fn ts_parser__materialize_pending_reduction(
    self_: &mut TSParser,
    pending: *mut PendingReduction,
) -> Subtree {
    let pending_ref = pending.as_mut().unwrap_unchecked();
    if !pending_ref.materialized.ptr.is_null() {
        ts_subtree_retain(pending_ref.materialized);
        return pending_ref.materialized;
    }

    let mut children = SubtreeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    };

    if !pending_ref.payload_children.contents.is_null() {
        array_reserve(
            subtree_array_as_array_mut(&mut children),
            pending_ref.payload_children.size,
        );
        for i in 0..pending_ref.payload_children.size {
            let payload = *array_get_ref(&pending_ref.payload_children, i);
            let child = if ts_stack_link_payload_is_pending_reduction(payload) {
                ts_parser__materialize_pending_reduction(
                    self_,
                    ts_stack_link_payload_pending_reduction(payload),
                )
            } else {
                let child = ts_stack_link_payload_subtree(payload);
                ts_subtree_retain(child);
                child
            };
            array_push(subtree_array_as_array_mut(&mut children), child);
        }

        for i in 0..pending_ref.payload_children.size {
            ts_stack_link_payload_release(
                *array_get_ref(&pending_ref.payload_children, i),
                &mut self_.tree_pool,
            );
        }
        array_delete(&mut pending_ref.payload_children);
    } else {
        children = ptr::read(&pending_ref.children);
        pending_ref.children = SubtreeArray {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
        };
    }

    let result = ts_parser__new_node(
        self_,
        pending_ref.symbol,
        &mut children,
        u32::from(pending_ref.production_id),
    );

    (*result.ptr).padding = pending_ref.padding;
    (*result.ptr).size = pending_ref.size;
    (*result.ptr).lookahead_bytes = pending_ref.lookahead_bytes;
    (*result.ptr).error_cost = pending_ref.error_cost;
    (*result.ptr).parse_state = pending_ref.parse_state;
    (*result.ptr).set_extra(pending_ref.flags & PENDING_REDUCTION_EXTRA != 0);
    (*result.ptr).set_fragile_left(pending_ref.flags & PENDING_REDUCTION_FRAGILE_LEFT != 0);
    (*result.ptr).set_fragile_right(pending_ref.flags & PENDING_REDUCTION_FRAGILE_RIGHT != 0);
    (*result.ptr)
        .set_has_external_tokens(pending_ref.flags & PENDING_REDUCTION_HAS_EXTERNAL_TOKENS != 0);
    (*result.ptr).set_has_external_scanner_state_change(
        pending_ref.flags & PENDING_REDUCTION_HAS_EXTERNAL_SCANNER_STATE_CHANGE != 0,
    );
    (*result.ptr)
        .set_depends_on_column(pending_ref.flags & PENDING_REDUCTION_DEPENDS_ON_COLUMN != 0);
    (*result.ptr).data.children.visible_child_count = pending_ref.visible_child_count;
    (*result.ptr).data.children.named_child_count = pending_ref.named_child_count;
    (*result.ptr).data.children.visible_descendant_count = pending_ref.visible_descendant_count;
    (*result.ptr).data.children.dynamic_precedence = pending_ref.dynamic_precedence;
    (*result.ptr).data.children.repeat_depth = pending_ref.repeat_depth;
    (*result.ptr).data.children.first_leaf.symbol = pending_ref.first_leaf_symbol;
    (*result.ptr).data.children.first_leaf.parse_state = pending_ref.first_leaf_parse_state;

    pending_ref.materialized = ts_subtree_from_mut(result);
    ts_subtree_retain(pending_ref.materialized);
    pending_ref.materialized
}

// ---------------------------------------------------------------------------
// ReduceActionSet helper
// ---------------------------------------------------------------------------

#[inline]
unsafe fn parser_subtree_child<'a>(parent: Subtree, index: u32) -> &'a Subtree {
    debug_assert!(index < ts_subtree_child_count(parent));
    ts_subtree_children(parent)
        .add(index as usize)
        .as_ref()
        .unwrap_unchecked()
}

#[inline]
const unsafe fn parser_subtree_children<'a>(parent: Subtree) -> &'a [Subtree] {
    std::slice::from_raw_parts(
        ts_subtree_children(parent),
        ts_subtree_child_count(parent) as usize,
    )
}

unsafe fn subtree_array_as_array(self_: &SubtreeArray) -> &Array<Subtree> {
    ptr::from_ref(self_)
        .cast::<Array<Subtree>>()
        .as_ref()
        .unwrap_unchecked()
}

unsafe fn subtree_array_as_array_mut(self_: &mut SubtreeArray) -> &mut Array<Subtree> {
    ptr::from_mut(self_)
        .cast::<Array<Subtree>>()
        .as_mut()
        .unwrap_unchecked()
}

unsafe fn mutable_subtree_array_as_array(self_: &MutableSubtreeArray) -> &Array<MutableSubtree> {
    ptr::from_ref(self_)
        .cast::<Array<MutableSubtree>>()
        .as_ref()
        .unwrap_unchecked()
}

unsafe fn mutable_subtree_array_as_array_mut(
    self_: &mut MutableSubtreeArray,
) -> &mut Array<MutableSubtree> {
    ptr::from_mut(self_)
        .cast::<Array<MutableSubtree>>()
        .as_mut()
        .unwrap_unchecked()
}

unsafe fn ts_range_array_as_array(self_: &TSRangeArray) -> &Array<TSRange> {
    ptr::from_ref(self_)
        .cast::<Array<TSRange>>()
        .as_ref()
        .unwrap_unchecked()
}

unsafe fn ts_range_array_as_array_mut(self_: &mut TSRangeArray) -> &mut Array<TSRange> {
    ptr::from_mut(self_)
        .cast::<Array<TSRange>>()
        .as_mut()
        .unwrap_unchecked()
}

unsafe fn ts_reduce_action_set_add(self_: &mut ReduceActionSet, new_action: ReduceAction) {
    for i in 0..self_.size {
        let action = array_get_ref(self_, i);
        if action.symbol == new_action.symbol && action.count == new_action.count {
            return;
        }
    }
    array_push(self_, new_action);
}

// ---------------------------------------------------------------------------
// Internal helpers — StringInput
// ---------------------------------------------------------------------------

unsafe extern "C" fn ts_string_input_read(
    payload: *mut c_void,
    byte: u32,
    _point: TSPoint,
    length: *mut u32,
) -> *const i8 {
    let input = payload.cast::<TSStringInput>().as_ref().unwrap_unchecked();
    if byte >= input.length {
        *length = 0;
        c"".as_ptr().cast::<i8>()
    } else {
        *length = input.length - byte;
        input.string.add(byte as usize)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — logging & breakdown
// ---------------------------------------------------------------------------

unsafe fn ts_parser__log(self_: &mut TSParser) {
    if let Some(log_fn) = self_.lexer.logger.log {
        log_fn(
            self_.lexer.logger.payload,
            TSLogTypeParse,
            self_.lexer.debug_buffer.as_ptr().cast::<i8>(),
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

unsafe fn ts_parser__breakdown_top_of_stack(self_: &mut TSParser, version: StackVersion) -> bool {
    let mut did_break_down = false;

    loop {
        let pop = ts_stack_pop_pending(parser_stack_mut(self_.stack), version);
        if pop.size == 0 {
            break;
        }

        did_break_down = true;
        let mut pending = false;
        for i in 0..pop.size {
            let mut slice = ptr::read(array_get_ref(&pop, i));
            let mut state = ts_stack_state(parser_stack_ref(self_.stack), slice.version);
            let parent = *array_get_ref(subtree_array_as_array(&slice.subtrees), 0);

            let n = ts_subtree_child_count(parent);
            for j in 0..n {
                let child = *parser_subtree_child(parent, j);
                pending = ts_subtree_child_count(child) > 0;

                if ts_subtree_is_error(child) {
                    state = ERROR_STATE;
                } else if !ts_subtree_extra(child) {
                    state = ts_language_next_state(self_.language, state, ts_subtree_symbol(child));
                }

                ts_subtree_retain(child);
                ts_stack_push(
                    parser_stack_mut(self_.stack),
                    slice.version,
                    child,
                    pending,
                    state,
                );
            }

            for j in 1..slice.subtrees.size {
                let tree = *array_get_ref(subtree_array_as_array(&slice.subtrees), j);
                ts_stack_push(
                    parser_stack_mut(self_.stack),
                    slice.version,
                    tree,
                    false,
                    state,
                );
            }

            ts_subtree_release(&mut self_.tree_pool, parent);
            array_delete(subtree_array_as_array_mut(&mut slice.subtrees));

            let parser = ptr::from_mut(self_);
            LOG!(
                parser,
                c"breakdown_top_of_stack tree:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(parent))
            );
            LOG_STACK!(parser);
        }

        if !pending {
            break;
        }
    }

    did_break_down
}

unsafe fn ts_parser__breakdown_lookahead(
    self_: &mut TSParser,
    lookahead: &mut Subtree,
    state: TSStateId,
) {
    let parser = ptr::from_mut(self_);
    let reusable_node = &mut self_.reusable_node;
    let mut did_descend = false;
    let mut tree = reusable_node_tree(reusable_node);
    while ts_subtree_child_count(tree) > 0 && ts_subtree_parse_state(tree) != state {
        LOG!(
            parser,
            c"state_mismatch sym:%s".as_ptr().cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(tree))
        );
        reusable_node_descend(reusable_node);
        tree = reusable_node_tree(reusable_node);
        did_descend = true;
    }

    if did_descend {
        ts_subtree_release(&mut self_.tree_pool, *lookahead);
        *lookahead = tree;
        ts_subtree_retain(*lookahead);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — version comparison
// ---------------------------------------------------------------------------

const unsafe fn ts_parser__compare_versions(a: ErrorStatus, b: ErrorStatus) -> ErrorComparison {
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

unsafe fn ts_parser__version_status(self_: &mut TSParser, version: StackVersion) -> ErrorStatus {
    let stack = parser_stack_mut(self_.stack);
    let mut cost = ts_stack_error_cost(stack, version);
    let is_paused = ts_stack_is_paused(stack, version);
    if is_paused {
        cost += ERROR_COST_PER_SKIPPED_TREE;
    }
    ErrorStatus {
        cost,
        node_count: ts_stack_node_count_since_error(stack, version),
        dynamic_precedence: ts_stack_dynamic_precedence(stack, version),
        is_in_error: is_paused || ts_stack_state(stack, version) == ERROR_STATE,
    }
}

unsafe fn ts_parser__better_version_exists(
    self_: &mut TSParser,
    version: StackVersion,
    is_in_error: bool,
    cost: u32,
) -> bool {
    if !self_.finished_tree.ptr.is_null() && ts_subtree_error_cost(self_.finished_tree) <= cost {
        return true;
    }

    let stack = parser_stack_mut(self_.stack);
    let position = ts_stack_position(stack, version);
    let status = ErrorStatus {
        cost,
        is_in_error,
        dynamic_precedence: ts_stack_dynamic_precedence(stack, version),
        node_count: ts_stack_node_count_since_error(stack, version),
    };

    let n = ts_stack_version_count(stack);
    for i in 0..n {
        if i == version
            || !ts_stack_is_active(stack, i)
            || ts_stack_position(stack, i).bytes < position.bytes
        {
            continue;
        }
        let status_i = ts_parser__version_status(self_, i);
        match ts_parser__compare_versions(status, status_i) {
            ErrorComparison::TakeRight => return true,
            ErrorComparison::PreferRight
                if ts_stack_can_merge(parser_stack_ref(self_.stack), i, version) =>
            {
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

unsafe fn ts_parser__call_main_lex_fn(self_: &mut TSParser, lex_mode: TSLexerMode) -> bool {
    if ts_language_is_wasm(self_.language) {
        ts_wasm_store_call_lex_main(self_.wasm_store, lex_mode.lex_state)
    } else {
        (parser_language_full(self_.language).lex_fn.unwrap())(
            &mut self_.lexer.data,
            lex_mode.lex_state,
        )
    }
}

unsafe fn ts_parser__call_keyword_lex_fn(self_: &mut TSParser) -> bool {
    if ts_language_is_wasm(self_.language) {
        ts_wasm_store_call_lex_keyword(self_.wasm_store, 0)
    } else {
        (parser_language_full(self_.language).keyword_lex_fn.unwrap())(&mut self_.lexer.data, 0)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — external scanner
// ---------------------------------------------------------------------------

unsafe fn ts_parser__external_scanner_create(self_: &mut TSParser) {
    if !self_.language.is_null() {
        let lang = parser_language_full(self_.language);
        if lang.external_scanner.states.is_null() {
            return;
        }

        if ts_language_is_wasm(self_.language) {
            self_.external_scanner_payload =
                ts_wasm_store_call_scanner_create(self_.wasm_store) as usize as *mut c_void;
            if ts_wasm_store_has_error(self_.wasm_store) {
                self_.has_scanner_error = true;
            }
        } else if let Some(create_fn) = lang.external_scanner.create {
            self_.external_scanner_payload = create_fn();
        }
    }
}

unsafe fn ts_parser__external_scanner_destroy(self_: &mut TSParser) {
    if !self_.language.is_null()
        && !self_.external_scanner_payload.is_null()
        && !ts_language_is_wasm(self_.language)
    {
        let lang = parser_language_full(self_.language);
        if let Some(destroy_fn) = lang.external_scanner.destroy {
            destroy_fn(self_.external_scanner_payload);
        }
    }
    self_.external_scanner_payload = ptr::null_mut();
}

unsafe fn ts_parser__external_scanner_serialize(self_: &mut TSParser) -> u32 {
    let length;
    if ts_language_is_wasm(self_.language) {
        length = ts_wasm_store_call_scanner_serialize(
            self_.wasm_store,
            self_.external_scanner_payload as usize as u32,
            self_.lexer.debug_buffer.as_mut_ptr().cast::<i8>(),
        );
        if ts_wasm_store_has_error(self_.wasm_store) {
            self_.has_scanner_error = true;
        }
    } else {
        length = (parser_language_full(self_.language)
            .external_scanner
            .serialize
            .unwrap())(
            self_.external_scanner_payload,
            self_.lexer.debug_buffer.as_mut_ptr().cast::<i8>(),
        );
    }
    debug_assert!(length as usize <= TREE_SITTER_SERIALIZATION_BUFFER_SIZE);
    length
}

unsafe fn ts_parser__external_scanner_deserialize(self_: &mut TSParser, external_token: Subtree) {
    let (data, length) = if !external_token.ptr.is_null() {
        let state = ts_subtree_external_scanner_state(&external_token);
        (ts_external_scanner_state_data(state), state.length)
    } else {
        (ptr::null(), 0)
    };

    if ts_language_is_wasm(self_.language) {
        ts_wasm_store_call_scanner_deserialize(
            self_.wasm_store,
            self_.external_scanner_payload as usize as u32,
            data.cast::<i8>(),
            length,
        );
        if ts_wasm_store_has_error(self_.wasm_store) {
            self_.has_scanner_error = true;
        }
    } else {
        (parser_language_full(self_.language)
            .external_scanner
            .deserialize
            .unwrap())(self_.external_scanner_payload, data.cast::<i8>(), length);
    }
}

unsafe fn ts_parser__external_scanner_scan(
    self_: &mut TSParser,
    external_lex_state: TSStateId,
) -> bool {
    let lang = parser_language_full(self_.language);
    if ts_language_is_wasm(self_.language) {
        let result = ts_wasm_store_call_scanner_scan(
            self_.wasm_store,
            self_.external_scanner_payload as usize as u32,
            u32::from(external_lex_state) * lang.external_token_count,
        );
        if ts_wasm_store_has_error(self_.wasm_store) {
            self_.has_scanner_error = true;
        }
        result
    } else {
        let valid_external_tokens =
            ts_language_enabled_external_tokens(self_.language, u32::from(external_lex_state));
        (lang.external_scanner.scan.unwrap())(
            self_.external_scanner_payload,
            &mut self_.lexer.data,
            valid_external_tokens,
        )
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — token reuse & lexing
// ---------------------------------------------------------------------------

unsafe fn ts_parser__can_reuse_first_leaf(
    self_: &TSParser,
    state: TSStateId,
    tree: Subtree,
    table_entry: &TableEntry,
) -> bool {
    let leaf_symbol = ts_subtree_leaf_symbol(tree);
    let current_lex_mode = ts_language_lex_mode_for_state(self_.language, state);

    // At the end of a non-terminal extra node, the lexer normally returns
    // NULL, which indicates that the parser should look for a reduce action
    // at symbol `0`. Avoid reusing tokens in this situation.
    if current_lex_mode.lex_state == u16::MAX {
        return false;
    }

    // If the token was created in a state with the same set of lookaheads, it is reusable.
    if table_entry.action_count > 0 {
        let leaf_state = ts_subtree_leaf_parse_state(tree);
        let leaf_lex_mode = ts_language_lex_mode_for_state(self_.language, leaf_state);
        if leaf_lex_mode.lex_state == current_lex_mode.lex_state
            && leaf_lex_mode.external_lex_state == current_lex_mode.external_lex_state
            && leaf_lex_mode.reserved_word_set_id == current_lex_mode.reserved_word_set_id
        {
            let lang = parser_language_full(self_.language);
            if leaf_symbol != lang.keyword_capture_token
                || (!ts_subtree_is_keyword(tree) && ts_subtree_parse_state(tree) == state)
            {
                return true;
            }
        }
    }

    // Empty tokens are not reusable in states with different lookaheads.
    if ts_subtree_size(tree).bytes == 0 && leaf_symbol != ts_builtin_sym_end {
        return false;
    }

    // If the current state allows external tokens or other tokens that conflict with this
    // token, this token is not reusable.
    current_lex_mode.external_lex_state == 0 && table_entry.is_reusable
}

/// Build the error token produced after skipping unrecognized input.
unsafe fn ts_parser__new_error_lookahead(
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
    ts_subtree_new_error(
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
unsafe fn ts_parser__resolve_lexed_symbol(
    self_: &mut TSParser,
    parse_state: TSStateId,
    found_external_token: bool,
) -> (TSSymbol, bool) {
    let lang = parser_language_full(self_.language);
    let mut symbol = self_.lexer.data.result_symbol;
    let mut is_keyword = false;

    if found_external_token {
        symbol = *lang.external_scanner.symbol_map.add(symbol as usize);
    } else if symbol == lang.keyword_capture_token && symbol != 0 {
        let end_byte = self_.lexer.token_end_position.bytes;
        let token_start_position = self_.lexer.token_start_position;
        ts_lexer_reset(&mut self_.lexer, token_start_position);
        ts_lexer_start(&mut self_.lexer);

        is_keyword = ts_parser__call_keyword_lex_fn(self_);

        if is_keyword
            && self_.lexer.token_end_position.bytes == end_byte
            && (ts_language_has_actions(
                self_.language,
                parse_state,
                self_.lexer.data.result_symbol,
            ) || ts_language_is_reserved_word(
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
unsafe fn ts_parser__new_leaf_lookahead(
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
        ts_parser__resolve_lexed_symbol(self_, parse_state, found_external_token);

    let result = ts_subtree_new_leaf(
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
        let mut_result = ts_subtree_to_mut_unsafe(result);
        let external_scanner_state =
            ptr::addr_of_mut!((*mut_result.ptr).data.external_scanner_state)
                .cast::<ExternalScannerState>()
                .as_mut()
                .unwrap_unchecked();
        ts_external_scanner_state_init(
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
unsafe fn ts_parser__lex(
    self_: &mut TSParser,
    version: StackVersion,
    parse_state: TSStateId,
) -> Subtree {
    let parser = ptr::from_mut(self_);
    let lang = parser_language_full(self_.language);
    let mut lex_mode = ts_language_lex_mode_for_state(self_.language, parse_state);
    if lex_mode.lex_state == u16::MAX {
        LOG!(
            parser,
            c"no_lookahead_after_non_terminal_extra"
                .as_ptr()
                .cast::<i8>()
        );
        return NULL_SUBTREE;
    }

    let stack = parser_stack_ref(self_.stack);
    let start_position = ts_stack_position(stack, version);
    let external_token = ts_stack_last_external_token(stack, version);

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
    ts_lexer_reset(&mut self_.lexer, start_position);

    loop {
        let mut found_token;
        let current_position = self_.lexer.current_position;
        let column_data = self_.lexer.column_data;

        if lex_mode.external_lex_state != 0 {
            LOG!(
                parser,
                c"lex_external state:%d, row:%u, column:%u"
                    .as_ptr()
                    .cast::<i8>(),
                i32::from(lex_mode.external_lex_state),
                current_position.extent.row,
                current_position.extent.column
            );
            ts_lexer_start(&mut self_.lexer);
            ts_parser__external_scanner_deserialize(self_, external_token);
            found_token = ts_parser__external_scanner_scan(self_, lex_mode.external_lex_state);
            if self_.has_scanner_error {
                return NULL_SUBTREE;
            }
            ts_lexer_finish(&mut self_.lexer, &mut lookahead_end_byte);

            if found_token {
                external_scanner_state_len = ts_parser__external_scanner_serialize(self_);
                let external_scanner_state = ts_subtree_external_scanner_state(&external_token);
                external_scanner_state_changed = !ts_external_scanner_state_eq(
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
                        || !ts_stack_has_advanced_since_error(
                            parser_stack_ref(self_.stack),
                            version,
                        )
                        || token_is_extra
                    {
                        LOG!(
                            parser,
                            c"ignore_empty_external_token symbol:%s"
                                .as_ptr()
                                .cast::<i8>(),
                            SYM_NAME!(
                                parser,
                                *lang
                                    .external_scanner
                                    .symbol_map
                                    .add(self_.lexer.data.result_symbol as usize)
                            )
                        );
                        found_token = false;
                    }
                }
            }

            if found_token {
                found_external_token = true;
                called_get_column = self_.lexer.did_get_column;
                break;
            }

            ts_lexer_reset(&mut self_.lexer, current_position);
            self_.lexer.column_data = column_data;
        }

        LOG!(
            parser,
            c"lex_internal state:%d, row:%u, column:%u"
                .as_ptr()
                .cast::<i8>(),
            i32::from(lex_mode.lex_state),
            current_position.extent.row,
            current_position.extent.column
        );
        ts_lexer_start(&mut self_.lexer);
        found_token = ts_parser__call_main_lex_fn(self_, lex_mode);
        ts_lexer_finish(&mut self_.lexer, &mut lookahead_end_byte);
        if found_token {
            break;
        }

        if !error_mode {
            error_mode = true;
            lex_mode = ts_language_lex_mode_for_state(self_.language, ERROR_STATE);
            ts_lexer_reset(&mut self_.lexer, start_position);
            continue;
        }

        if !skipped_error {
            LOG!(parser, c"skip_unrecognized_character".as_ptr().cast::<i8>());
            skipped_error = true;
            error_start_position = self_.lexer.token_start_position;
            error_end_position = self_.lexer.token_start_position;
            first_error_character = self_.lexer.data.lookahead;
        }

        if self_.lexer.current_position.bytes == error_end_position.bytes {
            if (self_.lexer.data.eof.unwrap())(std::ptr::addr_of!(self_.lexer.data)) {
                self_.lexer.data.result_symbol = ts_builtin_sym_error;
                break;
            }
            (self_.lexer.data.advance.unwrap())(&mut self_.lexer.data, false);
        }

        error_end_position = self_.lexer.current_position;
    }

    let result = if skipped_error {
        ts_parser__new_error_lookahead(
            self_,
            parse_state,
            start_position,
            error_start_position,
            error_end_position,
            lookahead_end_byte,
            first_error_character,
        )
    } else {
        ts_parser__new_leaf_lookahead(
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

    LOG_LOOKAHEAD!(
        parser,
        SYM_NAME!(parser, ts_subtree_symbol(result)),
        ts_subtree_total_size(result).bytes
    );
    result
}

unsafe fn ts_parser__get_cached_token(
    self_: &TSParser,
    state: TSStateId,
    position: usize,
    last_external_token: Subtree,
) -> Option<(Subtree, TableEntry)> {
    let cache = &self_.token_cache;
    if !cache.token.ptr.is_null()
        && cache.byte_index == position as u32
        && ts_subtree_external_scanner_state_eq(&cache.last_external_token, &last_external_token)
    {
        let mut table_entry = TableEntry::empty();
        ts_language_table_entry(
            self_.language,
            state,
            ts_subtree_symbol(cache.token),
            &mut table_entry,
        );
        if ts_parser__can_reuse_first_leaf(self_, state, cache.token, &table_entry) {
            ts_subtree_retain(cache.token);
            return Some((cache.token, table_entry));
        }
    }
    None
}

unsafe fn ts_parser__set_cached_token(
    self_: &mut TSParser,
    byte_index: u32,
    last_external_token: Subtree,
    token: Subtree,
) {
    let cache = &mut self_.token_cache;
    if !token.ptr.is_null() {
        ts_subtree_retain(token);
    }
    if !last_external_token.ptr.is_null() {
        ts_subtree_retain(last_external_token);
    }
    if !cache.token.ptr.is_null() {
        ts_subtree_release(&mut self_.tree_pool, cache.token);
    }
    if !cache.last_external_token.ptr.is_null() {
        ts_subtree_release(&mut self_.tree_pool, cache.last_external_token);
    }
    cache.token = token;
    cache.byte_index = byte_index;
    cache.last_external_token = last_external_token;
}

/// Find the initial lookahead for one stack version.
///
/// The parser tries sources in cheapest-to-most-expensive order:
///
/// 1. Reuse a compatible subtree from the old tree during incremental parsing.
/// 2. Reuse the parser's one-token cache for another version at this position.
/// 3. Ask the lexer to scan a fresh token.
///
/// The returned `needs_lex` flag tells `ts_parser__advance` whether step 3 is
/// still required. `did_reuse` records only old-tree reuse, because successful
/// shifts must advance the reusable-node cursor.
unsafe fn ts_parser__get_initial_lookahead(
    self_: &mut TSParser,
    version: StackVersion,
    state: &mut TSStateId,
    position: u32,
    last_external_token: Subtree,
    allow_node_reuse: bool,
) -> (bool, Subtree, TableEntry, bool) {
    let mut did_reuse = true;
    let mut lookahead = NULL_SUBTREE;
    let mut table_entry = TableEntry::empty();

    if allow_node_reuse {
        lookahead = ts_parser__reuse_node(
            self_,
            version,
            state,
            position,
            last_external_token,
            &mut table_entry,
        );
    }

    if lookahead.ptr.is_null() {
        did_reuse = false;
        if let Some((token, cached_table_entry)) =
            ts_parser__get_cached_token(self_, *state, position as usize, last_external_token)
        {
            lookahead = token;
            table_entry = cached_table_entry;
        }
    }

    let needs_lex = lookahead.ptr.is_null();
    (did_reuse, lookahead, table_entry, needs_lex)
}

/// Lex a token for the current stack version and prepare its parse-table entry.
///
/// A null lookahead is meaningful when parsing a non-terminal extra: it asks the
/// parser to consult the EOF entry for a forced reduction, after which lexing
/// resumes from the new parse state.
unsafe fn ts_parser__lex_lookahead(
    self_: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    position: u32,
    last_external_token: Subtree,
    lookahead: &mut Subtree,
    table_entry: &mut TableEntry,
) -> bool {
    *lookahead = ts_parser__lex(self_, version, state);
    if self_.has_scanner_error {
        return false;
    }

    if !lookahead.ptr.is_null() {
        ts_parser__set_cached_token(self_, position, last_external_token, *lookahead);
        ts_language_table_entry(
            self_.language,
            state,
            ts_subtree_symbol(*lookahead),
            table_entry,
        );
    } else {
        ts_language_table_entry(self_.language, state, ts_builtin_sym_end, table_entry);
    }

    true
}

unsafe fn ts_parser__has_included_range_difference(
    self_: &TSParser,
    start_position: u32,
    end_position: u32,
) -> bool {
    ts_range_array_intersects_ref(
        &self_.included_range_differences,
        self_.included_range_difference_index,
        start_position,
        end_position,
    )
}

unsafe fn ts_parser__reuse_node(
    self_: &mut TSParser,
    version: StackVersion,
    state: &mut TSStateId,
    position: u32,
    last_external_token: Subtree,
    table_entry: &mut TableEntry,
) -> Subtree {
    let parser = ptr::from_mut(self_);
    let mut result;
    loop {
        result = reusable_node_tree(&self_.reusable_node);
        if result.ptr.is_null() {
            break;
        }
        let byte_offset = reusable_node_byte_offset(&self_.reusable_node);

        // Do not reuse an EOF node if the included ranges array has changes
        // later on in the file.
        let end_byte_offset = if ts_subtree_is_eof(result) {
            u32::MAX
        } else {
            byte_offset + ts_subtree_total_bytes(result)
        };

        if byte_offset > position {
            LOG!(
                parser,
                c"before_reusable_node symbol:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(result))
            );
            break;
        }

        if byte_offset < position {
            LOG!(
                parser,
                c"past_reusable_node symbol:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(result))
            );
            if end_byte_offset <= position || !reusable_node_descend(&mut self_.reusable_node) {
                reusable_node_advance(&mut self_.reusable_node);
            }
            continue;
        }

        if !ts_subtree_external_scanner_state_eq(
            &self_.reusable_node.last_external_token,
            &last_external_token,
        ) {
            LOG!(
                parser,
                c"reusable_node_has_different_external_scanner_state symbol:%s"
                    .as_ptr()
                    .cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(result))
            );
            reusable_node_advance(&mut self_.reusable_node);
            continue;
        }

        let mut reason: *const i8 = ptr::null();
        if ts_subtree_has_changes(result) {
            reason = c"has_changes".as_ptr().cast::<i8>();
        } else if ts_subtree_is_error(result) {
            reason = c"is_error".as_ptr().cast::<i8>();
        } else if ts_subtree_missing(result) {
            reason = c"is_missing".as_ptr().cast::<i8>();
        } else if ts_subtree_is_fragile(result) {
            reason = c"is_fragile".as_ptr().cast::<i8>();
        } else if ts_parser__has_included_range_difference(self_, byte_offset, end_byte_offset) {
            reason = c"contains_different_included_range".as_ptr().cast::<i8>();
        }

        if !reason.is_null() {
            LOG!(
                parser,
                c"cant_reuse_node_%s tree:%s".as_ptr().cast::<i8>(),
                reason,
                SYM_NAME!(parser, ts_subtree_symbol(result))
            );
            if !reusable_node_descend(&mut self_.reusable_node) {
                reusable_node_advance(&mut self_.reusable_node);
                ts_parser__breakdown_top_of_stack(self_, version);
                *state = ts_stack_state(parser_stack_ref(self_.stack), version);
            }
            continue;
        }

        let leaf_symbol = ts_subtree_leaf_symbol(result);
        ts_language_table_entry(self_.language, *state, leaf_symbol, table_entry);
        if !ts_parser__can_reuse_first_leaf(self_, *state, result, table_entry) {
            LOG!(
                parser,
                c"cant_reuse_node symbol:%s, first_leaf_symbol:%s"
                    .as_ptr()
                    .cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(result)),
                SYM_NAME!(parser, leaf_symbol)
            );
            reusable_node_advance_past_leaf(&mut self_.reusable_node);
            break;
        }

        LOG!(
            parser,
            c"reuse_node symbol:%s".as_ptr().cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(result))
        );
        ts_subtree_retain(result);
        return result;
    }

    NULL_SUBTREE
}

// ---------------------------------------------------------------------------
// Internal helpers — tree selection
// ---------------------------------------------------------------------------

unsafe fn ts_parser__select_tree(self_: &mut TSParser, left: Subtree, right: Subtree) -> bool {
    let parser = ptr::from_mut(self_);
    if left.ptr.is_null() {
        return true;
    }
    if right.ptr.is_null() {
        return false;
    }

    if ts_subtree_error_cost(right) < ts_subtree_error_cost(left) {
        LOG!(
            parser,
            c"select_smaller_error symbol:%s, over_symbol:%s"
                .as_ptr()
                .cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(right)),
            SYM_NAME!(parser, ts_subtree_symbol(left))
        );
        return true;
    }

    if ts_subtree_error_cost(left) < ts_subtree_error_cost(right) {
        LOG!(
            parser,
            c"select_smaller_error symbol:%s, over_symbol:%s"
                .as_ptr()
                .cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(left)),
            SYM_NAME!(parser, ts_subtree_symbol(right))
        );
        return false;
    }

    if ts_subtree_dynamic_precedence(right) > ts_subtree_dynamic_precedence(left) {
        LOG!(
            parser,
            c"select_higher_precedence symbol:%s, prec:%d, over_symbol:%s, other_prec:%d"
                .as_ptr()
                .cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(right)),
            ts_subtree_dynamic_precedence(right),
            SYM_NAME!(parser, ts_subtree_symbol(left)),
            ts_subtree_dynamic_precedence(left)
        );
        return true;
    }

    if ts_subtree_dynamic_precedence(left) > ts_subtree_dynamic_precedence(right) {
        LOG!(
            parser,
            c"select_higher_precedence symbol:%s, prec:%d, over_symbol:%s, other_prec:%d"
                .as_ptr()
                .cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(left)),
            ts_subtree_dynamic_precedence(left),
            SYM_NAME!(parser, ts_subtree_symbol(right)),
            ts_subtree_dynamic_precedence(right)
        );
        return false;
    }

    if ts_subtree_error_cost(left) > 0 {
        return true;
    }

    let comparison = ts_subtree_compare(left, right, &mut self_.tree_pool);
    match comparison {
        -1 => {
            LOG!(
                parser,
                c"select_earlier symbol:%s, over_symbol:%s"
                    .as_ptr()
                    .cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(left)),
                SYM_NAME!(parser, ts_subtree_symbol(right))
            );
            false
        }
        1 => {
            LOG!(
                parser,
                c"select_earlier symbol:%s, over_symbol:%s"
                    .as_ptr()
                    .cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(right)),
                SYM_NAME!(parser, ts_subtree_symbol(left))
            );
            true
        }
        _ => {
            LOG!(
                parser,
                c"select_existing symbol:%s, over_symbol:%s"
                    .as_ptr()
                    .cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(left)),
                SYM_NAME!(parser, ts_subtree_symbol(right))
            );
            false
        }
    }
}

unsafe fn ts_parser__select_children(
    self_: &mut TSParser,
    left: Subtree,
    children: &SubtreeArray,
) -> bool {
    let scratch_trees = subtree_array_as_array_mut(&mut self_.scratch_trees);
    let children = subtree_array_as_array(children);
    array_assign(scratch_trees, children);

    let scratch_tree = ts_subtree_new_node(
        ts_subtree_symbol(left),
        &mut self_.scratch_trees,
        0,
        self_.language,
    );

    ts_parser__select_tree(self_, left, ts_subtree_from_mut(scratch_tree))
}

unsafe fn ts_parser__new_node(
    self_: &mut TSParser,
    symbol: TSSymbol,
    children: &mut SubtreeArray,
    production_id: u32,
) -> MutableSubtree {
    if self_.tree_arena.is_null() {
        ts_subtree_new_node(symbol, children, production_id, self_.language)
    } else {
        let result = ts_subtree_new_node_in_arena(
            self_.tree_arena,
            symbol,
            children.contents,
            children.size,
            production_id,
            self_.language,
        );
        array_delete(subtree_array_as_array_mut(children));
        result
    }
}

unsafe fn ts_parser__builder_span_subtrees(
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

unsafe fn ts_parser__new_node_from_builder_span(
    self_: &mut TSParser,
    symbol: TSSymbol,
    children: &SubtreeArray,
    production_id: u32,
) -> MutableSubtree {
    if self_.tree_arena.is_null() {
        let mut owned_children = SubtreeArray {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
        };
        array_reserve(
            subtree_array_as_array_mut(&mut owned_children),
            children.size,
        );
        if children.size > 0 {
            ptr::copy_nonoverlapping(
                children.contents,
                owned_children.contents,
                children.size as usize,
            );
        }
        owned_children.size = children.size;
        ts_subtree_new_node(symbol, &mut owned_children, production_id, self_.language)
    } else {
        ts_subtree_new_node_in_arena(
            self_.tree_arena,
            symbol,
            children.contents,
            children.size,
            production_id,
            self_.language,
        )
    }
}

unsafe fn ts_parser__release_builder_span(self_: &mut TSParser, span: StackSliceSpan) {
    if span.size == 0 {
        return;
    }
    let contents = self_
        .reduce_builder
        .subtrees
        .contents
        .add(span.start as usize);
    for i in 0..span.size {
        ts_subtree_release(&mut self_.tree_pool, *contents.add(i as usize));
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — shift/reduce/accept
// ---------------------------------------------------------------------------

unsafe fn ts_parser__shift(
    self_: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    lookahead: Subtree,
    extra: bool,
) {
    let is_leaf = ts_subtree_child_count(lookahead) == 0;
    let subtree_to_push = if extra != ts_subtree_extra(lookahead) && is_leaf {
        let mut result = ts_subtree_make_mut(&mut self_.tree_pool, lookahead);
        ts_subtree_set_extra(&mut result, extra);
        ts_subtree_from_mut(result)
    } else {
        lookahead
    };

    ts_stack_push(
        parser_stack_mut(self_.stack),
        version,
        subtree_to_push,
        !is_leaf,
        state,
    );
    if ts_subtree_has_external_tokens(subtree_to_push) {
        ts_stack_set_last_external_token(
            parser_stack_mut(self_.stack),
            version,
            ts_subtree_last_external_token(subtree_to_push),
        );
    }
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
/// The no-old-tree path writes pop results into `reduce_builder`, avoiding the
/// allocation-heavy `StackSliceArray` used by the incremental path.
unsafe fn ts_parser__reduce(
    self_: &mut TSParser,
    version: StackVersion,
    symbol: TSSymbol,
    count: u32,
    dynamic_precedence: i32,
    production_id: u16,
    is_fragile: bool,
    end_of_non_terminal_extra: bool,
) -> StackVersion {
    if !self_.old_tree.ptr.is_null() {
        return ts_parser__reduce_with_slices(
            self_,
            version,
            symbol,
            count,
            dynamic_precedence,
            production_id,
            is_fragile,
            end_of_non_terminal_extra,
        );
    }

    let parser = ptr::from_mut(self_);
    let initial_version_count = ts_stack_version_count(parser_stack_ref(self_.stack));

    ts_stack_pop_count_into(
        parser_stack_mut(self_.stack),
        version,
        count,
        &mut self_.reduce_builder,
    );
    let mut removed_version_count: u32 = 0;
    let stack = parser_stack_mut(self_.stack);
    let halted_version_count = ts_stack_halted_version_count(stack);
    let mut i: u32 = 0;
    let pop_size = self_.reduce_builder.slices.size;
    while i < pop_size {
        let span = *array_get_ref(&self_.reduce_builder.slices, i);
        let slice_version = span.version - removed_version_count;

        // Limit max versions
        if slice_version > MAX_VERSION_COUNT + MAX_VERSION_COUNT_OVERFLOW + halted_version_count {
            ts_stack_remove_version(stack, slice_version);
            ts_parser__release_builder_span(self_, span);
            removed_version_count += 1;
            while i + 1 < pop_size {
                LOG!(
                    parser,
                    c"aborting reduce with too many versions"
                        .as_ptr()
                        .cast::<i8>()
                );
                let next_span = *array_get_ref(&self_.reduce_builder.slices, i + 1);
                if next_span.version != span.version {
                    break;
                }
                ts_parser__release_builder_span(self_, next_span);
                i += 1;
            }
            i += 1;
            continue;
        }

        // Remove trailing extras from children
        let mut children = ts_parser__builder_span_subtrees(&self_.reduce_builder, span);
        ts_subtree_array_remove_trailing_extras(&mut children, &mut self_.trailing_extras);

        let mut parent = ts_parser__new_node_from_builder_span(
            self_,
            symbol,
            &children,
            u32::from(production_id),
        );

        // Handle merged stack versions
        while i + 1 < pop_size {
            let next_span = *array_get_ref(&self_.reduce_builder.slices, i + 1);
            if next_span.version != span.version {
                break;
            }
            i += 1;

            let mut next_slice_children =
                ts_parser__builder_span_subtrees(&self_.reduce_builder, next_span);
            ts_subtree_array_remove_trailing_extras(
                &mut next_slice_children,
                &mut self_.trailing_extras2,
            );

            if ts_parser__select_children(self_, ts_subtree_from_mut(parent), &next_slice_children)
            {
                ts_subtree_array_clear(&mut self_.tree_pool, &mut self_.trailing_extras);
                ts_subtree_release(&mut self_.tree_pool, ts_subtree_from_mut(parent));
                array_swap(
                    subtree_array_as_array_mut(&mut self_.trailing_extras),
                    subtree_array_as_array_mut(&mut self_.trailing_extras2),
                );
                parent = ts_parser__new_node_from_builder_span(
                    self_,
                    symbol,
                    &next_slice_children,
                    u32::from(production_id),
                );
            } else {
                array_clear(subtree_array_as_array_mut(&mut self_.trailing_extras2));
                ts_parser__release_builder_span(self_, next_span);
            }
        }

        let state = ts_stack_state(stack, slice_version);
        let next_state = if symbol != ts_builtin_sym_error
            && symbol != ts_builtin_sym_error_repeat
            && u32::from(symbol) >= parser_language_full(self_.language).token_count
        {
            ts_language_lookup(self_.language, state, symbol)
        } else {
            ts_language_next_state(self_.language, state, symbol)
        };
        if end_of_non_terminal_extra && next_state == state {
            (*parent.ptr).set_extra(true);
        }
        if is_fragile || pop_size > 1 || initial_version_count > 1 {
            (*parent.ptr).set_fragile_left(true);
            (*parent.ptr).set_fragile_right(true);
            (*parent.ptr).parse_state = TS_TREE_STATE_NONE;
        } else {
            (*parent.ptr).parse_state = state;
        }
        (*parent.ptr).data.children.dynamic_precedence += dynamic_precedence;

        // Push the parent node and trailing extras
        ts_stack_push(
            stack,
            slice_version,
            ts_subtree_from_mut(parent),
            false,
            next_state,
        );
        for j in 0..self_.trailing_extras.size {
            ts_stack_push(
                stack,
                slice_version,
                *array_get_ref(subtree_array_as_array(&self_.trailing_extras), j),
                false,
                next_state,
            );
        }

        for j in 0..slice_version {
            if j == version {
                continue;
            }
            if ts_stack_merge(stack, j, slice_version) {
                removed_version_count += 1;
                break;
            }
        }

        i += 1;
    }
    self_.reduce_builder.slices.size = 0;
    self_.reduce_builder.subtrees.size = 0;

    if ts_stack_version_count(stack) > initial_version_count {
        initial_version_count
    } else {
        STACK_VERSION_NONE
    }
}

#[allow(clippy::too_many_arguments)]
/// Incremental parsing variant of `ts_parser__reduce`.
///
/// This path keeps concrete `StackSlice` arrays because old-tree reuse and
/// breakdown can make the child ownership rules differ from the fresh parse
/// path. It intentionally mirrors `ts_parser__reduce` so parity bugs are easier
/// to audit.
unsafe fn ts_parser__reduce_with_slices(
    self_: &mut TSParser,
    version: StackVersion,
    symbol: TSSymbol,
    count: u32,
    dynamic_precedence: i32,
    production_id: u16,
    is_fragile: bool,
    end_of_non_terminal_extra: bool,
) -> StackVersion {
    let parser = ptr::from_mut(self_);
    let initial_version_count = ts_stack_version_count(parser_stack_ref(self_.stack));

    let pop = ts_stack_pop_count(parser_stack_mut(self_.stack), version, count);
    let mut removed_version_count: u32 = 0;
    let stack = parser_stack_mut(self_.stack);
    let halted_version_count = ts_stack_halted_version_count(stack);
    let mut i: u32 = 0;
    while i < pop.size {
        let mut slice = ptr::read(array_get_ref(&pop, i));
        let slice_version = slice.version - removed_version_count;

        if slice_version > MAX_VERSION_COUNT + MAX_VERSION_COUNT_OVERFLOW + halted_version_count {
            ts_stack_remove_version(stack, slice_version);
            ts_subtree_array_delete(&mut self_.tree_pool, &mut slice.subtrees);
            removed_version_count += 1;
            while i + 1 < pop.size {
                LOG!(
                    parser,
                    c"aborting reduce with too many versions"
                        .as_ptr()
                        .cast::<i8>()
                );
                let mut next_slice = ptr::read(array_get_ref(&pop, i + 1));
                if next_slice.version != slice.version {
                    break;
                }
                ts_subtree_array_delete(&mut self_.tree_pool, &mut next_slice.subtrees);
                i += 1;
            }
            i += 1;
            continue;
        }

        let mut children = slice.subtrees;
        ts_subtree_array_remove_trailing_extras(&mut children, &mut self_.trailing_extras);
        let mut parent =
            ts_parser__new_node(self_, symbol, &mut children, u32::from(production_id));

        while i + 1 < pop.size {
            let mut next_slice = ptr::read(array_get_ref(&pop, i + 1));
            if next_slice.version != slice.version {
                break;
            }
            i += 1;

            let mut next_slice_children = SubtreeArray {
                contents: next_slice.subtrees.contents,
                size: next_slice.subtrees.size,
                capacity: next_slice.subtrees.capacity,
            };
            ts_subtree_array_remove_trailing_extras(
                &mut next_slice_children,
                &mut self_.trailing_extras2,
            );

            if ts_parser__select_children(self_, ts_subtree_from_mut(parent), &next_slice_children)
            {
                ts_subtree_array_clear(&mut self_.tree_pool, &mut self_.trailing_extras);
                ts_subtree_release(&mut self_.tree_pool, ts_subtree_from_mut(parent));
                array_swap(
                    subtree_array_as_array_mut(&mut self_.trailing_extras),
                    subtree_array_as_array_mut(&mut self_.trailing_extras2),
                );
                parent = ts_parser__new_node(
                    self_,
                    symbol,
                    &mut next_slice_children,
                    u32::from(production_id),
                );
            } else {
                array_clear(subtree_array_as_array_mut(&mut self_.trailing_extras2));
                ts_subtree_array_delete(&mut self_.tree_pool, &mut next_slice.subtrees);
            }
        }

        let state = ts_stack_state(stack, slice_version);
        let next_state = if symbol != ts_builtin_sym_error
            && symbol != ts_builtin_sym_error_repeat
            && u32::from(symbol) >= parser_language_full(self_.language).token_count
        {
            ts_language_lookup(self_.language, state, symbol)
        } else {
            ts_language_next_state(self_.language, state, symbol)
        };
        if end_of_non_terminal_extra && next_state == state {
            (*parent.ptr).set_extra(true);
        }
        if is_fragile || pop.size > 1 || initial_version_count > 1 {
            (*parent.ptr).set_fragile_left(true);
            (*parent.ptr).set_fragile_right(true);
            (*parent.ptr).parse_state = TS_TREE_STATE_NONE;
        } else {
            (*parent.ptr).parse_state = state;
        }
        (*parent.ptr).data.children.dynamic_precedence += dynamic_precedence;

        ts_stack_push(
            stack,
            slice_version,
            ts_subtree_from_mut(parent),
            false,
            next_state,
        );
        for j in 0..self_.trailing_extras.size {
            ts_stack_push(
                stack,
                slice_version,
                *array_get_ref(subtree_array_as_array(&self_.trailing_extras), j),
                false,
                next_state,
            );
        }

        for j in 0..slice_version {
            if j == version {
                continue;
            }
            if ts_stack_merge(stack, j, slice_version) {
                removed_version_count += 1;
                break;
            }
        }

        i += 1;
    }

    if ts_stack_version_count(stack) > initial_version_count {
        initial_version_count
    } else {
        STACK_VERSION_NONE
    }
}

unsafe fn ts_parser__accept(self_: &mut TSParser, version: StackVersion, lookahead: Subtree) {
    debug_assert!(ts_subtree_is_eof(lookahead));
    let stack = parser_stack_mut(self_.stack);
    ts_stack_push(stack, version, lookahead, false, 1);

    let pop = ts_stack_pop_all(stack, version);
    for i in 0..pop.size {
        let mut trees = ptr::read(&array_get_ref(&pop, i).subtrees);

        let mut root = NULL_SUBTREE;
        let mut j = i64::from(trees.size) - 1;
        while j >= 0 {
            let tree = *array_get_ref(subtree_array_as_array(&trees), j as u32);
            if !ts_subtree_extra(tree) {
                debug_assert!(!tree.data.is_inline());
                let child_count = ts_subtree_child_count(tree);
                let children = parser_subtree_children(tree);
                for child in children {
                    ts_subtree_retain(*child);
                }
                array_splice(
                    subtree_array_as_array_mut(&mut trees),
                    j as u32,
                    1,
                    child_count,
                    children.as_ptr(),
                );
                root = ts_subtree_from_mut(ts_parser__new_node(
                    self_,
                    ts_subtree_symbol(tree),
                    &mut trees,
                    u32::from((*tree.ptr).data.children.production_id),
                ));
                ts_subtree_release(&mut self_.tree_pool, tree);
                break;
            }
            j -= 1;
        }

        debug_assert!(!root.ptr.is_null());
        self_.accept_count += 1;

        if !self_.finished_tree.ptr.is_null() {
            if ts_parser__select_tree(self_, self_.finished_tree, root) {
                ts_subtree_release(&mut self_.tree_pool, self_.finished_tree);
                self_.finished_tree = root;
            } else {
                ts_subtree_release(&mut self_.tree_pool, root);
            }
        } else {
            self_.finished_tree = root;
        }
    }

    ts_stack_remove_version(stack, array_get_ref(&pop, 0).version);
    ts_stack_halt(stack, version);
}

// ---------------------------------------------------------------------------
// Internal helpers — error recovery
// ---------------------------------------------------------------------------

unsafe fn ts_parser__do_all_potential_reductions(
    self_: &mut TSParser,
    starting_version: StackVersion,
    lookahead_symbol: TSSymbol,
) -> bool {
    let lang = parser_language_full(self_.language);
    let initial_version_count = ts_stack_version_count(parser_stack_ref(self_.stack));

    let mut can_shift_lookahead_symbol = false;
    let mut version = starting_version;
    let mut i: u32 = 0;
    loop {
        let version_count = ts_stack_version_count(parser_stack_ref(self_.stack));
        if version >= version_count {
            break;
        }

        let merged = 'merge: {
            for j in initial_version_count..version {
                if ts_stack_merge(parser_stack_mut(self_.stack), j, version) {
                    break 'merge true;
                }
            }
            false
        };
        if merged {
            i += 1;
            continue;
        }

        let state = ts_stack_state(parser_stack_ref(self_.stack), version);
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
            ts_language_table_entry(self_.language, state, symbol, &mut entry);
            for j in 0..entry.action_count {
                let action = *entry.actions.add(j as usize);
                match action.type_ {
                    TSPARSE_ACTION_TYPE_SHIFT | TSPARSE_ACTION_TYPE_RECOVER
                        if !action.shift.extra && !action.shift.repetition =>
                    {
                        has_shift_action = true;
                    }
                    TSPARSE_ACTION_TYPE_REDUCE if action.reduce.child_count > 0 => {
                        ts_reduce_action_set_add(
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
            reduction_version = ts_parser__reduce(
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
            ts_stack_renumber_version(parser_stack_mut(self_.stack), reduction_version, version);
            i += 1;
            continue;
        } else if lookahead_symbol != 0 {
            ts_stack_remove_version(parser_stack_mut(self_.stack), version);
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

unsafe fn ts_parser__recover_to_state(
    self_: &mut TSParser,
    version: StackVersion,
    depth: u32,
    goal_state: TSStateId,
) -> bool {
    let stack = parser_stack_mut(self_.stack);
    let mut pop = ts_stack_pop_count(stack, version, depth);
    let mut previous_version = STACK_VERSION_NONE;

    let mut i: u32 = 0;
    while i < pop.size {
        let mut slice = ptr::read(array_get_ref(&pop, i));

        if slice.version == previous_version {
            ts_subtree_array_delete(&mut self_.tree_pool, &mut slice.subtrees);
            array_erase(&mut pop, i);
            continue;
        }

        if ts_stack_state(stack, slice.version) != goal_state {
            ts_stack_halt(stack, slice.version);
            ts_subtree_array_delete(&mut self_.tree_pool, &mut slice.subtrees);
            array_erase(&mut pop, i);
            continue;
        }

        let mut error_trees = ts_stack_pop_error(stack, slice.version);
        if error_trees.size > 0 {
            debug_assert!(error_trees.size == 1);
            let error_tree = *error_trees.contents;
            let error_child_count = ts_subtree_child_count(error_tree);
            if error_child_count > 0 {
                let error_children = parser_subtree_children(error_tree);
                array_splice(
                    subtree_array_as_array_mut(&mut slice.subtrees),
                    0,
                    0,
                    error_child_count,
                    error_children.as_ptr(),
                );
                for child in error_children {
                    ts_subtree_retain(*child);
                }
            }
            ts_subtree_array_delete(&mut self_.tree_pool, &mut error_trees);
        }

        ts_subtree_array_remove_trailing_extras(&mut slice.subtrees, &mut self_.trailing_extras);

        if slice.subtrees.size > 0 {
            let error = ts_subtree_new_error_node(&mut slice.subtrees, true, self_.language);
            ts_stack_push(stack, slice.version, error, false, goal_state);
        } else {
            array_delete(subtree_array_as_array_mut(&mut slice.subtrees));
        }

        for j in 0..self_.trailing_extras.size {
            let tree = *array_get_ref(subtree_array_as_array(&self_.trailing_extras), j);
            ts_stack_push(stack, slice.version, tree, false, goal_state);
        }

        previous_version = slice.version;
        i += 1;
    }

    previous_version != STACK_VERSION_NONE
}

unsafe fn ts_parser__recover(self_: &mut TSParser, version: StackVersion, mut lookahead: Subtree) {
    let parser = ptr::from_mut(self_);
    let mut did_recover = false;
    let stack = parser_stack_mut(self_.stack);
    let previous_version_count = ts_stack_version_count(stack);
    let position = ts_stack_position(stack, version);
    let summary = ts_stack_get_summary(stack, version);
    let node_count_since_error = ts_stack_node_count_since_error(stack, version);
    let current_error_cost = ts_stack_error_cost(stack, version);

    // Strategy 1: Find a previous state where the lookahead is valid.
    if !summary.is_null() && !ts_subtree_is_error(lookahead) {
        let summary = summary.as_ref().unwrap_unchecked();
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
                    if ts_stack_state(stack, j) == entry.state
                        && ts_stack_position(stack, j).bytes == position.bytes
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
            if ts_parser__better_version_exists(self_, version, false, new_cost) {
                break;
            }

            if ts_language_has_actions(self_.language, entry.state, ts_subtree_symbol(lookahead))
                && ts_parser__recover_to_state(self_, version, depth, entry.state)
            {
                did_recover = true;
                LOG!(
                    parser,
                    c"recover_to_previous state:%u, depth:%u"
                        .as_ptr()
                        .cast::<i8>(),
                    u32::from(entry.state),
                    depth
                );
                LOG_STACK!(parser);
                break;
            }
        }
    }

    // Remove halted versions
    let mut i = previous_version_count;
    while i < ts_stack_version_count(stack) {
        if !ts_stack_is_active(stack, i) {
            LOG!(
                parser,
                c"removed paused version:%u".as_ptr().cast::<i8>(),
                i
            );
            ts_stack_remove_version(stack, i);
            LOG_STACK!(parser);
        } else {
            i += 1;
        }
    }

    // EOF: wrap everything and terminate
    if ts_subtree_is_eof(lookahead) {
        LOG!(parser, c"recover_eof".as_ptr().cast::<i8>());
        let mut children: SubtreeArray = SubtreeArray {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
        };
        let parent = ts_subtree_new_error_node(&mut children, false, self_.language);
        ts_stack_push(stack, version, parent, false, 1);
        ts_parser__accept(self_, version, lookahead);
        return;
    }

    // Strategy 2: skip the current token
    if did_recover && ts_stack_version_count(stack) > MAX_VERSION_COUNT {
        ts_stack_halt(stack, version);
        ts_subtree_release(&mut self_.tree_pool, lookahead);
        return;
    }

    if did_recover && ts_subtree_has_external_scanner_state_change(lookahead) {
        ts_stack_halt(stack, version);
        ts_subtree_release(&mut self_.tree_pool, lookahead);
        return;
    }

    let new_cost = current_error_cost
        + ERROR_COST_PER_SKIPPED_TREE
        + ts_subtree_total_bytes(lookahead) * ERROR_COST_PER_SKIPPED_CHAR
        + ts_subtree_total_size(lookahead).extent.row * ERROR_COST_PER_SKIPPED_LINE;
    if ts_parser__better_version_exists(self_, version, false, new_cost) {
        ts_stack_halt(stack, version);
        ts_subtree_release(&mut self_.tree_pool, lookahead);
        return;
    }

    // Mark extra tokens
    let mut n: u32 = 0;
    let actions = ts_language_actions(self_.language, 1, ts_subtree_symbol(lookahead), &mut n);
    if n > 0
        && (*actions.add(n as usize - 1)).type_ == TSPARSE_ACTION_TYPE_SHIFT
        && (*actions.add(n as usize - 1)).shift.extra
    {
        let mut mutable_lookahead = ts_subtree_make_mut(&mut self_.tree_pool, lookahead);
        ts_subtree_set_extra(&mut mutable_lookahead, true);
        lookahead = ts_subtree_from_mut(mutable_lookahead);
    }

    // Wrap the lookahead in an ERROR
    LOG!(
        parser,
        c"skip_token symbol:%s".as_ptr().cast::<i8>(),
        SYM_NAME!(parser, ts_subtree_symbol(lookahead))
    );
    let mut children: SubtreeArray = SubtreeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    };
    array_reserve(subtree_array_as_array_mut(&mut children), 1);
    array_push(subtree_array_as_array_mut(&mut children), lookahead);
    let mut error_repeat =
        ts_parser__new_node(self_, ts_builtin_sym_error_repeat, &mut children, 0);

    // Merge with existing error on top of stack
    if node_count_since_error > 0 {
        let mut pop = ts_stack_pop_count(stack, version, 1);

        if pop.size > 1 {
            for pi in 1..pop.size {
                ts_subtree_array_delete(
                    &mut self_.tree_pool,
                    &mut array_get_mut(&mut pop, pi).subtrees,
                );
            }
            while ts_stack_version_count(stack) > array_get_ref(&pop, 0).version + 1 {
                ts_stack_remove_version(stack, array_get_ref(&pop, 0).version + 1);
            }
        }

        ts_stack_renumber_version(stack, array_get_ref(&pop, 0).version, version);
        let slot = &mut array_get_mut(&mut pop, 0).subtrees;
        array_push(
            subtree_array_as_array_mut(slot),
            ts_subtree_from_mut(error_repeat),
        );
        error_repeat = ts_parser__new_node(self_, ts_builtin_sym_error_repeat, slot, 0);
    }

    // Push the ERROR
    ts_stack_push(
        stack,
        version,
        ts_subtree_from_mut(error_repeat),
        false,
        ERROR_STATE,
    );
    if ts_subtree_has_external_tokens(lookahead) {
        ts_stack_set_last_external_token(stack, version, ts_subtree_last_external_token(lookahead));
    }

    let mut has_error = true;
    for vi in 0..ts_stack_version_count(stack) {
        let status = ts_parser__version_status(self_, vi);
        if !status.is_in_error {
            has_error = false;
            break;
        }
    }
    self_.has_error = has_error;
}

unsafe fn ts_parser__handle_error(self_: &mut TSParser, version: StackVersion, lookahead: Subtree) {
    let parser = ptr::from_mut(self_);
    let previous_version_count = ts_stack_version_count(parser_stack_ref(self_.stack));

    // Perform any reductions that can happen in this state, regardless of the lookahead. After
    // skipping one or more invalid tokens, the parser might find a token that would have allowed
    // a reduction to take place.
    ts_parser__do_all_potential_reductions(self_, version, 0);
    let version_count = ts_stack_version_count(parser_stack_ref(self_.stack));
    let position = ts_stack_position(parser_stack_ref(self_.stack), version);

    // Push a discontinuity onto the stack. Merge all of the stack versions that
    // were created in the previous step.
    let mut did_insert_missing_token = false;
    let mut v = version;
    while v < version_count {
        if !did_insert_missing_token {
            let state = ts_stack_state(parser_stack_ref(self_.stack), v);
            let language = parser_language_full(self_.language);
            let mut missing_symbol: TSSymbol = 1;
            while u32::from(missing_symbol) < language.token_count {
                let state_after_missing_symbol =
                    ts_language_next_state(self_.language, state, missing_symbol);
                if state_after_missing_symbol == 0 || state_after_missing_symbol == state {
                    missing_symbol += 1;
                    continue;
                }

                if ts_language_has_reduce_action(
                    self_.language,
                    state_after_missing_symbol,
                    ts_subtree_leaf_symbol(lookahead),
                ) {
                    // In case the parser is currently outside of any included range, the lexer will
                    // snap to the beginning of the next included range. The missing token's padding
                    // must be assigned to position it within the next included range.
                    ts_lexer_reset(&mut self_.lexer, position);
                    ts_lexer_mark_end(&mut self_.lexer);
                    let padding = length_sub(self_.lexer.token_end_position, position);
                    let lookahead_bytes =
                        ts_subtree_total_bytes(lookahead) + ts_subtree_lookahead_bytes(lookahead);

                    let version_with_missing_tree =
                        ts_stack_copy_version(parser_stack_mut(self_.stack), v);
                    let missing_tree = ts_subtree_new_missing_leaf(
                        &mut self_.tree_pool,
                        missing_symbol,
                        padding,
                        lookahead_bytes,
                        self_.language,
                    );
                    ts_stack_push(
                        parser_stack_mut(self_.stack),
                        version_with_missing_tree,
                        missing_tree,
                        false,
                        state_after_missing_symbol,
                    );

                    if ts_parser__do_all_potential_reductions(
                        self_,
                        version_with_missing_tree,
                        ts_subtree_leaf_symbol(lookahead),
                    ) {
                        LOG!(
                            parser,
                            c"recover_with_missing symbol:%s, state:%u"
                                .as_ptr()
                                .cast::<i8>(),
                            SYM_NAME!(parser, missing_symbol),
                            u32::from(ts_stack_state(
                                parser_stack_ref(self_.stack),
                                version_with_missing_tree,
                            ))
                        );
                        did_insert_missing_token = true;
                        break;
                    }
                }
                missing_symbol += 1;
            }
        }

        ts_stack_push(
            parser_stack_mut(self_.stack),
            v,
            NULL_SUBTREE,
            false,
            ERROR_STATE,
        );
        v = if v == version {
            previous_version_count
        } else {
            v + 1
        };
    }

    for _i in previous_version_count..version_count {
        let did_merge = ts_stack_merge(
            parser_stack_mut(self_.stack),
            version,
            previous_version_count,
        );
        debug_assert!(did_merge);
    }

    ts_stack_record_summary(parser_stack_mut(self_.stack), version, MAX_SUMMARY_DEPTH);

    // Begin recovery with the current lookahead node, rather than waiting for the
    // next turn of the parse loop. This ensures that the tree accounts for the
    // current lookahead token's "lookahead bytes" value, which describes how far
    // the lexer needed to look ahead beyond the content of the token in order to
    // recognize it.
    let mut lookahead = lookahead;
    if ts_subtree_child_count(lookahead) > 0 {
        ts_parser__breakdown_lookahead(self_, &mut lookahead, ERROR_STATE);
    }
    ts_parser__recover(self_, version, lookahead);

    LOG_STACK!(parser);
}

// ---------------------------------------------------------------------------
// Internal helpers — advance & condense
// ---------------------------------------------------------------------------

unsafe fn ts_parser__check_progress(
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
                ts_subtree_release(&mut self_.tree_pool, *lookahead);
            }
        }
        return false;
    }
    true
}

/// Advance one stack version until it shifts, accepts, recovers, pauses, or halts.
///
/// This is the parser action interpreter. It first obtains a lookahead from old
/// tree reuse, token cache, or lexing. Then it repeatedly reads the parse-table
/// entry for `(state, lookahead)` and executes its actions. Reductions keep the
/// same lookahead and continue in the new goto state; shifts consume the
/// lookahead and return to the outer parse loop.
unsafe fn ts_parser__advance(
    self_: &mut TSParser,
    version: StackVersion,
    allow_node_reuse: bool,
) -> bool {
    let parser = ptr::from_mut(self_);
    let stack = parser_stack_ref(self_.stack);
    let mut state = ts_stack_state(stack, version);
    let position = ts_stack_position(stack, version).bytes;
    let last_external_token = ts_stack_last_external_token(stack, version);

    let (did_reuse, mut lookahead, mut table_entry, mut needs_lex) =
        ts_parser__get_initial_lookahead(
            self_,
            version,
            &mut state,
            position,
            last_external_token,
            allow_node_reuse,
        );

    loop {
        if needs_lex {
            needs_lex = false;
            if !ts_parser__lex_lookahead(
                self_,
                version,
                state,
                position,
                last_external_token,
                &mut lookahead,
                &mut table_entry,
            ) {
                return false;
            }
        }

        // If a progress callback was provided, then check every
        // time a fixed number of parse actions has been processed.
        if !ts_parser__check_progress(self_, Some(&mut lookahead), Some(position), 1) {
            return false;
        }

        // Process each parse action for the current lookahead token in
        // the current state. If there are multiple actions, then this is
        // an ambiguous state. REDUCE actions always create a new stack
        // version, whereas SHIFT actions update the existing stack version
        // and terminate this loop.
        let mut did_reduce = false;
        let mut last_reduction_version = STACK_VERSION_NONE;
        for i in 0..table_entry.action_count {
            let action = *table_entry.actions.add(i as usize);

            match action.type_ {
                TSPARSE_ACTION_TYPE_SHIFT => {
                    if action.shift.repetition {
                        break;
                    }
                    let next_state;
                    if action.shift.extra {
                        next_state = state;
                        LOG!(parser, c"shift_extra".as_ptr().cast::<i8>());
                    } else {
                        next_state = action.shift.state;
                        LOG!(
                            parser,
                            c"shift state:%u".as_ptr().cast::<i8>(),
                            u32::from(next_state)
                        );
                    }

                    if ts_subtree_child_count(lookahead) > 0 {
                        ts_parser__breakdown_lookahead(self_, &mut lookahead, state);
                        let next_state = ts_language_next_state(
                            self_.language,
                            state,
                            ts_subtree_symbol(lookahead),
                        );
                        ts_parser__shift(self_, version, next_state, lookahead, action.shift.extra);
                    } else {
                        ts_parser__shift(self_, version, next_state, lookahead, action.shift.extra);
                    }
                    if did_reuse {
                        reusable_node_advance(&mut self_.reusable_node);
                    }
                    return true;
                }

                TSPARSE_ACTION_TYPE_REDUCE => {
                    let is_fragile = table_entry.action_count > 1;
                    let end_of_non_terminal_extra = lookahead.ptr.is_null();
                    LOG!(
                        parser,
                        c"reduce sym:%s, child_count:%u".as_ptr().cast::<i8>(),
                        SYM_NAME!(parser, action.reduce.symbol),
                        u32::from(action.reduce.child_count)
                    );
                    let reduction_version = ts_parser__reduce(
                        self_,
                        version,
                        action.reduce.symbol,
                        u32::from(action.reduce.child_count),
                        i32::from(action.reduce.dynamic_precedence),
                        action.reduce.production_id,
                        is_fragile,
                        end_of_non_terminal_extra,
                    );
                    did_reduce = true;
                    if reduction_version != STACK_VERSION_NONE {
                        last_reduction_version = reduction_version;
                    }
                }

                TSPARSE_ACTION_TYPE_ACCEPT => {
                    LOG!(parser, c"accept".as_ptr().cast::<i8>());
                    ts_parser__accept(self_, version, lookahead);
                    return true;
                }

                TSPARSE_ACTION_TYPE_RECOVER => {
                    if ts_subtree_child_count(lookahead) > 0 {
                        ts_parser__breakdown_lookahead(self_, &mut lookahead, ERROR_STATE);
                    }

                    ts_parser__recover(self_, version, lookahead);
                    if did_reuse {
                        reusable_node_advance(&mut self_.reusable_node);
                    }
                    return true;
                }

                _ => {}
            }
        }

        // If a reduction was performed, then replace the current stack version
        // with one of the stack versions created by a reduction, and continue
        // processing this version of the stack with the same lookahead symbol.
        if last_reduction_version != STACK_VERSION_NONE {
            ts_stack_renumber_version(
                parser_stack_mut(self_.stack),
                last_reduction_version,
                version,
            );
            LOG_STACK!(parser);
            state = ts_stack_state(parser_stack_ref(self_.stack), version);

            // At the end of a non-terminal extra rule, the lexer will return a
            // null subtree, because the parser needs to perform a fixed reduction
            // regardless of the lookahead node. After performing that reduction,
            // (and completing the non-terminal extra rule) run the lexer again based
            // on the current parse state.
            if lookahead.ptr.is_null() {
                needs_lex = true;
            } else {
                ts_language_table_entry(
                    self_.language,
                    state,
                    ts_subtree_leaf_symbol(lookahead),
                    &mut table_entry,
                );
            }

            continue;
        }

        // A reduction was performed, but was merged into an existing stack version.
        // This version can be discarded.
        if did_reduce {
            if !lookahead.ptr.is_null() {
                ts_subtree_release(&mut self_.tree_pool, lookahead);
            }
            ts_stack_halt(parser_stack_mut(self_.stack), version);
            return true;
        }

        // If the current lookahead token is a keyword that is not valid, but the
        // default word token *is* valid, then treat the lookahead token as the word
        // token instead.
        let keyword_capture_token = parser_language_full(self_.language).keyword_capture_token;
        if ts_subtree_is_keyword(lookahead)
            && ts_subtree_symbol(lookahead) != keyword_capture_token
            && !ts_language_is_reserved_word(self_.language, state, ts_subtree_symbol(lookahead))
        {
            ts_language_table_entry(
                self_.language,
                state,
                keyword_capture_token,
                &mut table_entry,
            );
            if table_entry.action_count > 0 {
                LOG!(
                    parser,
                    c"switch from_keyword:%s, to_word_token:%s"
                        .as_ptr()
                        .cast::<i8>(),
                    TREE_NAME!(parser, lookahead),
                    SYM_NAME!(parser, keyword_capture_token)
                );

                let mut mutable_lookahead = ts_subtree_make_mut(&mut self_.tree_pool, lookahead);
                ts_subtree_set_symbol(
                    &mut mutable_lookahead,
                    keyword_capture_token,
                    self_.language,
                );
                lookahead = ts_subtree_from_mut(mutable_lookahead);
                continue;
            }
        }

        // If the current lookahead token is not valid and the previous subtree on
        // the stack was reused from an old tree, then it wasn't actually valid to
        // reuse that previous subtree. Remove it from the stack, and in its place,
        // push each of its children. Then try again to process the current lookahead.
        if ts_parser__breakdown_top_of_stack(self_, version) {
            state = ts_stack_state(parser_stack_ref(self_.stack), version);
            ts_subtree_release(&mut self_.tree_pool, lookahead);
            needs_lex = true;
            continue;
        }

        // Otherwise, there is definitely an error in this version of the parse stack.
        // Mark this version as paused and continue processing any other stack
        // versions that exist. If some other version advances successfully, then
        // this version can simply be removed. But if all versions end up paused,
        // then error recovery is needed.
        LOG!(
            parser,
            c"detect_error lookahead:%s".as_ptr().cast::<i8>(),
            TREE_NAME!(parser, lookahead)
        );
        ts_stack_pause(parser_stack_mut(self_.stack), version, lookahead);
        return true;
    }
}

unsafe fn ts_parser__condense_stack(self_: &mut TSParser) -> u32 {
    let mut made_changes = false;
    let mut min_error_cost = u32::MAX;
    let mut i: StackVersion = 0;
    while i < ts_stack_version_count(parser_stack_ref(self_.stack)) {
        // Prune any versions that have been marked for removal.
        if ts_stack_is_halted(parser_stack_ref(self_.stack), i) {
            ts_stack_remove_version(parser_stack_mut(self_.stack), i);
            continue;
        }

        // Keep track of the minimum error cost of any stack version so
        // that it can be returned.
        let status_i = ts_parser__version_status(self_, i);
        if !status_i.is_in_error && status_i.cost < min_error_cost {
            min_error_cost = status_i.cost;
        }

        // Examine each pair of stack versions, removing any versions that
        // are clearly worse than another version. Ensure that the versions
        // are ordered from most promising to least promising.
        let mut j: StackVersion = 0;
        while j < i {
            let status_j = ts_parser__version_status(self_, j);

            match ts_parser__compare_versions(status_j, status_i) {
                ErrorComparison::TakeLeft => {
                    made_changes = true;
                    ts_stack_remove_version(parser_stack_mut(self_.stack), i);
                    i -= 1;
                    break;
                }

                ErrorComparison::PreferLeft | ErrorComparison::None => {
                    if ts_stack_merge(parser_stack_mut(self_.stack), j, i) {
                        made_changes = true;
                        i -= 1;
                        break;
                    }
                }

                ErrorComparison::PreferRight => {
                    made_changes = true;
                    if ts_stack_merge(parser_stack_mut(self_.stack), j, i) {
                        i -= 1;
                        break;
                    }
                    ts_stack_swap_versions(parser_stack_mut(self_.stack), i, j);
                }

                ErrorComparison::TakeRight => {
                    made_changes = true;
                    ts_stack_remove_version(parser_stack_mut(self_.stack), j);
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
    while ts_stack_version_count(parser_stack_ref(self_.stack)) > MAX_VERSION_COUNT {
        ts_stack_remove_version(parser_stack_mut(self_.stack), MAX_VERSION_COUNT);
        made_changes = true;
    }

    // If the best-performing stack version is currently paused, or all
    // versions are paused, then resume the best paused version and begin
    // the error recovery process. Otherwise, remove the paused versions.
    if ts_stack_version_count(parser_stack_ref(self_.stack)) > 0 {
        let mut has_unpaused_version = false;
        let mut i: StackVersion = 0;
        let mut n = ts_stack_version_count(parser_stack_ref(self_.stack));
        while i < n {
            if ts_stack_is_paused(parser_stack_ref(self_.stack), i) {
                if !has_unpaused_version && self_.accept_count < MAX_VERSION_COUNT {
                    LOG!(self_, c"resume version:%u".as_ptr().cast::<i8>(), i);
                    min_error_cost = ts_stack_error_cost(parser_stack_ref(self_.stack), i);
                    let lookahead = ts_stack_resume(parser_stack_mut(self_.stack), i);
                    ts_parser__handle_error(self_, i, lookahead);
                    has_unpaused_version = true;
                } else {
                    ts_stack_remove_version(parser_stack_mut(self_.stack), i);
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
        LOG!(self_, c"condense".as_ptr().cast::<i8>());
        LOG_STACK!(self_);
    }

    min_error_cost
}

unsafe fn ts_parser__balance_subtree(self_: &mut TSParser) -> bool {
    let finished_tree = self_.finished_tree;

    // If we haven't canceled balancing in progress before, then we want to clear the tree stack and
    // push the initial finished tree onto it. Otherwise, if we're resuming balancing after a
    // cancellation, we don't want to clear the tree stack.
    if !self_.canceled_balancing {
        array_clear(mutable_subtree_array_as_array_mut(
            &mut self_.tree_pool.tree_stack,
        ));
        if ts_subtree_child_count(finished_tree) > 0 && (*finished_tree.ptr).ref_count == 1 {
            array_push(
                mutable_subtree_array_as_array_mut(&mut self_.tree_pool.tree_stack),
                ts_subtree_to_mut_unsafe(finished_tree),
            );
        }
    }

    while self_.tree_pool.tree_stack.size > 0 {
        if !ts_parser__check_progress(self_, None, None, 1) {
            return false;
        }

        let tree = *array_back_ref(mutable_subtree_array_as_array(&self_.tree_pool.tree_stack));

        if (*tree.ptr).data.children.repeat_depth > 0 {
            let tree_subtree = ts_subtree_from_mut(tree);
            let children = parser_subtree_children(tree_subtree);
            let child1 = *children.get_unchecked(0);
            let child2 = *children.get_unchecked((*tree.ptr).child_count as usize - 1);
            let repeat_delta = i64::from(ts_subtree_repeat_depth(child1))
                - i64::from(ts_subtree_repeat_depth(child2));
            if repeat_delta > 0 {
                let n = repeat_delta as u32;

                let mut i = n / 2;
                while i > 0 {
                    ts_subtree_compress(tree, i, self_.language, &mut self_.tree_pool.tree_stack);

                    // We scale the operation count increment in `ts_parser__check_progress` proportionately to the compression
                    // size since larger values of i take longer to process. Shifting by 4 empirically provides good check
                    // intervals (e.g. 193 operations when i=3100) to prevent blocking during large compressions.
                    let operations = if i >> 4 > 0 { i >> 4 } else { 1 };
                    if !ts_parser__check_progress(self_, None, None, operations) {
                        return false;
                    }
                    i /= 2;
                }
            }
        }

        array_pop(mutable_subtree_array_as_array_mut(
            &mut self_.tree_pool.tree_stack,
        ));

        for i in 0..(*tree.ptr).child_count {
            let tree_subtree = ts_subtree_from_mut(tree);
            let child = *parser_subtree_child(tree_subtree, i);
            if ts_subtree_child_count(child) > 0 && (*child.ptr).ref_count == 1 {
                array_push(
                    mutable_subtree_array_as_array_mut(&mut self_.tree_pool.tree_stack),
                    ts_subtree_to_mut_unsafe(child),
                );
            }
        }
    }

    true
}

unsafe fn ts_parser_has_outstanding_parse(self_: &TSParser) -> bool {
    self_.canceled_balancing
        || !self_.external_scanner_payload.is_null()
        || ts_stack_state(parser_stack_ref(self_.stack), 0) != 1
        || ts_stack_node_count_since_error(parser_stack_mut(self_.stack), 0) != 0
}

unsafe fn ts_parser__take_finished_tree(self_: &mut TSParser) -> *mut TSTree {
    let arena = self_.tree_arena;
    self_.tree_arena = ptr::null_mut();
    let result = ts_tree_new_with_arena(
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
    let self_ = ts_calloc(1, core::mem::size_of::<TSParser>()).cast::<TSParser>();
    let parser = self_.as_mut().unwrap_unchecked();
    ts_lexer_init(&mut parser.lexer);
    array_init(&mut parser.reduce_actions);
    array_reserve(&mut parser.reduce_actions, 4);
    array_init(&mut parser.pending_reductions);
    parser.tree_pool = ts_subtree_pool_new(32);
    parser.stack = ts_stack_new(&mut parser.tree_pool);
    parser.reduce_builder = ts_stack_pop_builder_new();
    parser.finished_tree = NULL_SUBTREE;
    parser.tree_arena = ptr::null_mut();
    parser.reusable_node = reusable_node_new();
    parser.dot_graph_file = ptr::null_mut();
    parser.language = ptr::null();
    parser.has_scanner_error = false;
    parser.has_error = false;
    parser.canceled_balancing = false;
    parser.external_scanner_payload = ptr::null_mut();
    parser.operation_count = 0;
    parser.old_tree = NULL_SUBTREE;
    let new_array: Array<TSRange> = array_new();
    parser.included_range_differences = TSRangeArray {
        contents: new_array.contents,
        size: new_array.size,
        capacity: new_array.capacity,
    };
    parser.included_range_difference_index = 0;
    ts_parser__set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_delete(self_: *mut TSParser) {
    if self_.is_null() {
        return;
    }

    ts_parser_set_language(self_, ptr::null());
    let parser = self_.as_mut().unwrap_unchecked();
    ts_stack_delete(parser_stack_mut(parser.stack));
    ts_parser__clear_pending_reductions(parser);
    if !parser.reduce_actions.contents.is_null() {
        array_delete(&mut parser.reduce_actions);
    }
    if !parser.pending_reductions.contents.is_null() {
        array_delete(&mut parser.pending_reductions);
    }
    if !parser.included_range_differences.contents.is_null() {
        array_delete(ts_range_array_as_array_mut(
            &mut parser.included_range_differences,
        ));
    }
    if !parser.old_tree.ptr.is_null() {
        ts_subtree_release(&mut parser.tree_pool, parser.old_tree);
        parser.old_tree = NULL_SUBTREE;
    }
    if !parser.tree_arena.is_null() {
        ts_tree_arena_release(parser.tree_arena);
        parser.tree_arena = ptr::null_mut();
    }
    ts_wasm_store_delete(parser.wasm_store);
    ts_lexer_delete(&mut parser.lexer);
    ts_parser__set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    ts_subtree_pool_delete(&mut parser.tree_pool);
    reusable_node_delete(&mut parser.reusable_node);
    ts_stack_pop_builder_delete(&mut parser.reduce_builder);
    array_delete(subtree_array_as_array_mut(&mut parser.trailing_extras));
    array_delete(subtree_array_as_array_mut(&mut parser.trailing_extras2));
    array_delete(subtree_array_as_array_mut(&mut parser.scratch_trees));
    ts_free(self_.cast::<c_void>());
}

// ---------------------------------------------------------------------------
// Exported functions — configuration
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_parser_language(self_: *const TSParser) -> *const TSLanguage {
    let parser = self_.as_ref().unwrap_unchecked();
    parser.language
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_language(
    self_: *mut TSParser,
    language: *const TSLanguage,
) -> bool {
    ts_parser_reset(self_);
    let parser = self_.as_mut().unwrap_unchecked();
    ts_language_delete(parser.language);
    parser.language = ptr::null();

    if !language.is_null() {
        let language_full = parser_language_full(language);
        if language_full.abi_version > TREE_SITTER_LANGUAGE_VERSION
            || language_full.abi_version < TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION
        {
            return false;
        }

        if ts_language_is_wasm(language)
            && (parser.wasm_store.is_null()
                || !ts_wasm_store_start(parser.wasm_store, &mut parser.lexer.data, language))
        {
            return false;
        }
    }

    parser.language = ts_language_copy(language);
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_logger(self_: *const TSParser) -> TSLogger {
    let parser = self_.as_ref().unwrap_unchecked();
    ptr::read(&parser.lexer.logger)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_logger(self_: *mut TSParser, logger: TSLogger) {
    let parser = self_.as_mut().unwrap_unchecked();
    parser.lexer.logger = logger;
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_print_dot_graphs(self_: *mut TSParser, fd: i32) {
    let parser = self_.as_mut().unwrap_unchecked();
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
    let parser = self_.as_mut().unwrap_unchecked();
    ts_lexer_set_included_ranges(&mut parser.lexer, ranges, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_included_ranges(
    self_: *const TSParser,
    count: *mut u32,
) -> *const TSRange {
    let parser = self_.as_ref().unwrap_unchecked();
    ts_lexer_included_ranges(&parser.lexer, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_reset(self_: *mut TSParser) {
    let parser = self_.as_mut().unwrap_unchecked();
    ts_parser__external_scanner_destroy(parser);
    if !parser.wasm_store.is_null() {
        ts_wasm_store_reset(parser.wasm_store);
    }

    if !parser.old_tree.ptr.is_null() {
        ts_subtree_release(&mut parser.tree_pool, parser.old_tree);
        parser.old_tree = NULL_SUBTREE;
    }

    reusable_node_clear(&mut parser.reusable_node);
    ts_lexer_reset(&mut parser.lexer, length_zero());
    ts_stack_clear(parser_stack_mut(parser.stack));
    ts_parser__clear_pending_reductions(parser);
    ts_parser__set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    if !parser.finished_tree.ptr.is_null() {
        ts_subtree_release(&mut parser.tree_pool, parser.finished_tree);
        parser.finished_tree = NULL_SUBTREE;
    }
    if !parser.tree_arena.is_null() {
        ts_tree_arena_release(parser.tree_arena);
        parser.tree_arena = ptr::null_mut();
    }
    parser.accept_count = 0;
    parser.has_scanner_error = false;
    parser.has_error = false;
    parser.canceled_balancing = false;
    parser.parse_options = ts_parse_options_none();
    parser.parse_state = ts_parse_state_empty();
}

// ---------------------------------------------------------------------------
// Exported functions — parsing
// ---------------------------------------------------------------------------

#[no_mangle]
/// Parse one input document and return a new tree.
///
/// The driver owns the outer GLR loop:
/// - initialize lexer, external scanner, arena, and optional old-tree reuse;
/// - process every active stack version until none can advance normally;
/// - condense/merge/prune stack versions;
/// - recover when all versions are paused at errors;
/// - balance the accepted tree and transfer arena ownership into `TSTree`.
///
/// Returning null means parsing was canceled, scanner setup failed, or wasm
/// support was unavailable. In all cases parser-owned scratch state is reset
/// before returning unless the parse is intentionally resumable.
pub unsafe extern "C" fn ts_parser_parse(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    input: TSInput,
) -> *mut TSTree {
    let parser = self_.as_mut().unwrap_unchecked();
    let mut result: *mut TSTree = ptr::null_mut();
    if parser.language.is_null() || input.read.is_none() {
        return ptr::null_mut();
    }

    if ts_language_is_wasm(parser.language) {
        if parser.wasm_store.is_null() {
            return ptr::null_mut();
        }
        ts_wasm_store_start(parser.wasm_store, &mut parser.lexer.data, parser.language);
    }

    ts_lexer_set_input(&mut parser.lexer, input);
    array_clear(ts_range_array_as_array_mut(
        &mut parser.included_range_differences,
    ));
    parser.included_range_difference_index = 0;

    parser.operation_count = 0;

    if ts_parser_has_outstanding_parse(parser) {
        LOG!(self_, c"resume_parsing".as_ptr().cast::<i8>());
        if parser.canceled_balancing {
            // goto balance
            debug_assert!(!parser.finished_tree.ptr.is_null());
            if !ts_parser__balance_subtree(parser) {
                parser.canceled_balancing = true;
                return ptr::null_mut();
            }
            parser.canceled_balancing = false;
            LOG!(self_, c"done".as_ptr().cast::<i8>());
            LOG_TREE!(self_, parser.finished_tree);

            result = ts_parser__take_finished_tree(parser);

            // goto exit
            ts_parser_reset(self_);
            return result;
        }
    } else {
        ts_parser__external_scanner_create(parser);
        if parser.has_scanner_error {
            // goto exit
            ts_parser_reset(self_);
            return result;
        }
        parser.tree_arena = ts_tree_arena_new();

        if let Some(old_tree) = old_tree.as_ref() {
            ts_subtree_retain(old_tree.root);
            parser.old_tree = old_tree.root;
            let old_included_ranges =
                ts_range_slice(old_tree.included_ranges, old_tree.included_range_count);
            let new_included_ranges = ts_range_slice(
                parser.lexer.included_ranges,
                parser.lexer.included_range_count,
            );
            ts_range_array_get_changed_ranges_ref(
                old_included_ranges,
                new_included_ranges,
                &mut parser.included_range_differences,
            );
            reusable_node_reset(&mut parser.reusable_node, old_tree.root);
            LOG!(self_, c"parse_after_edit".as_ptr().cast::<i8>());
            LOG_TREE!(self_, parser.old_tree);
            for i in 0..parser.included_range_differences.size {
                let range = array_get_ref(
                    ts_range_array_as_array(&parser.included_range_differences),
                    i,
                );
                LOG!(
                    self_,
                    c"different_included_range %u - %u".as_ptr().cast::<i8>(),
                    range.start_byte,
                    range.end_byte
                );
            }
        } else {
            reusable_node_clear(&mut parser.reusable_node);
            LOG!(self_, c"new_parse".as_ptr().cast::<i8>());
        }
    }

    let mut position: u32 = 0;
    let mut last_position: u32 = 0;
    let mut version_count: StackVersion;
    loop {
        let mut version: StackVersion = 0;
        loop {
            version_count = ts_stack_version_count(parser_stack_ref(parser.stack));
            if version >= version_count {
                break;
            }

            let allow_node_reuse = version_count == 1;
            while ts_stack_is_active(parser_stack_ref(parser.stack), version) {
                LOG!(
                    self_,
                    c"process version:%u, version_count:%u, state:%d, row:%u, col:%u"
                        .as_ptr()
                        .cast::<i8>(),
                    version,
                    ts_stack_version_count(parser_stack_ref(parser.stack)),
                    i32::from(ts_stack_state(parser_stack_ref(parser.stack), version)),
                    ts_stack_position(parser_stack_ref(parser.stack), version)
                        .extent
                        .row,
                    ts_stack_position(parser_stack_ref(parser.stack), version)
                        .extent
                        .column
                );

                if !ts_parser__advance(parser, version, allow_node_reuse) {
                    if parser.has_scanner_error {
                        // goto exit
                        ts_parser_reset(self_);
                        return result;
                    }
                    return ptr::null_mut();
                }

                LOG_STACK!(self_);

                position = ts_stack_position(parser_stack_ref(parser.stack), version).bytes;
                if position > last_position || (version > 0 && position == last_position) {
                    last_position = position;
                    break;
                }
            }
            version += 1;
        }

        // After advancing each version of the stack, re-sort the versions by their cost,
        // removing any versions that are no longer worth pursuing.
        let min_error_cost = ts_parser__condense_stack(parser);

        // If there's already a finished parse tree that's better than any in-progress version,
        // then terminate parsing. Clear the parse stack to remove any extra references to subtrees
        // within the finished tree, ensuring that these subtrees can be safely mutated in-place
        // for rebalancing.
        if !parser.finished_tree.ptr.is_null()
            && ts_subtree_error_cost(parser.finished_tree) < min_error_cost
        {
            ts_stack_clear(parser_stack_mut(parser.stack));
            break;
        }

        while parser.included_range_difference_index < parser.included_range_differences.size {
            let range = array_get_ref(
                ts_range_array_as_array(&parser.included_range_differences),
                parser.included_range_difference_index,
            );
            if range.end_byte <= position {
                parser.included_range_difference_index += 1;
            } else {
                break;
            }
        }

        if version_count == 0 {
            break;
        }
    }

    // balance:
    debug_assert!(!parser.finished_tree.ptr.is_null());
    if !ts_parser__balance_subtree(parser) {
        parser.canceled_balancing = true;
        return ptr::null_mut();
    }
    parser.canceled_balancing = false;
    LOG!(self_, c"done".as_ptr().cast::<i8>());
    LOG_TREE!(self_, parser.finished_tree);

    result = ts_parser__take_finished_tree(parser);

    // exit:
    ts_parser_reset(self_);
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_parse_with_options(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    input: TSInput,
    parse_options: TSParseOptions,
) -> *mut TSTree {
    {
        let parser = self_.as_mut().unwrap_unchecked();
        parser.parse_options = parse_options;
        parser.parse_state.payload = parse_options.payload;
    }
    let result = ts_parser_parse(self_, old_tree, input);
    // Reset parser options before further parse calls.
    let parser = self_.as_mut().unwrap_unchecked();
    parser.parse_options = ts_parse_options_none();
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_parse_string(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    string: *const i8,
    length: u32,
) -> *mut TSTree {
    ts_parser_parse_string_encoding(self_, old_tree, string, length, TSInputEncodingUTF8)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_parse_string_encoding(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    string: *const i8,
    length: u32,
    encoding: TSInputEncoding,
) -> *mut TSTree {
    let input = TSStringInput { string, length };
    ts_parser_parse(
        self_,
        old_tree,
        TSInput {
            payload: std::ptr::addr_of!(input) as *mut c_void,
            read: Some(ts_string_input_read),
            encoding,
            decode: None,
        },
    )
}

// ---------------------------------------------------------------------------
// Exported functions — WASM
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_wasm_store(self_: *mut TSParser, store: *mut TSWasmStore) {
    let parser = self_.as_ref().unwrap_unchecked();
    if !parser.language.is_null() && ts_language_is_wasm(parser.language) {
        // Copy the assigned language into the new store.
        let copy = ts_language_copy(parser.language);
        ts_parser_set_language(self_, copy);
        ts_language_delete(copy);
    }

    let parser = self_.as_mut().unwrap_unchecked();
    ts_wasm_store_delete(parser.wasm_store);
    parser.wasm_store = store;
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_take_wasm_store(self_: *mut TSParser) -> *mut TSWasmStore {
    let parser = self_.as_ref().unwrap_unchecked();
    if !parser.language.is_null() && ts_language_is_wasm(parser.language) {
        ts_parser_set_language(self_, ptr::null());
    }

    let parser = self_.as_mut().unwrap_unchecked();
    let result = parser.wasm_store;
    parser.wasm_store = ptr::null_mut();
    result
}
