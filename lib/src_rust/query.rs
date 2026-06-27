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
    TSQueryCursorState, TSQueryPredicateStep, TSRange, TSStateId, TSSymbol,
};

use super::stack::{
    array_clear, array_delete, array_get_mut, array_get_ref, array_grow_by, array_init, array_new,
    array_push, array_splice, Array,
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
