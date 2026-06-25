#![allow(dead_code)]
#![allow(non_snake_case)]

use core::ffi::c_void;
use std::ptr;

use crate::ffi::{
    TSInput, TSInputEncoding, TSInputEncodingUTF8, TSLanguage, TSLogger,
    TSLogTypeParse, TSParseOptions, TSParseState, TSPoint, TSRange, TSStateId, TSSymbol,
    TSWasmStore,
};

use super::alloc::{ts_calloc, ts_free};
use super::error_costs::{
    ERROR_COST_PER_SKIPPED_CHAR, ERROR_COST_PER_SKIPPED_LINE,
    ERROR_COST_PER_SKIPPED_TREE, ERROR_STATE,
};
use super::get_changed_ranges::{
    ts_range_array_get_changed_ranges, ts_range_array_intersects, TSRangeArray,
};
use super::language::{
    ts_language_actions, ts_language_enabled_external_tokens,
    ts_language_copy, ts_language_delete, ts_language_has_actions,
    ts_language_has_reduce_action, ts_language_is_reserved_word, ts_language_lex_mode_for_state,
    ts_language_next_state, ts_language_symbol_name, ts_language_table_entry,
    TableEntry, TSLanguageFull, TSLexer, TSLexerMode,
    TSParseActionTypeAccept as TSPARSE_ACTION_TYPE_ACCEPT,
    TSParseActionTypeRecover as TSPARSE_ACTION_TYPE_RECOVER,
    TSParseActionTypeReduce as TSPARSE_ACTION_TYPE_REDUCE,
    TSParseActionTypeShift as TSPARSE_ACTION_TYPE_SHIFT,
};
use super::length::{length_sub, length_zero};
use super::lexer::{
    ts_lexer_delete, ts_lexer_finish, ts_lexer_included_ranges, ts_lexer_init, ts_lexer_mark_end,
    ts_lexer_reset, ts_lexer_set_included_ranges, ts_lexer_set_input, ts_lexer_start,
    Lexer,
};
use super::stack::{
    array_assign, array_back_ref, array_clear, array_delete, array_erase,
    array_get_mut, array_get_ref, array_init, array_new, array_pop, array_push, array_reserve,
    array_splice, array_swap, Array, Stack, StackSlice, StackSliceArray, StackSummary,
    StackSummaryEntry,
    StackVersion, STACK_VERSION_NONE,
    // Stack functions (now Rust-only)
    ts_stack_can_merge, ts_stack_clear, ts_stack_copy_version, ts_stack_delete,
    ts_stack_dynamic_precedence, ts_stack_error_cost, ts_stack_get_summary,
    ts_stack_halt, ts_stack_halted_version_count, ts_stack_has_advanced_since_error,
    ts_stack_is_active, ts_stack_is_halted, ts_stack_is_paused,
    ts_stack_last_external_token, ts_stack_merge, ts_stack_new,
    ts_stack_node_count_since_error, ts_stack_pause, ts_stack_pop_all,
    ts_stack_pop_count, ts_stack_pop_error, ts_stack_pop_pending, ts_stack_position,
    ts_stack_print_dot_graph, ts_stack_push, ts_stack_record_summary,
    ts_stack_remove_version, ts_stack_renumber_version, ts_stack_resume,
    ts_stack_set_last_external_token, ts_stack_state, ts_stack_swap_versions,
    ts_stack_version_count,
};
use super::subtree::{
    ts_builtin_sym_end, ts_builtin_sym_error, ts_builtin_sym_error_repeat,
    ts_subtree_child_count, ts_subtree_children, ts_subtree_dynamic_precedence,
    ts_subtree_error_cost, ts_subtree_extra, ts_subtree_from_mut,
    ts_subtree_has_changes, ts_subtree_has_external_tokens,
    ts_subtree_has_external_scanner_state_change, ts_subtree_is_error,
    ts_subtree_is_eof, ts_subtree_is_fragile, ts_subtree_is_keyword,
    ts_subtree_leaf_parse_state, ts_subtree_leaf_symbol,
    ts_subtree_lookahead_bytes, ts_subtree_missing,
    ts_subtree_parse_state, ts_subtree_repeat_depth, ts_subtree_set_extra,
    ts_subtree_size, ts_subtree_symbol, ts_subtree_to_mut_unsafe,
    ts_subtree_total_bytes, ts_subtree_total_size,
    ExternalScannerState, MutableSubtree, MutableSubtreeArray, Subtree,
    SubtreeArray, SubtreePool, NULL_SUBTREE, TS_TREE_STATE_NONE,
    // Subtree functions (now Rust-only)
    ts_external_scanner_state_data, ts_external_scanner_state_eq,
    ts_external_scanner_state_init, ts_subtree_array_clear, ts_subtree_array_delete,
    ts_subtree_array_remove_trailing_extras, ts_subtree_compare, ts_subtree_compress,
    ts_subtree_external_scanner_state, ts_subtree_external_scanner_state_eq,
    ts_subtree_last_external_token, ts_subtree_make_mut, ts_subtree_new_error,
    ts_subtree_new_error_node, ts_subtree_new_leaf, ts_subtree_new_missing_leaf,
    ts_subtree_new_node, ts_subtree_pool_delete, ts_subtree_pool_new,
    ts_subtree_print_dot_graph, ts_subtree_release, ts_subtree_retain, ts_subtree_set_symbol,
};
use super::tree::{ts_tree_new, TSTree};

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
    fn memcmp(s1: *const c_void, s2: *const c_void, n: usize) -> i32;

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
            ts_stack_print_dot_graph(parser_stack_mut((*$self_).stack), (*$self_).language, (*$self_).dot_graph_file);
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
                    b'\t' => { *buf.add(off) = b'\\' as i8; off += 1; *buf.add(off) = b't' as i8; off += 1; }
                    b'\n' => { *buf.add(off) = b'\\' as i8; off += 1; *buf.add(off) = b'n' as i8; off += 1; }
                    0x0b  => { *buf.add(off) = b'\\' as i8; off += 1; *buf.add(off) = b'v' as i8; off += 1; }
                    0x0c  => { *buf.add(off) = b'\\' as i8; off += 1; *buf.add(off) = b'f' as i8; off += 1; }
                    b'\r' => { *buf.add(off) = b'\\' as i8; off += 1; *buf.add(off) = b'r' as i8; off += 1; }
                    b'\\' => { *buf.add(off) = b'\\' as i8; off += 1; *buf.add(off) = b'\\' as i8; off += 1; }
                    _     => { *buf.add(off) = *symbol.add(i); off += 1; }
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

/// `ReduceAction` — from `reduce_action.h`
#[repr(C)]
#[derive(Clone, Copy)]
struct ReduceAction {
    count: u32,
    symbol: TSSymbol,
    dynamic_precedence: i32,
    production_id: u16,
}

/// `ReduceActionSet` — Array(ReduceAction)
type ReduceActionSet = Array<ReduceAction>;

/// `StackEntry` — for `ReusableNode` (from `reusable_node.h`)
#[repr(C)]
#[derive(Clone, Copy)]
struct StackEntry {
    tree: Subtree,
    child_index: u32,
    byte_offset: u32,
}

/// `ReusableNode` — for incremental reparsing (from `reusable_node.h`)
#[repr(C)]
struct ReusableNode {
    stack: Array<StackEntry>,
    last_external_token: Subtree,
}

/// `TokenCache` — cached lookahead token
#[repr(C)]
struct TokenCache {
    token: Subtree,
    last_external_token: Subtree,
    byte_index: u32,
}

/// `ErrorStatus` — for comparing parse versions
#[repr(C)]
#[derive(Clone, Copy)]
struct ErrorStatus {
    cost: u32,
    node_count: u32,
    dynamic_precedence: i32,
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

/// `TSParser` — the main parser struct
#[repr(C)]
pub struct TSParser {
    lexer: Lexer,
    stack: *mut Stack,
    tree_pool: SubtreePool,
    language: *const TSLanguage,
    wasm_store: *mut TSWasmStore,
    reduce_actions: ReduceActionSet,
    finished_tree: Subtree,
    trailing_extras: SubtreeArray,
    trailing_extras2: SubtreeArray,
    scratch_trees: SubtreeArray,
    token_cache: TokenCache,
    reusable_node: ReusableNode,
    external_scanner_payload: *mut c_void,
    dot_graph_file: *mut c_void,
    accept_count: u32,
    operation_count: u32,
    old_tree: Subtree,
    included_range_differences: TSRangeArray,
    parse_options: TSParseOptions,
    parse_state: TSParseState,
    included_range_difference_index: u32,
    has_scanner_error: bool,
    canceled_balancing: bool,
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
unsafe fn parser_ptr_mut<'a>(parser: *mut TSParser) -> &'a mut TSParser {
    parser.as_mut().unwrap_unchecked()
}

#[inline]
const unsafe fn parser_ptr_ref<'a>(parser: *const TSParser) -> &'a TSParser {
    parser.as_ref().unwrap_unchecked()
}

#[inline]
unsafe fn parser_language_full<'a>(language: *const TSLanguage) -> &'a TSLanguageFull {
    language.cast::<TSLanguageFull>().as_ref().unwrap_unchecked()
}

// ---------------------------------------------------------------------------
// ReusableNode inline helpers (from reusable_node.h)
// ---------------------------------------------------------------------------

const unsafe fn reusable_node_new() -> ReusableNode {
    ReusableNode {
        stack: array_new(),
        last_external_token: NULL_SUBTREE,
    }
}

unsafe fn reusable_node_clear(self_: &mut ReusableNode) {
    array_clear(&mut self_.stack);
    self_.last_external_token = NULL_SUBTREE;
}

unsafe fn stack_entry_array_back(self_: &Array<StackEntry>) -> &StackEntry {
    array_back_ref(self_)
}

unsafe fn reusable_node_last_entry(self_: &ReusableNode) -> Option<&StackEntry> {
    if self_.stack.size > 0 {
        Some(stack_entry_array_back(&self_.stack))
    } else {
        None
    }
}

unsafe fn reusable_node_tree(self_: &ReusableNode) -> Subtree {
    reusable_node_last_entry(self_)
        .map_or(NULL_SUBTREE, |entry| entry.tree)
}

unsafe fn reusable_node_byte_offset(self_: &ReusableNode) -> u32 {
    reusable_node_last_entry(self_)
        .map_or(u32::MAX, |entry| entry.byte_offset)
}

unsafe fn reusable_node_delete(self_: &mut ReusableNode) {
    array_delete(&mut self_.stack);
}

unsafe fn reusable_node_advance(self_: &mut ReusableNode) {
    let Some(last_entry) = reusable_node_last_entry(self_).copied() else {
        return;
    };
    let byte_offset = last_entry.byte_offset + ts_subtree_total_bytes(last_entry.tree);
    if ts_subtree_has_external_tokens(last_entry.tree) {
        self_.last_external_token = ts_subtree_last_external_token(last_entry.tree);
    }

    let mut tree;
    let mut next_index;
    loop {
        let popped_entry = array_pop(&mut self_.stack);
        next_index = popped_entry.child_index + 1;
        if self_.stack.size == 0 {
            return;
        }
        tree = reusable_node_last_entry(self_)
            .map_or(NULL_SUBTREE, |entry| entry.tree);
        if ts_subtree_child_count(tree) > next_index {
            break;
        }
    }

    array_push(&mut self_.stack, StackEntry {
        tree: *parser_subtree_child(tree, next_index),
        child_index: next_index,
        byte_offset,
    });
}

unsafe fn reusable_node_descend(self_: &mut ReusableNode) -> bool {
    let Some(last_entry) = reusable_node_last_entry(self_).copied() else {
        return false;
    };
    if ts_subtree_child_count(last_entry.tree) > 0 {
        array_push(&mut self_.stack, StackEntry {
            tree: *parser_subtree_child(last_entry.tree, 0),
            child_index: 0,
            byte_offset: last_entry.byte_offset,
        });
        true
    } else {
        false
    }
}

unsafe fn reusable_node_advance_past_leaf(self_: &mut ReusableNode) {
    while reusable_node_descend(self_) {}
    reusable_node_advance(self_);
}

unsafe fn reusable_node_reset(self_: &mut ReusableNode, tree: Subtree) {
    reusable_node_clear(self_);
    array_push(&mut self_.stack, StackEntry {
        tree,
        child_index: 0,
        byte_offset: 0,
    });

    // Never reuse the root node, because it has a non-standard internal structure
    // due to transformations that are applied when it is accepted: adding the EOF
    // child and any extra children.
    if !reusable_node_descend(self_) {
        reusable_node_clear(self_);
    }
}

// ---------------------------------------------------------------------------
// ReduceActionSet helper
// ---------------------------------------------------------------------------

unsafe fn reduce_action_set_get(self_: &ReduceActionSet, index: u32) -> &ReduceAction {
    array_get_ref(self_, index)
}

unsafe fn stack_slice_array_get(self_: &StackSliceArray, index: u32) -> &StackSlice {
    array_get_ref(self_, index)
}

unsafe fn stack_slice_array_get_mut(self_: &mut StackSliceArray, index: u32) -> &mut StackSlice {
    array_get_mut(self_, index)
}

unsafe fn stack_slice_array_read(self_: &StackSliceArray, index: u32) -> StackSlice {
    ptr::read(stack_slice_array_get(self_, index))
}

#[inline]
unsafe fn parser_subtree_child<'a>(parent: Subtree, index: u32) -> &'a Subtree {
    parser_subtree_children(parent).get_unchecked(index as usize)
}

#[inline]
unsafe fn parser_subtree_children<'a>(parent: Subtree) -> &'a [Subtree] {
    std::slice::from_raw_parts(
        ts_subtree_children(parent),
        ts_subtree_child_count(parent) as usize,
    )
}

const unsafe fn stack_slice_subtrees_read_ref(self_: &StackSlice) -> SubtreeArray {
    ptr::read(&self_.subtrees)
}

unsafe fn subtree_array_as_array(self_: &SubtreeArray) -> &Array<Subtree> {
    &*ptr::from_ref(self_).cast::<Array<Subtree>>()
}

unsafe fn subtree_array_as_array_mut(self_: &mut SubtreeArray) -> &mut Array<Subtree> {
    &mut *ptr::from_mut(self_).cast::<Array<Subtree>>()
}

unsafe fn mutable_subtree_array_as_array(
    self_: &MutableSubtreeArray,
) -> &Array<MutableSubtree> {
    &*ptr::from_ref(self_).cast::<Array<MutableSubtree>>()
}

unsafe fn mutable_subtree_array_as_array_mut(
    self_: &mut MutableSubtreeArray,
) -> &mut Array<MutableSubtree> {
    &mut *ptr::from_mut(self_).cast::<Array<MutableSubtree>>()
}

unsafe fn ts_range_array_as_array(self_: &TSRangeArray) -> &Array<TSRange> {
    &*ptr::from_ref(self_).cast::<Array<TSRange>>()
}

unsafe fn ts_range_array_as_array_mut(self_: &mut TSRangeArray) -> &mut Array<TSRange> {
    &mut *ptr::from_mut(self_).cast::<Array<TSRange>>()
}

unsafe fn subtree_array_get(self_: &SubtreeArray, index: u32) -> Subtree {
    *array_get_ref(subtree_array_as_array(self_), index)
}

unsafe fn stack_summary_array_get(self_: &StackSummary, index: u32) -> &StackSummaryEntry {
    array_get_ref(self_, index)
}

unsafe fn mutable_subtree_array_back(self_: &MutableSubtreeArray) -> MutableSubtree {
    *array_back_ref(mutable_subtree_array_as_array(self_))
}

unsafe fn ts_range_array_get(self_: &TSRangeArray, index: u32) -> &TSRange {
    array_get_ref(ts_range_array_as_array(self_), index)
}

const unsafe fn ts_logger_read_ref(self_: &TSLogger) -> TSLogger {
    ptr::read(self_)
}

unsafe fn ts_reduce_action_set_add(
    self_: &mut ReduceActionSet,
    new_action: ReduceAction,
) {
    for i in 0..self_.size {
        let action = reduce_action_set_get(self_, i);
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
        fprintf(self_.dot_graph_file, c"graph {\nlabel=\"".as_ptr().cast::<i8>());
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

unsafe fn ts_parser__breakdown_top_of_stack(
    self_: &mut TSParser,
    version: StackVersion,
) -> bool {
    let mut did_break_down = false;

    loop {
        let pop = ts_stack_pop_pending(parser_stack_mut(self_.stack), version);
        if pop.size == 0 {
            break;
        }

        did_break_down = true;
        let mut pending = false;
        for i in 0..pop.size {
            let mut slice = stack_slice_array_read(&pop, i);
            let mut state = ts_stack_state(parser_stack_ref(self_.stack), slice.version);
            let parent = subtree_array_get(&slice.subtrees, 0);

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
                ts_stack_push(parser_stack_mut(self_.stack), slice.version, child, pending, state);
            }

            for j in 1..slice.subtrees.size {
                let tree = subtree_array_get(&slice.subtrees, j);
                ts_stack_push(parser_stack_mut(self_.stack), slice.version, tree, false, state);
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
        LOG!(parser, c"state_mismatch sym:%s".as_ptr().cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(tree)));
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

unsafe fn ts_parser__version_status(
    self_: &mut TSParser,
    version: StackVersion,
) -> ErrorStatus {
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
    if !self_.finished_tree.ptr.is_null()
        && ts_subtree_error_cost(self_.finished_tree) <= cost
    {
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
                if ts_stack_can_merge(parser_stack_ref(self_.stack), i, version) => {
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

unsafe fn ts_parser__call_main_lex_fn(
    self_: &mut TSParser,
    lex_mode: TSLexerMode,
) -> bool {
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
        (parser_language_full(self_.language)
            .keyword_lex_fn
            .unwrap())(&mut self_.lexer.data, 0)
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

unsafe fn ts_parser__external_scanner_deserialize(
    self_: &mut TSParser,
    external_token: Subtree,
) {
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
            .unwrap())(
            self_.external_scanner_payload,
            data.cast::<i8>(),
            length,
        );
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
        let valid_external_tokens = ts_language_enabled_external_tokens(
            self_.language,
            u32::from(external_lex_state),
        );
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
    let lang = parser_language_full(self_.language);
    let leaf_symbol = ts_subtree_leaf_symbol(tree);
    let leaf_state = ts_subtree_leaf_parse_state(tree);
    let current_lex_mode = ts_language_lex_mode_for_state(self_.language, state);
    let leaf_lex_mode = ts_language_lex_mode_for_state(self_.language, leaf_state);

    // At the end of a non-terminal extra node, the lexer normally returns
    // NULL, which indicates that the parser should look for a reduce action
    // at symbol `0`. Avoid reusing tokens in this situation.
    if current_lex_mode.lex_state == u16::MAX {
        return false;
    }

    // If the token was created in a state with the same set of lookaheads, it is reusable.
    if table_entry.action_count > 0
        && memcmp(
            ptr::from_ref(&leaf_lex_mode).cast::<c_void>(),
            ptr::from_ref(&current_lex_mode).cast::<c_void>(),
            core::mem::size_of::<TSLexerMode>(),
        ) == 0
        && (leaf_symbol != lang.keyword_capture_token
            || (!ts_subtree_is_keyword(tree) && ts_subtree_parse_state(tree) == state))
    {
        return true;
    }

    // Empty tokens are not reusable in states with different lookaheads.
    if ts_subtree_size(tree).bytes == 0 && leaf_symbol != ts_builtin_sym_end {
        return false;
    }

    // If the current state allows external tokens or other tokens that conflict with this
    // token, this token is not reusable.
    current_lex_mode.external_lex_state == 0 && table_entry.is_reusable
}

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
            c"no_lookahead_after_non_terminal_extra".as_ptr().cast::<i8>()
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
            LOG!(parser, c"lex_external state:%d, row:%u, column:%u".as_ptr().cast::<i8>(),
                i32::from(lex_mode.external_lex_state),
                current_position.extent.row,
                current_position.extent.column);
            ts_lexer_start(&mut self_.lexer);
            ts_parser__external_scanner_deserialize(self_, external_token);
            found_token = ts_parser__external_scanner_scan(self_, lex_mode.external_lex_state);
            if self_.has_scanner_error {
                return NULL_SUBTREE;
            }
            ts_lexer_finish(&mut self_.lexer, &mut lookahead_end_byte);

            if found_token {
                external_scanner_state_len = ts_parser__external_scanner_serialize(self_);
                let external_scanner_state =
                    ts_subtree_external_scanner_state(&external_token);
                external_scanner_state_changed = !ts_external_scanner_state_eq(
                    external_scanner_state,
                    self_.lexer.debug_buffer.as_ptr(),
                    external_scanner_state_len,
                );

                if self_.lexer.token_end_position.bytes <= current_position.bytes
                    && !external_scanner_state_changed
                {
                    let symbol = *lang.external_scanner.symbol_map.add(
                        self_.lexer.data.result_symbol as usize,
                    );
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
                        LOG!(parser,
                            c"ignore_empty_external_token symbol:%s".as_ptr().cast::<i8>(),
                            SYM_NAME!(parser, *lang.external_scanner.symbol_map.add(
                                self_.lexer.data.result_symbol as usize
                            )));
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

        LOG!(parser, c"lex_internal state:%d, row:%u, column:%u".as_ptr().cast::<i8>(),
            i32::from(lex_mode.lex_state),
            current_position.extent.row,
            current_position.extent.column);
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

    let result;
    if skipped_error {
        let padding = length_sub(error_start_position, start_position);
        let size = length_sub(error_end_position, error_start_position);
        let lookahead_bytes = lookahead_end_byte - error_end_position.bytes;
        result = ts_subtree_new_error(
            &mut self_.tree_pool,
            first_error_character,
            padding,
            size,
            lookahead_bytes,
            parse_state,
            self_.language,
        );
    } else {
        let mut is_keyword = false;
        let mut symbol = self_.lexer.data.result_symbol;
        let padding = length_sub(self_.lexer.token_start_position, start_position);
        let size = length_sub(
            self_.lexer.token_end_position,
            self_.lexer.token_start_position,
        );
        let lookahead_bytes = lookahead_end_byte - self_.lexer.token_end_position.bytes;

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

        result = ts_subtree_new_leaf(
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
    }

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

unsafe fn ts_parser__has_included_range_difference(
    self_: &TSParser,
    start_position: u32,
    end_position: u32,
) -> bool {
    ts_range_array_intersects(
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
            LOG!(parser, c"before_reusable_node symbol:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(result)));
            break;
        }

        if byte_offset < position {
            LOG!(parser, c"past_reusable_node symbol:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(result)));
            if end_byte_offset <= position || !reusable_node_descend(&mut self_.reusable_node) {
                reusable_node_advance(&mut self_.reusable_node);
            }
            continue;
        }

        if !ts_subtree_external_scanner_state_eq(
            &self_.reusable_node.last_external_token,
            &last_external_token,
        ) {
            LOG!(parser, c"reusable_node_has_different_external_scanner_state symbol:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(result)));
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
            LOG!(parser, c"cant_reuse_node_%s tree:%s".as_ptr().cast::<i8>(),
                reason, SYM_NAME!(parser, ts_subtree_symbol(result)));
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
            LOG!(parser, c"cant_reuse_node symbol:%s, first_leaf_symbol:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(result)),
                SYM_NAME!(parser, leaf_symbol));
            reusable_node_advance_past_leaf(&mut self_.reusable_node);
            break;
        }

        LOG!(parser, c"reuse_node symbol:%s".as_ptr().cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(result)));
        ts_subtree_retain(result);
        return result;
    }

    NULL_SUBTREE
}

// ---------------------------------------------------------------------------
// Internal helpers — tree selection
// ---------------------------------------------------------------------------

unsafe fn ts_parser__select_tree(
    self_: &mut TSParser,
    left: Subtree,
    right: Subtree,
) -> bool {
    let parser = ptr::from_mut(self_);
    if left.ptr.is_null() {
        return true;
    }
    if right.ptr.is_null() {
        return false;
    }

    if ts_subtree_error_cost(right) < ts_subtree_error_cost(left) {
        LOG!(parser, c"select_smaller_error symbol:%s, over_symbol:%s".as_ptr().cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(right)),
            SYM_NAME!(parser, ts_subtree_symbol(left)));
        return true;
    }

    if ts_subtree_error_cost(left) < ts_subtree_error_cost(right) {
        LOG!(parser, c"select_smaller_error symbol:%s, over_symbol:%s".as_ptr().cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(left)),
            SYM_NAME!(parser, ts_subtree_symbol(right)));
        return false;
    }

    if ts_subtree_dynamic_precedence(right) > ts_subtree_dynamic_precedence(left) {
        LOG!(parser, c"select_higher_precedence symbol:%s, prec:%d, over_symbol:%s, other_prec:%d".as_ptr().cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(right)), ts_subtree_dynamic_precedence(right),
            SYM_NAME!(parser, ts_subtree_symbol(left)), ts_subtree_dynamic_precedence(left));
        return true;
    }

    if ts_subtree_dynamic_precedence(left) > ts_subtree_dynamic_precedence(right) {
        LOG!(parser, c"select_higher_precedence symbol:%s, prec:%d, over_symbol:%s, other_prec:%d".as_ptr().cast::<i8>(),
            SYM_NAME!(parser, ts_subtree_symbol(left)), ts_subtree_dynamic_precedence(left),
            SYM_NAME!(parser, ts_subtree_symbol(right)), ts_subtree_dynamic_precedence(right));
        return false;
    }

    if ts_subtree_error_cost(left) > 0 {
        return true;
    }

    let comparison = ts_subtree_compare(left, right, &mut self_.tree_pool);
    match comparison {
        -1 => {
            LOG!(parser, c"select_earlier symbol:%s, over_symbol:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(left)),
                SYM_NAME!(parser, ts_subtree_symbol(right)));
            false
        }
        1 => {
            LOG!(parser, c"select_earlier symbol:%s, over_symbol:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(right)),
                SYM_NAME!(parser, ts_subtree_symbol(left)));
            true
        }
        _ => {
            LOG!(parser, c"select_existing symbol:%s, over_symbol:%s".as_ptr().cast::<i8>(),
                SYM_NAME!(parser, ts_subtree_symbol(left)),
                SYM_NAME!(parser, ts_subtree_symbol(right)));
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
    let parser = ptr::from_mut(self_);
    let initial_version_count = ts_stack_version_count(parser_stack_ref(self_.stack));

    let pop = ts_stack_pop_count(parser_stack_mut(self_.stack), version, count);
    let mut removed_version_count: u32 = 0;
    let stack = parser_stack_mut(self_.stack);
    let halted_version_count = ts_stack_halted_version_count(stack);
    let mut i: u32 = 0;
    while i < pop.size {
        let mut slice = stack_slice_array_read(&pop, i);
        let slice_version = slice.version - removed_version_count;

        // Limit max versions
        if slice_version > MAX_VERSION_COUNT + MAX_VERSION_COUNT_OVERFLOW + halted_version_count {
            ts_stack_remove_version(stack, slice_version);
            ts_subtree_array_delete(&mut self_.tree_pool, &mut slice.subtrees);
            removed_version_count += 1;
            while i + 1 < pop.size {
                LOG!(
                    parser,
                    c"aborting reduce with too many versions".as_ptr().cast::<i8>()
                );
                let mut next_slice = stack_slice_array_read(&pop, i + 1);
                if next_slice.version != slice.version {
                    break;
                }
                ts_subtree_array_delete(&mut self_.tree_pool, &mut next_slice.subtrees);
                i += 1;
            }
            i += 1;
            continue;
        }

        // Remove trailing extras from children
        let mut children = slice.subtrees;
        ts_subtree_array_remove_trailing_extras(&mut children, &mut self_.trailing_extras);

        let mut parent = ts_subtree_new_node(
            symbol,
            &mut children,
            u32::from(production_id),
            self_.language,
        );

        // Handle merged stack versions
        while i + 1 < pop.size {
            let mut next_slice = stack_slice_array_read(&pop, i + 1);
            if next_slice.version != slice.version {
                break;
            }
            i += 1;

            // Make a shallow copy of the subtrees (C does struct copy)
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
                parent = ts_subtree_new_node(
                    symbol,
                    &mut next_slice_children,
                    u32::from(production_id),
                    self_.language,
                );
            } else {
                array_clear(
                    subtree_array_as_array_mut(&mut self_.trailing_extras2),
                );
                // Use the original size from next_slice.subtrees to delete all subtrees
                ts_subtree_array_delete(&mut self_.tree_pool, &mut next_slice.subtrees);
            }
        }

        let state = ts_stack_state(stack, slice_version);
        let next_state = ts_language_next_state(self_.language, state, symbol);
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
                subtree_array_get(&self_.trailing_extras, j),
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

unsafe fn ts_parser__accept(
    self_: &mut TSParser,
    version: StackVersion,
    lookahead: Subtree,
) {
    debug_assert!(ts_subtree_is_eof(lookahead));
    let stack = parser_stack_mut(self_.stack);
    ts_stack_push(stack, version, lookahead, false, 1);

    let pop = ts_stack_pop_all(stack, version);
    for i in 0..pop.size {
        let mut trees = stack_slice_subtrees_read_ref(stack_slice_array_get(&pop, i));

        let mut root = NULL_SUBTREE;
        let mut j = i64::from(trees.size) - 1;
        while j >= 0 {
            let tree = subtree_array_get(&trees, j as u32);
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
                root = ts_subtree_from_mut(ts_subtree_new_node(
                    ts_subtree_symbol(tree),
                    &mut trees,
                    u32::from((*tree.ptr).data.children.production_id),
                    self_.language,
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

    ts_stack_remove_version(stack, stack_slice_array_get(&pop, 0).version);
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
                        if !action.shift.extra && !action.shift.repetition => {
                            has_shift_action = true;
                        }
                    TSPARSE_ACTION_TYPE_REDUCE
                        if action.reduce.child_count > 0 => {
                            ts_reduce_action_set_add(
                                &mut self_.reduce_actions,
                                ReduceAction {
                                    symbol: action.reduce.symbol,
                                    count: u32::from(action.reduce.child_count),
                                    dynamic_precedence: i32::from(
                                        action.reduce.dynamic_precedence,
                                    ),
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
            let action = reduce_action_set_get(&self_.reduce_actions, j);
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
        let mut slice = stack_slice_array_read(&pop, i);

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

        ts_subtree_array_remove_trailing_extras(
            &mut slice.subtrees,
            &mut self_.trailing_extras,
        );

        if slice.subtrees.size > 0 {
            let error = ts_subtree_new_error_node(
                &mut slice.subtrees,
                true,
                self_.language,
            );
            ts_stack_push(stack, slice.version, error, false, goal_state);
        } else {
            array_delete(subtree_array_as_array_mut(&mut slice.subtrees));
        }

        for j in 0..self_.trailing_extras.size {
            let tree = subtree_array_get(&self_.trailing_extras, j);
            ts_stack_push(stack, slice.version, tree, false, goal_state);
        }

        previous_version = slice.version;
        i += 1;
    }

    previous_version != STACK_VERSION_NONE
}

unsafe fn ts_parser__recover(
    self_: &mut TSParser,
    version: StackVersion,
    mut lookahead: Subtree,
) {
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
            let entry = *stack_summary_array_get(summary, i);

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

            if ts_language_has_actions(
                self_.language,
                entry.state,
                ts_subtree_symbol(lookahead),
            ) && ts_parser__recover_to_state(self_, version, depth, entry.state)
            {
                did_recover = true;
                LOG!(parser, c"recover_to_previous state:%u, depth:%u".as_ptr().cast::<i8>(),
                    u32::from(entry.state), depth);
                LOG_STACK!(parser);
                break;
            }
        }
    }

    // Remove halted versions
    let mut i = previous_version_count;
    while i < ts_stack_version_count(stack) {
        if !ts_stack_is_active(stack, i) {
            LOG!(parser, c"removed paused version:%u".as_ptr().cast::<i8>(), i);
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
    let actions = ts_language_actions(
        self_.language,
        1,
        ts_subtree_symbol(lookahead),
        &mut n,
    );
    if n > 0
        && (*actions.add(n as usize - 1)).type_ == TSPARSE_ACTION_TYPE_SHIFT
        && (*actions.add(n as usize - 1)).shift.extra
    {
        let mut mutable_lookahead = ts_subtree_make_mut(&mut self_.tree_pool, lookahead);
        ts_subtree_set_extra(&mut mutable_lookahead, true);
        lookahead = ts_subtree_from_mut(mutable_lookahead);
    }

    // Wrap the lookahead in an ERROR
    LOG!(parser, c"skip_token symbol:%s".as_ptr().cast::<i8>(),
        SYM_NAME!(parser, ts_subtree_symbol(lookahead)));
    let mut children: SubtreeArray = SubtreeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    };
    array_reserve(subtree_array_as_array_mut(&mut children), 1);
    array_push(subtree_array_as_array_mut(&mut children), lookahead);
    let mut error_repeat = ts_subtree_new_node(
        ts_builtin_sym_error_repeat,
        &mut children,
        0,
        self_.language,
    );

    // Merge with existing error on top of stack
    if node_count_since_error > 0 {
        let mut pop = ts_stack_pop_count(stack, version, 1);

        if pop.size > 1 {
            for pi in 1..pop.size {
                ts_subtree_array_delete(
                    &mut self_.tree_pool,
                    &mut stack_slice_array_get_mut(&mut pop, pi).subtrees,
                );
            }
            while ts_stack_version_count(stack) > stack_slice_array_get(&pop, 0).version + 1 {
                ts_stack_remove_version(
                    stack,
                    stack_slice_array_get(&pop, 0).version + 1,
                );
            }
        }

        ts_stack_renumber_version(
            stack,
            stack_slice_array_get(&pop, 0).version,
            version,
        );
        let slot = &mut stack_slice_array_get_mut(&mut pop, 0).subtrees;
        array_push(
            subtree_array_as_array_mut(slot),
            ts_subtree_from_mut(error_repeat),
        );
        error_repeat = ts_subtree_new_node(
            ts_builtin_sym_error_repeat,
            slot,
            0,
            self_.language,
        );
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
        ts_stack_set_last_external_token(
            stack,
            version,
            ts_subtree_last_external_token(lookahead),
        );
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

unsafe fn ts_parser__handle_error(
    self_: &mut TSParser,
    version: StackVersion,
    lookahead: Subtree,
) {
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
                            c"recover_with_missing symbol:%s, state:%u".as_ptr().cast::<i8>(),
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

    ts_stack_record_summary(
        parser_stack_mut(self_.stack),
        version,
        MAX_SUMMARY_DEPTH,
    );

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
    if let Some(position) = position {
        self_.parse_state.current_byte_offset = position;
        self_.parse_state.has_error = self_.has_error;
    }
    if self_.operation_count == 0
        && self_.parse_options.progress_callback.is_some()
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

    let mut did_reuse = true;
    let mut lookahead = NULL_SUBTREE;
    let mut table_entry = TableEntry::empty();

    // If possible, reuse a node from the previous syntax tree.
    if allow_node_reuse {
        lookahead = ts_parser__reuse_node(
            self_,
            version,
            &mut state,
            position,
            last_external_token,
            &mut table_entry,
        );
    }

    // If no node from the previous syntax tree could be reused, then try to
    // reuse the token previously returned by the lexer.
    if lookahead.ptr.is_null() {
        did_reuse = false;
        if let Some((token, cached_table_entry)) = ts_parser__get_cached_token(
            self_,
            state,
            position as usize,
            last_external_token,
        ) {
            lookahead = token;
            table_entry = cached_table_entry;
        }
    }

    let mut needs_lex = lookahead.ptr.is_null();
    loop {
        // Otherwise, re-run the lexer.
        if needs_lex {
            needs_lex = false;
            lookahead = ts_parser__lex(self_, version, state);
            if self_.has_scanner_error {
                return false;
            }

            if !lookahead.ptr.is_null() {
                ts_parser__set_cached_token(self_, position, last_external_token, lookahead);
                ts_language_table_entry(
                    self_.language,
                    state,
                    ts_subtree_symbol(lookahead),
                    &mut table_entry,
                );
            }
            // When parsing a non-terminal extra, a null lookahead indicates the
            // end of the rule. The reduction is stored in the EOF table entry.
            // After the reduction, the lexer needs to be run again.
            else {
                ts_language_table_entry(
                    self_.language,
                    state,
                    ts_builtin_sym_end,
                    &mut table_entry,
                );
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
                        LOG!(parser, c"shift state:%u".as_ptr().cast::<i8>(), u32::from(next_state));
                    }

                    if ts_subtree_child_count(lookahead) > 0 {
                        ts_parser__breakdown_lookahead(self_, &mut lookahead, state);
                        let next_state = ts_language_next_state(
                            self_.language,
                            state,
                            ts_subtree_symbol(lookahead),
                        );
                        ts_parser__shift(
                            self_,
                            version,
                            next_state,
                            lookahead,
                            action.shift.extra,
                        );
                    } else {
                        ts_parser__shift(
                            self_,
                            version,
                            next_state,
                            lookahead,
                            action.shift.extra,
                        );
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
            && !ts_language_is_reserved_word(
                self_.language,
                state,
                ts_subtree_symbol(lookahead),
            )
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
                    c"switch from_keyword:%s, to_word_token:%s".as_ptr().cast::<i8>(),
                    TREE_NAME!(parser, lookahead),
                    SYM_NAME!(parser, keyword_capture_token)
                );

                let mut mutable_lookahead =
                    ts_subtree_make_mut(&mut self_.tree_pool, lookahead);
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
        LOG!(parser, c"detect_error lookahead:%s".as_ptr().cast::<i8>(), TREE_NAME!(parser, lookahead));
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
        array_clear(mutable_subtree_array_as_array_mut(&mut self_.tree_pool.tree_stack));
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

        let tree = mutable_subtree_array_back(&self_.tree_pool.tree_stack);

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
                    ts_subtree_compress(
                        tree,
                        i,
                        self_.language,
                        std::ptr::addr_of_mut!(self_.tree_pool.tree_stack),
                    );

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

        array_pop(mutable_subtree_array_as_array_mut(&mut self_.tree_pool.tree_stack));

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

// ---------------------------------------------------------------------------
// Exported functions — lifecycle
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_parser_new() -> *mut TSParser {
    let self_ = ts_calloc(1, core::mem::size_of::<TSParser>()).cast::<TSParser>();
    let parser = parser_ptr_mut(self_);
    ts_lexer_init(&mut parser.lexer);
    array_init(&mut parser.reduce_actions);
    array_reserve(std::ptr::addr_of_mut!(parser.reduce_actions), 4);
    parser.tree_pool = ts_subtree_pool_new(32);
    parser.stack = ts_stack_new(&mut parser.tree_pool);
    parser.finished_tree = NULL_SUBTREE;
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
    let parser = parser_ptr_mut(self_);
    ts_stack_delete(parser_stack_mut(parser.stack));
    if !parser.reduce_actions.contents.is_null() {
        array_delete(std::ptr::addr_of_mut!(parser.reduce_actions));
    }
    if !parser.included_range_differences.contents.is_null() {
        array_delete(ts_range_array_as_array_mut(&mut parser.included_range_differences));
    }
    if !parser.old_tree.ptr.is_null() {
        ts_subtree_release(&mut parser.tree_pool, parser.old_tree);
        parser.old_tree = NULL_SUBTREE;
    }
    ts_wasm_store_delete(parser.wasm_store);
    ts_lexer_delete(&mut parser.lexer);
    ts_parser__set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    ts_subtree_pool_delete(&mut parser.tree_pool);
    reusable_node_delete(&mut parser.reusable_node);
    array_delete(subtree_array_as_array_mut(&mut parser.trailing_extras));
    array_delete(subtree_array_as_array_mut(&mut parser.trailing_extras2));
    array_delete(subtree_array_as_array_mut(&mut parser.scratch_trees));
    ts_free(self_.cast::<c_void>());
}

// ---------------------------------------------------------------------------
// Exported functions — configuration
// ---------------------------------------------------------------------------

#[no_mangle]
pub const unsafe extern "C" fn ts_parser_language(self_: *const TSParser) -> *const TSLanguage {
    let parser = parser_ptr_ref(self_);
    parser.language
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_language(
    self_: *mut TSParser,
    language: *const TSLanguage,
) -> bool {
    ts_parser_reset(self_);
    let parser = parser_ptr_mut(self_);
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
pub const unsafe extern "C" fn ts_parser_logger(self_: *const TSParser) -> TSLogger {
    let parser = parser_ptr_ref(self_);
    ts_logger_read_ref(&parser.lexer.logger)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_logger(
    self_: *mut TSParser,
    logger: TSLogger,
) {
    let parser = parser_ptr_mut(self_);
    parser.lexer.logger = logger;
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_print_dot_graphs(
    self_: *mut TSParser,
    fd: i32,
) {
    let parser = parser_ptr_mut(self_);
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
    let parser = parser_ptr_mut(self_);
    ts_lexer_set_included_ranges(&mut parser.lexer, ranges, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_included_ranges(
    self_: *const TSParser,
    count: *mut u32,
) -> *const TSRange {
    let parser = parser_ptr_ref(self_);
    ts_lexer_included_ranges(&parser.lexer, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_reset(self_: *mut TSParser) {
    let parser = parser_ptr_mut(self_);
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
    ts_parser__set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    if !parser.finished_tree.ptr.is_null() {
        ts_subtree_release(&mut parser.tree_pool, parser.finished_tree);
        parser.finished_tree = NULL_SUBTREE;
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
pub unsafe extern "C" fn ts_parser_parse(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    input: TSInput,
) -> *mut TSTree {
    let parser = parser_ptr_mut(self_);
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
    array_clear(ts_range_array_as_array_mut(&mut parser.included_range_differences));
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

            result = ts_tree_new(
                parser.finished_tree,
                parser.language,
                parser.lexer.included_ranges,
                parser.lexer.included_range_count,
            );
            parser.finished_tree = NULL_SUBTREE;

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

        if let Some(old_tree) = old_tree.as_ref() {
            ts_subtree_retain(old_tree.root);
            parser.old_tree = old_tree.root;
            ts_range_array_get_changed_ranges(
                old_tree.included_ranges,
                old_tree.included_range_count,
                parser.lexer.included_ranges,
                parser.lexer.included_range_count,
                &mut parser.included_range_differences,
            );
            reusable_node_reset(&mut parser.reusable_node, old_tree.root);
            LOG!(self_, c"parse_after_edit".as_ptr().cast::<i8>());
            LOG_TREE!(self_, parser.old_tree);
            for i in 0..parser.included_range_differences.size {
                let range = ts_range_array_get(&parser.included_range_differences, i);
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
                    c"process version:%u, version_count:%u, state:%d, row:%u, col:%u".as_ptr().cast::<i8>(),
                    version,
                    ts_stack_version_count(parser_stack_ref(parser.stack)),
                    i32::from(ts_stack_state(parser_stack_ref(parser.stack), version)),
                    ts_stack_position(parser_stack_ref(parser.stack), version).extent.row,
                    ts_stack_position(parser_stack_ref(parser.stack), version).extent.column
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

        while parser.included_range_difference_index < parser.included_range_differences.size
        {
            let range = ts_range_array_get(
                &parser.included_range_differences,
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

    result = ts_tree_new(
        parser.finished_tree,
        parser.language,
        parser.lexer.included_ranges,
        parser.lexer.included_range_count,
    );
    parser.finished_tree = NULL_SUBTREE;

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
        let parser = parser_ptr_mut(self_);
        parser.parse_options = parse_options;
        parser.parse_state.payload = parse_options.payload;
    }
    let result = ts_parser_parse(self_, old_tree, input);
    // Reset parser options before further parse calls.
    let parser = parser_ptr_mut(self_);
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
pub unsafe extern "C" fn ts_parser_set_wasm_store(
    self_: *mut TSParser,
    store: *mut TSWasmStore,
) {
    let parser = parser_ptr_ref(self_);
    if !parser.language.is_null() && ts_language_is_wasm(parser.language) {
        // Copy the assigned language into the new store.
        let copy = ts_language_copy(parser.language);
        ts_parser_set_language(self_, copy);
        ts_language_delete(copy);
    }

    let parser = parser_ptr_mut(self_);
    ts_wasm_store_delete(parser.wasm_store);
    parser.wasm_store = store;
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_take_wasm_store(
    self_: *mut TSParser,
) -> *mut TSWasmStore {
    let parser = parser_ptr_ref(self_);
    if !parser.language.is_null() && ts_language_is_wasm(parser.language) {
        ts_parser_set_language(self_, ptr::null());
    }

    let parser = parser_ptr_mut(self_);
    let result = parser.wasm_store;
    parser.wasm_store = ptr::null_mut();
    result
}
