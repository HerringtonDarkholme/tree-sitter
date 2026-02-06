#![allow(dead_code)]
#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]

use core::ffi::c_void;
use std::ptr;

use crate::ffi::{TSInputEdit, TSLanguage, TSPoint, TSStateId, TSSymbol};

use super::alloc::{ts_calloc, ts_free, ts_malloc, ts_realloc};
use super::error_costs::*;
use super::length::{length_add, length_saturating_sub, length_sub, length_zero, Length};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const TS_TREE_STATE_NONE: TSStateId = u16::MAX;
const TS_MAX_INLINE_TREE_LENGTH: u8 = u8::MAX;
const TS_MAX_TREE_POOL_SIZE: u32 = 32;

pub const ts_builtin_sym_error: TSSymbol = u16::MAX;
pub const ts_builtin_sym_end: TSSymbol = 0;
pub const ts_builtin_sym_error_repeat: TSSymbol = ts_builtin_sym_error - 1;

// ---------------------------------------------------------------------------
// C types from parser.h that are not in the Rust bindings
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSSymbolMetadata {
    pub visible: bool,
    pub named: bool,
    pub supertype: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSFieldMapEntry {
    pub field_id: u16,
    pub child_index: u8,
    pub inherited: bool,
}

// ---------------------------------------------------------------------------
// ExternalScannerState
// ---------------------------------------------------------------------------

const EXTERNAL_SCANNER_STATE_INLINE_SIZE: usize = 24;

#[repr(C)]
pub struct ExternalScannerState {
    data: ExternalScannerStateData,
    pub length: u32,
}

#[repr(C)]
pub union ExternalScannerStateData {
    pub long_data: *mut u8,
    pub short_data: [u8; EXTERNAL_SCANNER_STATE_INLINE_SIZE],
}

// ---------------------------------------------------------------------------
// SubtreeInlineData — bitfield-packed inline node
// ---------------------------------------------------------------------------

/// Compact inline representation of a subtree (fits in a pointer-sized word).
/// The `is_inline` bit overlaps with the LSB of a pointer, distinguishing
/// inline nodes from heap-allocated ones.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SubtreeInlineData {
    // Little-endian layout (matches the C struct on LE platforms):
    //   byte 0: is_inline:1, visible:1, named:1, extra:1, has_changes:1, is_missing:1, is_keyword:1, unused:1  (LE bit order)
    //   byte 1: symbol
    //   bytes 2-3: parse_state (u16)
    //   byte 4: padding_columns
    //   byte 5: padding_rows:4, lookahead_bytes:4
    //   byte 6: padding_bytes
    //   byte 7: size_bytes
    pub is_inline: bool,
    pub visible: bool,
    pub named: bool,
    pub extra: bool,
    pub has_changes: bool,
    pub is_missing: bool,
    pub is_keyword: bool,
    pub symbol: u8,
    pub parse_state: u16,
    pub padding_columns: u8,
    pub padding_rows: u8,    // 4-bit field
    pub lookahead_bytes: u8, // 4-bit field
    pub padding_bytes: u8,
    pub size_bytes: u8,
}

// ---------------------------------------------------------------------------
// SubtreeHeapData — heap-allocated node
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct SubtreeHeapData {
    pub ref_count: u32, // volatile / atomic
    pub padding: Length,
    pub size: Length,
    pub lookahead_bytes: u32,
    pub error_cost: u32,
    pub child_count: u32,
    pub symbol: TSSymbol,
    pub parse_state: TSStateId,

    // Bitfield flags
    pub visible: bool,
    pub named: bool,
    pub extra: bool,
    pub fragile_left: bool,
    pub fragile_right: bool,
    pub has_changes: bool,
    pub has_external_tokens: bool,
    pub has_external_scanner_state_change: bool,
    pub depends_on_column: bool,
    pub is_missing: bool,
    pub is_keyword: bool,

    // Anonymous union: children-info / external_scanner_state / lookahead_char
    pub data: SubtreeHeapDataContent,
}

#[repr(C)]
pub union SubtreeHeapDataContent {
    pub children: SubtreeChildrenData,
    pub external_scanner_state: std::mem::ManuallyDrop<ExternalScannerState>,
    pub lookahead_char: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SubtreeChildrenData {
    pub visible_child_count: u32,
    pub named_child_count: u32,
    pub visible_descendant_count: u32,
    pub dynamic_precedence: i32,
    pub repeat_depth: u16,
    pub production_id: u16,
    pub first_leaf: FirstLeaf,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FirstLeaf {
    pub symbol: TSSymbol,
    pub parse_state: TSStateId,
}

// ---------------------------------------------------------------------------
// Subtree / MutableSubtree — the core union types
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub union Subtree {
    pub data: SubtreeInlineData,
    pub ptr: *const SubtreeHeapData,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union MutableSubtree {
    pub data: SubtreeInlineData,
    pub ptr: *mut SubtreeHeapData,
}

pub const NULL_SUBTREE: Subtree = Subtree {
    ptr: ptr::null(),
};

// ---------------------------------------------------------------------------
// SubtreeArray / MutableSubtreeArray (replaces Array(Subtree))
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct SubtreeArray {
    pub contents: *mut Subtree,
    pub size: u32,
    pub capacity: u32,
}

#[repr(C)]
pub struct MutableSubtreeArray {
    pub contents: *mut MutableSubtree,
    pub size: u32,
    pub capacity: u32,
}

// ---------------------------------------------------------------------------
// SubtreePool
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct SubtreePool {
    pub free_trees: MutableSubtreeArray,
    pub tree_stack: MutableSubtreeArray,
}

// ---------------------------------------------------------------------------
// Internal helper: Edit (local to subtree edit logic)
// ---------------------------------------------------------------------------

struct Edit {
    start: Length,
    old_end: Length,
    new_end: Length,
}

// ---------------------------------------------------------------------------
// extern "C" — functions from not-yet-rewritten C modules
// ---------------------------------------------------------------------------

extern "C" {
    fn ts_language_symbol_metadata(language: *const TSLanguage, symbol: TSSymbol)
        -> TSSymbolMetadata;
    fn ts_language_symbol_name(
        language: *const TSLanguage,
        symbol: TSSymbol,
    ) -> *const i8;
    fn ts_language_alias_sequence(
        language: *const TSLanguage,
        production_id: u32,
    ) -> *const TSSymbol;
    fn ts_language_field_map(
        language: *const TSLanguage,
        production_id: u32,
        start: *mut *const TSFieldMapEntry,
        end: *mut *const TSFieldMapEntry,
    );
    fn ts_language_write_symbol_as_dot_string(
        language: *const TSLanguage,
        f: *mut c_void, // FILE*
        symbol: TSSymbol,
    );
}

// ===========================================================================
// ExternalScannerState functions
// ===========================================================================

pub unsafe fn ts_external_scanner_state_init(
    self_: *mut ExternalScannerState,
    data: *const u8,
    length: u32,
) {
    todo!()
}

pub unsafe fn ts_external_scanner_state_copy(
    self_: *const ExternalScannerState,
) -> ExternalScannerState {
    todo!()
}

pub unsafe fn ts_external_scanner_state_delete(self_: *mut ExternalScannerState) {
    todo!()
}

pub unsafe fn ts_external_scanner_state_data(
    self_: *const ExternalScannerState,
) -> *const u8 {
    todo!()
}

pub unsafe fn ts_external_scanner_state_eq(
    self_: *const ExternalScannerState,
    buffer: *const u8,
    length: u32,
) -> bool {
    todo!()
}

// ===========================================================================
// SubtreeArray functions
// ===========================================================================

pub unsafe fn ts_subtree_array_copy(self_: SubtreeArray, dest: *mut SubtreeArray) {
    todo!()
}

pub unsafe fn ts_subtree_array_clear(pool: *mut SubtreePool, self_: *mut SubtreeArray) {
    todo!()
}

pub unsafe fn ts_subtree_array_delete(pool: *mut SubtreePool, self_: *mut SubtreeArray) {
    todo!()
}

pub unsafe fn ts_subtree_array_remove_trailing_extras(
    self_: *mut SubtreeArray,
    destination: *mut SubtreeArray,
) {
    todo!()
}

pub unsafe fn ts_subtree_array_reverse(self_: *mut SubtreeArray) {
    todo!()
}

// ===========================================================================
// SubtreePool functions
// ===========================================================================

pub unsafe fn ts_subtree_pool_new(capacity: u32) -> SubtreePool {
    todo!()
}

pub unsafe fn ts_subtree_pool_delete(self_: *mut SubtreePool) {
    todo!()
}

unsafe fn ts_subtree_pool_allocate(self_: *mut SubtreePool) -> *mut SubtreeHeapData {
    todo!()
}

unsafe fn ts_subtree_pool_free(self_: *mut SubtreePool, tree: *mut SubtreeHeapData) {
    todo!()
}

// ===========================================================================
// Subtree inline helpers (from subtree.h static inline functions)
// ===========================================================================

#[inline]
pub fn ts_subtree_symbol(self_: Subtree) -> TSSymbol {
    todo!()
}

#[inline]
pub fn ts_subtree_visible(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub fn ts_subtree_named(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub fn ts_subtree_extra(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub fn ts_subtree_has_changes(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub fn ts_subtree_missing(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub fn ts_subtree_is_keyword(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub fn ts_subtree_parse_state(self_: Subtree) -> TSStateId {
    todo!()
}

#[inline]
pub fn ts_subtree_lookahead_bytes(self_: Subtree) -> u32 {
    todo!()
}

#[inline]
pub fn ts_subtree_alloc_size(child_count: u32) -> usize {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_children(self_: Subtree) -> *mut Subtree {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_set_extra(self_: *mut MutableSubtree, is_extra: bool) {
    todo!()
}

#[inline]
pub fn ts_subtree_leaf_symbol(self_: Subtree) -> TSSymbol {
    todo!()
}

#[inline]
pub fn ts_subtree_leaf_parse_state(self_: Subtree) -> TSStateId {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_padding(self_: Subtree) -> Length {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_size(self_: Subtree) -> Length {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_total_size(self_: Subtree) -> Length {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_total_bytes(self_: Subtree) -> u32 {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_child_count(self_: Subtree) -> u32 {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_repeat_depth(self_: Subtree) -> u32 {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_is_repetition(self_: Subtree) -> u32 {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_visible_descendant_count(self_: Subtree) -> u32 {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_visible_child_count(self_: Subtree) -> u32 {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_error_cost(self_: Subtree) -> u32 {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_dynamic_precedence(self_: Subtree) -> i32 {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_production_id(self_: Subtree) -> u16 {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_fragile_left(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_fragile_right(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_has_external_tokens(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_has_external_scanner_state_change(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_depends_on_column(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub unsafe fn ts_subtree_is_fragile(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub fn ts_subtree_is_error(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub fn ts_subtree_is_eof(self_: Subtree) -> bool {
    todo!()
}

#[inline]
pub fn ts_subtree_from_mut(self_: MutableSubtree) -> Subtree {
    todo!()
}

#[inline]
pub fn ts_subtree_to_mut_unsafe(self_: Subtree) -> MutableSubtree {
    todo!()
}

// ===========================================================================
// Subtree private helpers
// ===========================================================================

#[inline]
fn ts_subtree_can_inline(padding: Length, size: Length, lookahead_bytes: u32) -> bool {
    todo!()
}

unsafe fn ts_subtree_set_has_changes(self_: *mut MutableSubtree) {
    todo!()
}

// ===========================================================================
// Subtree construction functions (from subtree.c)
// ===========================================================================

pub unsafe fn ts_subtree_new_leaf(
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
) -> Subtree {
    todo!()
}

pub unsafe fn ts_subtree_new_error(
    pool: *mut SubtreePool,
    lookahead_char: i32,
    padding: Length,
    size: Length,
    bytes_scanned: u32,
    parse_state: TSStateId,
    language: *const TSLanguage,
) -> Subtree {
    todo!()
}

pub unsafe fn ts_subtree_clone(self_: Subtree) -> MutableSubtree {
    todo!()
}

pub unsafe fn ts_subtree_new_node(
    symbol: TSSymbol,
    children: *mut SubtreeArray,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    todo!()
}

pub unsafe fn ts_subtree_new_error_node(
    children: *mut SubtreeArray,
    extra: bool,
    language: *const TSLanguage,
) -> Subtree {
    todo!()
}

pub unsafe fn ts_subtree_new_missing_leaf(
    pool: *mut SubtreePool,
    symbol: TSSymbol,
    padding: Length,
    lookahead_bytes: u32,
    language: *const TSLanguage,
) -> Subtree {
    todo!()
}

// ===========================================================================
// Subtree mutation / ownership functions
// ===========================================================================

pub unsafe fn ts_subtree_set_symbol(
    self_: *mut MutableSubtree,
    symbol: TSSymbol,
    language: *const TSLanguage,
) {
    todo!()
}

pub unsafe fn ts_subtree_make_mut(pool: *mut SubtreePool, self_: Subtree) -> MutableSubtree {
    todo!()
}

pub unsafe fn ts_subtree_retain(self_: Subtree) {
    todo!()
}

pub unsafe fn ts_subtree_release(pool: *mut SubtreePool, self_: Subtree) {
    todo!()
}

// ===========================================================================
// Subtree tree-balancing / summarization
// ===========================================================================

pub unsafe fn ts_subtree_compress(
    self_: MutableSubtree,
    count: u32,
    language: *const TSLanguage,
    stack: *mut MutableSubtreeArray,
) {
    todo!()
}

pub unsafe fn ts_subtree_summarize_children(
    self_: MutableSubtree,
    language: *const TSLanguage,
) {
    todo!()
}

// ===========================================================================
// Subtree comparison / query
// ===========================================================================

pub unsafe fn ts_subtree_compare(
    left: Subtree,
    right: Subtree,
    pool: *mut SubtreePool,
) -> i32 {
    todo!()
}

pub unsafe fn ts_subtree_edit(
    self_: Subtree,
    edit: *const TSInputEdit,
    pool: *mut SubtreePool,
) -> Subtree {
    todo!()
}

pub unsafe fn ts_subtree_last_external_token(tree: Subtree) -> Subtree {
    todo!()
}

pub unsafe fn ts_subtree_external_scanner_state(
    self_: Subtree,
) -> *const ExternalScannerState {
    todo!()
}

pub unsafe fn ts_subtree_external_scanner_state_eq(self_: Subtree, other: Subtree) -> bool {
    todo!()
}

// ===========================================================================
// Subtree string / debug output
// ===========================================================================

/// Write a character to a string buffer for debug output.
unsafe fn ts_subtree__write_char_to_string(
    str_: *mut u8,
    n: usize,
    chr: i32,
) -> usize {
    todo!()
}

/// Recursive helper for ts_subtree_string.
unsafe fn ts_subtree__write_to_string(
    self_: Subtree,
    string: *mut u8,
    limit: usize,
    language: *const TSLanguage,
    include_all: bool,
    alias_symbol: TSSymbol,
    alias_is_named: bool,
    field_name: *const i8,
) -> usize {
    todo!()
}

pub unsafe fn ts_subtree_string(
    self_: Subtree,
    alias_symbol: TSSymbol,
    alias_is_named: bool,
    language: *const TSLanguage,
    include_all: bool,
) -> *mut i8 {
    todo!()
}

/// Recursive helper for ts_subtree_print_dot_graph.
unsafe fn ts_subtree__print_dot_graph(
    self_: *const Subtree,
    start_offset: u32,
    language: *const TSLanguage,
    alias_symbol: TSSymbol,
    f: *mut c_void, // FILE*
) {
    todo!()
}

pub unsafe fn ts_subtree_print_dot_graph(
    self_: Subtree,
    language: *const TSLanguage,
    f: *mut c_void, // FILE*
) {
    todo!()
}
