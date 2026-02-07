#![allow(dead_code)]
#![allow(non_snake_case)]

use core::ffi::c_void;
use std::ptr;

use crate::ffi::{
    TSInput, TSInputEncoding, TSInputEncodingUTF8, TSLanguage, TSLogger, TSLogType,
    TSLogTypeParse, TSParseOptions, TSParseState, TSPoint, TSRange, TSStateId, TSSymbol,
    TSTree as FfiTSTree, TSWasmStore,
};

use super::alloc::{ts_calloc, ts_free, ts_malloc, ts_realloc};
use super::error_costs::{
    ERROR_COST_PER_RECOVERY, ERROR_COST_PER_SKIPPED_CHAR, ERROR_COST_PER_SKIPPED_LINE,
    ERROR_COST_PER_SKIPPED_TREE, ERROR_STATE,
};
use super::get_changed_ranges::TSRangeArray;
use super::language::{
    ts_language_actions, ts_language_alias_sequence, ts_language_enabled_external_tokens,
    ts_language_field_map, ts_language_has_actions, ts_language_has_reduce_action,
    TableEntry, TSLanguageFull, TSLexer, TSLexerMode, TSLexMode,
    TSParseAction, TSParseActionReduce, TSParseActionShift,
    TSParseActionTypeAccept, TSParseActionTypeRecover, TSParseActionTypeReduce,
    TSParseActionTypeShift,
};
use super::length::{length_add, length_sub, length_zero, Length};
use super::lexer::{ColumnData, Lexer};
use super::stack::{
    array_assign, array_back, array_clear, array_delete, array_erase, array_front,
    array_get, array_grow, array_init, array_new, array_pop, array_push, array_reserve,
    array_splice, array_swap, Array, Stack, StackSlice, StackSliceArray, StackSummary,
    StackSummaryEntry, StackVersion, STACK_VERSION_NONE,
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
    ts_subtree_lookahead_bytes, ts_subtree_missing, ts_subtree_padding,
    ts_subtree_parse_state, ts_subtree_repeat_depth, ts_subtree_set_extra,
    ts_subtree_size, ts_subtree_symbol, ts_subtree_to_mut_unsafe,
    ts_subtree_total_bytes, ts_subtree_total_size, ts_subtree_visible,
    ExternalScannerState, MutableSubtree, MutableSubtreeArray, Subtree,
    SubtreeArray, SubtreePool, NULL_SUBTREE, TS_TREE_STATE_NONE,
};
use super::tree::TSTree;

// ---------------------------------------------------------------------------
// Extern C functions (exported from other Rust modules)
// ---------------------------------------------------------------------------

extern "C" {
    // subtree.rs
    fn ts_subtree_pool_new(size: u32) -> SubtreePool;
    fn ts_subtree_pool_delete(self_: *mut SubtreePool);
    fn ts_subtree_retain(self_: Subtree);
    fn ts_subtree_release(pool: *mut SubtreePool, self_: Subtree);
    fn ts_subtree_new_leaf(
        pool: *mut SubtreePool,
        symbol: TSSymbol,
        padding: Length,
        size: Length,
        lookahead_bytes: u32,
        parse_state: TSStateId,
        has_external_tokens: bool,
        depends_on_column: bool,
        is_keyword: bool,
        language: *const TSLanguage,
    ) -> Subtree;
    fn ts_subtree_new_node(
        symbol: TSSymbol,
        children: *mut SubtreeArray,
        production_id: u32,
        language: *const TSLanguage,
    ) -> MutableSubtree;
    fn ts_subtree_new_error_node(
        children: *mut SubtreeArray,
        extra: bool,
        language: *const TSLanguage,
    ) -> Subtree;
    fn ts_subtree_new_missing_leaf(
        pool: *mut SubtreePool,
        symbol: TSSymbol,
        padding: Length,
        lookahead_bytes: u32,
        language: *const TSLanguage,
    ) -> Subtree;
    fn ts_subtree_edit(
        self_: Subtree,
        edit: *const crate::ffi::TSInputEdit,
        pool: *mut SubtreePool,
    ) -> Subtree;
    fn ts_subtree_compare(a: Subtree, b: Subtree, pool: *mut SubtreePool) -> i32;
    fn ts_subtree_last_external_token(self_: Subtree) -> Subtree;
    fn ts_subtree_external_scanner_state_eq(a: Subtree, b: Subtree) -> bool;
    fn ts_subtree_print_dot_graph(
        self_: Subtree,
        language: *const TSLanguage,
        f: *mut c_void,
    );
    fn ts_subtree_make_mut(
        pool: *mut SubtreePool,
        self_: Subtree,
    ) -> MutableSubtree;
    fn ts_subtree_set_symbol(
        self_: *mut MutableSubtree,
        symbol: TSSymbol,
        language: *const TSLanguage,
    );
    fn ts_subtree_compress(
        self_: MutableSubtree,
        count: u32,
        language: *const TSLanguage,
        stack: *mut Array<MutableSubtree>,
    );
    fn ts_subtree_new_error(
        pool: *mut SubtreePool,
        character: i32,
        padding: Length,
        size: Length,
        lookahead_bytes: u32,
        parse_state: TSStateId,
        language: *const TSLanguage,
    ) -> Subtree;
    fn ts_subtree_external_scanner_state(self_: Subtree) -> *const ExternalScannerState;
    fn ts_subtree_array_clear(pool: *mut SubtreePool, self_: *mut SubtreeArray);
    fn ts_subtree_array_delete(pool: *mut SubtreePool, self_: *mut SubtreeArray);
    fn ts_subtree_array_remove_trailing_extras(
        self_: *mut SubtreeArray,
        extras: *mut SubtreeArray,
    );
    fn ts_external_scanner_state_data(self_: *const ExternalScannerState) -> *const i8;
    fn ts_external_scanner_state_init(
        self_: *mut ExternalScannerState,
        data: *const i8,
        length: u32,
    );
    fn ts_external_scanner_state_eq(
        self_: *const ExternalScannerState,
        other_data: *const i8,
        other_length: u32,
    ) -> bool;

    // lexer.rs
    fn ts_lexer_init(self_: *mut Lexer);
    fn ts_lexer_set_input(self_: *mut Lexer, input: TSInput);
    fn ts_lexer_start(self_: *mut Lexer);
    fn ts_lexer_finish(self_: *mut Lexer, lookahead_end_byte: *mut u32);
    fn ts_lexer_advance_to_end(self_: *mut Lexer);
    fn ts_lexer_mark_end(self_: *mut Lexer);
    fn ts_lexer_included_ranges(
        self_: *const Lexer,
        count: *mut u32,
    ) -> *const TSRange;
    fn ts_lexer_set_included_ranges(
        self_: *mut Lexer,
        ranges: *const TSRange,
        count: u32,
    ) -> bool;
    fn ts_lexer_reset(self_: *mut Lexer, position: Length);
    fn ts_lexer_delete(self_: *mut Lexer);

    // language.rs
    fn ts_language_next_state(
        self_: *const TSLanguage,
        state: TSStateId,
        symbol: TSSymbol,
    ) -> TSStateId;
    fn ts_language_symbol_name(
        self_: *const TSLanguage,
        symbol: TSSymbol,
    ) -> *const i8;
    fn ts_language_version(self_: *const TSLanguage) -> u32;
    fn ts_language_is_wasm(self_: *const TSLanguage) -> bool;
    fn ts_language_copy(self_: *const TSLanguage) -> *const TSLanguage;
    fn ts_language_delete(self_: *const TSLanguage);
    fn ts_language_table_entry(
        self_: *const TSLanguage,
        state: TSStateId,
        symbol: TSSymbol,
        result: *mut TableEntry,
    );
    fn ts_language_lex_mode_for_state(
        self_: *const TSLanguage,
        state: TSStateId,
    ) -> TSLexerMode;
    fn ts_language_is_reserved_word(
        self_: *const TSLanguage,
        state: TSStateId,
        symbol: TSSymbol,
    ) -> bool;

    // tree.rs
    fn ts_tree_new(
        root: Subtree,
        language: *const TSLanguage,
        included_ranges: *const TSRange,
        included_range_count: u32,
    ) -> *mut TSTree;

    // get_changed_ranges.rs
    fn ts_range_array_intersects(
        self_: *const TSRangeArray,
        start_index: u32,
        start_byte: u32,
        end_byte: u32,
    ) -> bool;
    fn ts_range_array_get_changed_ranges(
        old_ranges: *const TSRange,
        old_range_count: u32,
        new_ranges: *const TSRange,
        new_range_count: u32,
        differences: *mut TSRangeArray,
    );

    // wasm_store.c (still in C)
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
        if (*$self_).lexer.logger.log.is_some() || !(*$self_).dot_graph_file.is_null() {
            snprintf(
                (*$self_).lexer.debug_buffer.as_mut_ptr() as *mut i8,
                TREE_SITTER_SERIALIZATION_BUFFER_SIZE,
                $($arg),+
            );
            ts_parser__log($self_);
        }
    };
}

macro_rules! LOG_STACK {
    ($self_:expr) => {
        if !(*$self_).dot_graph_file.is_null() {
            ts_stack_print_dot_graph((*$self_).stack, (*$self_).language, (*$self_).dot_graph_file);
            fputs(b"\n\n\0".as_ptr() as *const i8, (*$self_).dot_graph_file);
        }
    };
}

macro_rules! LOG_TREE {
    ($self_:expr, $tree:expr) => {
        if !(*$self_).dot_graph_file.is_null() {
            ts_subtree_print_dot_graph($tree, (*$self_).language, (*$self_).dot_graph_file);
            fputs(b"\n\0".as_ptr() as *const i8, (*$self_).dot_graph_file);
        }
    };
}

macro_rules! LOG_LOOKAHEAD {
    ($self_:expr, $symbol_name:expr, $size:expr) => {
        if (*$self_).lexer.logger.log.is_some() || !(*$self_).dot_graph_file.is_null() {
            let buf = (*$self_).lexer.debug_buffer.as_mut_ptr() as *mut i8;
            let symbol = $symbol_name;
            let mut off = snprintf(
                buf,
                TREE_SITTER_SERIALIZATION_BUFFER_SIZE,
                b"lexed_lookahead sym:\0".as_ptr() as *const i8,
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
                b", size:%u\0".as_ptr() as *const i8,
                $size,
            );
            ts_parser__log($self_);
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

/// ReduceAction — from reduce_action.h
#[repr(C)]
#[derive(Clone, Copy)]
struct ReduceAction {
    count: u32,
    symbol: TSSymbol,
    dynamic_precedence: i32,
    production_id: u16,
}

/// ReduceActionSet — Array(ReduceAction)
type ReduceActionSet = Array<ReduceAction>;

/// StackEntry — for ReusableNode (from reusable_node.h)
#[repr(C)]
#[derive(Clone, Copy)]
struct StackEntry {
    tree: Subtree,
    child_index: u32,
    byte_offset: u32,
}

/// ReusableNode — for incremental reparsing (from reusable_node.h)
#[repr(C)]
struct ReusableNode {
    stack: Array<StackEntry>,
    last_external_token: Subtree,
}

/// TokenCache — cached lookahead token
#[repr(C)]
struct TokenCache {
    token: Subtree,
    last_external_token: Subtree,
    byte_index: u32,
}

/// ErrorStatus — for comparing parse versions
#[repr(C)]
#[derive(Clone, Copy)]
struct ErrorStatus {
    cost: u32,
    node_count: u32,
    dynamic_precedence: i32,
    is_in_error: bool,
}

/// ErrorComparison
#[derive(PartialEq, Eq)]
enum ErrorComparison {
    TakeLeft,
    PreferLeft,
    None,
    PreferRight,
    TakeRight,
}

/// TSStringInput — for string-based parsing
#[repr(C)]
struct TSStringInput {
    string: *const i8,
    length: u32,
}

/// TSParser — the main parser struct
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

// ---------------------------------------------------------------------------
// ReusableNode inline helpers (from reusable_node.h)
// ---------------------------------------------------------------------------

unsafe fn reusable_node_new() -> ReusableNode {
    ReusableNode {
        stack: array_new(),
        last_external_token: NULL_SUBTREE,
    }
}

unsafe fn reusable_node_clear(self_: *mut ReusableNode) {
    array_clear(&mut (*self_).stack);
    (*self_).last_external_token = NULL_SUBTREE;
}

unsafe fn reusable_node_tree(self_: *mut ReusableNode) -> Subtree {
    if (*self_).stack.size > 0 {
        (*array_back(&(*self_).stack)).tree
    } else {
        NULL_SUBTREE
    }
}

unsafe fn reusable_node_byte_offset(self_: *mut ReusableNode) -> u32 {
    if (*self_).stack.size > 0 {
        (*array_back(&(*self_).stack)).byte_offset
    } else {
        u32::MAX
    }
}

unsafe fn reusable_node_delete(self_: *mut ReusableNode) {
    array_delete(&mut (*self_).stack);
}

unsafe fn reusable_node_advance(self_: *mut ReusableNode) {
    let last_entry = *array_back(&(*self_).stack);
    let byte_offset = last_entry.byte_offset + ts_subtree_total_bytes(last_entry.tree);
    if ts_subtree_has_external_tokens(last_entry.tree) {
        (*self_).last_external_token = ts_subtree_last_external_token(last_entry.tree);
    }

    let mut tree;
    let mut next_index;
    loop {
        let popped_entry = array_pop(&mut (*self_).stack);
        next_index = popped_entry.child_index + 1;
        if (*self_).stack.size == 0 {
            return;
        }
        tree = (*array_back(&(*self_).stack)).tree;
        if ts_subtree_child_count(tree) > next_index {
            break;
        }
    }

    array_push(&mut (*self_).stack, StackEntry {
        tree: *ts_subtree_children(tree).add(next_index as usize),
        child_index: next_index,
        byte_offset,
    });
}

unsafe fn reusable_node_descend(self_: *mut ReusableNode) -> bool {
    let last_entry = *array_back(&(*self_).stack);
    if ts_subtree_child_count(last_entry.tree) > 0 {
        array_push(&mut (*self_).stack, StackEntry {
            tree: *ts_subtree_children(last_entry.tree),
            child_index: 0,
            byte_offset: last_entry.byte_offset,
        });
        true
    } else {
        false
    }
}

unsafe fn reusable_node_advance_past_leaf(self_: *mut ReusableNode) {
    while reusable_node_descend(self_) {}
    reusable_node_advance(self_);
}

unsafe fn reusable_node_reset(self_: *mut ReusableNode, tree: Subtree) {
    reusable_node_clear(self_);
    array_push(&mut (*self_).stack, StackEntry {
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

unsafe fn ts_reduce_action_set_add(
    self_: *mut ReduceActionSet,
    new_action: ReduceAction,
) {
    for i in 0..(*self_).size {
        let action = *array_get(self_ as *const Array<ReduceAction>, i);
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
    let self_ = payload as *const TSStringInput;
    if byte >= (*self_).length {
        *length = 0;
        b"\0".as_ptr() as *const i8
    } else {
        *length = (*self_).length - byte;
        (*self_).string.add(byte as usize)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — logging & breakdown
// ---------------------------------------------------------------------------

unsafe fn ts_parser__log(self_: *mut TSParser) {
    if let Some(log_fn) = (*self_).lexer.logger.log {
        log_fn(
            (*self_).lexer.logger.payload,
            TSLogTypeParse,
            (*self_).lexer.debug_buffer.as_ptr() as *const i8,
        );
    }

    if !(*self_).dot_graph_file.is_null() {
        fprintf((*self_).dot_graph_file, b"graph {\nlabel=\"\0".as_ptr() as *const i8);
        let mut chr = (*self_).lexer.debug_buffer.as_ptr();
        while *chr != 0 {
            if *chr == b'"' || *chr == b'\\' {
                fputc(b'\\' as i32, (*self_).dot_graph_file);
            }
            fputc(*chr as i32, (*self_).dot_graph_file);
            chr = chr.add(1);
        }
        fprintf((*self_).dot_graph_file, b"\"\n}\n\n\0".as_ptr() as *const i8);
    }
}

unsafe fn ts_parser__breakdown_top_of_stack(
    self_: *mut TSParser,
    version: StackVersion,
) -> bool {
    let mut did_break_down = false;
    let mut pending = false;

    loop {
        let pop = ts_stack_pop_pending((*self_).stack, version);
        if pop.size == 0 {
            break;
        }

        did_break_down = true;
        pending = false;
        for i in 0..pop.size {
            let mut slice = ptr::read(array_get(&pop as *const StackSliceArray, i));
            let mut state = ts_stack_state((*self_).stack, slice.version);
            let parent = *slice.subtrees.contents;

            let n = ts_subtree_child_count(parent);
            for j in 0..n {
                let child = *ts_subtree_children(parent).add(j as usize);
                pending = ts_subtree_child_count(child) > 0;

                if ts_subtree_is_error(child) {
                    state = ERROR_STATE;
                } else if !ts_subtree_extra(child) {
                    state = ts_language_next_state((*self_).language, state, ts_subtree_symbol(child));
                }

                ts_subtree_retain(child);
                ts_stack_push((*self_).stack, slice.version, child, pending, state);
            }

            for j in 1..slice.subtrees.size {
                let tree = *slice.subtrees.contents.add(j as usize);
                ts_stack_push((*self_).stack, slice.version, tree, false, state);
            }

            ts_subtree_release(&mut (*self_).tree_pool, parent);
            array_delete(&mut slice.subtrees as *mut SubtreeArray as *mut Array<Subtree>);

            LOG!(self_, b"breakdown_top_of_stack tree:%s\0".as_ptr() as *const i8,
                SYM_NAME!(self_, ts_subtree_symbol(parent)));
            LOG_STACK!(self_);
        }

        if !pending {
            break;
        }
    }

    did_break_down
}

unsafe fn ts_parser__breakdown_lookahead(
    self_: *mut TSParser,
    lookahead: *mut Subtree,
    state: TSStateId,
    reusable_node: *mut ReusableNode,
) {
    let mut did_descend = false;
    let mut tree = reusable_node_tree(reusable_node);
    while ts_subtree_child_count(tree) > 0 && ts_subtree_parse_state(tree) != state {
        LOG!(self_, b"state_mismatch sym:%s\0".as_ptr() as *const i8,
            SYM_NAME!(self_, ts_subtree_symbol(tree)));
        reusable_node_descend(reusable_node);
        tree = reusable_node_tree(reusable_node);
        did_descend = true;
    }

    if did_descend {
        ts_subtree_release(&mut (*self_).tree_pool, *lookahead);
        *lookahead = tree;
        ts_subtree_retain(*lookahead);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — version comparison
// ---------------------------------------------------------------------------

unsafe fn ts_parser__compare_versions(
    _self_: *mut TSParser,
    a: ErrorStatus,
    b: ErrorStatus,
) -> ErrorComparison {
    if !a.is_in_error && b.is_in_error {
        if a.cost < b.cost {
            return ErrorComparison::TakeLeft;
        } else {
            return ErrorComparison::PreferLeft;
        }
    }

    if a.is_in_error && !b.is_in_error {
        if b.cost < a.cost {
            return ErrorComparison::TakeRight;
        } else {
            return ErrorComparison::PreferRight;
        }
    }

    if a.cost < b.cost {
        if (b.cost - a.cost) * (1 + a.node_count) > MAX_COST_DIFFERENCE {
            return ErrorComparison::TakeLeft;
        } else {
            return ErrorComparison::PreferLeft;
        }
    }

    if b.cost < a.cost {
        if (a.cost - b.cost) * (1 + b.node_count) > MAX_COST_DIFFERENCE {
            return ErrorComparison::TakeRight;
        } else {
            return ErrorComparison::PreferRight;
        }
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
    self_: *mut TSParser,
    version: StackVersion,
) -> ErrorStatus {
    let mut cost = ts_stack_error_cost((*self_).stack, version);
    let is_paused = ts_stack_is_paused((*self_).stack, version);
    if is_paused {
        cost += ERROR_COST_PER_SKIPPED_TREE;
    }
    ErrorStatus {
        cost,
        node_count: ts_stack_node_count_since_error((*self_).stack, version),
        dynamic_precedence: ts_stack_dynamic_precedence((*self_).stack, version),
        is_in_error: is_paused || ts_stack_state((*self_).stack, version) == ERROR_STATE,
    }
}

unsafe fn ts_parser__better_version_exists(
    self_: *mut TSParser,
    version: StackVersion,
    is_in_error: bool,
    cost: u32,
) -> bool {
    if !(*self_).finished_tree.ptr.is_null()
        && ts_subtree_error_cost((*self_).finished_tree) <= cost
    {
        return true;
    }

    let position = ts_stack_position((*self_).stack, version);
    let status = ErrorStatus {
        cost,
        is_in_error,
        dynamic_precedence: ts_stack_dynamic_precedence((*self_).stack, version),
        node_count: ts_stack_node_count_since_error((*self_).stack, version),
    };

    let n = ts_stack_version_count((*self_).stack);
    for i in 0..n {
        if i == version
            || !ts_stack_is_active((*self_).stack, i)
            || ts_stack_position((*self_).stack, i).bytes < position.bytes
        {
            continue;
        }
        let status_i = ts_parser__version_status(self_, i);
        match ts_parser__compare_versions(self_, status, status_i) {
            ErrorComparison::TakeRight => return true,
            ErrorComparison::PreferRight => {
                if ts_stack_can_merge((*self_).stack, i, version) {
                    return true;
                }
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
    self_: *mut TSParser,
    lex_mode: TSLexerMode,
) -> bool {
    if ts_language_is_wasm((*self_).language) {
        ts_wasm_store_call_lex_main((*self_).wasm_store, lex_mode.lex_state)
    } else {
        let lang = (*self_).language as *const TSLanguageFull;
        ((*lang).lex_fn.unwrap())(&mut (*self_).lexer.data, lex_mode.lex_state)
    }
}

unsafe fn ts_parser__call_keyword_lex_fn(self_: *mut TSParser) -> bool {
    if ts_language_is_wasm((*self_).language) {
        ts_wasm_store_call_lex_keyword((*self_).wasm_store, 0)
    } else {
        let lang = (*self_).language as *const TSLanguageFull;
        ((*lang).keyword_lex_fn.unwrap())(&mut (*self_).lexer.data, 0)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — external scanner
// ---------------------------------------------------------------------------

unsafe fn ts_parser__external_scanner_create(self_: *mut TSParser) {
    let lang = (*self_).language as *const TSLanguageFull;
    if !(*self_).language.is_null() && !(*lang).external_scanner.states.is_null() {
        if ts_language_is_wasm((*self_).language) {
            (*self_).external_scanner_payload =
                ts_wasm_store_call_scanner_create((*self_).wasm_store) as usize as *mut c_void;
            if ts_wasm_store_has_error((*self_).wasm_store) {
                (*self_).has_scanner_error = true;
            }
        } else if let Some(create_fn) = (*lang).external_scanner.create {
            (*self_).external_scanner_payload = create_fn();
        }
    }
}

unsafe fn ts_parser__external_scanner_destroy(self_: *mut TSParser) {
    let lang = (*self_).language as *const TSLanguageFull;
    if !(*self_).language.is_null()
        && !(*self_).external_scanner_payload.is_null()
        && (*lang).external_scanner.destroy.is_some()
        && !ts_language_is_wasm((*self_).language)
    {
        ((*lang).external_scanner.destroy.unwrap())((*self_).external_scanner_payload);
    }
    (*self_).external_scanner_payload = ptr::null_mut();
}

unsafe fn ts_parser__external_scanner_serialize(self_: *mut TSParser) -> u32 {
    let lang = (*self_).language as *const TSLanguageFull;
    let length;
    if ts_language_is_wasm((*self_).language) {
        length = ts_wasm_store_call_scanner_serialize(
            (*self_).wasm_store,
            (*self_).external_scanner_payload as usize as u32,
            (*self_).lexer.debug_buffer.as_mut_ptr() as *mut i8,
        );
        if ts_wasm_store_has_error((*self_).wasm_store) {
            (*self_).has_scanner_error = true;
        }
    } else {
        length = ((*lang).external_scanner.serialize.unwrap())(
            (*self_).external_scanner_payload,
            (*self_).lexer.debug_buffer.as_mut_ptr() as *mut i8,
        );
    }
    debug_assert!(length as usize <= TREE_SITTER_SERIALIZATION_BUFFER_SIZE);
    length
}

unsafe fn ts_parser__external_scanner_deserialize(
    self_: *mut TSParser,
    external_token: Subtree,
) {
    let lang = (*self_).language as *const TSLanguageFull;
    let mut data: *const i8 = ptr::null();
    let mut length: u32 = 0;
    if !external_token.ptr.is_null() {
        let state = ts_subtree_external_scanner_state(external_token);
        data = ts_external_scanner_state_data(state);
        length = (*state).length;
    }

    if ts_language_is_wasm((*self_).language) {
        ts_wasm_store_call_scanner_deserialize(
            (*self_).wasm_store,
            (*self_).external_scanner_payload as usize as u32,
            data,
            length,
        );
        if ts_wasm_store_has_error((*self_).wasm_store) {
            (*self_).has_scanner_error = true;
        }
    } else {
        ((*lang).external_scanner.deserialize.unwrap())(
            (*self_).external_scanner_payload,
            data,
            length,
        );
    }
}

unsafe fn ts_parser__external_scanner_scan(
    self_: *mut TSParser,
    external_lex_state: TSStateId,
) -> bool {
    let lang = (*self_).language as *const TSLanguageFull;
    if ts_language_is_wasm((*self_).language) {
        let result = ts_wasm_store_call_scanner_scan(
            (*self_).wasm_store,
            (*self_).external_scanner_payload as usize as u32,
            external_lex_state as u32 * (*lang).external_token_count,
        );
        if ts_wasm_store_has_error((*self_).wasm_store) {
            (*self_).has_scanner_error = true;
        }
        result
    } else {
        let valid_external_tokens = ts_language_enabled_external_tokens(
            (*self_).language,
            external_lex_state as u32,
        );
        ((*lang).external_scanner.scan.unwrap())(
            (*self_).external_scanner_payload,
            &mut (*self_).lexer.data,
            valid_external_tokens,
        )
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — token reuse & lexing
// ---------------------------------------------------------------------------

unsafe fn ts_parser__can_reuse_first_leaf(
    self_: *mut TSParser,
    state: TSStateId,
    tree: Subtree,
    table_entry: *mut TableEntry,
) -> bool {
    let lang = (*self_).language as *const TSLanguageFull;
    let leaf_symbol = ts_subtree_leaf_symbol(tree);
    let leaf_state = ts_subtree_leaf_parse_state(tree);
    let current_lex_mode = ts_language_lex_mode_for_state((*self_).language, state);
    let leaf_lex_mode = ts_language_lex_mode_for_state((*self_).language, leaf_state);

    // At the end of a non-terminal extra node, the lexer normally returns
    // NULL, which indicates that the parser should look for a reduce action
    // at symbol `0`. Avoid reusing tokens in this situation.
    if current_lex_mode.lex_state == u16::MAX {
        return false;
    }

    // If the token was created in a state with the same set of lookaheads, it is reusable.
    if (*table_entry).action_count > 0
        && memcmp(
            &leaf_lex_mode as *const TSLexerMode as *const c_void,
            &current_lex_mode as *const TSLexerMode as *const c_void,
            core::mem::size_of::<TSLexerMode>(),
        ) == 0
        && (leaf_symbol != (*lang).keyword_capture_token
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
    current_lex_mode.external_lex_state == 0 && (*table_entry).is_reusable
}

unsafe fn ts_parser__lex(
    self_: *mut TSParser,
    version: StackVersion,
    parse_state: TSStateId,
) -> Subtree {
    let lang = (*self_).language as *const TSLanguageFull;
    let mut lex_mode = ts_language_lex_mode_for_state((*self_).language, parse_state);
    if lex_mode.lex_state == u16::MAX {
        LOG!(self_, b"no_lookahead_after_non_terminal_extra\0".as_ptr() as *const i8);
        return NULL_SUBTREE;
    }

    let start_position = ts_stack_position((*self_).stack, version);
    let external_token = ts_stack_last_external_token((*self_).stack, version);

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
    ts_lexer_reset(&mut (*self_).lexer, start_position);

    loop {
        let mut found_token;
        let current_position = (*self_).lexer.current_position;
        let column_data = (*self_).lexer.column_data;

        if lex_mode.external_lex_state != 0 {
            LOG!(self_, b"lex_external state:%d, row:%u, column:%u\0".as_ptr() as *const i8,
                lex_mode.external_lex_state as i32,
                current_position.extent.row,
                current_position.extent.column);
            ts_lexer_start(&mut (*self_).lexer);
            ts_parser__external_scanner_deserialize(self_, external_token);
            found_token = ts_parser__external_scanner_scan(self_, lex_mode.external_lex_state);
            if (*self_).has_scanner_error {
                return NULL_SUBTREE;
            }
            ts_lexer_finish(&mut (*self_).lexer, &mut lookahead_end_byte);

            if found_token {
                external_scanner_state_len = ts_parser__external_scanner_serialize(self_);
                external_scanner_state_changed = !ts_external_scanner_state_eq(
                    ts_subtree_external_scanner_state(external_token),
                    (*self_).lexer.debug_buffer.as_ptr() as *const i8,
                    external_scanner_state_len,
                );

                if (*self_).lexer.token_end_position.bytes <= current_position.bytes
                    && !external_scanner_state_changed
                {
                    let symbol = *(*lang).external_scanner.symbol_map.add(
                        (*self_).lexer.data.result_symbol as usize,
                    );
                    let next_parse_state =
                        ts_language_next_state((*self_).language, parse_state, symbol);
                    let token_is_extra = next_parse_state == parse_state;
                    if error_mode
                        || !ts_stack_has_advanced_since_error((*self_).stack, version)
                        || token_is_extra
                    {
                        LOG!(self_,
                            b"ignore_empty_external_token symbol:%s\0".as_ptr() as *const i8,
                            SYM_NAME!(self_, *(*lang).external_scanner.symbol_map.add(
                                (*self_).lexer.data.result_symbol as usize
                            )));
                        found_token = false;
                    }
                }
            }

            if found_token {
                found_external_token = true;
                called_get_column = (*self_).lexer.did_get_column;
                break;
            }

            ts_lexer_reset(&mut (*self_).lexer, current_position);
            (*self_).lexer.column_data = column_data;
        }

        LOG!(self_, b"lex_internal state:%d, row:%u, column:%u\0".as_ptr() as *const i8,
            lex_mode.lex_state as i32,
            current_position.extent.row,
            current_position.extent.column);
        ts_lexer_start(&mut (*self_).lexer);
        found_token = ts_parser__call_main_lex_fn(self_, lex_mode);
        ts_lexer_finish(&mut (*self_).lexer, &mut lookahead_end_byte);
        if found_token {
            break;
        }

        if !error_mode {
            error_mode = true;
            lex_mode = ts_language_lex_mode_for_state((*self_).language, ERROR_STATE);
            ts_lexer_reset(&mut (*self_).lexer, start_position);
            continue;
        }

        if !skipped_error {
            LOG!(self_, b"skip_unrecognized_character\0".as_ptr() as *const i8);
            skipped_error = true;
            error_start_position = (*self_).lexer.token_start_position;
            error_end_position = (*self_).lexer.token_start_position;
            first_error_character = (*self_).lexer.data.lookahead;
        }

        if (*self_).lexer.current_position.bytes == error_end_position.bytes {
            if ((*self_).lexer.data.eof.unwrap())(&(*self_).lexer.data as *const _) {
                (*self_).lexer.data.result_symbol = ts_builtin_sym_error;
                break;
            }
            ((*self_).lexer.data.advance.unwrap())(&mut (*self_).lexer.data, false);
        }

        error_end_position = (*self_).lexer.current_position;
    }

    let result;
    if skipped_error {
        let padding = length_sub(error_start_position, start_position);
        let size = length_sub(error_end_position, error_start_position);
        let lookahead_bytes = lookahead_end_byte - error_end_position.bytes;
        result = ts_subtree_new_error(
            &mut (*self_).tree_pool,
            first_error_character,
            padding,
            size,
            lookahead_bytes,
            parse_state,
            (*self_).language,
        );
    } else {
        let mut is_keyword = false;
        let mut symbol = (*self_).lexer.data.result_symbol;
        let padding = length_sub((*self_).lexer.token_start_position, start_position);
        let size = length_sub(
            (*self_).lexer.token_end_position,
            (*self_).lexer.token_start_position,
        );
        let lookahead_bytes =
            lookahead_end_byte - (*self_).lexer.token_end_position.bytes;

        if found_external_token {
            symbol = *(*lang).external_scanner.symbol_map.add(symbol as usize);
        } else if symbol == (*lang).keyword_capture_token && symbol != 0 {
            let end_byte = (*self_).lexer.token_end_position.bytes;
            ts_lexer_reset(&mut (*self_).lexer, (*self_).lexer.token_start_position);
            ts_lexer_start(&mut (*self_).lexer);

            is_keyword = ts_parser__call_keyword_lex_fn(self_);

            if is_keyword
                && (*self_).lexer.token_end_position.bytes == end_byte
                && (ts_language_has_actions(
                    (*self_).language,
                    parse_state,
                    (*self_).lexer.data.result_symbol,
                ) || ts_language_is_reserved_word(
                    (*self_).language,
                    parse_state,
                    (*self_).lexer.data.result_symbol,
                ))
            {
                symbol = (*self_).lexer.data.result_symbol;
            }
        }

        result = ts_subtree_new_leaf(
            &mut (*self_).tree_pool,
            symbol,
            padding,
            size,
            lookahead_bytes,
            parse_state,
            found_external_token,
            called_get_column,
            is_keyword,
            (*self_).language,
        );

        if found_external_token {
            let mut_result = ts_subtree_to_mut_unsafe(result);
            ts_external_scanner_state_init(
                ptr::addr_of_mut!((*mut_result.ptr).data.external_scanner_state)
                    as *mut ExternalScannerState,
                (*self_).lexer.debug_buffer.as_ptr() as *const i8,
                external_scanner_state_len,
            );
            (*mut_result.ptr).set_has_external_scanner_state_change(external_scanner_state_changed);
        }
    }

    LOG_LOOKAHEAD!(
        self_,
        SYM_NAME!(self_, ts_subtree_symbol(result)),
        ts_subtree_total_size(result).bytes
    );
    result
}

unsafe fn ts_parser__get_cached_token(
    self_: *mut TSParser,
    state: TSStateId,
    position: usize,
    last_external_token: Subtree,
    table_entry: *mut TableEntry,
) -> Subtree {
    let cache = &(*self_).token_cache;
    if !cache.token.ptr.is_null()
        && cache.byte_index == position as u32
        && ts_subtree_external_scanner_state_eq(cache.last_external_token, last_external_token)
    {
        ts_language_table_entry(
            (*self_).language,
            state,
            ts_subtree_symbol(cache.token),
            table_entry,
        );
        if ts_parser__can_reuse_first_leaf(self_, state, cache.token, table_entry) {
            ts_subtree_retain(cache.token);
            return cache.token;
        }
    }
    NULL_SUBTREE
}

unsafe fn ts_parser__set_cached_token(
    self_: *mut TSParser,
    byte_index: u32,
    last_external_token: Subtree,
    token: Subtree,
) {
    let cache = &mut (*self_).token_cache;
    if !token.ptr.is_null() {
        ts_subtree_retain(token);
    }
    if !last_external_token.ptr.is_null() {
        ts_subtree_retain(last_external_token);
    }
    if !cache.token.ptr.is_null() {
        ts_subtree_release(&mut (*self_).tree_pool, cache.token);
    }
    if !cache.last_external_token.ptr.is_null() {
        ts_subtree_release(&mut (*self_).tree_pool, cache.last_external_token);
    }
    cache.token = token;
    cache.byte_index = byte_index;
    cache.last_external_token = last_external_token;
}

unsafe fn ts_parser__has_included_range_difference(
    self_: *const TSParser,
    start_position: u32,
    end_position: u32,
) -> bool {
    ts_range_array_intersects(
        &(*self_).included_range_differences,
        (*self_).included_range_difference_index,
        start_position,
        end_position,
    )
}

unsafe fn ts_parser__reuse_node(
    self_: *mut TSParser,
    version: StackVersion,
    state: *mut TSStateId,
    position: u32,
    last_external_token: Subtree,
    table_entry: *mut TableEntry,
) -> Subtree {
    let mut result;
    loop {
        result = reusable_node_tree(&mut (*self_).reusable_node);
        if result.ptr.is_null() {
            break;
        }
        let byte_offset = reusable_node_byte_offset(&mut (*self_).reusable_node);
        let mut end_byte_offset = byte_offset + ts_subtree_total_bytes(result);

        // Do not reuse an EOF node if the included ranges array has changes
        // later on in the file.
        if ts_subtree_is_eof(result) {
            end_byte_offset = u32::MAX;
        }

        if byte_offset > position {
            LOG!(self_, b"before_reusable_node symbol:%s\0".as_ptr() as *const i8,
                SYM_NAME!(self_, ts_subtree_symbol(result)));
            break;
        }

        if byte_offset < position {
            LOG!(self_, b"past_reusable_node symbol:%s\0".as_ptr() as *const i8,
                SYM_NAME!(self_, ts_subtree_symbol(result)));
            if end_byte_offset <= position || !reusable_node_descend(&mut (*self_).reusable_node) {
                reusable_node_advance(&mut (*self_).reusable_node);
            }
            continue;
        }

        if !ts_subtree_external_scanner_state_eq(
            (*self_).reusable_node.last_external_token,
            last_external_token,
        ) {
            LOG!(self_, b"reusable_node_has_different_external_scanner_state symbol:%s\0".as_ptr() as *const i8,
                SYM_NAME!(self_, ts_subtree_symbol(result)));
            reusable_node_advance(&mut (*self_).reusable_node);
            continue;
        }

        let mut reason: *const i8 = ptr::null();
        if ts_subtree_has_changes(result) {
            reason = b"has_changes\0".as_ptr() as *const i8;
        } else if ts_subtree_is_error(result) {
            reason = b"is_error\0".as_ptr() as *const i8;
        } else if ts_subtree_missing(result) {
            reason = b"is_missing\0".as_ptr() as *const i8;
        } else if ts_subtree_is_fragile(result) {
            reason = b"is_fragile\0".as_ptr() as *const i8;
        } else if ts_parser__has_included_range_difference(self_, byte_offset, end_byte_offset) {
            reason = b"contains_different_included_range\0".as_ptr() as *const i8;
        }

        if !reason.is_null() {
            LOG!(self_, b"cant_reuse_node_%s tree:%s\0".as_ptr() as *const i8,
                reason, SYM_NAME!(self_, ts_subtree_symbol(result)));
            if !reusable_node_descend(&mut (*self_).reusable_node) {
                reusable_node_advance(&mut (*self_).reusable_node);
                ts_parser__breakdown_top_of_stack(self_, version);
                *state = ts_stack_state((*self_).stack, version);
            }
            continue;
        }

        let leaf_symbol = ts_subtree_leaf_symbol(result);
        ts_language_table_entry((*self_).language, *state, leaf_symbol, table_entry);
        if !ts_parser__can_reuse_first_leaf(self_, *state, result, table_entry) {
            LOG!(self_, b"cant_reuse_node symbol:%s, first_leaf_symbol:%s\0".as_ptr() as *const i8,
                SYM_NAME!(self_, ts_subtree_symbol(result)),
                SYM_NAME!(self_, leaf_symbol));
            reusable_node_advance_past_leaf(&mut (*self_).reusable_node);
            break;
        }

        LOG!(self_, b"reuse_node symbol:%s\0".as_ptr() as *const i8,
            SYM_NAME!(self_, ts_subtree_symbol(result)));
        ts_subtree_retain(result);
        return result;
    }

    NULL_SUBTREE
}

// ---------------------------------------------------------------------------
// Internal helpers — tree selection
// ---------------------------------------------------------------------------

unsafe fn ts_parser__select_tree(
    self_: *mut TSParser,
    left: Subtree,
    right: Subtree,
) -> bool {
    if left.ptr.is_null() {
        return true;
    }
    if right.ptr.is_null() {
        return false;
    }

    if ts_subtree_error_cost(right) < ts_subtree_error_cost(left) {
        LOG!(self_, b"select_smaller_error symbol:%s, over_symbol:%s\0".as_ptr() as *const i8,
            SYM_NAME!(self_, ts_subtree_symbol(right)),
            SYM_NAME!(self_, ts_subtree_symbol(left)));
        return true;
    }

    if ts_subtree_error_cost(left) < ts_subtree_error_cost(right) {
        LOG!(self_, b"select_smaller_error symbol:%s, over_symbol:%s\0".as_ptr() as *const i8,
            SYM_NAME!(self_, ts_subtree_symbol(left)),
            SYM_NAME!(self_, ts_subtree_symbol(right)));
        return false;
    }

    if ts_subtree_dynamic_precedence(right) > ts_subtree_dynamic_precedence(left) {
        LOG!(self_, b"select_higher_precedence symbol:%s, prec:%d, over_symbol:%s, other_prec:%d\0".as_ptr() as *const i8,
            SYM_NAME!(self_, ts_subtree_symbol(right)), ts_subtree_dynamic_precedence(right),
            SYM_NAME!(self_, ts_subtree_symbol(left)), ts_subtree_dynamic_precedence(left));
        return true;
    }

    if ts_subtree_dynamic_precedence(left) > ts_subtree_dynamic_precedence(right) {
        LOG!(self_, b"select_higher_precedence symbol:%s, prec:%d, over_symbol:%s, other_prec:%d\0".as_ptr() as *const i8,
            SYM_NAME!(self_, ts_subtree_symbol(left)), ts_subtree_dynamic_precedence(left),
            SYM_NAME!(self_, ts_subtree_symbol(right)), ts_subtree_dynamic_precedence(right));
        return false;
    }

    if ts_subtree_error_cost(left) > 0 {
        return true;
    }

    let comparison = ts_subtree_compare(left, right, &mut (*self_).tree_pool);
    match comparison {
        -1 => {
            LOG!(self_, b"select_earlier symbol:%s, over_symbol:%s\0".as_ptr() as *const i8,
                SYM_NAME!(self_, ts_subtree_symbol(left)),
                SYM_NAME!(self_, ts_subtree_symbol(right)));
            false
        }
        1 => {
            LOG!(self_, b"select_earlier symbol:%s, over_symbol:%s\0".as_ptr() as *const i8,
                SYM_NAME!(self_, ts_subtree_symbol(right)),
                SYM_NAME!(self_, ts_subtree_symbol(left)));
            true
        }
        _ => {
            LOG!(self_, b"select_existing symbol:%s, over_symbol:%s\0".as_ptr() as *const i8,
                SYM_NAME!(self_, ts_subtree_symbol(left)),
                SYM_NAME!(self_, ts_subtree_symbol(right)));
            false
        }
    }
}

unsafe fn ts_parser__select_children(
    self_: *mut TSParser,
    left: Subtree,
    children: *const SubtreeArray,
) -> bool {
    array_assign(
        &mut (*self_).scratch_trees as *mut SubtreeArray as *mut Array<Subtree>,
        children as *const Array<Subtree>,
    );

    let scratch_tree = ts_subtree_new_node(
        ts_subtree_symbol(left),
        &mut (*self_).scratch_trees,
        0,
        (*self_).language,
    );

    ts_parser__select_tree(self_, left, ts_subtree_from_mut(scratch_tree))
}

// ---------------------------------------------------------------------------
// Internal helpers — shift/reduce/accept
// ---------------------------------------------------------------------------

unsafe fn ts_parser__shift(
    self_: *mut TSParser,
    version: StackVersion,
    state: TSStateId,
    lookahead: Subtree,
    extra: bool,
) {
    let is_leaf = ts_subtree_child_count(lookahead) == 0;
    let mut subtree_to_push = lookahead;
    if extra != ts_subtree_extra(lookahead) && is_leaf {
        let mut result = ts_subtree_make_mut(&mut (*self_).tree_pool, lookahead);
        ts_subtree_set_extra(&mut result, extra);
        subtree_to_push = ts_subtree_from_mut(result);
    }

    ts_stack_push((*self_).stack, version, subtree_to_push, !is_leaf, state);
    if ts_subtree_has_external_tokens(subtree_to_push) {
        ts_stack_set_last_external_token(
            (*self_).stack,
            version,
            ts_subtree_last_external_token(subtree_to_push),
        );
    }
}

unsafe fn ts_parser__reduce(
    self_: *mut TSParser,
    version: StackVersion,
    symbol: TSSymbol,
    count: u32,
    dynamic_precedence: i32,
    production_id: u16,
    is_fragile: bool,
    end_of_non_terminal_extra: bool,
) -> StackVersion {
    let initial_version_count = ts_stack_version_count((*self_).stack);

    let pop = ts_stack_pop_count((*self_).stack, version, count);
    let mut removed_version_count: u32 = 0;
    let halted_version_count = ts_stack_halted_version_count((*self_).stack);
    let mut i: u32 = 0;
    while i < pop.size {
        let mut slice = ptr::read(array_get(&pop as *const StackSliceArray, i));
        let slice_version = slice.version - removed_version_count;

        // Limit max versions
        if slice_version > MAX_VERSION_COUNT + MAX_VERSION_COUNT_OVERFLOW + halted_version_count {
            ts_stack_remove_version((*self_).stack, slice_version);
            ts_subtree_array_delete(&mut (*self_).tree_pool, &mut slice.subtrees);
            removed_version_count += 1;
            while i + 1 < pop.size {
                LOG!(self_, b"aborting reduce with too many versions\0".as_ptr() as *const i8);
                let mut next_slice = ptr::read(array_get(&pop as *const StackSliceArray, i + 1));
                if next_slice.version != slice.version {
                    break;
                }
                ts_subtree_array_delete(&mut (*self_).tree_pool, &mut next_slice.subtrees);
                i += 1;
            }
            i += 1;
            continue;
        }

        // Remove trailing extras from children
        let mut children = slice.subtrees;
        ts_subtree_array_remove_trailing_extras(&mut children, &mut (*self_).trailing_extras);

        let mut parent = ts_subtree_new_node(
            symbol,
            &mut children,
            production_id as u32,
            (*self_).language,
        );

        // Handle merged stack versions
        while i + 1 < pop.size {
            let mut next_slice = ptr::read(array_get(&pop as *const StackSliceArray, i + 1));
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
                &mut (*self_).trailing_extras2,
            );

            if ts_parser__select_children(
                self_,
                ts_subtree_from_mut(parent),
                &next_slice_children,
            ) {
                ts_subtree_array_clear(&mut (*self_).tree_pool, &mut (*self_).trailing_extras);
                ts_subtree_release(&mut (*self_).tree_pool, ts_subtree_from_mut(parent));
                array_swap(
                    &mut (*self_).trailing_extras as *mut SubtreeArray as *mut Array<Subtree>,
                    &mut (*self_).trailing_extras2 as *mut SubtreeArray as *mut Array<Subtree>,
                );
                parent = ts_subtree_new_node(
                    symbol,
                    &mut next_slice_children,
                    production_id as u32,
                    (*self_).language,
                );
            } else {
                array_clear(
                    &mut (*self_).trailing_extras2 as *mut SubtreeArray as *mut Array<Subtree>,
                );
                // Use the original size from next_slice.subtrees to delete all subtrees
                ts_subtree_array_delete(&mut (*self_).tree_pool, &mut next_slice.subtrees);
            }
        }

        let state = ts_stack_state((*self_).stack, slice_version);
        let next_state = ts_language_next_state((*self_).language, state, symbol);
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
            (*self_).stack,
            slice_version,
            ts_subtree_from_mut(parent),
            false,
            next_state,
        );
        for j in 0..(*self_).trailing_extras.size {
            ts_stack_push(
                (*self_).stack,
                slice_version,
                *(*self_).trailing_extras.contents.add(j as usize),
                false,
                next_state,
            );
        }

        for j in 0..slice_version {
            if j == version {
                continue;
            }
            if ts_stack_merge((*self_).stack, j, slice_version) {
                removed_version_count += 1;
                break;
            }
        }

        i += 1;
    }

    if ts_stack_version_count((*self_).stack) > initial_version_count {
        initial_version_count
    } else {
        STACK_VERSION_NONE
    }
}

unsafe fn ts_parser__accept(
    self_: *mut TSParser,
    version: StackVersion,
    lookahead: Subtree,
) {
    debug_assert!(ts_subtree_is_eof(lookahead));
    ts_stack_push((*self_).stack, version, lookahead, false, 1);

    let pop = ts_stack_pop_all((*self_).stack, version);
    for i in 0..pop.size {
        let trees_ptr = &mut (*array_get(&pop as *const StackSliceArray, i)).subtrees;
        let mut trees = ptr::read(trees_ptr);

        let mut root = NULL_SUBTREE;
        let mut j = trees.size as i64 - 1;
        while j >= 0 {
            let tree = *trees.contents.add(j as usize);
            if !ts_subtree_extra(tree) {
                debug_assert!(!tree.data.is_inline());
                let child_count = ts_subtree_child_count(tree);
                let children = ts_subtree_children(tree);
                for k in 0..child_count {
                    ts_subtree_retain(*children.add(k as usize));
                }
                array_splice(
                    &mut trees as *mut SubtreeArray as *mut Array<Subtree>,
                    j as u32,
                    1,
                    child_count,
                    children,
                );
                root = ts_subtree_from_mut(ts_subtree_new_node(
                    ts_subtree_symbol(tree),
                    &mut trees,
                    (*tree.ptr).data.children.production_id as u32,
                    (*self_).language,
                ));
                ts_subtree_release(&mut (*self_).tree_pool, tree);
                break;
            }
            j -= 1;
        }

        debug_assert!(!root.ptr.is_null());
        (*self_).accept_count += 1;

        if !(*self_).finished_tree.ptr.is_null() {
            if ts_parser__select_tree(self_, (*self_).finished_tree, root) {
                ts_subtree_release(&mut (*self_).tree_pool, (*self_).finished_tree);
                (*self_).finished_tree = root;
            } else {
                ts_subtree_release(&mut (*self_).tree_pool, root);
            }
        } else {
            (*self_).finished_tree = root;
        }
    }

    ts_stack_remove_version(
        (*self_).stack,
        (*array_get(&pop as *const StackSliceArray, 0)).version,
    );
    ts_stack_halt((*self_).stack, version);
}

// ---------------------------------------------------------------------------
// Internal helpers — error recovery
// ---------------------------------------------------------------------------

unsafe fn ts_parser__do_all_potential_reductions(
    self_: *mut TSParser,
    starting_version: StackVersion,
    lookahead_symbol: TSSymbol,
) -> bool {
    let lang = (*self_).language as *const TSLanguageFull;
    let initial_version_count = ts_stack_version_count((*self_).stack);

    let mut can_shift_lookahead_symbol = false;
    let mut version = starting_version;
    let mut i: u32 = 0;
    loop {
        let version_count = ts_stack_version_count((*self_).stack);
        if version >= version_count {
            break;
        }

        let mut merged = false;
        for j in initial_version_count..version {
            if ts_stack_merge((*self_).stack, j, version) {
                merged = true;
                break;
            }
        }
        if merged {
            i += 1;
            continue;
        }

        let state = ts_stack_state((*self_).stack, version);
        let mut has_shift_action = false;
        array_clear(&mut (*self_).reduce_actions);

        let first_symbol: TSSymbol;
        let end_symbol: TSSymbol;
        if lookahead_symbol != 0 {
            first_symbol = lookahead_symbol;
            end_symbol = lookahead_symbol + 1;
        } else {
            first_symbol = 1;
            end_symbol = (*lang).token_count as TSSymbol;
        }

        let mut symbol = first_symbol;
        while symbol < end_symbol {
            let mut entry = core::mem::zeroed::<TableEntry>();
            ts_language_table_entry((*self_).language, state, symbol, &mut entry);
            for j in 0..entry.action_count {
                let action = *entry.actions.add(j as usize);
                match action.type_ {
                    TSParseActionTypeShift | TSParseActionTypeRecover => {
                        if !action.shift.extra && !action.shift.repetition {
                            has_shift_action = true;
                        }
                    }
                    TSParseActionTypeReduce => {
                        if action.reduce.child_count > 0 {
                            ts_reduce_action_set_add(
                                &mut (*self_).reduce_actions,
                                ReduceAction {
                                    symbol: action.reduce.symbol,
                                    count: action.reduce.child_count as u32,
                                    dynamic_precedence: action.reduce.dynamic_precedence as i32,
                                    production_id: action.reduce.production_id,
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
            symbol += 1;
        }

        let mut reduction_version = STACK_VERSION_NONE;
        for j in 0..(*self_).reduce_actions.size {
            let action = *array_get(&(*self_).reduce_actions as *const ReduceActionSet, j);
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
            ts_stack_renumber_version((*self_).stack, reduction_version, version);
            i += 1;
            continue;
        } else if lookahead_symbol != 0 {
            ts_stack_remove_version((*self_).stack, version);
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
    self_: *mut TSParser,
    version: StackVersion,
    depth: u32,
    goal_state: TSStateId,
) -> bool {
    let mut pop = ts_stack_pop_count((*self_).stack, version, depth);
    let mut previous_version = STACK_VERSION_NONE;

    let mut i: u32 = 0;
    while i < pop.size {
        let mut slice = ptr::read(array_get(&pop as *const StackSliceArray, i));

        if slice.version == previous_version {
            ts_subtree_array_delete(&mut (*self_).tree_pool, &mut slice.subtrees);
            array_erase(&mut pop, i);
            continue;
        }

        if ts_stack_state((*self_).stack, slice.version) != goal_state {
            ts_stack_halt((*self_).stack, slice.version);
            ts_subtree_array_delete(&mut (*self_).tree_pool, &mut slice.subtrees);
            array_erase(&mut pop, i);
            continue;
        }

        let mut error_trees = ts_stack_pop_error((*self_).stack, slice.version);
        if error_trees.size > 0 {
            debug_assert!(error_trees.size == 1);
            let error_tree = *error_trees.contents;
            let error_child_count = ts_subtree_child_count(error_tree);
            if error_child_count > 0 {
                array_splice(
                    &mut slice.subtrees as *mut SubtreeArray as *mut Array<Subtree>,
                    0,
                    0,
                    error_child_count,
                    ts_subtree_children(error_tree),
                );
                for j in 0..error_child_count {
                    ts_subtree_retain(*slice.subtrees.contents.add(j as usize));
                }
            }
            ts_subtree_array_delete(&mut (*self_).tree_pool, &mut error_trees);
        }

        ts_subtree_array_remove_trailing_extras(
            &mut slice.subtrees,
            &mut (*self_).trailing_extras,
        );

        if slice.subtrees.size > 0 {
            let error = ts_subtree_new_error_node(
                &mut slice.subtrees,
                true,
                (*self_).language,
            );
            ts_stack_push((*self_).stack, slice.version, error, false, goal_state);
        } else {
            array_delete(&mut slice.subtrees as *mut SubtreeArray as *mut Array<Subtree>);
        }

        for j in 0..(*self_).trailing_extras.size {
            let tree = *(*self_).trailing_extras.contents.add(j as usize);
            ts_stack_push((*self_).stack, slice.version, tree, false, goal_state);
        }

        previous_version = slice.version;
        i += 1;
    }

    previous_version != STACK_VERSION_NONE
}

unsafe fn ts_parser__recover(
    self_: *mut TSParser,
    version: StackVersion,
    mut lookahead: Subtree,
) {
    let mut did_recover = false;
    let previous_version_count = ts_stack_version_count((*self_).stack);
    let position = ts_stack_position((*self_).stack, version);
    let summary = ts_stack_get_summary((*self_).stack, version);
    let node_count_since_error = ts_stack_node_count_since_error((*self_).stack, version);
    let current_error_cost = ts_stack_error_cost((*self_).stack, version);

    // Strategy 1: Find a previous state where the lookahead is valid.
    if !summary.is_null() && !ts_subtree_is_error(lookahead) {
        for i in 0..(*summary).size {
            let entry = *array_get(summary as *const StackSummary, i);

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
            let mut would_merge = false;
            for j in 0..previous_version_count {
                if ts_stack_state((*self_).stack, j) == entry.state
                    && ts_stack_position((*self_).stack, j).bytes == position.bytes
                {
                    would_merge = true;
                    break;
                }
            }
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
                (*self_).language,
                entry.state,
                ts_subtree_symbol(lookahead),
            ) {
                if ts_parser__recover_to_state(self_, version, depth, entry.state) {
                    did_recover = true;
                    LOG!(self_, b"recover_to_previous state:%u, depth:%u\0".as_ptr() as *const i8,
                        entry.state as u32, depth);
                    LOG_STACK!(self_);
                    break;
                }
            }
        }
    }

    // Remove halted versions
    let mut i = previous_version_count;
    while i < ts_stack_version_count((*self_).stack) {
        if !ts_stack_is_active((*self_).stack, i) {
            LOG!(self_, b"removed paused version:%u\0".as_ptr() as *const i8, i);
            ts_stack_remove_version((*self_).stack, i);
            LOG_STACK!(self_);
        } else {
            i += 1;
        }
    }

    // EOF: wrap everything and terminate
    if ts_subtree_is_eof(lookahead) {
        LOG!(self_, b"recover_eof\0".as_ptr() as *const i8);
        let mut children: SubtreeArray = SubtreeArray {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
        };
        let parent = ts_subtree_new_error_node(&mut children, false, (*self_).language);
        ts_stack_push((*self_).stack, version, parent, false, 1);
        ts_parser__accept(self_, version, lookahead);
        return;
    }

    // Strategy 2: skip the current token
    if did_recover && ts_stack_version_count((*self_).stack) > MAX_VERSION_COUNT {
        ts_stack_halt((*self_).stack, version);
        ts_subtree_release(&mut (*self_).tree_pool, lookahead);
        return;
    }

    if did_recover && ts_subtree_has_external_scanner_state_change(lookahead) {
        ts_stack_halt((*self_).stack, version);
        ts_subtree_release(&mut (*self_).tree_pool, lookahead);
        return;
    }

    let new_cost = current_error_cost
        + ERROR_COST_PER_SKIPPED_TREE
        + ts_subtree_total_bytes(lookahead) * ERROR_COST_PER_SKIPPED_CHAR
        + ts_subtree_total_size(lookahead).extent.row * ERROR_COST_PER_SKIPPED_LINE;
    if ts_parser__better_version_exists(self_, version, false, new_cost) {
        ts_stack_halt((*self_).stack, version);
        ts_subtree_release(&mut (*self_).tree_pool, lookahead);
        return;
    }

    // Mark extra tokens
    let mut n: u32 = 0;
    let actions = ts_language_actions(
        (*self_).language,
        1,
        ts_subtree_symbol(lookahead),
        &mut n,
    );
    if n > 0
        && (*actions.add(n as usize - 1)).type_ == TSParseActionTypeShift
        && (*actions.add(n as usize - 1)).shift.extra
    {
        let mut mutable_lookahead = ts_subtree_make_mut(&mut (*self_).tree_pool, lookahead);
        ts_subtree_set_extra(&mut mutable_lookahead, true);
        lookahead = ts_subtree_from_mut(mutable_lookahead);
    }

    // Wrap the lookahead in an ERROR
    LOG!(self_, b"skip_token symbol:%s\0".as_ptr() as *const i8,
        SYM_NAME!(self_, ts_subtree_symbol(lookahead)));
    let mut children: SubtreeArray = SubtreeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    };
    array_reserve(&mut children as *mut SubtreeArray as *mut Array<Subtree>, 1);
    array_push(&mut children as *mut SubtreeArray as *mut Array<Subtree>, lookahead);
    let mut error_repeat = ts_subtree_new_node(
        ts_builtin_sym_error_repeat,
        &mut children,
        0,
        (*self_).language,
    );

    // Merge with existing error on top of stack
    if node_count_since_error > 0 {
        let pop = ts_stack_pop_count((*self_).stack, version, 1);

        if pop.size > 1 {
            for pi in 1..pop.size {
                ts_subtree_array_delete(
                    &mut (*self_).tree_pool,
                    &mut (*array_get(&pop as *const StackSliceArray, pi)).subtrees,
                );
            }
            while ts_stack_version_count((*self_).stack)
                > (*array_get(&pop as *const StackSliceArray, 0)).version + 1
            {
                ts_stack_remove_version(
                    (*self_).stack,
                    (*array_get(&pop as *const StackSliceArray, 0)).version + 1,
                );
            }
        }

        ts_stack_renumber_version(
            (*self_).stack,
            (*array_get(&pop as *const StackSliceArray, 0)).version,
            version,
        );
        let slot = &mut (*array_get(&pop as *const StackSliceArray, 0)).subtrees;
        array_push(
            slot as *mut SubtreeArray as *mut Array<Subtree>,
            ts_subtree_from_mut(error_repeat),
        );
        error_repeat = ts_subtree_new_node(
            ts_builtin_sym_error_repeat,
            slot,
            0,
            (*self_).language,
        );
    }

    // Push the ERROR
    ts_stack_push(
        (*self_).stack,
        version,
        ts_subtree_from_mut(error_repeat),
        false,
        ERROR_STATE,
    );
    if ts_subtree_has_external_tokens(lookahead) {
        ts_stack_set_last_external_token(
            (*self_).stack,
            version,
            ts_subtree_last_external_token(lookahead),
        );
    }

    let mut has_error = true;
    for vi in 0..ts_stack_version_count((*self_).stack) {
        let status = ts_parser__version_status(self_, vi);
        if !status.is_in_error {
            has_error = false;
            break;
        }
    }
    (*self_).has_error = has_error;
}

unsafe fn ts_parser__handle_error(
    self_: *mut TSParser,
    version: StackVersion,
    lookahead: Subtree,
) {
    let previous_version_count = ts_stack_version_count((*self_).stack);

    // Perform any reductions that can happen in this state, regardless of the lookahead. After
    // skipping one or more invalid tokens, the parser might find a token that would have allowed
    // a reduction to take place.
    ts_parser__do_all_potential_reductions(self_, version, 0);
    let version_count = ts_stack_version_count((*self_).stack);
    let position = ts_stack_position((*self_).stack, version);

    // Push a discontinuity onto the stack. Merge all of the stack versions that
    // were created in the previous step.
    let mut did_insert_missing_token = false;
    let mut v = version;
    while v < version_count {
        if !did_insert_missing_token {
            let state = ts_stack_state((*self_).stack, v);
            let language = (*self_).language as *const TSLanguageFull;
            let mut missing_symbol: TSSymbol = 1;
            while (missing_symbol as u32) < (*language).token_count {
                let state_after_missing_symbol =
                    ts_language_next_state((*self_).language, state, missing_symbol);
                if state_after_missing_symbol == 0 || state_after_missing_symbol == state {
                    missing_symbol += 1;
                    continue;
                }

                if ts_language_has_reduce_action(
                    (*self_).language,
                    state_after_missing_symbol,
                    ts_subtree_leaf_symbol(lookahead),
                ) {
                    // In case the parser is currently outside of any included range, the lexer will
                    // snap to the beginning of the next included range. The missing token's padding
                    // must be assigned to position it within the next included range.
                    ts_lexer_reset(&mut (*self_).lexer, position);
                    ts_lexer_mark_end(&mut (*self_).lexer);
                    let padding = length_sub((*self_).lexer.token_end_position, position);
                    let lookahead_bytes =
                        ts_subtree_total_bytes(lookahead) + ts_subtree_lookahead_bytes(lookahead);

                    let version_with_missing_tree =
                        ts_stack_copy_version((*self_).stack, v);
                    let missing_tree = ts_subtree_new_missing_leaf(
                        &mut (*self_).tree_pool,
                        missing_symbol,
                        padding,
                        lookahead_bytes,
                        (*self_).language,
                    );
                    ts_stack_push(
                        (*self_).stack,
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
                            self_,
                            b"recover_with_missing symbol:%s, state:%u\0".as_ptr() as *const i8,
                            SYM_NAME!(self_, missing_symbol),
                            ts_stack_state((*self_).stack, version_with_missing_tree) as u32
                        );
                        did_insert_missing_token = true;
                        break;
                    }
                }
                missing_symbol += 1;
            }
        }

        ts_stack_push((*self_).stack, v, NULL_SUBTREE, false, ERROR_STATE);
        v = if v == version {
            previous_version_count
        } else {
            v + 1
        };
    }

    for _i in previous_version_count..version_count {
        let did_merge = ts_stack_merge((*self_).stack, version, previous_version_count);
        debug_assert!(did_merge);
    }

    ts_stack_record_summary((*self_).stack, version, MAX_SUMMARY_DEPTH);

    // Begin recovery with the current lookahead node, rather than waiting for the
    // next turn of the parse loop. This ensures that the tree accounts for the
    // current lookahead token's "lookahead bytes" value, which describes how far
    // the lexer needed to look ahead beyond the content of the token in order to
    // recognize it.
    let mut lookahead = lookahead;
    if ts_subtree_child_count(lookahead) > 0 {
        ts_parser__breakdown_lookahead(self_, &mut lookahead, ERROR_STATE, &mut (*self_).reusable_node);
    }
    ts_parser__recover(self_, version, lookahead);

    LOG_STACK!(self_);
}

// ---------------------------------------------------------------------------
// Internal helpers — advance & condense
// ---------------------------------------------------------------------------

unsafe fn ts_parser__check_progress(
    self_: *mut TSParser,
    lookahead: *mut Subtree,
    position: *const u32,
    operations: u32,
) -> bool {
    (*self_).operation_count += operations;
    if (*self_).operation_count >= OP_COUNT_PER_PARSER_CALLBACK_CHECK {
        (*self_).operation_count = 0;
    }
    if !position.is_null() {
        (*self_).parse_state.current_byte_offset = *position;
        (*self_).parse_state.has_error = (*self_).has_error;
    }
    if (*self_).operation_count == 0
        && (*self_).parse_options.progress_callback.is_some()
        && (*self_).parse_options.progress_callback.unwrap()(&mut (*self_).parse_state)
    {
        if !lookahead.is_null() && !(*lookahead).ptr.is_null() {
            ts_subtree_release(&mut (*self_).tree_pool, *lookahead);
        }
        return false;
    }
    true
}

unsafe fn ts_parser__advance(
    self_: *mut TSParser,
    version: StackVersion,
    allow_node_reuse: bool,
) -> bool {
    let mut state = ts_stack_state((*self_).stack, version);
    let position = ts_stack_position((*self_).stack, version).bytes;
    let last_external_token = ts_stack_last_external_token((*self_).stack, version);

    let mut did_reuse = true;
    let mut lookahead = NULL_SUBTREE;
    let mut table_entry = TableEntry {
        actions: ptr::null(),
        action_count: 0,
        is_reusable: false,
    };

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
        lookahead = ts_parser__get_cached_token(
            self_,
            state,
            position as usize,
            last_external_token,
            &mut table_entry,
        );
    }

    let mut needs_lex = lookahead.ptr.is_null();
    loop {
        // Otherwise, re-run the lexer.
        if needs_lex {
            needs_lex = false;
            lookahead = ts_parser__lex(self_, version, state);
            if (*self_).has_scanner_error {
                return false;
            }

            if !lookahead.ptr.is_null() {
                ts_parser__set_cached_token(self_, position, last_external_token, lookahead);
                ts_language_table_entry(
                    (*self_).language,
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
                    (*self_).language,
                    state,
                    ts_builtin_sym_end,
                    &mut table_entry,
                );
            }
        }

        // If a progress callback was provided, then check every
        // time a fixed number of parse actions has been processed.
        if !ts_parser__check_progress(self_, &mut lookahead, &position, 1) {
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
                TSParseActionTypeShift => {
                    if action.shift.repetition {
                        break;
                    }
                    let next_state;
                    if action.shift.extra {
                        next_state = state;
                        LOG!(self_, b"shift_extra\0".as_ptr() as *const i8);
                    } else {
                        next_state = action.shift.state;
                        LOG!(self_, b"shift state:%u\0".as_ptr() as *const i8, next_state as u32);
                    }

                    if ts_subtree_child_count(lookahead) > 0 {
                        ts_parser__breakdown_lookahead(
                            self_,
                            &mut lookahead,
                            state,
                            &mut (*self_).reusable_node,
                        );
                        let next_state = ts_language_next_state(
                            (*self_).language,
                            state,
                            ts_subtree_symbol(lookahead),
                        );
                        ts_parser__shift(self_, version, next_state, lookahead, action.shift.extra);
                    } else {
                        ts_parser__shift(self_, version, next_state, lookahead, action.shift.extra);
                    }
                    if did_reuse {
                        reusable_node_advance(&mut (*self_).reusable_node);
                    }
                    return true;
                }

                TSParseActionTypeReduce => {
                    let is_fragile = table_entry.action_count > 1;
                    let end_of_non_terminal_extra = lookahead.ptr.is_null();
                    LOG!(
                        self_,
                        b"reduce sym:%s, child_count:%u\0".as_ptr() as *const i8,
                        SYM_NAME!(self_, action.reduce.symbol),
                        action.reduce.child_count as u32
                    );
                    let reduction_version = ts_parser__reduce(
                        self_,
                        version,
                        action.reduce.symbol,
                        action.reduce.child_count as u32,
                        action.reduce.dynamic_precedence as i32,
                        action.reduce.production_id,
                        is_fragile,
                        end_of_non_terminal_extra,
                    );
                    did_reduce = true;
                    if reduction_version != STACK_VERSION_NONE {
                        last_reduction_version = reduction_version;
                    }
                }

                TSParseActionTypeAccept => {
                    LOG!(self_, b"accept\0".as_ptr() as *const i8);
                    ts_parser__accept(self_, version, lookahead);
                    return true;
                }

                TSParseActionTypeRecover => {
                    if ts_subtree_child_count(lookahead) > 0 {
                        ts_parser__breakdown_lookahead(
                            self_,
                            &mut lookahead,
                            ERROR_STATE,
                            &mut (*self_).reusable_node,
                        );
                    }

                    ts_parser__recover(self_, version, lookahead);
                    if did_reuse {
                        reusable_node_advance(&mut (*self_).reusable_node);
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
            ts_stack_renumber_version((*self_).stack, last_reduction_version, version);
            LOG_STACK!(self_);
            state = ts_stack_state((*self_).stack, version);

            // At the end of a non-terminal extra rule, the lexer will return a
            // null subtree, because the parser needs to perform a fixed reduction
            // regardless of the lookahead node. After performing that reduction,
            // (and completing the non-terminal extra rule) run the lexer again based
            // on the current parse state.
            if lookahead.ptr.is_null() {
                needs_lex = true;
            } else {
                ts_language_table_entry(
                    (*self_).language,
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
                ts_subtree_release(&mut (*self_).tree_pool, lookahead);
            }
            ts_stack_halt((*self_).stack, version);
            return true;
        }

        // If the current lookahead token is a keyword that is not valid, but the
        // default word token *is* valid, then treat the lookahead token as the word
        // token instead.
        let language = (*self_).language as *const TSLanguageFull;
        if ts_subtree_is_keyword(lookahead)
            && ts_subtree_symbol(lookahead) != (*language).keyword_capture_token
            && !ts_language_is_reserved_word(
                (*self_).language,
                state,
                ts_subtree_symbol(lookahead),
            )
        {
            ts_language_table_entry(
                (*self_).language,
                state,
                (*language).keyword_capture_token,
                &mut table_entry,
            );
            if table_entry.action_count > 0 {
                LOG!(
                    self_,
                    b"switch from_keyword:%s, to_word_token:%s\0".as_ptr() as *const i8,
                    TREE_NAME!(self_, lookahead),
                    SYM_NAME!(self_, (*language).keyword_capture_token)
                );

                let mut mutable_lookahead =
                    ts_subtree_make_mut(&mut (*self_).tree_pool, lookahead);
                ts_subtree_set_symbol(
                    &mut mutable_lookahead,
                    (*language).keyword_capture_token,
                    (*self_).language,
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
            state = ts_stack_state((*self_).stack, version);
            ts_subtree_release(&mut (*self_).tree_pool, lookahead);
            needs_lex = true;
            continue;
        }

        // Otherwise, there is definitely an error in this version of the parse stack.
        // Mark this version as paused and continue processing any other stack
        // versions that exist. If some other version advances successfully, then
        // this version can simply be removed. But if all versions end up paused,
        // then error recovery is needed.
        LOG!(self_, b"detect_error lookahead:%s\0".as_ptr() as *const i8, TREE_NAME!(self_, lookahead));
        ts_stack_pause((*self_).stack, version, lookahead);
        return true;
    }
}

unsafe fn ts_parser__condense_stack(self_: *mut TSParser) -> u32 {
    let mut made_changes = false;
    let mut min_error_cost = u32::MAX;
    let mut i: StackVersion = 0;
    while i < ts_stack_version_count((*self_).stack) {
        // Prune any versions that have been marked for removal.
        if ts_stack_is_halted((*self_).stack, i) {
            ts_stack_remove_version((*self_).stack, i);
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

            match ts_parser__compare_versions(self_, status_j, status_i) {
                ErrorComparison::TakeLeft => {
                    made_changes = true;
                    ts_stack_remove_version((*self_).stack, i);
                    i -= 1;
                    j = i;
                    break;
                }

                ErrorComparison::PreferLeft | ErrorComparison::None => {
                    if ts_stack_merge((*self_).stack, j, i) {
                        made_changes = true;
                        i -= 1;
                        j = i;
                        break;
                    }
                }

                ErrorComparison::PreferRight => {
                    made_changes = true;
                    if ts_stack_merge((*self_).stack, j, i) {
                        i -= 1;
                        j = i;
                        break;
                    } else {
                        ts_stack_swap_versions((*self_).stack, i, j);
                    }
                }

                ErrorComparison::TakeRight => {
                    made_changes = true;
                    ts_stack_remove_version((*self_).stack, j);
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
    while ts_stack_version_count((*self_).stack) > MAX_VERSION_COUNT {
        ts_stack_remove_version((*self_).stack, MAX_VERSION_COUNT);
        made_changes = true;
    }

    // If the best-performing stack version is currently paused, or all
    // versions are paused, then resume the best paused version and begin
    // the error recovery process. Otherwise, remove the paused versions.
    if ts_stack_version_count((*self_).stack) > 0 {
        let mut has_unpaused_version = false;
        let mut i: StackVersion = 0;
        let mut n = ts_stack_version_count((*self_).stack);
        while i < n {
            if ts_stack_is_paused((*self_).stack, i) {
                if !has_unpaused_version && (*self_).accept_count < MAX_VERSION_COUNT {
                    LOG!(self_, b"resume version:%u\0".as_ptr() as *const i8, i);
                    min_error_cost = ts_stack_error_cost((*self_).stack, i);
                    let lookahead = ts_stack_resume((*self_).stack, i);
                    ts_parser__handle_error(self_, i, lookahead);
                    has_unpaused_version = true;
                } else {
                    ts_stack_remove_version((*self_).stack, i);
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
        LOG!(self_, b"condense\0".as_ptr() as *const i8);
        LOG_STACK!(self_);
    }

    min_error_cost
}

unsafe fn ts_parser__balance_subtree(self_: *mut TSParser) -> bool {
    let finished_tree = (*self_).finished_tree;

    // If we haven't canceled balancing in progress before, then we want to clear the tree stack and
    // push the initial finished tree onto it. Otherwise, if we're resuming balancing after a
    // cancellation, we don't want to clear the tree stack.
    if !(*self_).canceled_balancing {
        array_clear(&mut (*self_).tree_pool.tree_stack as *mut MutableSubtreeArray as *mut Array<MutableSubtree>);
        if ts_subtree_child_count(finished_tree) > 0 && (*finished_tree.ptr).ref_count == 1 {
            array_push(
                &mut (*self_).tree_pool.tree_stack as *mut MutableSubtreeArray as *mut Array<MutableSubtree>,
                ts_subtree_to_mut_unsafe(finished_tree),
            );
        }
    }

    while (*self_).tree_pool.tree_stack.size > 0 {
        if !ts_parser__check_progress(self_, ptr::null_mut(), ptr::null(), 1) {
            return false;
        }

        let tree = *array_get(
            &(*self_).tree_pool.tree_stack as *const MutableSubtreeArray as *const Array<MutableSubtree>,
            (*self_).tree_pool.tree_stack.size - 1,
        );

        if (*tree.ptr).data.children.repeat_depth > 0 {
            let tree_subtree = ts_subtree_from_mut(tree);
            let child1 = *ts_subtree_children(tree_subtree).add(0);
            let child2 =
                *ts_subtree_children(tree_subtree).add((*tree.ptr).child_count as usize - 1);
            let repeat_delta =
                ts_subtree_repeat_depth(child1) as i64 - ts_subtree_repeat_depth(child2) as i64;
            if repeat_delta > 0 {
                let mut n = repeat_delta as u32;

                let mut i = n / 2;
                while i > 0 {
                    ts_subtree_compress(
                        tree,
                        i,
                        (*self_).language,
                        &mut (*self_).tree_pool.tree_stack as *mut MutableSubtreeArray as *mut Array<MutableSubtree>,
                    );
                    n -= i;

                    // We scale the operation count increment in `ts_parser__check_progress` proportionately to the compression
                    // size since larger values of i take longer to process. Shifting by 4 empirically provides good check
                    // intervals (e.g. 193 operations when i=3100) to prevent blocking during large compressions.
                    let operations = if i >> 4 > 0 { i >> 4 } else { 1 };
                    if !ts_parser__check_progress(self_, ptr::null_mut(), ptr::null(), operations) {
                        return false;
                    }
                    i /= 2;
                }
            }
        }

        array_pop(&mut (*self_).tree_pool.tree_stack as *mut MutableSubtreeArray as *mut Array<MutableSubtree>);

        for i in 0..(*tree.ptr).child_count {
            let tree_subtree = ts_subtree_from_mut(tree);
            let child = *ts_subtree_children(tree_subtree).add(i as usize);
            if ts_subtree_child_count(child) > 0 && (*child.ptr).ref_count == 1 {
                array_push(
                    &mut (*self_).tree_pool.tree_stack as *mut MutableSubtreeArray as *mut Array<MutableSubtree>,
                    ts_subtree_to_mut_unsafe(child),
                );
            }
        }
    }

    true
}

unsafe fn ts_parser_has_outstanding_parse(self_: *mut TSParser) -> bool {
    (*self_).canceled_balancing
        || !(*self_).external_scanner_payload.is_null()
        || ts_stack_state((*self_).stack, 0) != 1
        || ts_stack_node_count_since_error((*self_).stack, 0) != 0
}

// ---------------------------------------------------------------------------
// Exported functions — lifecycle
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_parser_new() -> *mut TSParser {
    let self_ = ts_calloc(1, core::mem::size_of::<TSParser>()) as *mut TSParser;
    ts_lexer_init(&mut (*self_).lexer);
    array_init(&mut (*self_).reduce_actions);
    array_reserve(&mut (*self_).reduce_actions as *mut ReduceActionSet, 4);
    (*self_).tree_pool = ts_subtree_pool_new(32);
    (*self_).stack = ts_stack_new(&mut (*self_).tree_pool);
    (*self_).finished_tree = NULL_SUBTREE;
    (*self_).reusable_node = reusable_node_new();
    (*self_).dot_graph_file = ptr::null_mut();
    (*self_).language = ptr::null();
    (*self_).has_scanner_error = false;
    (*self_).has_error = false;
    (*self_).canceled_balancing = false;
    (*self_).external_scanner_payload = ptr::null_mut();
    (*self_).operation_count = 0;
    (*self_).old_tree = NULL_SUBTREE;
    let new_array: Array<TSRange> = array_new();
    (*self_).included_range_differences = core::mem::transmute(new_array);
    (*self_).included_range_difference_index = 0;
    ts_parser__set_cached_token(self_, 0, NULL_SUBTREE, NULL_SUBTREE);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_delete(self_: *mut TSParser) {
    if self_.is_null() {
        return;
    }

    ts_parser_set_language(self_, ptr::null());
    ts_stack_delete((*self_).stack);
    if !(*self_).reduce_actions.contents.is_null() {
        array_delete(&mut (*self_).reduce_actions as *mut ReduceActionSet);
    }
    if !(*self_).included_range_differences.contents.is_null() {
        array_delete(&mut (*self_).included_range_differences as *mut TSRangeArray as *mut Array<TSRange>);
    }
    if !(*self_).old_tree.ptr.is_null() {
        ts_subtree_release(&mut (*self_).tree_pool, (*self_).old_tree);
        (*self_).old_tree = NULL_SUBTREE;
    }
    ts_wasm_store_delete((*self_).wasm_store);
    ts_lexer_delete(&mut (*self_).lexer);
    ts_parser__set_cached_token(self_, 0, NULL_SUBTREE, NULL_SUBTREE);
    ts_subtree_pool_delete(&mut (*self_).tree_pool);
    reusable_node_delete(&mut (*self_).reusable_node);
    array_delete(&mut (*self_).trailing_extras as *mut SubtreeArray as *mut Array<Subtree>);
    array_delete(&mut (*self_).trailing_extras2 as *mut SubtreeArray as *mut Array<Subtree>);
    array_delete(&mut (*self_).scratch_trees as *mut SubtreeArray as *mut Array<Subtree>);
    ts_free(self_ as *mut c_void);
}

// ---------------------------------------------------------------------------
// Exported functions — configuration
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_parser_language(self_: *const TSParser) -> *const TSLanguage {
    (*self_).language
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_language(
    self_: *mut TSParser,
    language: *const TSLanguage,
) -> bool {
    ts_parser_reset(self_);
    ts_language_delete((*self_).language);
    (*self_).language = ptr::null();

    if !language.is_null() {
        let lang_full = language as *const TSLanguageFull;
        if (*lang_full).abi_version > TREE_SITTER_LANGUAGE_VERSION
            || (*lang_full).abi_version < TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION
        {
            return false;
        }

        if ts_language_is_wasm(language) {
            if (*self_).wasm_store.is_null()
                || !ts_wasm_store_start((*self_).wasm_store, &mut (*self_).lexer.data, language)
            {
                return false;
            }
        }
    }

    (*self_).language = ts_language_copy(language);
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_logger(self_: *const TSParser) -> TSLogger {
    ptr::read(&(*self_).lexer.logger)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_logger(
    self_: *mut TSParser,
    logger: TSLogger,
) {
    (*self_).lexer.logger = logger;
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_print_dot_graphs(
    self_: *mut TSParser,
    fd: i32,
) {
    if !(*self_).dot_graph_file.is_null() {
        fclose((*self_).dot_graph_file);
    }

    if fd >= 0 {
        #[cfg(target_os = "windows")]
        {
            extern "C" {
                fn _fdopen(fd: i32, mode: *const i8) -> *mut c_void;
            }
            (*self_).dot_graph_file = _fdopen(fd, b"a\0".as_ptr() as *const i8);
        }
        #[cfg(not(target_os = "windows"))]
        {
            (*self_).dot_graph_file = fdopen(fd, b"a\0".as_ptr() as *const i8);
        }
    } else {
        (*self_).dot_graph_file = ptr::null_mut();
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_included_ranges(
    self_: *mut TSParser,
    ranges: *const TSRange,
    count: u32,
) -> bool {
    ts_lexer_set_included_ranges(&mut (*self_).lexer, ranges, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_included_ranges(
    self_: *const TSParser,
    count: *mut u32,
) -> *const TSRange {
    ts_lexer_included_ranges(&(*self_).lexer, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_reset(self_: *mut TSParser) {
    ts_parser__external_scanner_destroy(self_);
    if !(*self_).wasm_store.is_null() {
        ts_wasm_store_reset((*self_).wasm_store);
    }

    if !(*self_).old_tree.ptr.is_null() {
        ts_subtree_release(&mut (*self_).tree_pool, (*self_).old_tree);
        (*self_).old_tree = NULL_SUBTREE;
    }

    reusable_node_clear(&mut (*self_).reusable_node);
    ts_lexer_reset(&mut (*self_).lexer, length_zero());
    ts_stack_clear((*self_).stack);
    ts_parser__set_cached_token(self_, 0, NULL_SUBTREE, NULL_SUBTREE);
    if !(*self_).finished_tree.ptr.is_null() {
        ts_subtree_release(&mut (*self_).tree_pool, (*self_).finished_tree);
        (*self_).finished_tree = NULL_SUBTREE;
    }
    (*self_).accept_count = 0;
    (*self_).has_scanner_error = false;
    (*self_).has_error = false;
    (*self_).canceled_balancing = false;
    (*self_).parse_options = core::mem::zeroed();
    (*self_).parse_state = core::mem::zeroed();
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
    let mut result: *mut TSTree = ptr::null_mut();
    if (*self_).language.is_null() || input.read.is_none() {
        return ptr::null_mut();
    }

    if ts_language_is_wasm((*self_).language) {
        if (*self_).wasm_store.is_null() {
            return ptr::null_mut();
        }
        ts_wasm_store_start((*self_).wasm_store, &mut (*self_).lexer.data, (*self_).language);
    }

    ts_lexer_set_input(&mut (*self_).lexer, input);
    array_clear(&mut (*self_).included_range_differences as *mut TSRangeArray as *mut Array<TSRange>);
    (*self_).included_range_difference_index = 0;

    (*self_).operation_count = 0;

    if ts_parser_has_outstanding_parse(self_) {
        LOG!(self_, b"resume_parsing\0".as_ptr() as *const i8);
        if (*self_).canceled_balancing {
            // goto balance
            debug_assert!(!(*self_).finished_tree.ptr.is_null());
            if !ts_parser__balance_subtree(self_) {
                (*self_).canceled_balancing = true;
                return ptr::null_mut();
            }
            (*self_).canceled_balancing = false;
            LOG!(self_, b"done\0".as_ptr() as *const i8);
            LOG_TREE!(self_, (*self_).finished_tree);

            result = ts_tree_new(
                (*self_).finished_tree,
                (*self_).language,
                (*self_).lexer.included_ranges,
                (*self_).lexer.included_range_count,
            );
            (*self_).finished_tree = NULL_SUBTREE;

            // goto exit
            ts_parser_reset(self_);
            return result;
        }
    } else {
        ts_parser__external_scanner_create(self_);
        if (*self_).has_scanner_error {
            // goto exit
            ts_parser_reset(self_);
            return result;
        }

        if !old_tree.is_null() {
            ts_subtree_retain((*old_tree).root);
            (*self_).old_tree = (*old_tree).root;
            ts_range_array_get_changed_ranges(
                (*old_tree).included_ranges,
                (*old_tree).included_range_count,
                (*self_).lexer.included_ranges,
                (*self_).lexer.included_range_count,
                &mut (*self_).included_range_differences,
            );
            reusable_node_reset(&mut (*self_).reusable_node, (*old_tree).root);
            LOG!(self_, b"parse_after_edit\0".as_ptr() as *const i8);
            LOG_TREE!(self_, (*self_).old_tree);
            for i in 0..(*self_).included_range_differences.size {
                let range = array_get(
                    &(*self_).included_range_differences as *const TSRangeArray as *const Array<TSRange>,
                    i,
                );
                LOG!(
                    self_,
                    b"different_included_range %u - %u\0".as_ptr() as *const i8,
                    (*range).start_byte,
                    (*range).end_byte
                );
            }
        } else {
            reusable_node_clear(&mut (*self_).reusable_node);
            LOG!(self_, b"new_parse\0".as_ptr() as *const i8);
        }
    }

    let mut position: u32 = 0;
    let mut last_position: u32 = 0;
    let mut version_count: StackVersion;
    loop {
        let mut version: StackVersion = 0;
        loop {
            version_count = ts_stack_version_count((*self_).stack);
            if version >= version_count {
                break;
            }

            let allow_node_reuse = version_count == 1;
            while ts_stack_is_active((*self_).stack, version) {
                LOG!(
                    self_,
                    b"process version:%u, version_count:%u, state:%d, row:%u, col:%u\0".as_ptr() as *const i8,
                    version,
                    ts_stack_version_count((*self_).stack),
                    ts_stack_state((*self_).stack, version) as i32,
                    ts_stack_position((*self_).stack, version).extent.row,
                    ts_stack_position((*self_).stack, version).extent.column
                );

                if !ts_parser__advance(self_, version, allow_node_reuse) {
                    if (*self_).has_scanner_error {
                        // goto exit
                        ts_parser_reset(self_);
                        return result;
                    }
                    return ptr::null_mut();
                }

                LOG_STACK!(self_);

                position = ts_stack_position((*self_).stack, version).bytes;
                if position > last_position || (version > 0 && position == last_position) {
                    last_position = position;
                    break;
                }
            }
            version += 1;
        }

        // After advancing each version of the stack, re-sort the versions by their cost,
        // removing any versions that are no longer worth pursuing.
        let min_error_cost = ts_parser__condense_stack(self_);

        // If there's already a finished parse tree that's better than any in-progress version,
        // then terminate parsing. Clear the parse stack to remove any extra references to subtrees
        // within the finished tree, ensuring that these subtrees can be safely mutated in-place
        // for rebalancing.
        if !(*self_).finished_tree.ptr.is_null()
            && ts_subtree_error_cost((*self_).finished_tree) < min_error_cost
        {
            ts_stack_clear((*self_).stack);
            break;
        }

        while (*self_).included_range_difference_index < (*self_).included_range_differences.size
        {
            let range = array_get(
                &(*self_).included_range_differences as *const TSRangeArray as *const Array<TSRange>,
                (*self_).included_range_difference_index,
            );
            if (*range).end_byte <= position {
                (*self_).included_range_difference_index += 1;
            } else {
                break;
            }
        }

        if version_count == 0 {
            break;
        }
    }

    // balance:
    debug_assert!(!(*self_).finished_tree.ptr.is_null());
    if !ts_parser__balance_subtree(self_) {
        (*self_).canceled_balancing = true;
        return ptr::null_mut();
    }
    (*self_).canceled_balancing = false;
    LOG!(self_, b"done\0".as_ptr() as *const i8);
    LOG_TREE!(self_, (*self_).finished_tree);

    result = ts_tree_new(
        (*self_).finished_tree,
        (*self_).language,
        (*self_).lexer.included_ranges,
        (*self_).lexer.included_range_count,
    );
    (*self_).finished_tree = NULL_SUBTREE;

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
    (*self_).parse_options = parse_options;
    (*self_).parse_state.payload = parse_options.payload;
    let result = ts_parser_parse(self_, old_tree, input);
    // Reset parser options before further parse calls.
    (*self_).parse_options = core::mem::zeroed();
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
            payload: &input as *const TSStringInput as *mut c_void,
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
    if !(*self_).language.is_null() && ts_language_is_wasm((*self_).language) {
        // Copy the assigned language into the new store.
        let copy = ts_language_copy((*self_).language);
        ts_parser_set_language(self_, copy);
        ts_language_delete(copy);
    }

    ts_wasm_store_delete((*self_).wasm_store);
    (*self_).wasm_store = store;
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_take_wasm_store(
    self_: *mut TSParser,
) -> *mut TSWasmStore {
    if !(*self_).language.is_null() && ts_language_is_wasm((*self_).language) {
        ts_parser_set_language(self_, ptr::null());
    }

    let result = (*self_).wasm_store;
    (*self_).wasm_store = ptr::null_mut();
    result
}
