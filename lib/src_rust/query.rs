#![allow(dead_code)]
// The generated FFI quantifier/error constants are PascalCase (e.g.
// `TSQuantifierZero`); matching on them in patterns is correct but trips the
// non-upper-case-globals style lint, so it is allowed module-wide.
#![allow(non_upper_case_globals)]

//! Rust port of `lib/src/query.c` — the query compiler and matching engine.
//!
//! This module is being ported tier by tier and is NOT yet activated: `query.c`
//! is still the live implementation (see `remaining_lib.c`). Until the port is
//! complete and the `#include "./query.c"` is removed, everything here is dead
//! code that only needs to compile.
//!
//! The internal query structures (`QueryStep`, `QueryState`, `TSQuery`,
//! `TSQueryCursor`, …) are opaque outside `query.c` — only the 31 exported
//! `ts_query_*` functions and the public `api.h` types form the ABI. Their
//! layout is therefore private, so they are ported as natural Rust (real `bool`
//! and integer fields) rather than the C bitfield layout. The structure of the
//! C source is otherwise preserved closely to keep the port reviewable against
//! the original.

use crate::ffi::{
    TSFieldId, TSLanguage, TSNode, TSPoint, TSQuantifier, TSQuantifierOne, TSQuantifierOneOrMore,
    TSQuantifierZero, TSQuantifierZeroOrMore, TSQuantifierZeroOrOne, TSQueryCapture,
    TSQueryCursorOptions, TSQueryCursorState, TSQueryError, TSQueryErrorCapture, TSQueryErrorField,
    TSQueryErrorLanguage, TSQueryErrorNodeType, TSQueryErrorNone, TSQueryErrorStructure,
    TSQueryErrorSyntax, TSQueryMatch, TSQueryPredicateStep, TSQueryPredicateStepTypeCapture,
    TSQueryPredicateStepTypeDone, TSQueryPredicateStepTypeString, TSRange, TSStateId, TSSymbol,
    TSTreeCursor,
};

use super::alloc::{calloc, free, malloc};
use super::language::{
    language_alias_at, language_aliases_for_symbol, language_field_map, language_lookaheads,
    language_public_symbol, language_state_is_primary, language_symbol_count, language_token_count,
    lookahead_iterator__next, ts_language_abi_version, ts_language_copy, ts_language_delete,
    ts_language_field_id_for_name, ts_language_state_count, ts_language_subtypes,
    ts_language_symbol_for_name, ts_language_symbol_metadata, TSParseActionTypeReduce,
    TSParseActionTypeShift, LANGUAGE_VERSION_WITH_RESERVED_WORDS,
};
use super::node::{
    ts_node_child_by_field_id, ts_node_end_byte, ts_node_end_point, ts_node_is_missing,
    ts_node_is_named, ts_node_is_null, ts_node_start_byte, ts_node_start_point, ts_node_symbol,
};
use super::point::{point_eq, point_gt, point_gte, point_lt, point_lte, POINT_MAX};
use super::subtree::{
    subtree_is_repetition, subtree_symbol, ts_builtin_sym_error, Subtree, TSFieldMapEntry,
};
use super::tree_cursor::{
    tree_cursor_entry_slice, tree_cursor_goto_first_child_internal,
    tree_cursor_goto_next_sibling_internal, ts_tree_cursor_current_node,
    ts_tree_cursor_current_status, ts_tree_cursor_delete, ts_tree_cursor_goto_parent,
    ts_tree_cursor_parent_node, ts_tree_cursor_reset, TreeCursor, TreeCursorStep,
};
use super::unicode::ts_decode_utf8;
use super::utils::{
    array_assign, array_back_mut, array_back_ref, array_clear, array_delete, array_erase,
    array_get_mut, array_get_ref, array_grow_by, array_init, array_insert, array_new, array_pop,
    array_push, array_reserve, array_splice, Array,
};
use core::ffi::c_void;
use core::mem::size_of;

// Wide-character classification from libc. The query parser uses these on
// decoded code points exactly as the C source does, so binding them directly
// preserves the original (locale-dependent) behavior.
extern "C" {
    fn iswspace(wc: i32) -> i32;
    fn iswalnum(wc: i32) -> i32;
}

const MAX_STEP_CAPTURE_COUNT: usize = 3;
const MAX_NEGATED_FIELD_COUNT: usize = 8;
const MAX_STATE_PREDECESSOR_COUNT: usize = 256;
const MAX_ANALYSIS_STATE_DEPTH: usize = 8;
const MAX_ANALYSIS_ITERATION_COUNT: u32 = 256;

const PATTERN_DONE_MARKER: u16 = u16::MAX;
const NONE: u16 = u16::MAX;
const WILDCARD_SYMBOL: TSSymbol = 0;
const OP_COUNT_PER_QUERY_CALLBACK_CHECK: u32 = 100;

// ABI version bounds, mirroring the `tree_sitter/api.h` macros visible to each
// translation unit.
const TREE_SITTER_LANGUAGE_VERSION: u32 = 15;
const TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION: u32 = 13;

// Sentinel returned by `parse_pattern` when it hits a closing `)`/`]` belonging
// to the parent. Mirrors `static const TSQueryError PARENT_DONE = -1;`.
const PARENT_DONE: TSQueryError = u32::MAX;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A sequence of unicode characters derived from a UTF-8 string, used when
/// parsing queries from S-expressions.
struct Stream {
    input: *const u8,
    start: *const u8,
    end: *const u8,
    next: i32,
    next_size: u8,
}

/// A step in the process of matching a query. Each node within a query
/// S-expression corresponds to one of these steps; an entire pattern is a
/// sequence of them. See `query.c` for the meaning of each field.
#[derive(Clone, Copy)]
struct QueryStep {
    symbol: TSSymbol,
    supertype_symbol: TSSymbol,
    field: TSFieldId,
    capture_ids: [u16; MAX_STEP_CAPTURE_COUNT],
    depth: u16,
    alternative_index: u16,
    negated_field_list_id: u16,
    is_named: bool,
    is_immediate: bool,
    is_last_child: bool,
    is_pass_through: bool,
    is_dead_end: bool,
    alternative_is_immediate: bool,
    contains_captures: bool,
    root_pattern_guaranteed: bool,
    parent_pattern_guaranteed: bool,
    is_missing: bool,
}

/// A slice of an external array. Capture names, literal string values, and
/// predicate step information are each stored in one contiguous array; an
/// individual entry is a slice of one of those arrays.
#[derive(Clone, Copy)]
struct Slice {
    offset: u32,
    length: u32,
}

/// A two-way mapping of strings to ids.
struct SymbolTable {
    characters: Array<u8>,
    slices: Array<Slice>,
}

/// The quantifiers of a pattern's captures, indexed by capture id.
type CaptureQuantifiers = Array<u8>;

/// Information about the starting point for matching a particular pattern,
/// stored in a sorted `pattern_map` keyed by the symbol of the first step.
#[derive(Clone, Copy)]
struct PatternEntry {
    step_index: u16,
    pattern_index: u16,
    is_rooted: bool,
}

#[derive(Clone, Copy)]
struct QueryPattern {
    steps: Slice,
    predicate_steps: Slice,
    start_byte: u32,
    end_byte: u32,
    is_non_local: bool,
}

#[derive(Clone, Copy)]
struct StepOffset {
    byte_offset: u32,
    step_index: u16,
}

/// The state of an in-progress match of a particular pattern. A `TSQueryCursor`
/// tracks many of these at once. See `query.c` for per-field semantics.
#[derive(Clone, Copy)]
struct QueryState {
    id: u32,
    capture_list_id: u32,
    start_depth: u16,
    step_index: u16,
    pattern_index: u16,
    consumed_capture_count: u16,
    seeking_immediate_match: bool,
    has_in_progress_alternatives: bool,
    dead: bool,
    needs_parent: bool,
}

type CaptureList = Array<TSQueryCapture>;

/// A collection of *lists* of captures. Each query state maintains its own
/// list; to avoid repeated allocations, the pool keeps a fixed set of lists and
/// tracks which are in use (a length of `u32::MAX` marks a list as free).
struct CaptureListPool {
    list: Array<CaptureList>,
    empty_list: CaptureList,
    max_capture_list_count: u32,
    free_capture_list_count: u32,
}

/// The state needed for walking the parse table when analyzing a query pattern,
/// to determine at which steps the pattern might fail to match.
#[derive(Clone, Copy)]
struct AnalysisStateEntry {
    parse_state: TSStateId,
    parent_symbol: TSSymbol,
    child_index: u16,
    field_id: TSFieldId,
    done: bool,
}

#[derive(Clone, Copy)]
struct AnalysisState {
    stack: [AnalysisStateEntry; MAX_ANALYSIS_STATE_DEPTH],
    depth: u16,
    step_index: u16,
    root_symbol: TSSymbol,
}

type AnalysisStateSet = Array<*mut AnalysisState>;

struct QueryAnalysis {
    states: AnalysisStateSet,
    next_states: AnalysisStateSet,
    deeper_states: AnalysisStateSet,
    state_pool: AnalysisStateSet,
    final_step_indices: Array<u16>,
    finished_parent_symbols: Array<TSSymbol>,
    did_abort: bool,
}

/// A subset of the parse-table states used in constructing nodes with a certain
/// symbol, with information about the possible node each downstream state could
/// produce.
#[derive(Clone, Copy)]
struct AnalysisSubgraphNode {
    state: TSStateId,
    production_id: u16,
    child_index: u8,
    done: bool,
}

struct AnalysisSubgraph {
    symbol: TSSymbol,
    start_states: Array<TSStateId>,
    nodes: Array<AnalysisSubgraphNode>,
}

type AnalysisSubgraphArray = Array<AnalysisSubgraph>;

/// A map storing the predecessors of each parse state, used during analysis to
/// determine which parse states can lead to which reduce actions.
struct StatePredecessorMap {
    contents: *mut TSStateId,
}

/// A tree query, compiled from a string of S-expressions. The query itself is
/// immutable; the mutable execution state lives in a `TSQueryCursor`.
pub struct TSQuery {
    captures: SymbolTable,
    predicate_values: SymbolTable,
    capture_quantifiers: Array<CaptureQuantifiers>,
    steps: Array<QueryStep>,
    pattern_map: Array<PatternEntry>,
    predicate_steps: Array<TSQueryPredicateStep>,
    patterns: Array<QueryPattern>,
    step_offsets: Array<StepOffset>,
    negated_fields: Array<TSFieldId>,
    string_buffer: Array<u8>,
    repeat_symbols_with_rootless_patterns: Array<TSSymbol>,
    language: *const TSLanguage,
    wildcard_root_pattern_count: u16,
}

/// A stateful struct used to execute a query on a tree.
pub struct TSQueryCursor {
    query: *const TSQuery,
    cursor: TreeCursor,
    states: Array<QueryState>,
    finished_states: Array<QueryState>,
    capture_list_pool: CaptureListPool,
    depth: u32,
    max_start_depth: u32,
    included_range: TSRange,
    containing_range: TSRange,
    next_state_id: u32,
    query_options: *const TSQueryCursorOptions,
    query_state: TSQueryCursorState,
    operation_count: u32,
    on_visible_node: bool,
    ascending: bool,
    halted: bool,
    did_exceed_match_limit: bool,
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

/// Advance to the next unicode code point in the stream.
unsafe fn stream_advance(self_: &mut Stream) -> bool {
    self_.input = self_.input.add(self_.next_size as usize);
    if self_.input < self_.end {
        let size = ts_decode_utf8(
            self_.input,
            (self_.end as usize - self_.input as usize) as u32,
            &mut self_.next,
        );
        if size > 0 {
            self_.next_size = size as u8;
            return true;
        }
    } else {
        self_.next_size = 0;
        self_.next = 0;
    }
    false
}

/// Reset the stream to the given input position.
unsafe fn stream_reset(self_: &mut Stream, input: *const u8) {
    self_.input = input;
    self_.next_size = 0;
    stream_advance(self_);
}

unsafe fn stream_new(string: *const u8, length: u32) -> Stream {
    let mut self_ = Stream {
        next: 0,
        input: string,
        start: string,
        end: string.add(length as usize),
        next_size: 0,
    };
    stream_advance(&mut self_);
    self_
}

unsafe fn stream_skip_whitespace(self_: &mut Stream) {
    loop {
        if iswspace(self_.next) != 0 {
            stream_advance(self_);
        } else if self_.next == i32::from(b';') {
            // skip over comments
            stream_advance(self_);
            while self_.next != 0 && self_.next != i32::from(b'\n') {
                if !stream_advance(self_) {
                    break;
                }
            }
        } else {
            break;
        }
    }
}

unsafe fn stream_is_ident_start(self_: &Stream) -> bool {
    iswalnum(self_.next) != 0 || self_.next == i32::from(b'_') || self_.next == i32::from(b'-')
}

unsafe fn stream_scan_identifier(stream: &mut Stream) {
    loop {
        stream_advance(stream);
        if !(iswalnum(stream.next) != 0
            || stream.next == i32::from(b'_')
            || stream.next == i32::from(b'-')
            || stream.next == i32::from(b'.'))
        {
            break;
        }
    }
}

fn stream_offset(self_: &Stream) -> u32 {
    (self_.input as usize - self_.start as usize) as u32
}

// ---------------------------------------------------------------------------
// CaptureListPool
// ---------------------------------------------------------------------------

const fn capture_list_pool_new() -> CaptureListPool {
    CaptureListPool {
        list: array_new(),
        empty_list: array_new(),
        max_capture_list_count: u32::MAX,
        free_capture_list_count: 0,
    }
}

/// Mark every allocated capture list as free (length `u32::MAX`).
unsafe fn capture_list_pool_reset(self_: &mut CaptureListPool) {
    for i in 0..self_.list.size {
        array_get_mut(&mut self_.list, i).size = u32::MAX;
    }
    self_.free_capture_list_count = self_.list.size;
}

unsafe fn capture_list_pool_delete(self_: &mut CaptureListPool) {
    for i in 0..self_.list.size {
        array_delete(array_get_mut(&mut self_.list, i));
    }
    array_delete(&mut self_.list);
}

unsafe fn capture_list_pool_get(self_: &CaptureListPool, id: u16) -> &CaptureList {
    if u32::from(id) >= self_.list.size {
        return &self_.empty_list;
    }
    array_get_ref(&self_.list, u32::from(id))
}

unsafe fn capture_list_pool_get_mut(self_: &mut CaptureListPool, id: u16) -> &mut CaptureList {
    debug_assert!(u32::from(id) < self_.list.size);
    array_get_mut(&mut self_.list, u32::from(id))
}

/// The pool is empty if all allocated lists are in use and we have reached the
/// maximum allowed number of allocated lists.
const fn capture_list_pool_is_empty(self_: &CaptureListPool) -> bool {
    self_.free_capture_list_count == 0 && self_.list.size >= self_.max_capture_list_count
}

unsafe fn capture_list_pool_acquire(self_: &mut CaptureListPool) -> u16 {
    // First see if any already-allocated capture list is currently unused.
    if self_.free_capture_list_count > 0 {
        for i in 0..self_.list.size {
            if array_get_ref(&self_.list, i).size == u32::MAX {
                array_clear(array_get_mut(&mut self_.list, i));
                self_.free_capture_list_count -= 1;
                return i as u16;
            }
        }
    }

    // Otherwise allocate a new capture list, as long as that doesn't put us
    // over the requested maximum.
    let i = self_.list.size;
    if i >= self_.max_capture_list_count {
        return NONE;
    }
    let mut list: CaptureList = array_new();
    array_init(&mut list);
    array_push(&mut self_.list, list);
    i as u16
}

unsafe fn capture_list_pool_release(self_: &mut CaptureListPool, id: u16) {
    if u32::from(id) >= self_.list.size {
        return;
    }
    array_get_mut(&mut self_.list, u32::from(id)).size = u32::MAX;
    self_.free_capture_list_count += 1;
}

// ---------------------------------------------------------------------------
// Quantifiers
// ---------------------------------------------------------------------------

// Arms are kept 1:1 with the C `switch` cases for reviewability against query.c.
#[allow(clippy::match_same_arms)]
const fn quantifier_mul(left: TSQuantifier, right: TSQuantifier) -> TSQuantifier {
    match left {
        TSQuantifierZero => TSQuantifierZero,
        TSQuantifierZeroOrOne => match right {
            TSQuantifierZero => TSQuantifierZero,
            TSQuantifierZeroOrOne | TSQuantifierOne => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrMore | TSQuantifierOneOrMore => TSQuantifierZeroOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierZeroOrMore => match right {
            TSQuantifierZero => TSQuantifierZero,
            _ => TSQuantifierZeroOrMore,
        },
        TSQuantifierOne => right,
        TSQuantifierOneOrMore => match right {
            TSQuantifierZero => TSQuantifierZero,
            TSQuantifierZeroOrOne | TSQuantifierZeroOrMore => TSQuantifierZeroOrMore,
            TSQuantifierOne | TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        _ => TSQuantifierZero,
    }
}

#[allow(clippy::match_same_arms)]
const fn quantifier_join(left: TSQuantifier, right: TSQuantifier) -> TSQuantifier {
    match left {
        TSQuantifierZero => match right {
            TSQuantifierZero => TSQuantifierZero,
            TSQuantifierZeroOrOne | TSQuantifierOne => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrMore | TSQuantifierOneOrMore => TSQuantifierZeroOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierZeroOrOne => match right {
            TSQuantifierZero | TSQuantifierZeroOrOne | TSQuantifierOne => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrMore | TSQuantifierOneOrMore => TSQuantifierZeroOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierZeroOrMore => TSQuantifierZeroOrMore,
        TSQuantifierOne => match right {
            TSQuantifierZero | TSQuantifierZeroOrOne => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrMore => TSQuantifierZeroOrMore,
            TSQuantifierOne => TSQuantifierOne,
            TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierOneOrMore => match right {
            TSQuantifierZero | TSQuantifierZeroOrOne | TSQuantifierZeroOrMore => {
                TSQuantifierZeroOrMore
            }
            TSQuantifierOne | TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        _ => TSQuantifierZero,
    }
}

#[allow(clippy::match_same_arms)]
const fn quantifier_add(left: TSQuantifier, right: TSQuantifier) -> TSQuantifier {
    match left {
        TSQuantifierZero => right,
        TSQuantifierZeroOrOne => match right {
            TSQuantifierZero => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrOne | TSQuantifierZeroOrMore => TSQuantifierZeroOrMore,
            TSQuantifierOne | TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierZeroOrMore => match right {
            TSQuantifierZero | TSQuantifierZeroOrOne | TSQuantifierZeroOrMore => {
                TSQuantifierZeroOrMore
            }
            TSQuantifierOne | TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierOne => match right {
            TSQuantifierZero => TSQuantifierOne,
            _ => TSQuantifierOneOrMore,
        },
        TSQuantifierOneOrMore => TSQuantifierOneOrMore,
        _ => TSQuantifierZero,
    }
}

fn capture_quantifier_for_id(self_: &CaptureQuantifiers, id: u16) -> TSQuantifier {
    if self_.size <= u32::from(id) {
        TSQuantifierZero
    } else {
        unsafe { TSQuantifier::from(*array_get_ref(self_, u32::from(id))) }
    }
}

/// Add the given quantifier to the current value for `id`.
unsafe fn capture_quantifiers_add_for_id(
    self_: &mut CaptureQuantifiers,
    id: u16,
    quantifier: TSQuantifier,
) {
    if self_.size <= u32::from(id) {
        array_grow_by(self_, u32::from(id) + 1 - self_.size);
    }
    let own = array_get_mut(self_, u32::from(id));
    *own = quantifier_add(TSQuantifier::from(*own), quantifier) as u8;
}

/// Point-wise add the given quantifiers to the current values.
unsafe fn capture_quantifiers_add_all(
    self_: &mut CaptureQuantifiers,
    quantifiers: &CaptureQuantifiers,
) {
    if self_.size < quantifiers.size {
        array_grow_by(self_, quantifiers.size - self_.size);
    }
    for id in 0..quantifiers.size {
        let q = *array_get_ref(quantifiers, id);
        let own = array_get_mut(self_, id);
        *own = quantifier_add(TSQuantifier::from(*own), TSQuantifier::from(q)) as u8;
    }
}

/// Multiply (join under repetition) the current values by the given quantifier.
unsafe fn capture_quantifiers_mul(self_: &mut CaptureQuantifiers, quantifier: TSQuantifier) {
    for id in 0..self_.size {
        let own = array_get_mut(self_, id);
        *own = quantifier_mul(TSQuantifier::from(*own), quantifier) as u8;
    }
}

/// Point-wise join the quantifiers from a list of alternatives with the current
/// values.
unsafe fn capture_quantifiers_join_all(
    self_: &mut CaptureQuantifiers,
    quantifiers: &CaptureQuantifiers,
) {
    if self_.size < quantifiers.size {
        array_grow_by(self_, quantifiers.size - self_.size);
    }
    for id in 0..quantifiers.size {
        let q = *array_get_ref(quantifiers, id);
        let own = array_get_mut(self_, id);
        *own = quantifier_join(TSQuantifier::from(*own), TSQuantifier::from(q)) as u8;
    }
    for id in quantifiers.size..self_.size {
        let own = array_get_mut(self_, id);
        *own = quantifier_join(TSQuantifier::from(*own), TSQuantifierZero) as u8;
    }
}

// ---------------------------------------------------------------------------
// SymbolTable
// ---------------------------------------------------------------------------

const fn symbol_table_new() -> SymbolTable {
    SymbolTable {
        characters: array_new(),
        slices: array_new(),
    }
}

unsafe fn symbol_table_delete(self_: &mut SymbolTable) {
    array_delete(&mut self_.characters);
    array_delete(&mut self_.slices);
}

unsafe fn symbol_table_id_for_name(self_: &SymbolTable, name: *const u8, length: u32) -> i32 {
    let needle = core::slice::from_raw_parts(name, length as usize);
    for i in 0..self_.slices.size {
        let slice = *array_get_ref(&self_.slices, i);
        if slice.length == length {
            let candidate = core::slice::from_raw_parts(
                core::ptr::from_ref::<u8>(array_get_ref(&self_.characters, slice.offset)),
                length as usize,
            );
            if candidate == needle {
                return i as i32;
            }
        }
    }
    -1
}

unsafe fn symbol_table_name_for_id(self_: &SymbolTable, id: u16, length: &mut u32) -> *const u8 {
    let slice = *array_get_ref(&self_.slices, u32::from(id));
    *length = slice.length;
    core::ptr::from_ref::<u8>(array_get_ref(&self_.characters, slice.offset))
}

unsafe fn symbol_table_insert_name(self_: &mut SymbolTable, name: *const u8, length: u32) -> u16 {
    let id = symbol_table_id_for_name(self_, name, length);
    if id >= 0 {
        return id as u16;
    }
    let slice = Slice {
        offset: self_.characters.size,
        length,
    };
    array_grow_by(&mut self_.characters, length + 1);
    ptr_copy_into_chars(&mut self_.characters, slice.offset, name, length);
    let last = self_.characters.size - 1;
    *array_get_mut(&mut self_.characters, last) = 0;
    array_push(&mut self_.slices, slice);
    (self_.slices.size - 1) as u16
}

#[inline]
unsafe fn ptr_copy_into_chars(chars: &mut Array<u8>, offset: u32, src: *const u8, length: u32) {
    core::ptr::copy_nonoverlapping(
        src,
        core::ptr::from_mut::<u8>(array_get_mut(chars, offset)),
        length as usize,
    );
}

// ---------------------------------------------------------------------------
// QueryStep
// ---------------------------------------------------------------------------

const fn query_step_new(symbol: TSSymbol, depth: u16, is_immediate: bool) -> QueryStep {
    QueryStep {
        symbol,
        supertype_symbol: 0,
        field: 0,
        capture_ids: [NONE; MAX_STEP_CAPTURE_COUNT],
        depth,
        alternative_index: NONE,
        negated_field_list_id: 0,
        is_named: false,
        is_immediate,
        is_last_child: false,
        is_pass_through: false,
        is_dead_end: false,
        alternative_is_immediate: false,
        contains_captures: false,
        root_pattern_guaranteed: false,
        parent_pattern_guaranteed: false,
        is_missing: false,
    }
}

fn query_step_add_capture(self_: &mut QueryStep, capture_id: u16) {
    for slot in &mut self_.capture_ids {
        if *slot == NONE {
            *slot = capture_id;
            break;
        }
    }
}

fn query_step_remove_capture(self_: &mut QueryStep, capture_id: u16) {
    for i in 0..MAX_STEP_CAPTURE_COUNT {
        if self_.capture_ids[i] == capture_id {
            self_.capture_ids[i] = NONE;
            let mut i = i;
            while i + 1 < MAX_STEP_CAPTURE_COUNT {
                if self_.capture_ids[i + 1] == NONE {
                    break;
                }
                self_.capture_ids[i] = self_.capture_ids[i + 1];
                self_.capture_ids[i + 1] = NONE;
                i += 1;
            }
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Query parsing
// ---------------------------------------------------------------------------

/// Record a negated-field assertion for `step_index`, reusing an existing field
/// list in `negated_fields` when one matches exactly.
///
/// The negated-fields array stores a sequence of field lists separated by zero
/// terminators.
unsafe fn ts_query_add_negated_fields(
    self_: &mut TSQuery,
    step_index: u16,
    field_ids: *const TSFieldId,
    field_count: u16,
) {
    // Try to find the start index of an existing list that matches this new one.
    let mut failed_match = false;
    let mut match_count: u32 = 0;
    let mut start_i: u32 = 0;
    let mut i = 0;
    while i < self_.negated_fields.size {
        let existing_field_id = *array_get_ref(&self_.negated_fields, i);

        // At each zero value, terminate the match attempt. If we've exactly
        // matched the new field list, reuse this index; otherwise start over.
        if existing_field_id == 0 {
            if match_count == u32::from(field_count) {
                array_get_mut(&mut self_.steps, u32::from(step_index)).negated_field_list_id =
                    start_i as u16;
                return;
            }
            start_i = i + 1;
            match_count = 0;
            failed_match = false;
        }
        // If the existing list matches our new list so far, advance to the next
        // element of the new list.
        else if match_count < u32::from(field_count)
            && existing_field_id == *field_ids.add(match_count as usize)
            && !failed_match
        {
            match_count += 1;
        }
        // Otherwise, this existing list has failed to match.
        else {
            match_count = 0;
            failed_match = true;
        }
        i += 1;
    }

    let neg_size = self_.negated_fields.size;
    array_get_mut(&mut self_.steps, u32::from(step_index)).negated_field_list_id = neg_size as u16;
    array_splice(
        &mut self_.negated_fields,
        neg_size,
        0,
        u32::from(field_count),
        field_ids,
    );
    array_push(&mut self_.negated_fields, 0);
}

/// Parse a double-quoted string literal at the stream position into
/// `self_.string_buffer`, handling backslash escapes.
unsafe fn ts_query_parse_string_literal(self_: &mut TSQuery, stream: &mut Stream) -> TSQueryError {
    let string_start = stream.input;
    if stream.next != i32::from(b'"') {
        return TSQueryErrorSyntax;
    }
    stream_advance(stream);
    let mut prev_position = stream.input;

    let mut is_escaped = false;
    array_clear(&mut self_.string_buffer);
    loop {
        if is_escaped {
            is_escaped = false;
            if stream.next == i32::from(b'n') {
                array_push(&mut self_.string_buffer, b'\n');
            } else if stream.next == i32::from(b'r') {
                array_push(&mut self_.string_buffer, b'\r');
            } else if stream.next == i32::from(b't') {
                array_push(&mut self_.string_buffer, b'\t');
            } else if stream.next == i32::from(b'0') {
                array_push(&mut self_.string_buffer, b'\0');
            } else {
                let size = self_.string_buffer.size;
                array_splice(
                    &mut self_.string_buffer,
                    size,
                    0,
                    u32::from(stream.next_size),
                    stream.input,
                );
            }
            prev_position = stream.input.add(stream.next_size as usize);
        } else if stream.next == i32::from(b'\\') {
            let count = (stream.input as usize - prev_position as usize) as u32;
            let size = self_.string_buffer.size;
            array_splice(&mut self_.string_buffer, size, 0, count, prev_position);
            prev_position = stream.input.add(1);
            is_escaped = true;
        } else if stream.next == i32::from(b'"') {
            let count = (stream.input as usize - prev_position as usize) as u32;
            let size = self_.string_buffer.size;
            array_splice(&mut self_.string_buffer, size, 0, count, prev_position);
            stream_advance(stream);
            return TSQueryErrorNone;
        } else if stream.next == i32::from(b'\n') {
            stream_reset(stream, string_start);
            return TSQueryErrorSyntax;
        }
        if !stream_advance(stream) {
            stream_reset(stream, string_start);
            return TSQueryErrorSyntax;
        }
    }
}

/// Parse a single predicate, adding it to the query's `predicate_steps`.
///
/// Predicates are arbitrary S-expressions handled at a higher level (the
/// Rust/JS bindings); they may contain `@`-prefixed capture names,
/// double-quoted strings, and bare symbols.
unsafe fn ts_query_parse_predicate(self_: &mut TSQuery, stream: &mut Stream) -> TSQueryError {
    if !stream_is_ident_start(stream) {
        return TSQueryErrorSyntax;
    }
    let predicate_name = stream.input;
    stream_scan_identifier(stream);
    if stream.next != i32::from(b'?') && stream.next != i32::from(b'!') {
        return TSQueryErrorSyntax;
    }
    stream_advance(stream);
    let length = (stream.input as usize - predicate_name as usize) as u32;
    let id = symbol_table_insert_name(&mut self_.predicate_values, predicate_name, length);
    array_push(
        &mut self_.predicate_steps,
        TSQueryPredicateStep {
            type_: TSQueryPredicateStepTypeString,
            value_id: u32::from(id),
        },
    );
    stream_skip_whitespace(stream);

    loop {
        if stream.next == i32::from(b')') {
            stream_advance(stream);
            stream_skip_whitespace(stream);
            array_push(
                &mut self_.predicate_steps,
                TSQueryPredicateStep {
                    type_: TSQueryPredicateStepTypeDone,
                    value_id: 0,
                },
            );
            break;
        }
        // Parse an '@'-prefixed capture name.
        else if stream.next == i32::from(b'@') {
            stream_advance(stream);
            if !stream_is_ident_start(stream) {
                return TSQueryErrorSyntax;
            }
            let capture_name = stream.input;
            stream_scan_identifier(stream);
            let capture_length = (stream.input as usize - capture_name as usize) as u32;
            let capture_id =
                symbol_table_id_for_name(&self_.captures, capture_name, capture_length);
            if capture_id == -1 {
                stream_reset(stream, capture_name);
                return TSQueryErrorCapture;
            }
            array_push(
                &mut self_.predicate_steps,
                TSQueryPredicateStep {
                    type_: TSQueryPredicateStepTypeCapture,
                    value_id: capture_id as u32,
                },
            );
        }
        // Parse a string literal.
        else if stream.next == i32::from(b'"') {
            let e = ts_query_parse_string_literal(self_, stream);
            if e != TSQueryErrorNone {
                return e;
            }
            let query_id = symbol_table_insert_name(
                &mut self_.predicate_values,
                self_.string_buffer.contents,
                self_.string_buffer.size,
            );
            array_push(
                &mut self_.predicate_steps,
                TSQueryPredicateStep {
                    type_: TSQueryPredicateStepTypeString,
                    value_id: u32::from(query_id),
                },
            );
        }
        // Parse a bare symbol.
        else if stream_is_ident_start(stream) {
            let symbol_start = stream.input;
            stream_scan_identifier(stream);
            let symbol_length = (stream.input as usize - symbol_start as usize) as u32;
            let query_id =
                symbol_table_insert_name(&mut self_.predicate_values, symbol_start, symbol_length);
            array_push(
                &mut self_.predicate_steps,
                TSQueryPredicateStep {
                    type_: TSQueryPredicateStepTypeString,
                    value_id: u32::from(query_id),
                },
            );
        } else {
            return TSQueryErrorSyntax;
        }

        stream_skip_whitespace(stream);
    }

    TSQueryErrorNone
}

/// Read one S-expression pattern from the stream and incorporate it into the
/// query's step representation. Recurses for nested patterns.
///
/// The caller must pass a dedicated `capture_quantifiers`; it must not be shared
/// between calls.
unsafe fn ts_query_parse_pattern(
    self_: &mut TSQuery,
    stream: &mut Stream,
    depth: u32,
    is_immediate: bool,
    capture_quantifiers: &mut CaptureQuantifiers,
) -> TSQueryError {
    if stream.next == 0 {
        return TSQueryErrorSyntax;
    }
    if stream.next == i32::from(b')') || stream.next == i32::from(b']') {
        return PARENT_DONE;
    }

    let starting_step_index = self_.steps.size;

    // Store the byte offset of each step in the query.
    if self_.step_offsets.size == 0
        || array_back_ref(&self_.step_offsets).step_index != starting_step_index as u16
    {
        array_push(
            &mut self_.step_offsets,
            StepOffset {
                step_index: starting_step_index as u16,
                byte_offset: stream_offset(stream),
            },
        );
    }

    // An open bracket is the start of an alternation.
    if stream.next == i32::from(b'[') {
        stream_advance(stream);
        stream_skip_whitespace(stream);

        // Parse each branch, adding a placeholder step in between the branches.
        let mut branch_step_indices: Array<u32> = array_new();
        let mut branch_capture_quantifiers = array_new();
        loop {
            let start_index = self_.steps.size;
            let mut e = ts_query_parse_pattern(
                self_,
                stream,
                depth,
                is_immediate,
                &mut branch_capture_quantifiers,
            );

            if e == PARENT_DONE {
                if stream.next == i32::from(b']') && branch_step_indices.size > 0 {
                    stream_advance(stream);
                    break;
                }
                e = TSQueryErrorSyntax;
            }
            if e != TSQueryErrorNone {
                array_delete(&mut branch_capture_quantifiers);
                array_delete(&mut branch_step_indices);
                return e;
            }

            if start_index == starting_step_index {
                array_assign(capture_quantifiers, &branch_capture_quantifiers);
            } else {
                capture_quantifiers_join_all(capture_quantifiers, &branch_capture_quantifiers);
            }

            array_push(&mut branch_step_indices, start_index);
            array_push(&mut self_.steps, query_step_new(0, depth as u16, false));
            array_clear(&mut branch_capture_quantifiers);
        }
        let _ = array_pop(&mut self_.steps);

        // For all branches except the last, add the subsequent branch as an
        // alternative and link the end of the branch to the current step end.
        for i in 0..branch_step_indices.size - 1 {
            let step_index = *array_get_ref(&branch_step_indices, i);
            let next_step_index = *array_get_ref(&branch_step_indices, i + 1);
            let steps_size = self_.steps.size;
            array_get_mut(&mut self_.steps, step_index).alternative_index = next_step_index as u16;
            let end_step = array_get_mut(&mut self_.steps, next_step_index - 1);
            end_step.alternative_index = steps_size as u16;
            end_step.is_dead_end = true;
        }

        array_delete(&mut branch_capture_quantifiers);
        array_delete(&mut branch_step_indices);
    }
    // An open parenthesis can start a grouped sequence, a predicate, or a node.
    else if stream.next == i32::from(b'(') {
        stream_advance(stream);
        stream_skip_whitespace(stream);

        // Followed by a node: a grouped sequence.
        if stream.next == i32::from(b'(')
            || stream.next == i32::from(b'"')
            || stream.next == i32::from(b'[')
        {
            let mut child_is_immediate = is_immediate;
            let mut child_capture_quantifiers = array_new();
            loop {
                if stream.next == i32::from(b'.') {
                    child_is_immediate = true;
                    stream_advance(stream);
                    stream_skip_whitespace(stream);
                }
                let mut e = ts_query_parse_pattern(
                    self_,
                    stream,
                    depth,
                    child_is_immediate,
                    &mut child_capture_quantifiers,
                );
                if e == PARENT_DONE {
                    if stream.next == i32::from(b')') {
                        stream_advance(stream);
                        break;
                    }
                    e = TSQueryErrorSyntax;
                }
                if e != TSQueryErrorNone {
                    array_delete(&mut child_capture_quantifiers);
                    return e;
                }

                capture_quantifiers_add_all(capture_quantifiers, &child_capture_quantifiers);
                array_clear(&mut child_capture_quantifiers);
                child_is_immediate = false;
            }

            array_delete(&mut child_capture_quantifiers);
        }
        // A dot/pound character indicates the start of a predicate.
        else if stream.next == i32::from(b'.') || stream.next == i32::from(b'#') {
            stream_advance(stream);
            return ts_query_parse_predicate(self_, stream);
        }
        // Otherwise, the start of a named node.
        else {
            let symbol: TSSymbol;
            let mut is_missing = false;
            let node_name = stream.input;

            // Parse a normal node name.
            if stream_is_ident_start(stream) {
                stream_scan_identifier(stream);
                let length = (stream.input as usize - node_name as usize) as u32;
                let node_slice = core::slice::from_raw_parts(node_name, length as usize);

                // Parse the wildcard symbol.
                if length == 1 && *node_name == b'_' {
                    symbol = WILDCARD_SYMBOL;
                } else if b"MISSING".starts_with(node_slice) {
                    is_missing = true;
                    stream_skip_whitespace(stream);

                    if stream_is_ident_start(stream) {
                        let missing_node_name = stream.input;
                        stream_scan_identifier(stream);
                        let missing_node_length =
                            (stream.input as usize - missing_node_name as usize) as u32;
                        symbol = ts_language_symbol_for_name(
                            self_.language,
                            missing_node_name.cast::<i8>(),
                            missing_node_length,
                            true,
                        );
                        if symbol == 0 {
                            stream_reset(stream, missing_node_name);
                            return TSQueryErrorNodeType;
                        }
                    } else if stream.next == i32::from(b'"') {
                        let string_start = stream.input;
                        let e = ts_query_parse_string_literal(self_, stream);
                        if e != TSQueryErrorNone {
                            return e;
                        }
                        symbol = ts_language_symbol_for_name(
                            self_.language,
                            self_.string_buffer.contents.cast::<i8>(),
                            self_.string_buffer.size,
                            false,
                        );
                        if symbol == 0 {
                            stream_reset(stream, string_start.add(1));
                            return TSQueryErrorNodeType;
                        }
                    } else if stream.next == i32::from(b')') {
                        symbol = WILDCARD_SYMBOL;
                    } else {
                        stream_reset(stream, stream.input);
                        return TSQueryErrorSyntax;
                    }
                } else {
                    symbol = ts_language_symbol_for_name(
                        self_.language,
                        node_name.cast::<i8>(),
                        length,
                        true,
                    );
                    if symbol == 0 {
                        stream_reset(stream, node_name);
                        return TSQueryErrorNodeType;
                    }
                }
            } else {
                return TSQueryErrorSyntax;
            }

            // Add a step for the node.
            array_push(
                &mut self_.steps,
                query_step_new(symbol, depth as u16, is_immediate),
            );
            let step_index = self_.steps.size - 1;
            let is_supertype = ts_language_symbol_metadata(self_.language, symbol).supertype;
            {
                let step = array_get_mut(&mut self_.steps, step_index);
                if is_supertype {
                    step.supertype_symbol = step.symbol;
                    step.symbol = WILDCARD_SYMBOL;
                }
                if is_missing {
                    step.is_missing = true;
                }
                if symbol == WILDCARD_SYMBOL {
                    step.is_named = true;
                }
            }

            // Parse a supertype symbol.
            if stream.next == i32::from(b'/') {
                if array_get_ref(&self_.steps, step_index).supertype_symbol == 0 {
                    stream_reset(stream, node_name.sub(1)); // start of the node
                    return TSQueryErrorStructure;
                }

                stream_advance(stream);

                let subtype_node_name = stream.input;
                let new_symbol;
                if stream_is_ident_start(stream) {
                    // Named node.
                    stream_scan_identifier(stream);
                    let length = (stream.input as usize - subtype_node_name as usize) as u32;
                    new_symbol = ts_language_symbol_for_name(
                        self_.language,
                        subtype_node_name.cast::<i8>(),
                        length,
                        true,
                    );
                } else if stream.next == i32::from(b'"') {
                    // Anonymous leaf node.
                    let e = ts_query_parse_string_literal(self_, stream);
                    if e != TSQueryErrorNone {
                        return e;
                    }
                    new_symbol = ts_language_symbol_for_name(
                        self_.language,
                        self_.string_buffer.contents.cast::<i8>(),
                        self_.string_buffer.size,
                        false,
                    );
                } else {
                    return TSQueryErrorSyntax;
                }
                array_get_mut(&mut self_.steps, step_index).symbol = new_symbol;

                if new_symbol == 0 {
                    stream_reset(stream, subtype_node_name);
                    return TSQueryErrorNodeType;
                }

                // Get all the possible subtypes for the given supertype and
                // check whether the given subtype is valid.
                if ts_language_abi_version(self_.language) >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
                    let supertype_symbol = array_get_ref(&self_.steps, step_index).supertype_symbol;
                    let mut subtype_length: u32 = 0;
                    let subtypes =
                        ts_language_subtypes(self_.language, supertype_symbol, &mut subtype_length);

                    let mut subtype_is_valid = false;
                    for i in 0..subtype_length {
                        if *subtypes.add(i as usize)
                            == array_get_ref(&self_.steps, step_index).symbol
                        {
                            subtype_is_valid = true;
                            break;
                        }
                    }

                    // This subtype is not valid for the given supertype.
                    if !subtype_is_valid {
                        stream_reset(stream, node_name.sub(1)); // start of the node
                        return TSQueryErrorStructure;
                    }
                }
            }

            stream_skip_whitespace(stream);

            // Parse the child patterns.
            let mut child_is_immediate = false;
            let mut last_child_step_index: u16 = 0;
            let mut negated_field_count: u16 = 0;
            let mut negated_field_ids: [TSFieldId; MAX_NEGATED_FIELD_COUNT] =
                [0; MAX_NEGATED_FIELD_COUNT];
            let mut child_capture_quantifiers = array_new();
            loop {
                // Parse a negated field assertion.
                if stream.next == i32::from(b'!') {
                    stream_advance(stream);
                    stream_skip_whitespace(stream);
                    if !stream_is_ident_start(stream) {
                        array_delete(&mut child_capture_quantifiers);
                        return TSQueryErrorSyntax;
                    }
                    let field_name = stream.input;
                    stream_scan_identifier(stream);
                    let length = (stream.input as usize - field_name as usize) as u32;
                    stream_skip_whitespace(stream);

                    let field_id = ts_language_field_id_for_name(
                        self_.language,
                        field_name.cast::<i8>(),
                        length,
                    );
                    if field_id == 0 {
                        stream.input = field_name;
                        array_delete(&mut child_capture_quantifiers);
                        return TSQueryErrorField;
                    }

                    // Keep the field ids sorted.
                    if (negated_field_count as usize) < MAX_NEGATED_FIELD_COUNT {
                        negated_field_ids[negated_field_count as usize] = field_id;
                        negated_field_count += 1;
                    }

                    continue;
                }

                // Parse a sibling anchor.
                if stream.next == i32::from(b'.') {
                    child_is_immediate = true;
                    stream_advance(stream);
                    stream_skip_whitespace(stream);
                }

                let mut step_index = self_.steps.size as u16;
                let mut e = ts_query_parse_pattern(
                    self_,
                    stream,
                    depth + 1,
                    child_is_immediate,
                    &mut child_capture_quantifiers,
                );
                // If we only parsed a predicate (no new steps), step back one so
                // we don't index past the end of the array.
                if u32::from(step_index) == self_.steps.size {
                    step_index -= 1;
                }
                if e == PARENT_DONE {
                    if stream.next == i32::from(b')') {
                        if child_is_immediate {
                            if last_child_step_index == 0 {
                                array_delete(&mut child_capture_quantifiers);
                                return TSQueryErrorSyntax;
                            }
                            // Mark this step *and* its alternatives as the last
                            // child of the parent.
                            array_get_mut(&mut self_.steps, u32::from(last_child_step_index))
                                .is_last_child = true;
                            let mut alt =
                                array_get_ref(&self_.steps, u32::from(last_child_step_index))
                                    .alternative_index;
                            if alt != NONE && u32::from(alt) < self_.steps.size {
                                array_get_mut(&mut self_.steps, u32::from(alt)).is_last_child =
                                    true;
                                loop {
                                    let next_alt = array_get_ref(&self_.steps, u32::from(alt))
                                        .alternative_index;
                                    if next_alt != NONE && u32::from(next_alt) < self_.steps.size {
                                        alt = next_alt;
                                        array_get_mut(&mut self_.steps, u32::from(alt))
                                            .is_last_child = true;
                                    } else {
                                        break;
                                    }
                                }
                            }
                        }

                        if negated_field_count != 0 {
                            ts_query_add_negated_fields(
                                self_,
                                starting_step_index as u16,
                                negated_field_ids.as_ptr(),
                                negated_field_count,
                            );
                        }

                        stream_advance(stream);
                        break;
                    }
                    e = TSQueryErrorSyntax;
                }
                if e != TSQueryErrorNone {
                    array_delete(&mut child_capture_quantifiers);
                    return e;
                }

                capture_quantifiers_add_all(capture_quantifiers, &child_capture_quantifiers);

                last_child_step_index = step_index;
                child_is_immediate = false;
                array_clear(&mut child_capture_quantifiers);
            }
            array_delete(&mut child_capture_quantifiers);
        }
    }
    // Parse a wildcard pattern.
    else if stream.next == i32::from(b'_') {
        stream_advance(stream);
        stream_skip_whitespace(stream);

        // Add a step that matches any kind of node.
        array_push(
            &mut self_.steps,
            query_step_new(WILDCARD_SYMBOL, depth as u16, is_immediate),
        );
    }
    // Parse a double-quoted anonymous leaf node expression.
    else if stream.next == i32::from(b'"') {
        let string_start = stream.input;
        let e = ts_query_parse_string_literal(self_, stream);
        if e != TSQueryErrorNone {
            return e;
        }

        // Add a step for the node.
        let symbol = ts_language_symbol_for_name(
            self_.language,
            self_.string_buffer.contents.cast::<i8>(),
            self_.string_buffer.size,
            false,
        );
        if symbol == 0 {
            stream_reset(stream, string_start.add(1));
            return TSQueryErrorNodeType;
        }
        array_push(
            &mut self_.steps,
            query_step_new(symbol, depth as u16, is_immediate),
        );
    }
    // Parse a field-prefixed pattern.
    else if stream_is_ident_start(stream) {
        // Parse the field name.
        let field_name = stream.input;
        stream_scan_identifier(stream);
        let length = (stream.input as usize - field_name as usize) as u32;
        stream_skip_whitespace(stream);

        if stream.next != i32::from(b':') {
            stream_reset(stream, field_name);
            return TSQueryErrorSyntax;
        }
        stream_advance(stream);
        stream_skip_whitespace(stream);

        // Parse the pattern.
        let mut field_capture_quantifiers = array_new();
        let mut e = ts_query_parse_pattern(
            self_,
            stream,
            depth,
            is_immediate,
            &mut field_capture_quantifiers,
        );
        if e != TSQueryErrorNone {
            array_delete(&mut field_capture_quantifiers);
            if e == PARENT_DONE {
                e = TSQueryErrorSyntax;
            }
            return e;
        }

        // Add the field name to the first step of the pattern.
        let field_id =
            ts_language_field_id_for_name(self_.language, field_name.cast::<i8>(), length);
        if field_id == 0 {
            stream.input = field_name;
            array_delete(&mut field_capture_quantifiers);
            return TSQueryErrorField;
        }

        let mut step_index = starting_step_index;
        loop {
            array_get_mut(&mut self_.steps, step_index).field = field_id;
            let alt = array_get_ref(&self_.steps, step_index).alternative_index;
            let steps_size = self_.steps.size;
            if alt != NONE && u32::from(alt) > step_index && u32::from(alt) < steps_size {
                step_index = u32::from(alt);
            } else {
                break;
            }
        }

        capture_quantifiers_add_all(capture_quantifiers, &field_capture_quantifiers);
        array_delete(&mut field_capture_quantifiers);
    } else {
        return TSQueryErrorSyntax;
    }

    stream_skip_whitespace(stream);

    // Parse suffix modifiers for this pattern.
    let mut quantifier = TSQuantifierOne;
    loop {
        // One-or-more operator.
        if stream.next == i32::from(b'+') {
            quantifier = quantifier_join(TSQuantifierOneOrMore, quantifier);
            stream_advance(stream);
            stream_skip_whitespace(stream);
        }
        // Zero-or-more repetition operator.
        else if stream.next == i32::from(b'*') {
            quantifier = quantifier_join(TSQuantifierZeroOrMore, quantifier);
            stream_advance(stream);
            stream_skip_whitespace(stream);
        }
        // Optional operator.
        else if stream.next == i32::from(b'?') {
            quantifier = quantifier_join(TSQuantifierZeroOrOne, quantifier);
            stream_advance(stream);
            stream_skip_whitespace(stream);
        }
        // An '@'-prefixed capture pattern.
        else if stream.next == i32::from(b'@') {
            stream_advance(stream);
            if !stream_is_ident_start(stream) {
                return TSQueryErrorSyntax;
            }
            let capture_name = stream.input;
            stream_scan_identifier(stream);
            let length = (stream.input as usize - capture_name as usize) as u32;
            stream_skip_whitespace(stream);

            // Add the capture id to the first step of the pattern.
            let capture_id = symbol_table_insert_name(&mut self_.captures, capture_name, length);

            // Add the capture quantifier.
            capture_quantifiers_add_for_id(capture_quantifiers, capture_id, TSQuantifierOne);

            let mut step_index = starting_step_index;
            loop {
                query_step_add_capture(array_get_mut(&mut self_.steps, step_index), capture_id);
                let alt = array_get_ref(&self_.steps, step_index).alternative_index;
                let steps_size = self_.steps.size;
                if alt != NONE && u32::from(alt) > step_index && u32::from(alt) < steps_size {
                    step_index = u32::from(alt);
                } else {
                    break;
                }
            }
        }
        // No more suffix modifiers.
        else {
            break;
        }
    }

    match quantifier {
        TSQuantifierOneOrMore => {
            let mut repeat_step = query_step_new(WILDCARD_SYMBOL, depth as u16, false);
            repeat_step.alternative_index = starting_step_index as u16;
            repeat_step.is_pass_through = true;
            repeat_step.alternative_is_immediate = true;
            array_push(&mut self_.steps, repeat_step);
        }
        TSQuantifierZeroOrMore => {
            let mut repeat_step = query_step_new(WILDCARD_SYMBOL, depth as u16, false);
            repeat_step.alternative_index = starting_step_index as u16;
            repeat_step.is_pass_through = true;
            repeat_step.alternative_is_immediate = true;
            array_push(&mut self_.steps, repeat_step);

            // Stop when `alternative_index` is `NONE` or points to `repeat_step`
            // (just pushed at `steps.size - 1`) or beyond.
            let mut step_index = starting_step_index;
            loop {
                let alt = array_get_ref(&self_.steps, step_index).alternative_index;
                if alt != NONE && u32::from(alt) < self_.steps.size - 1 {
                    step_index = u32::from(alt);
                } else {
                    break;
                }
            }
            let size = self_.steps.size;
            array_get_mut(&mut self_.steps, step_index).alternative_index = size as u16;
        }
        TSQuantifierZeroOrOne => {
            let mut step_index = starting_step_index;
            loop {
                let alt = array_get_ref(&self_.steps, step_index).alternative_index;
                if alt != NONE && u32::from(alt) < self_.steps.size {
                    step_index = u32::from(alt);
                } else {
                    break;
                }
            }
            let size = self_.steps.size;
            array_get_mut(&mut self_.steps, step_index).alternative_index = size as u16;
        }
        _ => {}
    }

    capture_quantifiers_mul(capture_quantifiers, quantifier);

    TSQueryErrorNone
}

// ---------------------------------------------------------------------------
// Sorted-array search helpers
// ---------------------------------------------------------------------------
//
// These mirror the C `array_search_sorted_*` macros from array.h. The search
// returns `(index, exists)`: when the key is absent, `index` is the position
// where it should be inserted.

/// Binary search a sorted array by a `u16` key (mirrors `array_search_sorted_by`).
unsafe fn array_search_sorted_by_u16<T>(
    arr: &Array<T>,
    key: impl Fn(&T) -> u16,
    needle: u16,
) -> (u32, bool) {
    let mut index = 0u32;
    let mut exists = false;
    let mut size = arr.size;
    if size == 0 {
        return (index, exists);
    }
    while size > 1 {
        let half_size = size / 2;
        let mid_index = index + half_size;
        if key(array_get_ref(arr, mid_index)) <= needle {
            index = mid_index;
        }
        size -= half_size;
    }
    let value = key(array_get_ref(arr, index));
    if value == needle {
        exists = true;
    } else if value < needle {
        index += 1;
    }
    (index, exists)
}

/// Insert `value` into a `u16`-keyed sorted array if not already present
/// (mirrors `array_insert_sorted_by`).
unsafe fn array_insert_sorted_by_u16<T>(arr: &mut Array<T>, key: impl Fn(&T) -> u16, value: T) {
    let needle = key(&value);
    let (index, exists) = array_search_sorted_by_u16(arr, &key, needle);
    if !exists {
        array_insert(arr, index, value);
    }
}

// ---------------------------------------------------------------------------
// StatePredecessorMap
// ---------------------------------------------------------------------------

unsafe fn state_predecessor_map_new(language: *const TSLanguage) -> StatePredecessorMap {
    StatePredecessorMap {
        contents: calloc(
            ts_language_state_count(language) as usize * (MAX_STATE_PREDECESSOR_COUNT + 1),
            size_of::<TSStateId>(),
        )
        .cast::<TSStateId>(),
    }
}

unsafe fn state_predecessor_map_delete(self_: &mut StatePredecessorMap) {
    free(self_.contents.cast::<c_void>());
}

unsafe fn state_predecessor_map_add(
    self_: &mut StatePredecessorMap,
    state: TSStateId,
    predecessor: TSStateId,
) {
    let index = state as usize * (MAX_STATE_PREDECESSOR_COUNT + 1);
    let count = *self_.contents.add(index);
    if count == 0
        || ((count as usize) < MAX_STATE_PREDECESSOR_COUNT
            && *self_.contents.add(index + count as usize) != predecessor)
    {
        let new_count = count + 1;
        *self_.contents.add(index) = new_count;
        *self_.contents.add(index + new_count as usize) = predecessor;
    }
}

unsafe fn state_predecessor_map_get(
    self_: &StatePredecessorMap,
    state: TSStateId,
    count: &mut u32,
) -> *const TSStateId {
    let index = state as usize * (MAX_STATE_PREDECESSOR_COUNT + 1);
    *count = u32::from(*self_.contents.add(index));
    self_.contents.add(index + 1)
}

// ---------------------------------------------------------------------------
// AnalysisState
// ---------------------------------------------------------------------------

const fn analysis_state_top_index(depth: u16) -> usize {
    if depth == 0 {
        0
    } else {
        (depth - 1) as usize
    }
}

fn analysis_state_recursion_depth(self_: &AnalysisState) -> u32 {
    let mut result = 0;
    for i in 0..self_.depth as usize {
        let symbol = self_.stack[i].parent_symbol;
        for j in 0..i {
            if self_.stack[j].parent_symbol == symbol {
                result += 1;
                break;
            }
        }
    }
    result
}

/// Total ordering used to keep analysis-state sets sorted. Returns a negative,
/// zero, or positive value like the C `analysis_state__compare`.
unsafe fn analysis_state_compare(self_: *const AnalysisState, other: *const AnalysisState) -> i32 {
    let s = &*self_;
    let o = &*other;
    if s.depth < o.depth {
        return 1;
    }
    for i in 0..s.depth as usize {
        if i >= o.depth as usize {
            return -1;
        }
        let s1 = s.stack[i];
        let s2 = o.stack[i];
        if s1.child_index < s2.child_index {
            return -1;
        }
        if s1.child_index > s2.child_index {
            return 1;
        }
        if s1.parent_symbol < s2.parent_symbol {
            return -1;
        }
        if s1.parent_symbol > s2.parent_symbol {
            return 1;
        }
        if s1.parse_state < s2.parse_state {
            return -1;
        }
        if s1.parse_state > s2.parse_state {
            return 1;
        }
        if s1.field_id < s2.field_id {
            return -1;
        }
        if s1.field_id > s2.field_id {
            return 1;
        }
    }
    if s.step_index < o.step_index {
        return -1;
    }
    if s.step_index > o.step_index {
        return 1;
    }
    0
}

fn analysis_state_has_supertype(self_: &AnalysisState, symbol: TSSymbol) -> bool {
    for i in 0..self_.depth as usize {
        if self_.stack[i].parent_symbol == symbol {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// AnalysisStateSet
// ---------------------------------------------------------------------------

/// Obtain an `AnalysisState`, either by consuming one from the pool or by
/// allocating a fresh one, and initialize it from `borrowed_item`.
unsafe fn analysis_state_pool_clone_or_reuse(
    pool: &mut AnalysisStateSet,
    borrowed_item: *const AnalysisState,
) -> *mut AnalysisState {
    let new_item = if pool.size > 0 {
        array_pop(pool)
    } else {
        malloc(size_of::<AnalysisState>()).cast::<AnalysisState>()
    };
    core::ptr::write(new_item, *borrowed_item);
    new_item
}

/// Insert a clone of `borrowed_item` into the set, keeping it sorted and free
/// of duplicates.
unsafe fn analysis_state_set_insert_sorted(
    self_: &mut AnalysisStateSet,
    pool: &mut AnalysisStateSet,
    borrowed_item: *const AnalysisState,
) {
    let (index, exists) = analysis_state_set_search_sorted(self_, borrowed_item);
    if !exists {
        let new_item = analysis_state_pool_clone_or_reuse(pool, borrowed_item);
        array_insert(self_, index, new_item);
    }
}

unsafe fn analysis_state_set_search_sorted(
    self_: &AnalysisStateSet,
    needle: *const AnalysisState,
) -> (u32, bool) {
    let mut index = 0u32;
    let mut exists = false;
    let mut size = self_.size;
    if size == 0 {
        return (index, exists);
    }
    while size > 1 {
        let half_size = size / 2;
        let mid_index = index + half_size;
        if analysis_state_compare(*array_get_ref(self_, mid_index), needle) <= 0 {
            index = mid_index;
        }
        size -= half_size;
    }
    let comparison = analysis_state_compare(*array_get_ref(self_, index), needle);
    if comparison == 0 {
        exists = true;
    } else if comparison < 0 {
        index += 1;
    }
    (index, exists)
}

/// Append a clone of `borrowed_item`. The caller must ensure it is larger than
/// every item already present.
unsafe fn analysis_state_set_push(
    self_: &mut AnalysisStateSet,
    pool: &mut AnalysisStateSet,
    borrowed_item: *const AnalysisState,
) {
    let new_item = analysis_state_pool_clone_or_reuse(pool, borrowed_item);
    array_push(self_, new_item);
}

/// Return all items to the pool, emptying the set.
unsafe fn analysis_state_set_clear(self_: &mut AnalysisStateSet, pool: &mut AnalysisStateSet) {
    array_splice(pool, pool.size, 0, self_.size, self_.contents);
    array_clear(self_);
}

/// Free all memory owned by the set, including its items.
unsafe fn analysis_state_set_delete(self_: &mut AnalysisStateSet) {
    for i in 0..self_.size {
        free((*array_get_ref(self_, i)).cast::<c_void>());
    }
    array_delete(self_);
}

// ---------------------------------------------------------------------------
// QueryAnalysis
// ---------------------------------------------------------------------------

const fn query_analysis_new() -> QueryAnalysis {
    QueryAnalysis {
        states: array_new(),
        next_states: array_new(),
        deeper_states: array_new(),
        state_pool: array_new(),
        final_step_indices: array_new(),
        finished_parent_symbols: array_new(),
        did_abort: false,
    }
}

unsafe fn query_analysis_delete(self_: &mut QueryAnalysis) {
    analysis_state_set_delete(&mut self_.states);
    analysis_state_set_delete(&mut self_.next_states);
    analysis_state_set_delete(&mut self_.deeper_states);
    analysis_state_set_delete(&mut self_.state_pool);
    array_delete(&mut self_.final_step_indices);
    array_delete(&mut self_.finished_parent_symbols);
}

// ---------------------------------------------------------------------------
// AnalysisSubgraphNode
// ---------------------------------------------------------------------------

const fn analysis_subgraph_node_compare(
    self_: AnalysisSubgraphNode,
    other: AnalysisSubgraphNode,
) -> i32 {
    if self_.state < other.state {
        return -1;
    }
    if self_.state > other.state {
        return 1;
    }
    if self_.child_index < other.child_index {
        return -1;
    }
    if self_.child_index > other.child_index {
        return 1;
    }
    if !self_.done && other.done {
        return -1;
    }
    if self_.done && !other.done {
        return 1;
    }
    if self_.production_id < other.production_id {
        return -1;
    }
    if self_.production_id > other.production_id {
        return 1;
    }
    0
}

unsafe fn analysis_subgraph_nodes_search_sorted(
    nodes: &Array<AnalysisSubgraphNode>,
    needle: AnalysisSubgraphNode,
) -> (u32, bool) {
    let mut index = 0u32;
    let mut exists = false;
    let mut size = nodes.size;
    if size == 0 {
        return (index, exists);
    }
    while size > 1 {
        let half_size = size / 2;
        let mid_index = index + half_size;
        if analysis_subgraph_node_compare(*array_get_ref(nodes, mid_index), needle) <= 0 {
            index = mid_index;
        }
        size -= half_size;
    }
    let comparison = analysis_subgraph_node_compare(*array_get_ref(nodes, index), needle);
    if comparison == 0 {
        exists = true;
    } else if comparison < 0 {
        index += 1;
    }
    (index, exists)
}

// ---------------------------------------------------------------------------
// Pattern map
// ---------------------------------------------------------------------------

/// Binary-search the `pattern_map` for `needle` (the root symbol of a pattern).
/// Returns whether the symbol is present; if absent, `*result` is the insertion
/// index.
unsafe fn ts_query_pattern_map_search(self_: &TSQuery, needle: TSSymbol, result: &mut u32) -> bool {
    let mut base_index = u32::from(self_.wildcard_root_pattern_count);
    let mut size = self_.pattern_map.size - base_index;
    if size == 0 {
        *result = base_index;
        return false;
    }
    while size > 1 {
        let half_size = size / 2;
        let mid_index = base_index + half_size;
        let mid_symbol = array_get_ref(
            &self_.steps,
            u32::from(array_get_ref(&self_.pattern_map, mid_index).step_index),
        )
        .symbol;
        if needle > mid_symbol {
            base_index = mid_index;
        }
        size -= half_size;
    }

    let mut symbol = array_get_ref(
        &self_.steps,
        u32::from(array_get_ref(&self_.pattern_map, base_index).step_index),
    )
    .symbol;

    if needle > symbol {
        base_index += 1;
        if base_index < self_.pattern_map.size {
            symbol = array_get_ref(
                &self_.steps,
                u32::from(array_get_ref(&self_.pattern_map, base_index).step_index),
            )
            .symbol;
        }
    }

    *result = base_index;
    needle == symbol
}

/// Insert a new pattern's start index into the pattern map, keeping it ordered
/// by root symbol and then by pattern index.
unsafe fn ts_query_pattern_map_insert(
    self_: &mut TSQuery,
    symbol: TSSymbol,
    new_entry: PatternEntry,
) {
    let mut index = 0u32;
    ts_query_pattern_map_search(self_, symbol, &mut index);

    // Keep entries sorted by symbol and then by pattern_index, so states for
    // earlier patterns are initiated first.
    while index < self_.pattern_map.size {
        let entry = *array_get_ref(&self_.pattern_map, index);
        if array_get_ref(&self_.steps, u32::from(entry.step_index)).symbol == symbol
            && entry.pattern_index < new_entry.pattern_index
        {
            index += 1;
        } else {
            break;
        }
    }

    array_insert(&mut self_.pattern_map, index, new_entry);
}

// ---------------------------------------------------------------------------
// Query analysis
// ---------------------------------------------------------------------------

/// Walk the parse-table subgraph for each parent symbol, tracking all possible
/// sequences of progress through the pattern, to find where matching can
/// terminate. Mirrors `ts_query__perform_analysis`.
unsafe fn ts_query_perform_analysis(
    self_: &mut TSQuery,
    subgraphs: &AnalysisSubgraphArray,
    analysis: &mut QueryAnalysis,
) {
    let mut recursion_depth_limit: u32 = 0;
    let mut prev_final_step_count: u32 = 0;
    array_clear(&mut analysis.final_step_indices);
    array_clear(&mut analysis.finished_parent_symbols);

    let mut iteration: u32 = 0;
    loop {
        if iteration == MAX_ANALYSIS_ITERATION_COUNT {
            analysis.did_abort = true;
            break;
        }

        // If no further progress can be made within the current recursion depth
        // limit, bump it by one and continue processing the states that exceeded
        // the limit — but only if progress has been made since the last bump.
        if analysis.states.size == 0 {
            if analysis.deeper_states.size > 0
                && analysis.final_step_indices.size > prev_final_step_count
            {
                prev_final_step_count = analysis.final_step_indices.size;
                recursion_depth_limit += 1;
                core::mem::swap(&mut analysis.states, &mut analysis.deeper_states);
                iteration += 1;
                continue;
            }
            break;
        }

        analysis_state_set_clear(&mut analysis.next_states, &mut analysis.state_pool);
        let mut j = 0u32;
        while j < analysis.states.size {
            let state = *array_get_ref(&analysis.states, j);

            // Process states in order of ascending position, advancing the
            // least-progressed states first, to avoid processing a state twice.
            if analysis.next_states.size > 0 {
                let comparison =
                    analysis_state_compare(state, *array_back_ref(&analysis.next_states));
                if comparison == 0 {
                    analysis_state_set_insert_sorted(
                        &mut analysis.next_states,
                        &mut analysis.state_pool,
                        state,
                    );
                    j += 1;
                    continue;
                } else if comparison > 0 {
                    while j < analysis.states.size {
                        analysis_state_set_push(
                            &mut analysis.next_states,
                            &mut analysis.state_pool,
                            *array_get_ref(&analysis.states, j),
                        );
                        j += 1;
                    }
                    break;
                }
            }

            let top = analysis_state_top_index((*state).depth);
            let parse_state = (*state).stack[top].parse_state;
            let parent_symbol = (*state).stack[top].parent_symbol;
            let parent_field_id = (*state).stack[top].field_id;
            let child_index = (*state).stack[top].child_index;
            let step = *array_get_ref(&self_.steps, u32::from((*state).step_index));

            let (subgraph_index, exists) =
                array_search_sorted_by_u16(subgraphs, |s| s.symbol, parent_symbol);
            if !exists {
                j += 1;
                continue;
            }
            let subgraph = array_get_ref(subgraphs, subgraph_index);

            // Follow every possible path in the parse table, visiting only states
            // that are part of the subgraph for the current symbol.
            let mut lookahead_iterator = language_lookaheads(self_.language, parse_state);
            while lookahead_iterator__next(&mut lookahead_iterator) {
                let sym = lookahead_iterator.symbol;

                let mut successor = AnalysisSubgraphNode {
                    state: parse_state,
                    production_id: 0,
                    child_index: child_index as u8,
                    done: false,
                };
                if lookahead_iterator.action_count != 0 {
                    let action = lookahead_iterator
                        .actions
                        .add((lookahead_iterator.action_count - 1) as usize);
                    if (*action).type_ == TSParseActionTypeShift {
                        if !(*action).shift.extra {
                            successor.state = (*action).shift.state;
                            successor.child_index += 1;
                        }
                    } else {
                        continue;
                    }
                } else if lookahead_iterator.next_state != 0 {
                    successor.state = lookahead_iterator.next_state;
                    successor.child_index += 1;
                } else {
                    continue;
                }

                let (mut node_index, _exists) =
                    analysis_subgraph_nodes_search_sorted(&subgraph.nodes, successor);
                while node_index < subgraph.nodes.size {
                    let node = *array_get_ref(&subgraph.nodes, node_index);
                    node_index += 1;
                    if node.state != successor.state || node.child_index != successor.child_index {
                        break;
                    }

                    // Use the subgraph to determine the alias and field that
                    // will eventually be applied to this child node.
                    let alias = language_alias_at(
                        self_.language,
                        u32::from(node.production_id),
                        u32::from(child_index),
                    );
                    let visible_symbol = if alias != 0 {
                        alias
                    } else if ts_language_symbol_metadata(self_.language, sym).visible {
                        language_public_symbol(self_.language, sym)
                    } else {
                        0
                    };
                    let mut field_id = parent_field_id;
                    if field_id == 0 {
                        let mut field_map: *const TSFieldMapEntry = core::ptr::null();
                        let mut field_map_end: *const TSFieldMapEntry = core::ptr::null();
                        language_field_map(
                            self_.language,
                            u32::from(node.production_id),
                            &mut field_map,
                            &mut field_map_end,
                        );
                        let mut fm = field_map;
                        while fm != field_map_end {
                            if !(*fm).inherited
                                && u32::from((*fm).child_index) == u32::from(child_index)
                            {
                                field_id = (*fm).field_id;
                                break;
                            }
                            fm = fm.add(1);
                        }
                    }

                    // Create a new state that has advanced past this child.
                    let mut next_state: AnalysisState = *state;
                    let mut ntop = analysis_state_top_index(next_state.depth);
                    next_state.stack[ntop].child_index = u16::from(successor.child_index);
                    next_state.stack[ntop].parse_state = successor.state;
                    if node.done {
                        next_state.stack[ntop].done = true;
                    }

                    // Determine if this child would match the current step.
                    let mut does_match = false;

                    if step.symbol == ts_builtin_sym_error {
                        // ERROR nodes can appear anywhere.
                        does_match = true;
                    } else if visible_symbol != 0 {
                        does_match = true;
                        if step.symbol == WILDCARD_SYMBOL {
                            if step.is_named
                                && !ts_language_symbol_metadata(self_.language, visible_symbol)
                                    .named
                            {
                                does_match = false;
                            }
                        } else if step.symbol != visible_symbol {
                            does_match = false;
                        }
                        if step.field != 0 && step.field != field_id {
                            does_match = false;
                        }
                        if step.supertype_symbol != 0
                            && !analysis_state_has_supertype(&*state, step.supertype_symbol)
                        {
                            does_match = false;
                        }
                    }
                    // If this child is hidden, descend into it. Replace the top
                    // stack entry if it is done, otherwise push a new entry.
                    else if u32::from(sym) >= language_token_count(self_.language) {
                        if !next_state.stack[ntop].done {
                            if next_state.depth as usize + 1 >= MAX_ANALYSIS_STATE_DEPTH {
                                analysis.did_abort = true;
                                continue;
                            }
                            next_state.depth += 1;
                            ntop = analysis_state_top_index(next_state.depth);
                        }

                        next_state.stack[ntop] = AnalysisStateEntry {
                            parse_state,
                            parent_symbol: sym,
                            child_index: 0,
                            field_id,
                            done: false,
                        };

                        if analysis_state_recursion_depth(&next_state) > recursion_depth_limit {
                            analysis_state_set_insert_sorted(
                                &mut analysis.deeper_states,
                                &mut analysis.state_pool,
                                core::ptr::from_ref(&next_state),
                            );
                            continue;
                        }
                    }

                    // Pop the stack when the state reaches the end of a node.
                    while next_state.depth > 0 && next_state.stack[ntop].done {
                        next_state.depth -= 1;
                        ntop = analysis_state_top_index(next_state.depth);
                    }

                    // If this child matched, advance to the next step at the
                    // current depth, skipping descendant steps of the child.
                    let mut next_step = step;
                    if does_match {
                        loop {
                            next_state.step_index += 1;
                            next_step =
                                *array_get_ref(&self_.steps, u32::from(next_state.step_index));
                            if next_step.depth == PATTERN_DONE_MARKER
                                || next_step.depth <= step.depth
                            {
                                break;
                            }
                        }
                    } else if successor.state == parse_state {
                        continue;
                    }

                    loop {
                        // Skip pass-through states (used only for repetitions,
                        // which analysis does not need to process).
                        if next_step.is_pass_through {
                            next_state.step_index += 1;
                            next_step =
                                *array_get_ref(&self_.steps, u32::from(next_state.step_index));
                            continue;
                        }

                        // Record termination, or queue the state for the next
                        // iteration.
                        if !next_step.is_dead_end {
                            let did_finish_pattern = next_step.depth != step.depth;
                            if did_finish_pattern {
                                array_insert_sorted_by_u16(
                                    &mut analysis.finished_parent_symbols,
                                    |x| *x,
                                    (*state).root_symbol,
                                );
                            } else if next_state.depth == 0 {
                                array_insert_sorted_by_u16(
                                    &mut analysis.final_step_indices,
                                    |x| *x,
                                    next_state.step_index,
                                );
                            } else {
                                analysis_state_set_insert_sorted(
                                    &mut analysis.next_states,
                                    &mut analysis.state_pool,
                                    core::ptr::from_ref(&next_state),
                                );
                            }
                        }

                        // Follow an alternative step if there is one.
                        if does_match
                            && next_step.alternative_index != NONE
                            && next_step.alternative_index > next_state.step_index
                        {
                            next_state.step_index = next_step.alternative_index;
                            next_step =
                                *array_get_ref(&self_.steps, u32::from(next_state.step_index));
                        } else {
                            break;
                        }
                    }
                }
            }

            j += 1;
        }

        core::mem::swap(&mut analysis.states, &mut analysis.next_states);
        iteration += 1;
    }
}

const ZERO_ANALYSIS_ENTRY: AnalysisStateEntry = AnalysisStateEntry {
    parse_state: 0,
    parent_symbol: 0,
    child_index: 0,
    field_id: 0,
    done: false,
};

/// Statically analyze every pattern to determine where matching can fail, which
/// patterns can never match, and which repetition symbols can match rootless
/// patterns. Returns `false` (with `*error_offset` set) if a pattern is
/// structurally invalid. Mirrors `ts_query__analyze_patterns`.
unsafe fn ts_query_analyze_patterns(self_: &mut TSQuery, error_offset: &mut u32) -> bool {
    let mut non_rooted_pattern_start_steps: Array<u16> = array_new();
    for i in 0..self_.pattern_map.size {
        let pattern = *array_get_ref(&self_.pattern_map, i);
        if !pattern.is_rooted {
            let step = *array_get_ref(&self_.steps, u32::from(pattern.step_index));
            if step.symbol != WILDCARD_SYMBOL {
                array_push(&mut non_rooted_pattern_start_steps, i as u16);
            }
        }
    }

    // Walk forward through the steps, marking those that contain captures and
    // recording the indices of steps that have child steps.
    let mut parent_step_indices: Array<u32> = array_new();
    let mut all_patterns_are_valid = true;
    for i in 0..self_.steps.size {
        let step = *array_get_ref(&self_.steps, i);
        if step.depth == PATTERN_DONE_MARKER {
            let s = array_get_mut(&mut self_.steps, i);
            s.parent_pattern_guaranteed = true;
            s.root_pattern_guaranteed = true;
            continue;
        }

        let mut has_children = false;
        let is_wildcard = step.symbol == WILDCARD_SYMBOL;
        let mut contains_captures = step.capture_ids[0] != NONE;
        for j in (i + 1)..self_.steps.size {
            let next_step = *array_get_ref(&self_.steps, j);
            if next_step.depth == PATTERN_DONE_MARKER || next_step.depth <= step.depth {
                break;
            }
            if next_step.capture_ids[0] != NONE {
                contains_captures = true;
            }
            if !is_wildcard {
                let ns = array_get_mut(&mut self_.steps, j);
                ns.root_pattern_guaranteed = true;
                ns.parent_pattern_guaranteed = true;
            }
            has_children = true;
        }
        array_get_mut(&mut self_.steps, i).contains_captures = contains_captures;

        if has_children {
            if !is_wildcard {
                array_push(&mut parent_step_indices, i);
            } else if step.supertype_symbol != 0
                && ts_language_abi_version(self_.language) >= LANGUAGE_VERSION_WITH_RESERVED_WORDS
            {
                // Check that all child steps are valid subtypes of this supertype.
                let mut subtype_length: u32 = 0;
                let subtypes = ts_language_subtypes(
                    self_.language,
                    step.supertype_symbol,
                    &mut subtype_length,
                );
                for j in (i + 1)..self_.steps.size {
                    let child_step = *array_get_ref(&self_.steps, j);
                    if child_step.depth == PATTERN_DONE_MARKER || child_step.depth <= step.depth {
                        break;
                    }
                    if child_step.depth == step.depth + 1 && child_step.symbol != WILDCARD_SYMBOL {
                        let mut is_valid_subtype = false;
                        for k in 0..subtype_length {
                            if child_step.symbol == *subtypes.add(k as usize) {
                                is_valid_subtype = true;
                                break;
                            }
                        }
                        if !is_valid_subtype {
                            for offset_idx in 0..self_.step_offsets.size {
                                let step_offset = *array_get_ref(&self_.step_offsets, offset_idx);
                                if u32::from(step_offset.step_index) >= j {
                                    *error_offset = step_offset.byte_offset;
                                    // goto supertype_cleanup
                                    array_delete(&mut non_rooted_pattern_start_steps);
                                    array_delete(&mut parent_step_indices);
                                    return false;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Build an analysis subgraph for every parent symbol in the query, plus
    // every hidden symbol in the grammar (whose children may appear to belong
    // to a parent node).
    let mut subgraphs: AnalysisSubgraphArray = array_new();
    for i in 0..parent_step_indices.size {
        let parent_step_index = *array_get_ref(&parent_step_indices, i);
        let parent_symbol = array_get_ref(&self_.steps, parent_step_index).symbol;
        let subgraph = AnalysisSubgraph {
            symbol: parent_symbol,
            start_states: array_new(),
            nodes: array_new(),
        };
        array_insert_sorted_by_u16(&mut subgraphs, |s| s.symbol, subgraph);
    }
    for sym_u32 in language_token_count(self_.language)..language_symbol_count(self_.language) {
        let sym = sym_u32 as u16;
        if !ts_language_symbol_metadata(self_.language, sym).visible {
            let subgraph = AnalysisSubgraph {
                symbol: sym,
                start_states: array_new(),
                nodes: array_new(),
            };
            array_insert_sorted_by_u16(&mut subgraphs, |s| s.symbol, subgraph);
        }
    }

    // Scan the parse table to populate the subgraphs and the predecessor map.
    let mut predecessor_map = state_predecessor_map_new(self_.language);
    for state_u32 in 1..ts_language_state_count(self_.language) {
        let state = state_u32 as u16;
        let mut lookahead_iterator = language_lookaheads(self_.language, state);
        while lookahead_iterator__next(&mut lookahead_iterator) {
            if lookahead_iterator.action_count != 0 {
                for i in 0..lookahead_iterator.action_count {
                    let action = lookahead_iterator.actions.add(i as usize);
                    if (*action).type_ == TSParseActionTypeReduce {
                        let mut aliases: *const TSSymbol = core::ptr::null();
                        let mut aliases_end: *const TSSymbol = core::ptr::null();
                        language_aliases_for_symbol(
                            self_.language,
                            (*action).reduce.symbol,
                            &mut aliases,
                            &mut aliases_end,
                        );
                        let mut symbol = aliases;
                        while symbol < aliases_end {
                            let (subgraph_index, exists) =
                                array_search_sorted_by_u16(&subgraphs, |s| s.symbol, *symbol);
                            if exists {
                                let subgraph = array_get_mut(&mut subgraphs, subgraph_index);
                                if subgraph.nodes.size == 0
                                    || array_back_ref(&subgraph.nodes).state != state
                                {
                                    array_push(
                                        &mut subgraph.nodes,
                                        AnalysisSubgraphNode {
                                            state,
                                            production_id: (*action).reduce.production_id,
                                            child_index: (*action).reduce.child_count,
                                            done: true,
                                        },
                                    );
                                }
                            }
                            symbol = symbol.add(1);
                        }
                    } else if (*action).type_ == TSParseActionTypeShift && !(*action).shift.extra {
                        let next_state = (*action).shift.state;
                        state_predecessor_map_add(&mut predecessor_map, next_state, state);
                    }
                }
            } else if lookahead_iterator.next_state != 0 {
                if lookahead_iterator.next_state != state {
                    state_predecessor_map_add(
                        &mut predecessor_map,
                        lookahead_iterator.next_state,
                        state,
                    );
                }
                if language_state_is_primary(self_.language, state) {
                    let mut aliases: *const TSSymbol = core::ptr::null();
                    let mut aliases_end: *const TSSymbol = core::ptr::null();
                    language_aliases_for_symbol(
                        self_.language,
                        lookahead_iterator.symbol,
                        &mut aliases,
                        &mut aliases_end,
                    );
                    let mut symbol = aliases;
                    while symbol < aliases_end {
                        let (subgraph_index, exists) =
                            array_search_sorted_by_u16(&subgraphs, |s| s.symbol, *symbol);
                        if exists {
                            let subgraph = array_get_mut(&mut subgraphs, subgraph_index);
                            if subgraph.start_states.size == 0
                                || *array_back_ref(&subgraph.start_states) != state
                            {
                                array_push(&mut subgraph.start_states, state);
                            }
                        }
                        symbol = symbol.add(1);
                    }
                }
            }
        }
    }

    // Walk backward from each subgraph's end states, using the predecessor map
    // to compute the preceding states.
    let mut next_nodes: Array<AnalysisSubgraphNode> = array_new();
    let mut i = 0u32;
    while i < subgraphs.size {
        if array_get_ref(&subgraphs, i).nodes.size == 0 {
            array_delete(&mut array_get_mut(&mut subgraphs, i).start_states);
            array_erase(&mut subgraphs, i);
            continue;
        }
        array_assign(&mut next_nodes, &array_get_ref(&subgraphs, i).nodes);
        while next_nodes.size > 0 {
            let node = array_pop(&mut next_nodes);
            if node.child_index > 1 {
                let mut predecessor_count: u32 = 0;
                let predecessors =
                    state_predecessor_map_get(&predecessor_map, node.state, &mut predecessor_count);
                for j in 0..predecessor_count {
                    let predecessor_node = AnalysisSubgraphNode {
                        state: *predecessors.add(j as usize),
                        child_index: node.child_index - 1,
                        production_id: node.production_id,
                        done: false,
                    };
                    let subgraph = array_get_mut(&mut subgraphs, i);
                    let (index, exists) =
                        analysis_subgraph_nodes_search_sorted(&subgraph.nodes, predecessor_node);
                    if !exists {
                        array_insert(&mut subgraph.nodes, index, predecessor_node);
                        array_push(&mut next_nodes, predecessor_node);
                    }
                }
            }
        }
        i += 1;
    }

    // For each non-terminal pattern, determine whether it can match and where
    // matching could fail.
    let mut analysis = query_analysis_new();
    for i in 0..parent_step_indices.size {
        let parent_step_index = *array_get_ref(&parent_step_indices, i) as u16;
        let parent_depth = array_get_ref(&self_.steps, u32::from(parent_step_index)).depth;
        let parent_symbol = array_get_ref(&self_.steps, u32::from(parent_step_index)).symbol;
        if parent_symbol == ts_builtin_sym_error {
            continue;
        }

        // Find the subgraph for this pattern's root symbol; a terminal root is
        // an error.
        let (subgraph_index, exists) =
            array_search_sorted_by_u16(&subgraphs, |s| s.symbol, parent_symbol);
        if !exists {
            let first_child_step_index = parent_step_index + 1;
            let (j, _child_exists) = array_search_sorted_by_u16(
                &self_.step_offsets,
                |s| s.step_index,
                first_child_step_index,
            );
            *error_offset = array_get_ref(&self_.step_offsets, j).byte_offset;
            all_patterns_are_valid = false;
            break;
        }

        // Initialize an analysis state at every parse state where this parent
        // symbol can occur.
        analysis_state_set_clear(&mut analysis.states, &mut analysis.state_pool);
        analysis_state_set_clear(&mut analysis.deeper_states, &mut analysis.state_pool);
        let start_count = array_get_ref(&subgraphs, subgraph_index).start_states.size;
        for j in 0..start_count {
            let parse_state =
                *array_get_ref(&array_get_ref(&subgraphs, subgraph_index).start_states, j);
            let mut init = AnalysisState {
                stack: [ZERO_ANALYSIS_ENTRY; MAX_ANALYSIS_STATE_DEPTH],
                depth: 1,
                step_index: parent_step_index + 1,
                root_symbol: parent_symbol,
            };
            init.stack[0] = AnalysisStateEntry {
                parse_state,
                parent_symbol,
                child_index: 0,
                field_id: 0,
                done: false,
            };
            analysis_state_set_push(
                &mut analysis.states,
                &mut analysis.state_pool,
                core::ptr::from_ref(&init),
            );
        }

        analysis.did_abort = false;
        ts_query_perform_analysis(self_, &subgraphs, &mut analysis);

        // If analysis was incomplete, every step is fallible.
        if analysis.did_abort {
            let mut j = u32::from(parent_step_index) + 1;
            while j < self_.steps.size {
                let depth = array_get_ref(&self_.steps, j).depth;
                if depth <= parent_depth || depth == PATTERN_DONE_MARKER {
                    break;
                }
                if !array_get_ref(&self_.steps, j).is_dead_end {
                    let s = array_get_mut(&mut self_.steps, j);
                    s.parent_pattern_guaranteed = false;
                    s.root_pattern_guaranteed = false;
                }
                j += 1;
            }
            continue;
        }

        // If this pattern cannot match, record the offending offset.
        if analysis.finished_parent_symbols.size == 0 {
            let impossible_step_index = if analysis.final_step_indices.size > 0 {
                *array_back_ref(&analysis.final_step_indices)
            } else {
                // No final step means the parent step itself is unreachable.
                parent_step_index
            };
            let (mut j, _impossible_exists) = array_search_sorted_by_u16(
                &self_.step_offsets,
                |s| s.step_index,
                impossible_step_index,
            );
            if j >= self_.step_offsets.size {
                j = self_.step_offsets.size - 1;
            }
            *error_offset = array_get_ref(&self_.step_offsets, j).byte_offset;
            all_patterns_are_valid = false;
            break;
        }

        // Mark as fallible any step where a match terminated.
        for j in 0..analysis.final_step_indices.size {
            let final_step_index = *array_get_ref(&analysis.final_step_indices, j);
            let step = *array_get_ref(&self_.steps, u32::from(final_step_index));
            if step.depth != PATTERN_DONE_MARKER && step.depth > parent_depth && !step.is_dead_end {
                let s = array_get_mut(&mut self_.steps, u32::from(final_step_index));
                s.parent_pattern_guaranteed = false;
                s.root_pattern_guaranteed = false;
            }
        }
    }

    // Mark as indefinite any step with captures used in predicates.
    let mut predicate_capture_ids: Array<u16> = array_new();
    for i in 0..self_.patterns.size {
        let pattern = *array_get_ref(&self_.patterns, i);

        // Gather captures used in predicates for this pattern.
        array_clear(&mut predicate_capture_ids);
        let start = pattern.predicate_steps.offset;
        let end = start + pattern.predicate_steps.length;
        for j in start..end {
            let pstep = array_get_ref(&self_.predicate_steps, j);
            if pstep.type_ == TSQueryPredicateStepTypeCapture {
                let value_id = pstep.value_id as u16;
                array_insert_sorted_by_u16(&mut predicate_capture_ids, |x| *x, value_id);
            }
        }

        // Find steps that use these captures.
        let start = pattern.steps.offset;
        let end = start + pattern.steps.length;
        for j in start..end {
            for k in 0..MAX_STEP_CAPTURE_COUNT {
                let capture_id = array_get_ref(&self_.steps, j).capture_ids[k];
                if capture_id == NONE {
                    break;
                }
                let (_index, exists) =
                    array_search_sorted_by_u16(&predicate_capture_ids, |x| *x, capture_id);
                if exists {
                    array_get_mut(&mut self_.steps, j).root_pattern_guaranteed = false;
                    break;
                }
            }
        }
    }

    // Propagate fallibility backward: if a step is fallible, so are its
    // predecessors.
    let mut done = self_.steps.size == 0;
    while !done {
        done = true;
        let mut i = self_.steps.size - 1;
        while i > 0 {
            let step0 = *array_get_ref(&self_.steps, i);
            if step0.depth == PATTERN_DONE_MARKER {
                i -= 1;
                continue;
            }

            // Determine if this step is definite or has definite alternatives.
            let mut parent_pattern_guaranteed = false;
            let mut step = step0;
            loop {
                if step.root_pattern_guaranteed {
                    parent_pattern_guaranteed = true;
                    break;
                }
                if step.alternative_index == NONE || u32::from(step.alternative_index) < i {
                    break;
                }
                step = *array_get_ref(&self_.steps, u32::from(step.alternative_index));
            }

            // If not, mark its predecessor as indefinite.
            if !parent_pattern_guaranteed {
                let prev_step = *array_get_ref(&self_.steps, i - 1);
                if !prev_step.is_dead_end
                    && prev_step.depth != PATTERN_DONE_MARKER
                    && prev_step.root_pattern_guaranteed
                {
                    array_get_mut(&mut self_.steps, i - 1).root_pattern_guaranteed = false;
                    done = false;
                }
            }
            i -= 1;
        }
    }

    // Determine which repetition symbols can match non-rooted patterns; these
    // prevent certain range-restriction optimizations.
    analysis.did_abort = false;
    for i in 0..non_rooted_pattern_start_steps.size {
        let pattern_entry_index = *array_get_ref(&non_rooted_pattern_start_steps, i);
        let pattern_entry = *array_get_ref(&self_.pattern_map, u32::from(pattern_entry_index));

        analysis_state_set_clear(&mut analysis.states, &mut analysis.state_pool);
        analysis_state_set_clear(&mut analysis.deeper_states, &mut analysis.state_pool);
        for j in 0..subgraphs.size {
            let sym = array_get_ref(&subgraphs, j).symbol;
            let metadata = ts_language_symbol_metadata(self_.language, sym);
            if metadata.visible || metadata.named {
                continue;
            }
            let start_count = array_get_ref(&subgraphs, j).start_states.size;
            for k in 0..start_count {
                let parse_state = *array_get_ref(&array_get_ref(&subgraphs, j).start_states, k);
                let mut init = AnalysisState {
                    stack: [ZERO_ANALYSIS_ENTRY; MAX_ANALYSIS_STATE_DEPTH],
                    depth: 1,
                    step_index: pattern_entry.step_index,
                    root_symbol: sym,
                };
                init.stack[0] = AnalysisStateEntry {
                    parse_state,
                    parent_symbol: sym,
                    child_index: 0,
                    field_id: 0,
                    done: false,
                };
                analysis_state_set_push(
                    &mut analysis.states,
                    &mut analysis.state_pool,
                    core::ptr::from_ref(&init),
                );
            }
        }

        ts_query_perform_analysis(self_, &subgraphs, &mut analysis);

        if analysis.finished_parent_symbols.size > 0 {
            array_get_mut(&mut self_.patterns, u32::from(pattern_entry.pattern_index))
                .is_non_local = true;
        }

        for k in 0..analysis.finished_parent_symbols.size {
            let symbol = *array_get_ref(&analysis.finished_parent_symbols, k);
            array_insert_sorted_by_u16(
                &mut self_.repeat_symbols_with_rootless_patterns,
                |x| *x,
                symbol,
            );
        }
    }

    // Cleanup.
    for i in 0..subgraphs.size {
        array_delete(&mut array_get_mut(&mut subgraphs, i).start_states);
        array_delete(&mut array_get_mut(&mut subgraphs, i).nodes);
    }
    array_delete(&mut subgraphs);
    query_analysis_delete(&mut analysis);
    array_delete(&mut next_nodes);
    array_delete(&mut predicate_capture_ids);
    state_predecessor_map_delete(&mut predecessor_map);

    array_delete(&mut non_rooted_pattern_start_steps);
    array_delete(&mut parent_step_indices);

    all_patterns_are_valid
}

// ---------------------------------------------------------------------------
// Public query API
// ---------------------------------------------------------------------------
//
// These are written as `extern "C"` but without `#[no_mangle]` while `query.c`
// is still the live implementation (adding `#[no_mangle]` now would collide
// with the C symbols at link time). Tier 5 adds `#[no_mangle]` and removes
// `query.c` in a single atomic step.

#[no_mangle]
pub unsafe extern "C" fn ts_query_new(
    language: *const TSLanguage,
    source: *const i8,
    source_len: u32,
    error_offset: *mut u32,
    error_type: *mut TSQueryError,
) -> *mut TSQuery {
    if language.is_null()
        || ts_language_abi_version(language) > TREE_SITTER_LANGUAGE_VERSION
        || ts_language_abi_version(language) < TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION
    {
        *error_type = TSQueryErrorLanguage;
        return core::ptr::null_mut();
    }

    let self_ = malloc(size_of::<TSQuery>()).cast::<TSQuery>();
    core::ptr::write(
        self_,
        TSQuery {
            steps: array_new(),
            pattern_map: array_new(),
            captures: symbol_table_new(),
            capture_quantifiers: array_new(),
            predicate_values: symbol_table_new(),
            predicate_steps: array_new(),
            patterns: array_new(),
            step_offsets: array_new(),
            negated_fields: array_new(),
            string_buffer: array_new(),
            repeat_symbols_with_rootless_patterns: array_new(),
            language: ts_language_copy(language),
            wildcard_root_pattern_count: 0,
        },
    );
    let query = &mut *self_;

    array_push(&mut query.negated_fields, 0);

    // Parse all of the S-expressions in the given string.
    let mut stream = stream_new(source.cast::<u8>(), source_len);
    stream_skip_whitespace(&mut stream);
    while stream.input < stream.end {
        let pattern_index = query.patterns.size;
        let start_step_index = query.steps.size;
        let start_predicate_step_index = query.predicate_steps.size;
        array_push(
            &mut query.patterns,
            QueryPattern {
                steps: Slice {
                    offset: start_step_index,
                    length: 0,
                },
                predicate_steps: Slice {
                    offset: start_predicate_step_index,
                    length: 0,
                },
                start_byte: stream_offset(&stream),
                end_byte: 0,
                is_non_local: false,
            },
        );
        let mut capture_quantifiers = array_new();
        *error_type =
            ts_query_parse_pattern(query, &mut stream, 0, false, &mut capture_quantifiers);
        array_push(
            &mut query.steps,
            query_step_new(0, PATTERN_DONE_MARKER, false),
        );

        let steps_size = query.steps.size;
        let predicate_steps_size = query.predicate_steps.size;
        let end_byte = stream_offset(&stream);
        {
            let pattern = array_back_mut(&mut query.patterns);
            pattern.steps.length = steps_size - start_step_index;
            pattern.predicate_steps.length = predicate_steps_size - start_predicate_step_index;
            pattern.end_byte = end_byte;
        }

        // If any pattern could not be parsed, report the error and terminate.
        if *error_type != TSQueryErrorNone {
            if *error_type == PARENT_DONE {
                *error_type = TSQueryErrorSyntax;
            }
            *error_offset = stream_offset(&stream);
            array_delete(&mut capture_quantifiers);
            ts_query_delete(self_);
            return core::ptr::null_mut();
        }

        // Maintain a list of capture quantifiers for each pattern.
        array_push(&mut query.capture_quantifiers, capture_quantifiers);

        // Maintain a map that can look up patterns for a given root symbol.
        let mut wildcard_root_alternative_index = NONE;
        let mut start_step_index = start_step_index;
        loop {
            let step = *array_get_ref(&query.steps, start_step_index);

            // If a pattern has a wildcard at its root but a non-wildcard child,
            // skip matching the wildcard (the cursor checks for a parent later).
            if step.symbol == WILDCARD_SYMBOL && step.depth == 0 && step.field == 0 {
                let second_step = *array_get_ref(&query.steps, start_step_index + 1);
                if second_step.symbol != WILDCARD_SYMBOL
                    && second_step.depth == 1
                    && !second_step.is_immediate
                {
                    wildcard_root_alternative_index = step.alternative_index;
                    start_step_index += 1;
                }
            }

            let step = *array_get_ref(&query.steps, start_step_index);

            // Determine whether the pattern has a single root node.
            let start_depth = step.depth;
            let mut is_rooted = start_depth == 0;
            for step_index in (start_step_index + 1)..query.steps.size {
                let child_step = *array_get_ref(&query.steps, step_index);
                if child_step.is_dead_end {
                    break;
                }
                if child_step.depth == start_depth {
                    is_rooted = false;
                    break;
                }
            }

            ts_query_pattern_map_insert(
                query,
                step.symbol,
                PatternEntry {
                    step_index: start_step_index as u16,
                    pattern_index: pattern_index as u16,
                    is_rooted,
                },
            );
            if step.symbol == WILDCARD_SYMBOL {
                query.wildcard_root_pattern_count += 1;
            }

            // If there are alternatives or options at the root, add multiple
            // entries to the pattern map.
            if step.alternative_index != NONE {
                start_step_index = u32::from(step.alternative_index);
            } else if wildcard_root_alternative_index != NONE {
                start_step_index = u32::from(wildcard_root_alternative_index);
                wildcard_root_alternative_index = NONE;
            } else {
                break;
            }
        }
    }

    if !ts_query_analyze_patterns(query, &mut *error_offset) {
        *error_type = TSQueryErrorStructure;
        ts_query_delete(self_);
        return core::ptr::null_mut();
    }

    array_delete(&mut query.string_buffer);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_delete(self_: *mut TSQuery) {
    if self_.is_null() {
        return;
    }
    let query = &mut *self_;
    array_delete(&mut query.steps);
    array_delete(&mut query.pattern_map);
    array_delete(&mut query.predicate_steps);
    array_delete(&mut query.patterns);
    array_delete(&mut query.step_offsets);
    array_delete(&mut query.string_buffer);
    array_delete(&mut query.negated_fields);
    array_delete(&mut query.repeat_symbols_with_rootless_patterns);
    ts_language_delete(query.language);
    symbol_table_delete(&mut query.captures);
    symbol_table_delete(&mut query.predicate_values);
    for index in 0..query.capture_quantifiers.size {
        array_delete(array_get_mut(&mut query.capture_quantifiers, index));
    }
    array_delete(&mut query.capture_quantifiers);
    free(self_.cast::<c_void>());
}

#[no_mangle]
pub const unsafe extern "C" fn ts_query_pattern_count(self_: *const TSQuery) -> u32 {
    (*self_).patterns.size
}

#[no_mangle]
pub const unsafe extern "C" fn ts_query_capture_count(self_: *const TSQuery) -> u32 {
    (*self_).captures.slices.size
}

#[no_mangle]
pub const unsafe extern "C" fn ts_query_string_count(self_: *const TSQuery) -> u32 {
    (*self_).predicate_values.slices.size
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_capture_name_for_id(
    self_: *const TSQuery,
    index: u32,
    length: *mut u32,
) -> *const i8 {
    symbol_table_name_for_id(&(*self_).captures, index as u16, &mut *length).cast::<i8>()
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_capture_quantifier_for_id(
    self_: *const TSQuery,
    pattern_index: u32,
    capture_index: u32,
) -> TSQuantifier {
    let capture_quantifiers = array_get_ref(&(*self_).capture_quantifiers, pattern_index);
    capture_quantifier_for_id(capture_quantifiers, capture_index as u16)
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_string_value_for_id(
    self_: *const TSQuery,
    index: u32,
    length: *mut u32,
) -> *const i8 {
    symbol_table_name_for_id(&(*self_).predicate_values, index as u16, &mut *length).cast::<i8>()
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_predicates_for_pattern(
    self_: *const TSQuery,
    pattern_index: u32,
    step_count: *mut u32,
) -> *const TSQueryPredicateStep {
    let slice = array_get_ref(&(*self_).patterns, pattern_index).predicate_steps;
    *step_count = slice.length;
    if slice.length == 0 {
        return core::ptr::null();
    }
    core::ptr::from_ref(array_get_ref(&(*self_).predicate_steps, slice.offset))
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_start_byte_for_pattern(
    self_: *const TSQuery,
    pattern_index: u32,
) -> u32 {
    array_get_ref(&(*self_).patterns, pattern_index).start_byte
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_end_byte_for_pattern(
    self_: *const TSQuery,
    pattern_index: u32,
) -> u32 {
    array_get_ref(&(*self_).patterns, pattern_index).end_byte
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_is_pattern_rooted(
    self_: *const TSQuery,
    pattern_index: u32,
) -> bool {
    for i in 0..(*self_).pattern_map.size {
        let entry = array_get_ref(&(*self_).pattern_map, i);
        if u32::from(entry.pattern_index) == pattern_index && !entry.is_rooted {
            return false;
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_is_pattern_non_local(
    self_: *const TSQuery,
    pattern_index: u32,
) -> bool {
    if pattern_index < (*self_).patterns.size {
        array_get_ref(&(*self_).patterns, pattern_index).is_non_local
    } else {
        false
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_is_pattern_guaranteed_at_step(
    self_: *const TSQuery,
    byte_offset: u32,
) -> bool {
    let mut step_index = u32::MAX;
    for i in 0..(*self_).step_offsets.size {
        let step_offset = array_get_ref(&(*self_).step_offsets, i);
        if step_offset.byte_offset > byte_offset {
            break;
        }
        step_index = u32::from(step_offset.step_index);
    }
    if step_index < (*self_).steps.size {
        array_get_ref(&(*self_).steps, step_index).root_pattern_guaranteed
    } else {
        false
    }
}

/// Whether the step at `step_index` could fail to match (internal; `static` in
/// C). Used by the query cursor.
unsafe fn ts_query_step_is_fallible(self_: &TSQuery, step_index: u16) -> bool {
    debug_assert!(u32::from(step_index) + 1 < self_.steps.size);
    let step = array_get_ref(&self_.steps, u32::from(step_index));
    let next_step = array_get_ref(&self_.steps, u32::from(step_index) + 1);
    next_step.depth != PATTERN_DONE_MARKER
        && next_step.depth > step.depth
        && (!next_step.parent_pattern_guaranteed || step.symbol == WILDCARD_SYMBOL)
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_disable_capture(
    self_: *mut TSQuery,
    name: *const i8,
    length: u32,
) {
    // Remove capture information for any pattern step that previously captured
    // with the given name.
    let query = &mut *self_;
    let id = symbol_table_id_for_name(&query.captures, name.cast::<u8>(), length);
    if id != -1 {
        for i in 0..query.steps.size {
            query_step_remove_capture(array_get_mut(&mut query.steps, i), id as u16);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_disable_pattern(self_: *mut TSQuery, pattern_index: u32) {
    // Remove the given pattern from the pattern map. Its steps remain in the
    // `steps` array but will never be read.
    let query = &mut *self_;
    let mut i = 0u32;
    while i < query.pattern_map.size {
        if u32::from(array_get_ref(&query.pattern_map, i).pattern_index) == pattern_index {
            array_erase(&mut query.pattern_map, i);
        } else {
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Query cursor
// ---------------------------------------------------------------------------
//
// The internal cursor helpers take `*mut TSQueryCursor` and use raw-pointer
// field access, mirroring the C source (which threads `TSQueryCursor *`
// everywhere) and avoiding borrow-checker conflicts when mutating the states
// array while iterating it. As with the rest of the query API, the public
// functions are `extern "C"` without `#[no_mangle]` until tier 5.

/// `&TreeCursor` as a `*const TSTreeCursor` (layout-compatible public view).
#[inline]
const fn tc_const(cursor: &TreeCursor) -> *const TSTreeCursor {
    core::ptr::from_ref(cursor).cast::<TSTreeCursor>()
}

/// `&mut TreeCursor` as a `*mut TSTreeCursor`.
#[inline]
fn tc_mut(cursor: &mut TreeCursor) -> *mut TSTreeCursor {
    core::ptr::from_mut(cursor).cast::<TSTreeCursor>()
}

/// The subtree at the cursor's current position (top of its stack).
#[inline]
unsafe fn cursor_current_subtree(cursor: &TreeCursor) -> Subtree {
    *tree_cursor_entry_slice(&cursor.stack)
        .last()
        .unwrap_unchecked()
        .subtree
}

const EMPTY_RANGE: TSRange = TSRange {
    start_point: TSPoint { row: 0, column: 0 },
    end_point: POINT_MAX,
    start_byte: 0,
    end_byte: u32::MAX,
};

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_new() -> *mut TSQueryCursor {
    let self_ = malloc(size_of::<TSQueryCursor>()).cast::<TSQueryCursor>();
    core::ptr::write(
        self_,
        TSQueryCursor {
            query: core::ptr::null(),
            cursor: core::mem::zeroed(),
            states: array_new(),
            finished_states: array_new(),
            capture_list_pool: capture_list_pool_new(),
            depth: 0,
            max_start_depth: u32::MAX,
            included_range: EMPTY_RANGE,
            containing_range: EMPTY_RANGE,
            next_state_id: 0,
            query_options: core::ptr::null(),
            query_state: TSQueryCursorState {
                payload: core::ptr::null_mut(),
                current_byte_offset: 0,
            },
            operation_count: 0,
            on_visible_node: false,
            ascending: false,
            halted: false,
            did_exceed_match_limit: false,
        },
    );
    array_reserve(&mut (*self_).states, 8);
    array_reserve(&mut (*self_).finished_states, 8);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_delete(self_: *mut TSQueryCursor) {
    array_delete(&mut (*self_).states);
    array_delete(&mut (*self_).finished_states);
    ts_tree_cursor_delete(tc_mut(&mut (*self_).cursor));
    capture_list_pool_delete(&mut (*self_).capture_list_pool);
    free(self_.cast::<c_void>());
}

#[no_mangle]
pub const unsafe extern "C" fn ts_query_cursor_did_exceed_match_limit(
    self_: *const TSQueryCursor,
) -> bool {
    (*self_).did_exceed_match_limit
}

#[no_mangle]
pub const unsafe extern "C" fn ts_query_cursor_match_limit(self_: *const TSQueryCursor) -> u32 {
    (*self_).capture_list_pool.max_capture_list_count
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_set_match_limit(self_: *mut TSQueryCursor, limit: u32) {
    (*self_).capture_list_pool.max_capture_list_count = limit;
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_exec(
    self_: *mut TSQueryCursor,
    query: *const TSQuery,
    node: TSNode,
) {
    array_clear(&mut (*self_).states);
    array_clear(&mut (*self_).finished_states);
    ts_tree_cursor_reset(tc_mut(&mut (*self_).cursor), node);
    capture_list_pool_reset(&mut (*self_).capture_list_pool);
    (*self_).on_visible_node = true;
    (*self_).next_state_id = 0;
    (*self_).depth = 0;
    (*self_).ascending = false;
    (*self_).halted = false;
    (*self_).query = query;
    (*self_).did_exceed_match_limit = false;
    (*self_).operation_count = 0;
    (*self_).query_options = core::ptr::null();
    (*self_).query_state = TSQueryCursorState {
        payload: core::ptr::null_mut(),
        current_byte_offset: 0,
    };
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_exec_with_options(
    self_: *mut TSQueryCursor,
    query: *const TSQuery,
    node: TSNode,
    query_options: *const TSQueryCursorOptions,
) {
    ts_query_cursor_exec(self_, query, node);
    if !query_options.is_null() {
        (*self_).query_options = query_options;
        (*self_).query_state = TSQueryCursorState {
            payload: (*query_options).payload,
            current_byte_offset: 0,
        };
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_set_byte_range(
    self_: *mut TSQueryCursor,
    start_byte: u32,
    mut end_byte: u32,
) -> bool {
    if end_byte == 0 {
        end_byte = u32::MAX;
    }
    if start_byte > end_byte {
        return false;
    }
    (*self_).included_range.start_byte = start_byte;
    (*self_).included_range.end_byte = end_byte;
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_set_point_range(
    self_: *mut TSQueryCursor,
    start_point: TSPoint,
    mut end_point: TSPoint,
) -> bool {
    if end_point.row == 0 && end_point.column == 0 {
        end_point = POINT_MAX;
    }
    if point_gt(start_point, end_point) {
        return false;
    }
    (*self_).included_range.start_point = start_point;
    (*self_).included_range.end_point = end_point;
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_set_containing_byte_range(
    self_: *mut TSQueryCursor,
    start_byte: u32,
    mut end_byte: u32,
) -> bool {
    if end_byte == 0 {
        end_byte = u32::MAX;
    }
    if start_byte > end_byte {
        return false;
    }
    (*self_).containing_range.start_byte = start_byte;
    (*self_).containing_range.end_byte = end_byte;
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_set_containing_point_range(
    self_: *mut TSQueryCursor,
    start_point: TSPoint,
    mut end_point: TSPoint,
) -> bool {
    if end_point.row == 0 && end_point.column == 0 {
        end_point = POINT_MAX;
    }
    if point_gt(start_point, end_point) {
        return false;
    }
    (*self_).containing_range.start_point = start_point;
    (*self_).containing_range.end_point = end_point;
    true
}

/// Find the captured node that occurs earliest in the document across all
/// in-progress states. `is_definite` (when non-null) receives whether the
/// chosen capture is definite. Returns `(found, state_index, byte_offset,
/// pattern_index)`.
unsafe fn ts_query_cursor_first_in_progress_capture(
    self_: *mut TSQueryCursor,
    is_definite: *mut bool,
) -> (bool, u32, u32, u32) {
    let mut result = false;
    let mut state_index = u32::MAX;
    let mut byte_offset = u32::MAX;
    let mut pattern_index = u32::MAX;
    let mut i = 0u32;
    while i < (*self_).states.size {
        let state = core::ptr::from_mut::<QueryState>(array_get_mut(&mut (*self_).states, i));
        if (*state).dead {
            i += 1;
            continue;
        }

        let captures =
            capture_list_pool_get(&(*self_).capture_list_pool, (*state).capture_list_id as u16);
        if u32::from((*state).consumed_capture_count) >= captures.size {
            i += 1;
            continue;
        }

        let node = array_get_ref(captures, u32::from((*state).consumed_capture_count)).node;
        if ts_node_end_byte(node) <= (*self_).included_range.start_byte
            || point_lte(ts_node_end_point(node), (*self_).included_range.start_point)
        {
            (*state).consumed_capture_count += 1;
            continue;
        }

        let node_start_byte = ts_node_start_byte(node);
        if !result
            || node_start_byte < byte_offset
            || (node_start_byte == byte_offset && u32::from((*state).pattern_index) < pattern_index)
        {
            let step = array_get_ref(&(*(*self_).query).steps, u32::from((*state).step_index));
            if !is_definite.is_null() {
                // Conservative: the following step must not be immediate, since
                // this capture could be discarded if the next tree symbol isn't
                // the required one.
                *is_definite = step.root_pattern_guaranteed && !step.is_immediate;
            } else if step.root_pattern_guaranteed {
                i += 1;
                continue;
            }

            result = true;
            state_index = i;
            byte_offset = node_start_byte;
            pattern_index = u32::from((*state).pattern_index);
        }
        i += 1;
    }
    (result, state_index, byte_offset, pattern_index)
}

/// Determine which node is first in a depth-first traversal.
unsafe fn ts_query_cursor_compare_nodes(left: TSNode, right: TSNode) -> i32 {
    if left.id != right.id {
        let left_start = ts_node_start_byte(left);
        let right_start = ts_node_start_byte(right);
        if left_start < right_start {
            return -1;
        }
        if left_start > right_start {
            return 1;
        }
        let left_node_count = ts_node_end_byte(left);
        let right_node_count = ts_node_end_byte(right);
        if left_node_count > right_node_count {
            return -1;
        }
        if left_node_count < right_node_count {
            return 1;
        }
    }
    0
}

/// Determine if either state contains a superset of the other's captures.
unsafe fn ts_query_cursor_compare_captures(
    self_: *mut TSQueryCursor,
    left_state: *const QueryState,
    right_state: *const QueryState,
) -> (bool, bool) {
    let left_captures = core::ptr::from_ref::<CaptureList>(capture_list_pool_get(
        &(*self_).capture_list_pool,
        (*left_state).capture_list_id as u16,
    ));
    let right_captures = core::ptr::from_ref::<CaptureList>(capture_list_pool_get(
        &(*self_).capture_list_pool,
        (*right_state).capture_list_id as u16,
    ));
    let mut left_contains_right = true;
    let mut right_contains_left = true;
    let mut i = 0u32;
    let mut j = 0u32;
    loop {
        if i < (*left_captures).size {
            if j < (*right_captures).size {
                let left = array_get_ref(&*left_captures, i);
                let right = array_get_ref(&*right_captures, j);
                if left.node.id == right.node.id && left.index == right.index {
                    i += 1;
                    j += 1;
                } else {
                    match ts_query_cursor_compare_nodes(left.node, right.node) {
                        -1 => {
                            right_contains_left = false;
                            i += 1;
                        }
                        1 => {
                            left_contains_right = false;
                            j += 1;
                        }
                        _ => {
                            right_contains_left = false;
                            left_contains_right = false;
                            i += 1;
                            j += 1;
                        }
                    }
                }
            } else {
                right_contains_left = false;
                break;
            }
        } else {
            if j < (*right_captures).size {
                left_contains_right = false;
            }
            break;
        }
    }
    (left_contains_right, right_contains_left)
}

unsafe fn ts_query_cursor_add_state(self_: *mut TSQueryCursor, pattern: *const PatternEntry) {
    let step = array_get_ref(&(*(*self_).query).steps, u32::from((*pattern).step_index));
    let start_depth = (*self_).depth.wrapping_sub(u32::from(step.depth));
    let needs_parent = step.depth == 1;

    // Keep the states array in ascending order of start_depth and pattern_index.
    let mut index = (*self_).states.size;
    while index > 0 {
        let prev_state = array_get_ref(&(*self_).states, index - 1);
        if u32::from(prev_state.start_depth) < start_depth {
            break;
        }
        if u32::from(prev_state.start_depth) == start_depth {
            // Avoid inserting an unnecessary duplicate state.
            if prev_state.pattern_index == (*pattern).pattern_index
                && prev_state.step_index == (*pattern).step_index
            {
                return;
            }
            if prev_state.pattern_index <= (*pattern).pattern_index {
                break;
            }
        }
        index -= 1;
    }

    array_insert(
        &mut (*self_).states,
        index,
        QueryState {
            id: u32::MAX,
            capture_list_id: u32::from(NONE),
            step_index: (*pattern).step_index,
            pattern_index: (*pattern).pattern_index,
            start_depth: start_depth as u16,
            consumed_capture_count: 0,
            seeking_immediate_match: true,
            has_in_progress_alternatives: false,
            needs_parent,
            dead: false,
        },
    );
}

/// Acquire a capture list for the state, stealing one (and killing the earliest
/// state) if the pool is exhausted. Returns null if none can be obtained.
unsafe fn ts_query_cursor_prepare_to_capture(
    self_: *mut TSQueryCursor,
    capture_list_id: *mut u32,
    state_index_to_preserve: u32,
) -> *mut CaptureList {
    if *capture_list_id == u32::from(NONE) {
        *capture_list_id = u32::from(capture_list_pool_acquire(&mut (*self_).capture_list_pool));

        // If the pool is empty, terminate the state that captured the earliest
        // node and steal its capture list.
        if *capture_list_id == u32::from(NONE) {
            (*self_).did_exceed_match_limit = true;
            let (found, state_index, _byte_offset, _pattern_index) =
                ts_query_cursor_first_in_progress_capture(self_, core::ptr::null_mut());
            if found && state_index != state_index_to_preserve {
                let other_state = core::ptr::from_mut::<QueryState>(array_get_mut(
                    &mut (*self_).states,
                    state_index,
                ));
                *capture_list_id = (*other_state).capture_list_id;
                (*other_state).capture_list_id = u32::from(NONE);
                (*other_state).dead = true;
                let list = capture_list_pool_get_mut(
                    &mut (*self_).capture_list_pool,
                    *capture_list_id as u16,
                );
                array_clear(list);
                return list;
            }
            return core::ptr::null_mut();
        }
    }
    capture_list_pool_get_mut(&mut (*self_).capture_list_pool, *capture_list_id as u16)
}

unsafe fn ts_query_cursor_capture(
    self_: *mut TSQueryCursor,
    state: *mut QueryState,
    step: *const QueryStep,
    node: TSNode,
) {
    if (*state).dead {
        return;
    }
    let capture_list = ts_query_cursor_prepare_to_capture(
        self_,
        core::ptr::addr_of_mut!((*state).capture_list_id),
        u32::MAX,
    );
    if capture_list.is_null() {
        (*state).dead = true;
        return;
    }

    for j in 0..MAX_STEP_CAPTURE_COUNT {
        let capture_id = (*step).capture_ids[j];
        if capture_id == NONE {
            break;
        }
        array_push(
            &mut *capture_list,
            TSQueryCapture {
                node,
                index: u32::from(capture_id),
            },
        );
    }
}

/// Duplicate the state at `state_index`, inserting the copy immediately after.
/// Returns the index of the copy, or `None` if a capture list could not be
/// obtained.
unsafe fn ts_query_cursor_copy_state(self_: *mut TSQueryCursor, state_index: u32) -> Option<u32> {
    let mut copy = *array_get_ref(&(*self_).states, state_index);
    let original_capture_list_id = copy.capture_list_id;
    copy.capture_list_id = u32::from(NONE);

    // If the state has captures, copy its capture list.
    if original_capture_list_id != u32::from(NONE) {
        let new_captures = ts_query_cursor_prepare_to_capture(
            self_,
            core::ptr::addr_of_mut!(copy.capture_list_id),
            state_index,
        );
        if new_captures.is_null() {
            return None;
        }
        let old_captures = core::ptr::from_ref::<CaptureList>(capture_list_pool_get(
            &(*self_).capture_list_pool,
            original_capture_list_id as u16,
        ));
        array_splice(
            &mut *new_captures,
            (*new_captures).size,
            0,
            (*old_captures).size,
            (*old_captures).contents,
        );
    }

    array_insert(&mut (*self_).states, state_index + 1, copy);
    Some(state_index + 1)
}

unsafe fn ts_query_cursor_should_descend(
    self_: *mut TSQueryCursor,
    node_intersects_range: bool,
) -> bool {
    if node_intersects_range && (*self_).depth < (*self_).max_start_depth {
        return true;
    }

    // Descend if any in-progress match has remaining steps deeper in the tree.
    for i in 0..(*self_).states.size {
        let state = array_get_ref(&(*self_).states, i);
        let next_step = array_get_ref(&(*(*self_).query).steps, u32::from(state.step_index));
        if next_step.depth != PATTERN_DONE_MARKER
            && u32::from(state.start_depth) + u32::from(next_step.depth) > (*self_).depth
        {
            return true;
        }
    }

    if (*self_).depth >= (*self_).max_start_depth {
        return false;
    }

    // A hidden node may contain a root of a non-rooted pattern; descend, but
    // avoid expensive repetition nodes unless this query can match rootless
    // patterns within them.
    if !(*self_).on_visible_node {
        let subtree = cursor_current_subtree(&(*self_).cursor);
        if subtree_is_repetition(subtree) != 0 {
            let (_index, exists) = array_search_sorted_by_u16(
                &(*(*self_).query).repeat_symbols_with_rootless_patterns,
                |x| *x,
                subtree_symbol(subtree),
            );
            return exists;
        }
        return true;
    }

    false
}

const fn range_intersects(a: &TSRange, b: &TSRange) -> bool {
    let is_empty = a.start_byte == a.end_byte;
    (a.end_byte > b.start_byte || (is_empty && a.end_byte == b.start_byte))
        && (point_gt(a.end_point, b.start_point)
            || (is_empty && point_eq(a.end_point, b.start_point)))
        && a.start_byte < b.end_byte
        && point_lt(a.start_point, b.end_point)
}

const fn range_within(a: &TSRange, b: &TSRange) -> bool {
    a.start_byte >= b.start_byte
        && point_gte(a.start_point, b.start_point)
        && a.end_byte <= b.end_byte
        && point_lte(a.end_point, b.end_point)
}

/// Walk the tree, processing patterns until at least one finishes (its state is
/// stored in `finished_states`) or there are no more matches. Returns whether a
/// pattern finished.
unsafe fn ts_query_cursor_advance(self_: *mut TSQueryCursor, stop_on_definite_step: bool) -> bool {
    let mut did_match = false;
    loop {
        if (*self_).halted {
            while (*self_).states.size > 0 {
                let state = array_pop(&mut (*self_).states);
                capture_list_pool_release(
                    &mut (*self_).capture_list_pool,
                    state.capture_list_id as u16,
                );
            }
        }

        (*self_).operation_count += 1;
        if (*self_).operation_count == OP_COUNT_PER_QUERY_CALLBACK_CHECK {
            (*self_).operation_count = 0;
        }

        if !(*self_).query_options.is_null()
            && (*(*self_).query_options).progress_callback.is_some()
        {
            (*self_).query_state.current_byte_offset =
                ts_node_start_byte(ts_tree_cursor_current_node(tc_const(&(*self_).cursor)));
        }
        let callback_halt = (*self_).operation_count == 0
            && !(*self_).query_options.is_null()
            && (*(*self_).query_options).progress_callback.is_some()
            && ((*(*self_).query_options)
                .progress_callback
                .unwrap_unchecked())(core::ptr::addr_of_mut!((*self_).query_state));
        if did_match || (*self_).halted || callback_halt {
            return did_match;
        }

        // Exit the current node.
        if (*self_).ascending {
            if (*self_).on_visible_node {
                // Remove states that cannot make further progress.
                let mut deleted_count = 0u32;
                let n = (*self_).states.size;
                let mut i = 0u32;
                while i < n {
                    let state = *array_get_ref(&(*self_).states, i);
                    let step =
                        *array_get_ref(&(*(*self_).query).steps, u32::from(state.step_index));

                    if step.depth == PATTERN_DONE_MARKER
                        && (u32::from(state.start_depth) > (*self_).depth || (*self_).depth == 0)
                    {
                        // Pattern completed inside this node but was deferred.
                        array_push(&mut (*self_).finished_states, state);
                        did_match = true;
                        deleted_count += 1;
                    } else if step.depth != PATTERN_DONE_MARKER
                        && u32::from(state.start_depth) + u32::from(step.depth) > (*self_).depth
                    {
                        // Needed to match within this node, but failed.
                        capture_list_pool_release(
                            &mut (*self_).capture_list_pool,
                            state.capture_list_id as u16,
                        );
                        deleted_count += 1;
                    } else if deleted_count > 0 {
                        *array_get_mut(&mut (*self_).states, i - deleted_count) = state;
                    }
                    i += 1;
                }
                (*self_).states.size -= deleted_count;
            }

            // Step to the next sibling or up to the parent.
            match tree_cursor_goto_next_sibling_internal(&mut (*self_).cursor) {
                TreeCursorStep::Visible => {
                    if !(*self_).on_visible_node {
                        (*self_).depth = (*self_).depth.wrapping_add(1);
                        (*self_).on_visible_node = true;
                    }
                    (*self_).ascending = false;
                }
                TreeCursorStep::Hidden => {
                    if (*self_).on_visible_node {
                        (*self_).depth = (*self_).depth.wrapping_sub(1);
                        (*self_).on_visible_node = false;
                    }
                    (*self_).ascending = false;
                }
                TreeCursorStep::None => {
                    if ts_tree_cursor_goto_parent(tc_mut(&mut (*self_).cursor)) {
                        (*self_).depth = (*self_).depth.wrapping_sub(1);
                    } else {
                        (*self_).halted = true;
                    }
                }
            }
        }
        // Enter a new node.
        else {
            let node = ts_tree_cursor_current_node(tc_const(&(*self_).cursor));
            let parent_node = ts_tree_cursor_parent_node(tc_const(&(*self_).cursor));

            let parent_intersects_range = ts_node_is_null(parent_node)
                || range_intersects(
                    &TSRange {
                        start_point: ts_node_start_point(parent_node),
                        end_point: ts_node_end_point(parent_node),
                        start_byte: ts_node_start_byte(parent_node),
                        end_byte: ts_node_end_byte(parent_node),
                    },
                    &(*self_).included_range,
                );
            let node_range = TSRange {
                start_point: ts_node_start_point(node),
                end_point: ts_node_end_point(node),
                start_byte: ts_node_start_byte(node),
                end_byte: ts_node_end_byte(node),
            };
            let node_intersects_range =
                parent_intersects_range && range_intersects(&node_range, &(*self_).included_range);
            let node_intersects_containing_range =
                range_intersects(&node_range, &(*self_).containing_range);
            let node_within_containing_range =
                range_within(&node_range, &(*self_).containing_range);

            if node_within_containing_range && (*self_).on_visible_node {
                let symbol = ts_node_symbol(node);
                let is_named = ts_node_is_named(node);
                let is_missing = ts_node_is_missing(node);
                let mut field_id: TSFieldId = 0;
                let mut has_later_siblings = false;
                let mut has_later_named_siblings = false;
                let mut can_have_later_siblings_with_this_field = false;
                let mut supertypes: [TSSymbol; 8] = [0; 8];
                let mut supertype_count: u32 = 8;
                ts_tree_cursor_current_status(
                    tc_const(&(*self_).cursor),
                    &mut field_id,
                    &mut has_later_siblings,
                    &mut has_later_named_siblings,
                    &mut can_have_later_siblings_with_this_field,
                    supertypes.as_mut_ptr(),
                    &mut supertype_count,
                );

                let node_is_error = symbol == ts_builtin_sym_error;
                let parent_is_error = !ts_node_is_null(parent_node)
                    && ts_node_symbol(parent_node) == ts_builtin_sym_error;

                // Add states for patterns whose root node is a wildcard.
                if !node_is_error {
                    for i in 0..u32::from((*(*self_).query).wildcard_root_pattern_count) {
                        let pattern = core::ptr::from_ref::<PatternEntry>(array_get_ref(
                            &(*(*self_).query).pattern_map,
                            i,
                        ));
                        let step = array_get_ref(
                            &(*(*self_).query).steps,
                            u32::from((*pattern).step_index),
                        );
                        let start_depth = (*self_).depth.wrapping_sub(u32::from(step.depth));
                        if (if (*pattern).is_rooted {
                            node_intersects_range
                        } else {
                            parent_intersects_range && !parent_is_error
                        }) && (step.field == 0 || field_id == step.field)
                            && (step.supertype_symbol == 0 || supertype_count > 0)
                            && (start_depth <= (*self_).max_start_depth)
                        {
                            ts_query_cursor_add_state(self_, pattern);
                        }
                    }
                }

                // Add states for patterns whose root node matches this node.
                let mut i: u32 = 0;
                if ts_query_pattern_map_search(&*(*self_).query, symbol, &mut i) {
                    let mut pattern = core::ptr::from_ref::<PatternEntry>(array_get_ref(
                        &(*(*self_).query).pattern_map,
                        i,
                    ));
                    let mut step =
                        *array_get_ref(&(*(*self_).query).steps, u32::from((*pattern).step_index));
                    loop {
                        let start_depth = (*self_).depth.wrapping_sub(u32::from(step.depth));
                        if (if (*pattern).is_rooted {
                            node_intersects_range
                        } else {
                            parent_intersects_range && !parent_is_error
                        }) && (step.field == 0 || field_id == step.field)
                            && (start_depth <= (*self_).max_start_depth)
                        {
                            ts_query_cursor_add_state(self_, pattern);
                        }

                        i += 1;
                        if i == (*(*self_).query).pattern_map.size {
                            break;
                        }
                        pattern = core::ptr::from_ref::<PatternEntry>(array_get_ref(
                            &(*(*self_).query).pattern_map,
                            i,
                        ));
                        step = *array_get_ref(
                            &(*(*self_).query).steps,
                            u32::from((*pattern).step_index),
                        );
                        if step.symbol != symbol {
                            break;
                        }
                    }
                }

                // Update all in-progress states with the current node.
                let mut j: u32 = 0;
                while j < (*self_).states.size {
                    let mut state =
                        core::ptr::from_mut::<QueryState>(array_get_mut(&mut (*self_).states, j));
                    let step =
                        *array_get_ref(&(*(*self_).query).steps, u32::from((*state).step_index));
                    (*state).has_in_progress_alternatives = false;
                    let mut copy_count = 0u32;

                    if u32::from((*state).start_depth) + u32::from(step.depth) != (*self_).depth {
                        j += 1;
                        continue;
                    }

                    // Determine whether the node matches this step and whether a
                    // later sibling could match it.
                    let mut node_does_match;
                    if step.symbol == WILDCARD_SYMBOL {
                        if step.is_missing {
                            node_does_match = is_missing;
                        } else {
                            node_does_match = !node_is_error && (is_named || !step.is_named);
                        }
                    } else {
                        node_does_match = symbol == step.symbol && (!step.is_missing || is_missing);
                    }
                    let mut later_sibling_can_match =
                        if (step.is_immediate && is_named) || (*state).seeking_immediate_match {
                            false
                        } else {
                            has_later_siblings
                        };
                    if step.is_last_child && has_later_named_siblings {
                        node_does_match = false;
                    }
                    if step.supertype_symbol != 0 {
                        let mut has_supertype = false;
                        for k in 0..supertype_count {
                            if supertypes[k as usize] == step.supertype_symbol {
                                has_supertype = true;
                                break;
                            }
                        }
                        if !has_supertype {
                            node_does_match = false;
                        }
                    }
                    if step.field != 0 {
                        if step.field == field_id {
                            if !can_have_later_siblings_with_this_field {
                                later_sibling_can_match = false;
                            }
                        } else {
                            node_does_match = false;
                        }
                    }

                    if step.negated_field_list_id != 0 {
                        let mut idx = u32::from(step.negated_field_list_id);
                        loop {
                            let negated_field_id =
                                *array_get_ref(&(*(*self_).query).negated_fields, idx);
                            if negated_field_id != 0 {
                                idx += 1;
                                if !ts_node_child_by_field_id(node, negated_field_id)
                                    .id
                                    .is_null()
                                {
                                    node_does_match = false;
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }

                    // Remove the state immediately if it can never match.
                    if !node_does_match {
                        if later_sibling_can_match {
                            j += 1;
                        } else {
                            capture_list_pool_release(
                                &mut (*self_).capture_list_pool,
                                (*state).capture_list_id as u16,
                            );
                            array_erase(&mut (*self_).states, j);
                        }
                        continue;
                    }

                    // Split the state if it could also match a later sibling.
                    if later_sibling_can_match
                        && (step.contains_captures
                            || ts_query_step_is_fallible(&*(*self_).query, (*state).step_index))
                        && ts_query_cursor_copy_state(self_, j).is_some()
                    {
                        copy_count += 1;
                    }
                    // The states array may have moved; re-fetch the state.
                    state =
                        core::ptr::from_mut::<QueryState>(array_get_mut(&mut (*self_).states, j));

                    // If this pattern started with a wildcard (pattern map points
                    // to its second step), require a parent and capture it.
                    if (*state).needs_parent {
                        let parent = ts_tree_cursor_parent_node(tc_const(&(*self_).cursor));
                        if ts_node_is_null(parent) {
                            (*state).dead = true;
                        } else {
                            (*state).needs_parent = false;
                            let mut sw_index = u32::from((*state).step_index);
                            loop {
                                sw_index -= 1;
                                let sw = *array_get_ref(&(*(*self_).query).steps, sw_index);
                                if !(sw.is_dead_end || sw.is_pass_through || sw.depth > 0) {
                                    break;
                                }
                            }
                            if array_get_ref(&(*(*self_).query).steps, sw_index).capture_ids[0]
                                != NONE
                            {
                                let sw_step = core::ptr::from_ref::<QueryStep>(array_get_ref(
                                    &(*(*self_).query).steps,
                                    sw_index,
                                ));
                                ts_query_cursor_capture(self_, state, sw_step, parent);
                            }
                        }
                    }

                    // Capture the current node if needed.
                    if step.capture_ids[0] != NONE {
                        ts_query_cursor_capture(self_, state, core::ptr::from_ref(&step), node);
                    }

                    if (*state).dead {
                        array_erase(&mut (*self_).states, j);
                        // C does `j--; continue;`, hitting the `j += 1 + copy_count`
                        // loop step for a net `j += copy_count`; this skips over the
                        // split copy that was inserted at j+1 (and shifted into j by
                        // the erase), so it is not re-matched against the current node.
                        j += copy_count;
                        continue;
                    }

                    // Advance the state to the next step.
                    (*state).step_index += 1;
                    let next_step =
                        *array_get_ref(&(*(*self_).query).steps, u32::from((*state).step_index));

                    // Special-case unnamed wildcard immediately followed by an
                    // immediate step: keep seeking an immediate match.
                    (*state).seeking_immediate_match =
                        step.symbol == WILDCARD_SYMBOL && !step.is_named && next_step.is_immediate;

                    if stop_on_definite_step && next_step.root_pattern_guaranteed {
                        did_match = true;
                    }

                    // Expand the state's alternative-step chain by copying.
                    let mut end_index = j + 1;
                    let mut k = j;
                    while k < end_index {
                        let child_state = core::ptr::from_mut::<QueryState>(array_get_mut(
                            &mut (*self_).states,
                            k,
                        ));
                        let child_step = *array_get_ref(
                            &(*(*self_).query).steps,
                            u32::from((*child_state).step_index),
                        );
                        if child_step.alternative_index != NONE {
                            // A dead-end step jumps straight to its alternative.
                            if child_step.is_dead_end {
                                (*child_state).step_index = child_step.alternative_index;
                                continue;
                            }
                            // A pass-through step splits, then advances.
                            if child_step.is_pass_through {
                                (*child_state).step_index += 1;
                            }
                            if let Some(copy_index) = ts_query_cursor_copy_state(self_, k) {
                                end_index += 1;
                                copy_count += 1;
                                let copy = core::ptr::from_mut::<QueryState>(array_get_mut(
                                    &mut (*self_).states,
                                    copy_index,
                                ));
                                (*copy).step_index = child_step.alternative_index;
                                if child_step.alternative_is_immediate {
                                    (*copy).seeking_immediate_match = true;
                                }
                            }
                            if child_step.is_pass_through {
                                continue;
                            }
                        }
                        k += 1;
                    }

                    j += 1 + copy_count;
                }

                // Enforce the longest-match criteria, finishing completed states.
                let mut j: u32 = 0;
                while j < (*self_).states.size {
                    let state =
                        core::ptr::from_mut::<QueryState>(array_get_mut(&mut (*self_).states, j));
                    if (*state).dead {
                        array_erase(&mut (*self_).states, j);
                        continue;
                    }

                    let mut did_remove = false;
                    let mut k = j + 1;
                    while k < (*self_).states.size {
                        let other_state = core::ptr::from_mut::<QueryState>(array_get_mut(
                            &mut (*self_).states,
                            k,
                        ));
                        // States are sorted by start_depth and pattern_index.
                        if (*other_state).start_depth != (*state).start_depth
                            || (*other_state).pattern_index != (*state).pattern_index
                        {
                            break;
                        }
                        let (left_contains_right, right_contains_left) =
                            ts_query_cursor_compare_captures(self_, state, other_state);
                        if left_contains_right {
                            if (*state).step_index == (*other_state).step_index {
                                capture_list_pool_release(
                                    &mut (*self_).capture_list_pool,
                                    (*other_state).capture_list_id as u16,
                                );
                                array_erase(&mut (*self_).states, k);
                                continue;
                            }
                            (*other_state).has_in_progress_alternatives = true;
                        }
                        if right_contains_left {
                            if (*state).step_index == (*other_state).step_index {
                                capture_list_pool_release(
                                    &mut (*self_).capture_list_pool,
                                    (*state).capture_list_id as u16,
                                );
                                array_erase(&mut (*self_).states, j);
                                did_remove = true;
                                break;
                            }
                            (*state).has_in_progress_alternatives = true;
                        }
                        k += 1;
                    }

                    // If the state is at the end of its pattern, finish it.
                    if !did_remove {
                        let next_step = *array_get_ref(
                            &(*(*self_).query).steps,
                            u32::from((*state).step_index),
                        );
                        if next_step.depth == PATTERN_DONE_MARKER {
                            if (*state).has_in_progress_alternatives {
                                // defer finishing
                                j += 1;
                            } else {
                                array_push(&mut (*self_).finished_states, *state);
                                array_erase(&mut (*self_).states, j);
                                did_match = true;
                            }
                        } else {
                            j += 1;
                        }
                    }
                    // (if did_remove, state j was erased; revisit j)
                }
            }

            if node_intersects_containing_range
                && ts_query_cursor_should_descend(self_, node_intersects_range)
            {
                match tree_cursor_goto_first_child_internal(&mut (*self_).cursor) {
                    TreeCursorStep::Visible => {
                        (*self_).depth = (*self_).depth.wrapping_add(1);
                        (*self_).on_visible_node = true;
                        continue;
                    }
                    TreeCursorStep::Hidden => {
                        (*self_).on_visible_node = false;
                        continue;
                    }
                    TreeCursorStep::None => {}
                }
            }

            (*self_).ascending = true;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_next_match(
    self_: *mut TSQueryCursor,
    match_: *mut TSQueryMatch,
) -> bool {
    if (*self_).finished_states.size == 0 && !ts_query_cursor_advance(self_, false) {
        return false;
    }

    let state = core::ptr::from_mut::<QueryState>(array_get_mut(&mut (*self_).finished_states, 0));
    if (*state).id == u32::MAX {
        (*state).id = (*self_).next_state_id;
        (*self_).next_state_id += 1;
    }
    (*match_).id = (*state).id;
    (*match_).pattern_index = (*state).pattern_index;
    let captures =
        capture_list_pool_get(&(*self_).capture_list_pool, (*state).capture_list_id as u16);
    (*match_).captures = captures.contents;
    (*match_).capture_count = captures.size as u16;
    capture_list_pool_release(
        &mut (*self_).capture_list_pool,
        (*state).capture_list_id as u16,
    );
    array_erase(&mut (*self_).finished_states, 0);
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_remove_match(self_: *mut TSQueryCursor, match_id: u32) {
    for i in 0..(*self_).finished_states.size {
        let state = array_get_ref(&(*self_).finished_states, i);
        if state.id == match_id {
            capture_list_pool_release(
                &mut (*self_).capture_list_pool,
                state.capture_list_id as u16,
            );
            array_erase(&mut (*self_).finished_states, i);
            return;
        }
    }

    // Remove unfinished states too, to prevent future captures for the match.
    for i in 0..(*self_).states.size {
        let state = array_get_ref(&(*self_).states, i);
        if state.id == match_id {
            capture_list_pool_release(
                &mut (*self_).capture_list_pool,
                state.capture_list_id as u16,
            );
            array_erase(&mut (*self_).states, i);
            return;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_next_capture(
    self_: *mut TSQueryCursor,
    match_: *mut TSQueryMatch,
    capture_index: *mut u32,
) -> bool {
    // Return captures in document order even though they may be discovered out
    // of order, since patterns can overlap.
    loop {
        // Find the earliest capture in an unfinished match.
        let mut first_unfinished_state_is_definite = false;
        let (
            found_unfinished_state,
            first_unfinished_state_index,
            first_unfinished_capture_byte,
            first_unfinished_pattern_index,
        ) = ts_query_cursor_first_in_progress_capture(
            self_,
            &mut first_unfinished_state_is_definite,
        );

        // Then find the earliest capture in a finished match (must precede the
        // first unfinished capture).
        let mut first_finished_state: *mut QueryState = core::ptr::null_mut();
        let mut first_finished_capture_byte = first_unfinished_capture_byte;
        let mut first_finished_pattern_index = first_unfinished_pattern_index;
        let mut i = 0u32;
        while i < (*self_).finished_states.size {
            let state =
                core::ptr::from_mut::<QueryState>(array_get_mut(&mut (*self_).finished_states, i));
            let captures =
                capture_list_pool_get(&(*self_).capture_list_pool, (*state).capture_list_id as u16);

            // Remove states whose captures are all consumed.
            if u32::from((*state).consumed_capture_count) >= captures.size {
                capture_list_pool_release(
                    &mut (*self_).capture_list_pool,
                    (*state).capture_list_id as u16,
                );
                array_erase(&mut (*self_).finished_states, i);
                continue;
            }

            let node = array_get_ref(captures, u32::from((*state).consumed_capture_count)).node;

            let node_precedes_range = ts_node_end_byte(node) <= (*self_).included_range.start_byte
                || point_lte(ts_node_end_point(node), (*self_).included_range.start_point);
            let node_follows_range = ts_node_start_byte(node) >= (*self_).included_range.end_byte
                || point_gte(ts_node_start_point(node), (*self_).included_range.end_point);
            if node_precedes_range || node_follows_range {
                (*state).consumed_capture_count += 1;
                continue;
            }

            let node_start_byte = ts_node_start_byte(node);
            if node_start_byte < first_finished_capture_byte
                || (node_start_byte == first_finished_capture_byte
                    && u32::from((*state).pattern_index) < first_finished_pattern_index)
            {
                first_finished_state = state;
                first_finished_capture_byte = node_start_byte;
                first_finished_pattern_index = u32::from((*state).pattern_index);
            }
            i += 1;
        }

        // If a finished capture clearly precedes any unfinished capture, return
        // it and mark it consumed.
        let state: *mut QueryState = if !first_finished_state.is_null() {
            first_finished_state
        } else if first_unfinished_state_is_definite {
            core::ptr::from_mut::<QueryState>(array_get_mut(
                &mut (*self_).states,
                first_unfinished_state_index,
            ))
        } else {
            core::ptr::null_mut()
        };

        if !state.is_null() {
            if (*state).id == u32::MAX {
                (*state).id = (*self_).next_state_id;
                (*self_).next_state_id += 1;
            }
            (*match_).id = (*state).id;
            (*match_).pattern_index = (*state).pattern_index;
            let captures =
                capture_list_pool_get(&(*self_).capture_list_pool, (*state).capture_list_id as u16);
            (*match_).captures = captures.contents;
            (*match_).capture_count = captures.size as u16;
            *capture_index = u32::from((*state).consumed_capture_count);
            (*state).consumed_capture_count += 1;
            return true;
        }

        if capture_list_pool_is_empty(&(*self_).capture_list_pool) && found_unfinished_state {
            let clid =
                array_get_ref(&(*self_).states, first_unfinished_state_index).capture_list_id;
            capture_list_pool_release(&mut (*self_).capture_list_pool, clid as u16);
            array_erase(&mut (*self_).states, first_unfinished_state_index);
        }

        // No finished match ready; keep searching.
        if !ts_query_cursor_advance(self_, true) && (*self_).finished_states.size == 0 {
            return false;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_query_cursor_set_max_start_depth(
    self_: *mut TSQueryCursor,
    max_start_depth: u32,
) {
    (*self_).max_start_depth = max_start_depth;
}
