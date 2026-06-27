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
    TSFieldId, TSLanguage, TSQuantifier, TSQuantifierOne, TSQuantifierOneOrMore, TSQuantifierZero,
    TSQuantifierZeroOrMore, TSQuantifierZeroOrOne, TSQueryCapture, TSQueryCursorOptions,
    TSQueryCursorState, TSQueryError, TSQueryErrorCapture, TSQueryErrorField, TSQueryErrorNodeType,
    TSQueryErrorNone, TSQueryErrorStructure, TSQueryErrorSyntax, TSQueryPredicateStep,
    TSQueryPredicateStepTypeCapture, TSQueryPredicateStepTypeDone, TSQueryPredicateStepTypeString,
    TSRange, TSStateId, TSSymbol,
};

use super::language::{
    ts_language_abi_version, ts_language_field_id_for_name, ts_language_subtypes,
    ts_language_symbol_for_name, ts_language_symbol_metadata, LANGUAGE_VERSION_WITH_RESERVED_WORDS,
};
use super::stack::{
    array_back_ref, array_clear, array_delete, array_get_mut, array_get_ref, array_grow_by,
    array_init, array_new, array_pop, array_push, array_splice, Array,
};
use super::tree_cursor::TreeCursor;
use super::unicode::ts_decode_utf8;

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

const fn capture_quantifiers_new() -> CaptureQuantifiers {
    array_new()
}

unsafe fn capture_quantifiers_delete(self_: &mut CaptureQuantifiers) {
    array_delete(self_);
}

fn capture_quantifiers_clear(self_: &mut CaptureQuantifiers) {
    array_clear(self_);
}

/// Replace capture quantifiers with the given quantifiers.
unsafe fn capture_quantifiers_replace(
    self_: &mut CaptureQuantifiers,
    quantifiers: &CaptureQuantifiers,
) {
    array_clear(self_);
    array_splice(self_, self_.size, 0, quantifiers.size, quantifiers.contents);
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
                std::ptr::from_ref::<u8>(array_get_ref(&self_.characters, slice.offset)),
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
    std::ptr::from_ref::<u8>(array_get_ref(&self_.characters, slice.offset))
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
        std::ptr::from_mut::<u8>(array_get_mut(chars, offset)),
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
        let mut branch_capture_quantifiers = capture_quantifiers_new();
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
                capture_quantifiers_delete(&mut branch_capture_quantifiers);
                array_delete(&mut branch_step_indices);
                return e;
            }

            if start_index == starting_step_index {
                capture_quantifiers_replace(capture_quantifiers, &branch_capture_quantifiers);
            } else {
                capture_quantifiers_join_all(capture_quantifiers, &branch_capture_quantifiers);
            }

            array_push(&mut branch_step_indices, start_index);
            array_push(&mut self_.steps, query_step_new(0, depth as u16, false));
            capture_quantifiers_clear(&mut branch_capture_quantifiers);
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

        capture_quantifiers_delete(&mut branch_capture_quantifiers);
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
            let mut child_capture_quantifiers = capture_quantifiers_new();
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
                    capture_quantifiers_delete(&mut child_capture_quantifiers);
                    return e;
                }

                capture_quantifiers_add_all(capture_quantifiers, &child_capture_quantifiers);
                capture_quantifiers_clear(&mut child_capture_quantifiers);
                child_is_immediate = false;
            }

            capture_quantifiers_delete(&mut child_capture_quantifiers);
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
            let mut child_capture_quantifiers = capture_quantifiers_new();
            loop {
                // Parse a negated field assertion.
                if stream.next == i32::from(b'!') {
                    stream_advance(stream);
                    stream_skip_whitespace(stream);
                    if !stream_is_ident_start(stream) {
                        capture_quantifiers_delete(&mut child_capture_quantifiers);
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
                        capture_quantifiers_delete(&mut child_capture_quantifiers);
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
                                capture_quantifiers_delete(&mut child_capture_quantifiers);
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
                    capture_quantifiers_delete(&mut child_capture_quantifiers);
                    return e;
                }

                capture_quantifiers_add_all(capture_quantifiers, &child_capture_quantifiers);

                last_child_step_index = step_index;
                child_is_immediate = false;
                capture_quantifiers_clear(&mut child_capture_quantifiers);
            }
            capture_quantifiers_delete(&mut child_capture_quantifiers);
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
        let mut field_capture_quantifiers = capture_quantifiers_new();
        let mut e = ts_query_parse_pattern(
            self_,
            stream,
            depth,
            is_immediate,
            &mut field_capture_quantifiers,
        );
        if e != TSQueryErrorNone {
            capture_quantifiers_delete(&mut field_capture_quantifiers);
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
            capture_quantifiers_delete(&mut field_capture_quantifiers);
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
        capture_quantifiers_delete(&mut field_capture_quantifiers);
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
