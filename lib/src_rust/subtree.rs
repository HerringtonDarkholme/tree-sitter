#![allow(dead_code)]
#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]

use core::ffi::c_void;
use std::{
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::ffi::{TSInputEdit, TSLanguage, TSPoint, TSStateId, TSSymbol};

use super::alloc::{calloc, free, malloc, realloc};
use super::error_costs::{
    ERROR_COST_PER_MISSING_TREE, ERROR_COST_PER_RECOVERY, ERROR_COST_PER_SKIPPED_CHAR,
    ERROR_COST_PER_SKIPPED_LINE, ERROR_COST_PER_SKIPPED_TREE,
};
use super::language::{ts_language_symbol_metadata, ts_language_symbol_name};
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
    /// Whether the symbol contributes a visible node to public traversal.
    pub visible: bool,
    /// Whether the symbol is named rather than anonymous punctuation/token text.
    pub named: bool,
    /// Whether the symbol is a supertype.
    pub supertype: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSFieldMapEntry {
    /// Field id applied to the child.
    pub field_id: u16,
    /// Child index within the production.
    pub child_index: u8,
    /// Whether this field was inherited through hidden nodes.
    pub inherited: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSMapSlice {
    /// Offset into the corresponding flat table.
    pub index: u16,
    /// Number of entries in the slice.
    pub length: u16,
}

// ---------------------------------------------------------------------------
// ExternalScannerState
// ---------------------------------------------------------------------------

const EXTERNAL_SCANNER_STATE_INLINE_SIZE: usize = 24;

#[repr(C)]
pub struct ExternalScannerState {
    /// Inline or heap state bytes, selected by `length`.
    data: ExternalScannerStateData,
    /// Serialized byte count.
    pub length: u32,
}

// SAFETY: Only used in a read-only static (EMPTY_EXTERNAL_SCANNER_STATE).
unsafe impl Sync for ExternalScannerState {}

#[repr(C)]
pub union ExternalScannerStateData {
    /// Heap storage when serialized state exceeds inline capacity.
    pub long_data: *mut u8,
    /// Inline storage for the common small scanner-state case.
    pub short_data: [u8; EXTERNAL_SCANNER_STATE_INLINE_SIZE],
}

// ---------------------------------------------------------------------------
// SubtreeInlineData — bitfield-packed inline node
// ---------------------------------------------------------------------------

/// Compact inline representation of a subtree (fits in a pointer-sized word).
/// The `is_inline` bit overlaps with the LSB of a pointer, distinguishing
/// inline nodes from heap-allocated ones.
///
/// Little-endian layout (matches the C struct bitfields):
///   byte 0: `is_inline:1`, `visible:1`, `named:1`, `extra:1`,
///   `has_changes:1`, `is_missing:1`, `is_keyword:1`, `unused:1`
///   byte 1: `symbol`
///   bytes 2-3: `parse_state` (u16 LE)
///   byte 4: `padding_columns`
///   byte 5: `padding_rows:4` (low), `lookahead_bytes:4` (high)
///   byte 6: `padding_bytes`
///   byte 7: `size_bytes`
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SubtreeInlineData {
    /// Byte 0: packed bitfields (`is_inline`, `visible`, `named`, `extra`,
    /// `has_changes`, `is_missing`, `is_keyword`)
    pub flags: u8,
    pub symbol: u8,
    pub parse_state: u16,
    pub padding_columns: u8,
    /// Low 4 bits = `padding_rows`, high 4 bits = `lookahead_bytes`
    pub rows_and_lookahead: u8,
    pub padding_bytes: u8,
    pub size_bytes: u8,
}

// Bit positions in SubtreeInlineData.flags
const INLINE_IS_INLINE: u8 = 1 << 0;
const INLINE_VISIBLE: u8 = 1 << 1;
const INLINE_NAMED: u8 = 1 << 2;
const INLINE_EXTRA: u8 = 1 << 3;
const INLINE_HAS_CHANGES: u8 = 1 << 4;
const INLINE_IS_MISSING: u8 = 1 << 5;
const INLINE_IS_KEYWORD: u8 = 1 << 6;

impl SubtreeInlineData {
    #[inline(always)]
    pub const fn is_inline(self) -> bool {
        self.flags & INLINE_IS_INLINE != 0
    }
    #[inline(always)]
    pub const fn visible(self) -> bool {
        self.flags & INLINE_VISIBLE != 0
    }
    #[inline(always)]
    pub const fn named(self) -> bool {
        self.flags & INLINE_NAMED != 0
    }
    #[inline(always)]
    pub const fn extra(self) -> bool {
        self.flags & INLINE_EXTRA != 0
    }
    #[inline(always)]
    pub const fn has_changes(self) -> bool {
        self.flags & INLINE_HAS_CHANGES != 0
    }
    #[inline(always)]
    pub const fn is_missing(self) -> bool {
        self.flags & INLINE_IS_MISSING != 0
    }
    #[inline(always)]
    pub const fn is_keyword(self) -> bool {
        self.flags & INLINE_IS_KEYWORD != 0
    }
    #[inline(always)]
    pub const fn padding_rows(self) -> u8 {
        self.rows_and_lookahead & 0x0F
    }
    #[inline(always)]
    pub const fn lookahead_bytes(self) -> u8 {
        (self.rows_and_lookahead >> 4) & 0x0F
    }

    #[inline(always)]
    pub fn set_is_inline(&mut self, v: bool) {
        if v {
            self.flags |= INLINE_IS_INLINE
        } else {
            self.flags &= !INLINE_IS_INLINE
        }
    }
    #[inline(always)]
    pub fn set_visible(&mut self, v: bool) {
        if v {
            self.flags |= INLINE_VISIBLE
        } else {
            self.flags &= !INLINE_VISIBLE
        }
    }
    #[inline(always)]
    pub fn set_named(&mut self, v: bool) {
        if v {
            self.flags |= INLINE_NAMED
        } else {
            self.flags &= !INLINE_NAMED
        }
    }
    #[inline(always)]
    pub fn set_extra(&mut self, v: bool) {
        if v {
            self.flags |= INLINE_EXTRA
        } else {
            self.flags &= !INLINE_EXTRA
        }
    }
    #[inline(always)]
    pub fn set_has_changes(&mut self, v: bool) {
        if v {
            self.flags |= INLINE_HAS_CHANGES
        } else {
            self.flags &= !INLINE_HAS_CHANGES
        }
    }
    #[inline(always)]
    pub fn set_is_missing(&mut self, v: bool) {
        if v {
            self.flags |= INLINE_IS_MISSING
        } else {
            self.flags &= !INLINE_IS_MISSING
        }
    }
    #[inline(always)]
    pub fn set_is_keyword(&mut self, v: bool) {
        if v {
            self.flags |= INLINE_IS_KEYWORD
        } else {
            self.flags &= !INLINE_IS_KEYWORD
        }
    }
    #[inline(always)]
    pub fn set_padding_rows(&mut self, v: u8) {
        self.rows_and_lookahead = (self.rows_and_lookahead & 0xF0) | (v & 0x0F)
    }
    #[inline(always)]
    pub fn set_lookahead_bytes(&mut self, v: u8) {
        self.rows_and_lookahead = (self.rows_and_lookahead & 0x0F) | ((v & 0x0F) << 4)
    }
}

// ---------------------------------------------------------------------------
// SubtreeHeapData — heap-allocated node
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct SubtreeHeapData {
    /// Intrusive reference count for heap-owned subtrees.
    pub ref_count: u32, // volatile / atomic
    /// Leading padding before this subtree's content.
    pub padding: Length,
    /// Content size excluding padding and lookahead bytes.
    pub size: Length,
    /// Bytes scanned past token end to recognize this subtree.
    pub lookahead_bytes: u32,
    /// Accumulated error cost for recovery comparison.
    pub error_cost: u32,
    /// Number of direct children. Zero means leaf payload in `data`.
    pub child_count: u32,
    /// Grammar symbol for this subtree.
    pub symbol: TSSymbol,
    /// Parse state recorded on this subtree.
    pub parse_state: TSStateId,

    /// Packed bitfield flags (11 bits used, matches C bitfield layout)
    /// bit 0: `visible`, bit 1: `named`, bit 2: `extra`, bit 3: `fragile_left`,
    /// bit 4: `fragile_right`, bit 5: `has_changes`, bit 6: `has_external_tokens`,
    /// bit 7: `has_external_scanner_state_change`, bit 8: `depends_on_column`,
    /// bit 9: `is_missing`, bit 10: `is_keyword`, bit 11: `arena_owned`
    pub flags: u16,
    // 2 bytes padding here for 4-byte alignment of the union (inserted by repr(C))

    // Anonymous union: children-info / external_scanner_state / lookahead_char
    pub data: SubtreeHeapDataContent,
}

// Bit positions in SubtreeHeapData.flags
const HEAP_VISIBLE: u16 = 1 << 0;
const HEAP_NAMED: u16 = 1 << 1;
const HEAP_EXTRA: u16 = 1 << 2;
const HEAP_FRAGILE_LEFT: u16 = 1 << 3;
const HEAP_FRAGILE_RIGHT: u16 = 1 << 4;
const HEAP_HAS_CHANGES: u16 = 1 << 5;
const HEAP_HAS_EXTERNAL_TOKENS: u16 = 1 << 6;
const HEAP_HAS_EXTERNAL_SCANNER_STATE_CHANGE: u16 = 1 << 7;
const HEAP_DEPENDS_ON_COLUMN: u16 = 1 << 8;
const HEAP_IS_MISSING: u16 = 1 << 9;
const HEAP_IS_KEYWORD: u16 = 1 << 10;
const HEAP_ARENA_OWNED: u16 = 1 << 11;

impl SubtreeHeapData {
    #[inline(always)]
    pub const fn visible(&self) -> bool {
        self.flags & HEAP_VISIBLE != 0
    }
    #[inline(always)]
    pub const fn named(&self) -> bool {
        self.flags & HEAP_NAMED != 0
    }
    #[inline(always)]
    pub const fn extra(&self) -> bool {
        self.flags & HEAP_EXTRA != 0
    }
    #[inline(always)]
    pub const fn fragile_left(&self) -> bool {
        self.flags & HEAP_FRAGILE_LEFT != 0
    }
    #[inline(always)]
    pub const fn fragile_right(&self) -> bool {
        self.flags & HEAP_FRAGILE_RIGHT != 0
    }
    #[inline(always)]
    pub const fn has_changes(&self) -> bool {
        self.flags & HEAP_HAS_CHANGES != 0
    }
    #[inline(always)]
    pub const fn has_external_tokens(&self) -> bool {
        self.flags & HEAP_HAS_EXTERNAL_TOKENS != 0
    }
    #[inline(always)]
    pub const fn has_external_scanner_state_change(&self) -> bool {
        self.flags & HEAP_HAS_EXTERNAL_SCANNER_STATE_CHANGE != 0
    }
    #[inline(always)]
    pub const fn depends_on_column(&self) -> bool {
        self.flags & HEAP_DEPENDS_ON_COLUMN != 0
    }
    #[inline(always)]
    pub const fn is_missing(&self) -> bool {
        self.flags & HEAP_IS_MISSING != 0
    }
    #[inline(always)]
    pub const fn is_keyword(&self) -> bool {
        self.flags & HEAP_IS_KEYWORD != 0
    }
    #[inline(always)]
    pub const fn arena_owned(&self) -> bool {
        self.flags & HEAP_ARENA_OWNED != 0
    }

    #[inline(always)]
    pub fn set_visible(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_VISIBLE
        } else {
            self.flags &= !HEAP_VISIBLE
        }
    }
    #[inline(always)]
    pub fn set_named(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_NAMED
        } else {
            self.flags &= !HEAP_NAMED
        }
    }
    #[inline(always)]
    pub fn set_extra(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_EXTRA
        } else {
            self.flags &= !HEAP_EXTRA
        }
    }
    #[inline(always)]
    pub fn set_fragile_left(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_FRAGILE_LEFT
        } else {
            self.flags &= !HEAP_FRAGILE_LEFT
        }
    }
    #[inline(always)]
    pub fn set_fragile_right(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_FRAGILE_RIGHT
        } else {
            self.flags &= !HEAP_FRAGILE_RIGHT
        }
    }
    #[inline(always)]
    pub fn set_has_changes(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_HAS_CHANGES
        } else {
            self.flags &= !HEAP_HAS_CHANGES
        }
    }
    #[inline(always)]
    pub fn set_has_external_tokens(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_HAS_EXTERNAL_TOKENS
        } else {
            self.flags &= !HEAP_HAS_EXTERNAL_TOKENS
        }
    }
    #[inline(always)]
    pub fn set_has_external_scanner_state_change(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_HAS_EXTERNAL_SCANNER_STATE_CHANGE
        } else {
            self.flags &= !HEAP_HAS_EXTERNAL_SCANNER_STATE_CHANGE
        }
    }
    #[inline(always)]
    pub fn set_depends_on_column(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_DEPENDS_ON_COLUMN
        } else {
            self.flags &= !HEAP_DEPENDS_ON_COLUMN
        }
    }
    #[inline(always)]
    pub fn set_is_missing(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_IS_MISSING
        } else {
            self.flags &= !HEAP_IS_MISSING
        }
    }
    #[inline(always)]
    pub fn set_is_keyword(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_IS_KEYWORD
        } else {
            self.flags &= !HEAP_IS_KEYWORD
        }
    }
    #[inline(always)]
    pub fn set_arena_owned(&mut self, v: bool) {
        if v {
            self.flags |= HEAP_ARENA_OWNED
        } else {
            self.flags &= !HEAP_ARENA_OWNED
        }
    }

    /// Build flags from individual booleans (for struct initialization)
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn make_flags(
        visible: bool,
        named: bool,
        extra: bool,
        fragile_left: bool,
        fragile_right: bool,
        has_changes: bool,
        has_external_tokens: bool,
        has_external_scanner_state_change: bool,
        depends_on_column: bool,
        is_missing: bool,
        is_keyword: bool,
    ) -> u16 {
        u16::from(visible)
            | u16::from(named) << 1
            | u16::from(extra) << 2
            | u16::from(fragile_left) << 3
            | u16::from(fragile_right) << 4
            | u16::from(has_changes) << 5
            | u16::from(has_external_tokens) << 6
            | u16::from(has_external_scanner_state_change) << 7
            | u16::from(depends_on_column) << 8
            | u16::from(is_missing) << 9
            | u16::from(is_keyword) << 10
    }
}

#[repr(C)]
pub union SubtreeHeapDataContent {
    /// Aggregate child metadata for internal nodes.
    pub children: SubtreeChildrenData,
    /// Serialized scanner state for external-token leaves.
    pub external_scanner_state: std::mem::ManuallyDrop<ExternalScannerState>,
    /// First skipped character for error leaves.
    pub lookahead_char: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SubtreeChildrenData {
    /// Number of direct visible children.
    pub visible_child_count: u32,
    /// Number of direct named children.
    pub named_child_count: u32,
    /// Number of visible descendants below this node.
    pub visible_descendant_count: u32,
    /// Dynamic precedence accumulated from children.
    pub dynamic_precedence: i32,
    /// Repetition nesting depth for balancing repeated nodes.
    pub repeat_depth: u16,
    /// Production id used for fields and aliases.
    pub production_id: u16,
    /// First leaf summary for reuse and node state APIs.
    pub first_leaf: FirstLeaf,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FirstLeaf {
    /// Symbol of the first leaf under this subtree.
    pub symbol: TSSymbol,
    /// Parse state of the first leaf under this subtree.
    pub parse_state: TSStateId,
}

// ---------------------------------------------------------------------------
// Subtree / MutableSubtree — the core union types
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub union Subtree {
    /// Inline representation when `data.is_inline()` is set.
    pub data: SubtreeInlineData,
    /// Heap representation otherwise.
    pub ptr: *const SubtreeHeapData,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union MutableSubtree {
    /// Inline representation when `data.is_inline()` is set.
    pub data: SubtreeInlineData,
    /// Mutable heap representation otherwise.
    pub ptr: *mut SubtreeHeapData,
}

pub const NULL_SUBTREE: Subtree = Subtree { ptr: ptr::null() };

// Compile-time layout assertions — catch ABI mismatches immediately
const _: () = assert!(std::mem::size_of::<SubtreeInlineData>() == 8);
const _: () = assert!(std::mem::size_of::<Subtree>() == 8);
const _: () = assert!(std::mem::size_of::<MutableSubtree>() == 8);
const _: () = assert!(std::mem::size_of::<ExternalScannerState>() == 32);
const _: () = assert!(std::mem::size_of::<FirstLeaf>() == 4);

// ---------------------------------------------------------------------------
// SubtreeArray / MutableSubtreeArray (replaces Array(Subtree))
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct SubtreeArray {
    /// Child storage.
    pub contents: *mut Subtree,
    /// Number of initialized children.
    pub size: u32,
    /// Allocated child capacity.
    pub capacity: u32,
}

#[repr(C)]
pub struct MutableSubtreeArray {
    /// Mutable subtree storage.
    pub contents: *mut MutableSubtree,
    /// Number of initialized entries.
    pub size: u32,
    /// Allocated entry capacity.
    pub capacity: u32,
}

pub const fn subtree_array_new() -> SubtreeArray {
    SubtreeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    }
}

// ---------------------------------------------------------------------------
// SubtreePool
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct SubtreePool {
    /// Free list of heap subtree allocations.
    pub free_trees: MutableSubtreeArray,
    /// Scratch stack used by iterative release/compress operations.
    pub tree_stack: MutableSubtreeArray,
}

/// Arena for tree-owned internal nodes.
///
/// Parser reductions can allocate accepted internal nodes in this arena. The
/// returned `TSTree` retains the arena, so copying a tree only bumps the arena
/// refcount instead of cloning every internal node.
#[repr(C)]
pub struct TreeArena {
    /// Shared ownership count across copied trees.
    ref_count: AtomicU32,
    /// Singly linked list of allocated pages.
    pages: *mut TreeArenaPage,
    /// Page currently used for bump allocation.
    current_page: *mut TreeArenaPage,
}

#[repr(C)]
struct TreeArenaPage {
    /// Next older page in the arena list.
    next: *mut TreeArenaPage,
    /// Bump allocation buffer.
    contents: *mut u8,
    /// Bytes currently used in `contents`.
    size: usize,
    /// Allocated byte capacity.
    capacity: usize,
}

// ---------------------------------------------------------------------------
// Internal helper: Edit (local to subtree edit logic)
// ---------------------------------------------------------------------------

struct Edit {
    /// Edited range start in old coordinates.
    start: Length,
    /// Edited range end in old coordinates.
    old_end: Length,
    /// Edited range end in new coordinates.
    new_end: Length,
}

// ---------------------------------------------------------------------------
// Partial TSLanguage layout (mirrors parser.h) for static-inline helpers
// ---------------------------------------------------------------------------

#[repr(C)]
struct TSLanguageData {
    abi_version: u32,
    symbol_count: u32,
    alias_count: u32,
    token_count: u32,
    external_token_count: u32,
    state_count: u32,
    large_state_count: u32,
    production_id_count: u32,
    field_count: u32,
    max_alias_sequence_length: u16,
    // repr(C) adds implicit padding here to align the next pointer
    parse_table: *const u16,
    small_parse_table: *const u16,
    small_parse_table_map: *const u32,
    parse_actions: *const c_void,
    symbol_names: *const *const i8,
    field_names: *const *const i8,
    field_map_slices: *const TSMapSlice,
    field_map_entries: *const TSFieldMapEntry,
    symbol_metadata: *const TSSymbolMetadata,
    public_symbol_map: *const TSSymbol,
    alias_map: *const u16,
    alias_sequences: *const TSSymbol,
}

/// Rust re-implementation of the static inline `language_alias_sequence` from `language.h`.
#[inline]
const unsafe fn language_alias_sequence(
    language: *const TSLanguage,
    production_id: u32,
) -> *const TSSymbol {
    let lang = language.cast::<TSLanguageData>();
    if production_id != 0 {
        (*lang)
            .alias_sequences
            .add(production_id as usize * (*lang).max_alias_sequence_length as usize)
    } else {
        ptr::null()
    }
}

// ---------------------------------------------------------------------------
// Static data
// ---------------------------------------------------------------------------

static EMPTY_EXTERNAL_SCANNER_STATE: ExternalScannerState = ExternalScannerState {
    data: ExternalScannerStateData {
        short_data: [0; EXTERNAL_SCANNER_STATE_INLINE_SIZE],
    },
    length: 0,
};

// ===========================================================================
// ExternalScannerState functions
// ===========================================================================

pub unsafe fn external_scanner_state_init(
    self_: &mut ExternalScannerState,
    data: *const u8,
    length: u32,
) {
    self_.length = length;
    if length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        self_.data.long_data = malloc(length as usize).cast::<u8>();
        ptr::copy_nonoverlapping(data, self_.data.long_data, length as usize);
    } else {
        ptr::copy_nonoverlapping(data, self_.data.short_data.as_mut_ptr(), length as usize);
    }
}

pub unsafe fn external_scanner_state_copy(self_: &ExternalScannerState) -> ExternalScannerState {
    let mut result = ExternalScannerState {
        data: ExternalScannerStateData {
            short_data: self_.data.short_data,
        },
        length: self_.length,
    };
    if self_.length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        result.data.long_data = malloc(self_.length as usize).cast::<u8>();
        ptr::copy_nonoverlapping(
            self_.data.long_data,
            result.data.long_data,
            self_.length as usize,
        );
    }
    result
}

pub unsafe fn external_scanner_state_delete(self_: &mut ExternalScannerState) {
    if self_.length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        free(self_.data.long_data.cast::<c_void>());
    }
}

pub const unsafe fn external_scanner_state_data(self_: &ExternalScannerState) -> *const u8 {
    if self_.length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        self_.data.long_data
    } else {
        self_.data.short_data.as_ptr()
    }
}

pub unsafe fn external_scanner_state_eq(
    self_: &ExternalScannerState,
    buffer: *const u8,
    length: u32,
) -> bool {
    if self_.length != length {
        return false;
    }
    if length == 0 {
        return true;
    }
    let length = length as usize;
    std::slice::from_raw_parts(external_scanner_state_data(self_), length)
        == std::slice::from_raw_parts(buffer, length)
}

// ===========================================================================
// SubtreeArray helpers (replaces array.h macros)
// ===========================================================================

/// Grow array capacity if needed to fit `count` more elements.
unsafe fn array_grow(arr: &mut SubtreeArray, count: u32) {
    let new_size = arr.size + count;
    if new_size > arr.capacity {
        let mut new_capacity = arr.capacity * 2;
        if new_capacity < 8 {
            new_capacity = 8;
        }
        if new_capacity < new_size {
            new_capacity = new_size;
        }
        arr.contents = realloc(
            arr.contents.cast::<c_void>(),
            new_capacity as usize * std::mem::size_of::<Subtree>(),
        )
        .cast::<Subtree>();
        arr.capacity = new_capacity;
    }
}

/// Push a subtree onto the end of the array.
unsafe fn array_push_subtree(arr: &mut SubtreeArray, element: Subtree) {
    array_grow(arr, 1);
    ptr::write(arr.contents.add(arr.size as usize), element);
    arr.size += 1;
}

// ===========================================================================
// SubtreeArray functions
// ===========================================================================

pub unsafe fn subtree_array_copy(self_: &SubtreeArray, dest: &mut SubtreeArray) {
    dest.size = self_.size;
    dest.capacity = self_.capacity;
    dest.contents = self_.contents;
    if self_.capacity > 0 {
        dest.contents =
            calloc(self_.capacity as usize, std::mem::size_of::<Subtree>()).cast::<Subtree>();
        ptr::copy_nonoverlapping(self_.contents, dest.contents, self_.size as usize);
        for i in 0..self_.size {
            subtree_retain(*dest.contents.add(i as usize));
        }
    }
}

pub unsafe fn subtree_array_clear(pool: &mut SubtreePool, self_: &mut SubtreeArray) {
    for i in 0..self_.size {
        subtree_release(pool, *self_.contents.add(i as usize));
    }
    self_.size = 0;
}

pub unsafe fn subtree_array_delete(pool: &mut SubtreePool, self_: &mut SubtreeArray) {
    subtree_array_clear(pool, self_);
    if !self_.contents.is_null() {
        free(self_.contents.cast::<c_void>());
    }
    self_.contents = ptr::null_mut();
    self_.size = 0;
    self_.capacity = 0;
}

pub unsafe fn subtree_array_remove_trailing_extras(
    self_: &mut SubtreeArray,
    destination: &mut SubtreeArray,
) {
    destination.size = 0;
    while self_.size > 0 {
        let last = *self_.contents.add(self_.size as usize - 1);
        if subtree_extra(last) {
            self_.size -= 1;
            array_push_subtree(destination, last);
        } else {
            break;
        }
    }
    subtree_array_reverse(destination);
}

pub unsafe fn subtree_array_reverse(self_: &mut SubtreeArray) {
    let limit = self_.size / 2;
    for i in 0..limit {
        let reverse_index = self_.size as usize - 1 - i as usize;
        let a = self_.contents.add(i as usize);
        let b = self_.contents.add(reverse_index);
        ptr::swap(a, b);
    }
}

// ===========================================================================
// MutableSubtreeArray helpers
// ===========================================================================

unsafe fn mutable_array_grow(arr: &mut MutableSubtreeArray, count: u32) {
    let new_size = arr.size + count;
    if new_size > arr.capacity {
        let mut new_capacity = arr.capacity * 2;
        if new_capacity < 8 {
            new_capacity = 8;
        }
        if new_capacity < new_size {
            new_capacity = new_size;
        }
        arr.contents = realloc(
            arr.contents.cast::<c_void>(),
            new_capacity as usize * std::mem::size_of::<MutableSubtree>(),
        )
        .cast::<MutableSubtree>();
        arr.capacity = new_capacity;
    }
}

pub unsafe fn mutable_array_push(arr: &mut MutableSubtreeArray, element: MutableSubtree) {
    mutable_array_grow(arr, 1);
    ptr::write(arr.contents.add(arr.size as usize), element);
    arr.size += 1;
}

unsafe fn mutable_array_pop(arr: &mut MutableSubtreeArray) -> MutableSubtree {
    arr.size -= 1;
    ptr::read(arr.contents.add(arr.size as usize))
}

unsafe fn mutable_array_delete(arr: &mut MutableSubtreeArray) {
    if !arr.contents.is_null() {
        free(arr.contents.cast::<c_void>());
    }
    arr.contents = ptr::null_mut();
    arr.size = 0;
    arr.capacity = 0;
}

const fn mutable_array_new() -> MutableSubtreeArray {
    MutableSubtreeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    }
}

unsafe fn mutable_array_reserve(arr: &mut MutableSubtreeArray, new_capacity: u32) {
    if new_capacity > arr.capacity {
        arr.contents = realloc(
            arr.contents.cast::<c_void>(),
            new_capacity as usize * std::mem::size_of::<MutableSubtree>(),
        )
        .cast::<MutableSubtree>();
        arr.capacity = new_capacity;
    }
}

// ===========================================================================
// TreeArena functions
// ===========================================================================

const TREE_ARENA_PAGE_SIZE: usize = 16 * 1024;

const fn align_up(value: usize, alignment: usize) -> usize {
    debug_assert!(alignment.is_power_of_two());
    (value + alignment - 1) & !(alignment - 1)
}

pub unsafe fn tree_arena_new() -> *mut TreeArena {
    let arena = malloc(std::mem::size_of::<TreeArena>()).cast::<TreeArena>();
    ptr::write(
        arena,
        TreeArena {
            ref_count: AtomicU32::new(1),
            pages: ptr::null_mut(),
            current_page: ptr::null_mut(),
        },
    );
    arena
}

pub unsafe fn tree_arena_retain(arena: *mut TreeArena) {
    if !arena.is_null() {
        let prev = (*arena).ref_count.fetch_add(1, Ordering::SeqCst);
        debug_assert!(prev.wrapping_add(1) != 0);
    }
}

pub unsafe fn tree_arena_release(arena: *mut TreeArena) {
    if arena.is_null() {
        return;
    }

    if (*arena).ref_count.fetch_sub(1, Ordering::SeqCst) != 1 {
        return;
    }

    let mut page = (*arena).pages;
    while !page.is_null() {
        let next = (*page).next;
        free((*page).contents.cast::<c_void>());
        free(page.cast::<c_void>());
        page = next;
    }
    free(arena.cast::<c_void>());
}

/// Try to satisfy an arena allocation from the current bump page.
unsafe fn tree_arena_try_current_page(
    arena: &mut TreeArena,
    size: usize,
    alignment: usize,
) -> *mut c_void {
    if !arena.current_page.is_null() {
        let page = arena.current_page.as_mut().unwrap_unchecked();
        let offset = align_up(page.size, alignment);
        if offset + size <= page.capacity {
            page.size = offset + size;
            return page.contents.add(offset).cast::<c_void>();
        }
    }
    ptr::null_mut()
}

/// Allocate a new arena page and return the first allocation from it.
unsafe fn tree_arena_alloc_new_page(
    arena: &mut TreeArena,
    size: usize,
    alignment: usize,
) -> *mut c_void {
    let capacity = TREE_ARENA_PAGE_SIZE.max(size + alignment);
    let page = malloc(std::mem::size_of::<TreeArenaPage>()).cast::<TreeArenaPage>();
    let contents = malloc(capacity).cast::<u8>();
    ptr::write(
        page,
        TreeArenaPage {
            next: arena.pages,
            contents,
            size: size,
            capacity,
        },
    );
    arena.pages = page;
    arena.current_page = page;
    contents.cast::<c_void>()
}

/// Allocate bytes from the tree arena.
///
/// Internal nodes are stored as `[Subtree children...][SubtreeHeapData]`. The
/// arena uses page-sized bump allocation because accepted trees free all arena
/// nodes together when the last copied `TSTree` is deleted.
unsafe fn tree_arena_alloc(arena: *mut TreeArena, size: usize, alignment: usize) -> *mut c_void {
    debug_assert!(!arena.is_null());
    let arena = arena.as_mut().unwrap_unchecked();

    let result = tree_arena_try_current_page(arena, size, alignment);
    if !result.is_null() {
        return result;
    }

    tree_arena_alloc_new_page(arena, size, alignment)
}

// ===========================================================================
// SubtreePool functions
// ===========================================================================

pub unsafe fn subtree_pool_new(capacity: u32) -> SubtreePool {
    let mut pool = SubtreePool {
        free_trees: mutable_array_new(),
        tree_stack: mutable_array_new(),
    };
    mutable_array_reserve(&mut pool.free_trees, capacity);
    pool
}

pub unsafe fn subtree_pool_delete(self_: &mut SubtreePool) {
    if !self_.free_trees.contents.is_null() {
        for i in 0..self_.free_trees.size {
            let tree = *self_.free_trees.contents.add(i as usize);
            free(tree.ptr.cast::<c_void>());
        }
        mutable_array_delete(&mut self_.free_trees);
    }
    if !self_.tree_stack.contents.is_null() {
        mutable_array_delete(&mut self_.tree_stack);
    }
}

unsafe fn subtree_pool_allocate(self_: &mut SubtreePool) -> *mut SubtreeHeapData {
    if self_.free_trees.size > 0 {
        mutable_array_pop(&mut self_.free_trees).ptr
    } else {
        malloc(std::mem::size_of::<SubtreeHeapData>()).cast::<SubtreeHeapData>()
    }
}

unsafe fn subtree_pool_free(self_: &mut SubtreePool, tree: MutableSubtree) {
    if self_.free_trees.capacity > 0 && self_.free_trees.size < TS_MAX_TREE_POOL_SIZE {
        mutable_array_push(&mut self_.free_trees, tree);
    } else {
        free(tree.ptr.cast::<c_void>());
    }
}

// ===========================================================================
// Subtree inline helpers (from subtree.h static inline functions)
// ===========================================================================

#[inline]
pub unsafe fn subtree_symbol(self_: Subtree) -> TSSymbol {
    if self_.data.is_inline() {
        TSSymbol::from(self_.data.symbol)
    } else {
        (*self_.ptr).symbol
    }
}

#[inline]
pub const unsafe fn subtree_visible(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.visible()
    } else {
        (*self_.ptr).visible()
    }
}

#[inline]
pub const unsafe fn subtree_named(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.named()
    } else {
        (*self_.ptr).named()
    }
}

#[inline]
pub const unsafe fn subtree_extra(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.extra()
    } else {
        (*self_.ptr).extra()
    }
}

#[inline]
pub const unsafe fn subtree_has_changes(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.has_changes()
    } else {
        (*self_.ptr).has_changes()
    }
}

#[inline]
pub const unsafe fn subtree_missing(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.is_missing()
    } else {
        (*self_.ptr).is_missing()
    }
}

#[inline]
pub const unsafe fn subtree_is_keyword(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.is_keyword()
    } else {
        (*self_.ptr).is_keyword()
    }
}

#[inline]
pub const unsafe fn subtree_parse_state(self_: Subtree) -> TSStateId {
    if self_.data.is_inline() {
        self_.data.parse_state
    } else {
        (*self_.ptr).parse_state
    }
}

#[inline]
pub unsafe fn subtree_lookahead_bytes(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        u32::from(self_.data.lookahead_bytes())
    } else {
        (*self_.ptr).lookahead_bytes
    }
}

#[inline]
pub const fn subtree_alloc_size(child_count: u32) -> usize {
    child_count as usize * std::mem::size_of::<Subtree>() + std::mem::size_of::<SubtreeHeapData>()
}

#[inline]
pub const unsafe fn subtree_children(self_: Subtree) -> *mut Subtree {
    if self_.data.is_inline() {
        ptr::null_mut()
    } else {
        self_
            .ptr
            .cast_mut()
            .cast::<Subtree>()
            .sub((*self_.ptr).child_count as usize)
    }
}

#[inline]
const unsafe fn subtree_children_slice<'a>(self_: Subtree) -> &'a [Subtree] {
    let count = subtree_child_count(self_) as usize;
    if count == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(subtree_children(self_), count)
    }
}

#[inline]
unsafe fn mutable_subtree_children<'a>(self_: MutableSubtree) -> &'a mut [Subtree] {
    let count = (*self_.ptr).child_count as usize;
    if count == 0 {
        &mut []
    } else {
        std::slice::from_raw_parts_mut(subtree_children(subtree_from_mut(self_)), count)
    }
}

#[inline]
unsafe fn mutable_subtree_data_mut<'a>(self_: MutableSubtree) -> &'a mut SubtreeHeapData {
    self_.ptr.as_mut().unwrap_unchecked()
}

#[inline]
unsafe fn subtree_data_ref<'a>(self_: Subtree) -> &'a SubtreeHeapData {
    self_.ptr.as_ref().unwrap_unchecked()
}

#[inline]
unsafe fn external_scanner_state_mut<'a>(
    state: *mut ExternalScannerState,
) -> &'a mut ExternalScannerState {
    state.as_mut().unwrap_unchecked()
}

#[inline]
unsafe fn mutable_subtree_child(self_: MutableSubtree, index: usize) -> Subtree {
    *mutable_subtree_children(self_).get_unchecked(index)
}

#[inline]
unsafe fn mutable_subtree_child_mut<'a>(self_: MutableSubtree, index: usize) -> &'a mut Subtree {
    mutable_subtree_children(self_).get_unchecked_mut(index)
}

#[inline]
pub unsafe fn subtree_set_extra(self_: &mut MutableSubtree, is_extra: bool) {
    if self_.data.is_inline() {
        self_.data.set_extra(is_extra);
    } else {
        (*self_.ptr).set_extra(is_extra);
    }
}

// --- #25: leaf_symbol, leaf_parse_state ---

#[inline]
pub unsafe fn subtree_leaf_symbol(self_: Subtree) -> TSSymbol {
    if self_.data.is_inline() {
        return TSSymbol::from(self_.data.symbol);
    }
    if (*self_.ptr).child_count == 0 {
        return (*self_.ptr).symbol;
    }
    (*self_.ptr).data.children.first_leaf.symbol
}

#[inline]
pub const unsafe fn subtree_leaf_parse_state(self_: Subtree) -> TSStateId {
    if self_.data.is_inline() {
        return self_.data.parse_state;
    }
    if (*self_.ptr).child_count == 0 {
        return (*self_.ptr).parse_state;
    }
    (*self_.ptr).data.children.first_leaf.parse_state
}

// --- #26: padding, size, total_size, total_bytes ---

#[inline]
pub unsafe fn subtree_padding(self_: Subtree) -> Length {
    if self_.data.is_inline() {
        Length {
            bytes: u32::from(self_.data.padding_bytes),
            extent: TSPoint {
                row: u32::from(self_.data.padding_rows()),
                column: u32::from(self_.data.padding_columns),
            },
        }
    } else {
        (*self_.ptr).padding
    }
}

#[inline]
pub unsafe fn subtree_size(self_: Subtree) -> Length {
    if self_.data.is_inline() {
        Length {
            bytes: u32::from(self_.data.size_bytes),
            extent: TSPoint {
                row: 0,
                column: u32::from(self_.data.size_bytes),
            },
        }
    } else {
        (*self_.ptr).size
    }
}

#[inline]
pub unsafe fn subtree_total_size(self_: Subtree) -> Length {
    length_add(subtree_padding(self_), subtree_size(self_))
}

#[inline]
pub unsafe fn subtree_total_bytes(self_: Subtree) -> u32 {
    subtree_total_size(self_).bytes
}

// --- #27: child_count, repeat_depth, is_repetition ---

#[inline]
pub const unsafe fn subtree_child_count(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        0
    } else {
        (*self_.ptr).child_count
    }
}

#[inline]
pub unsafe fn subtree_repeat_depth(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        0
    } else {
        u32::from((*self_.ptr).data.children.repeat_depth)
    }
}

#[inline]
pub unsafe fn subtree_is_repetition(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        0
    } else {
        u32::from(!(*self_.ptr).named() && !(*self_.ptr).visible() && (*self_.ptr).child_count != 0)
    }
}

// --- #28: visible_descendant_count, visible_child_count ---

#[inline]
pub const unsafe fn subtree_visible_descendant_count(self_: Subtree) -> u32 {
    if self_.data.is_inline() || (*self_.ptr).child_count == 0 {
        0
    } else {
        (*self_.ptr).data.children.visible_descendant_count
    }
}

#[inline]
pub const unsafe fn subtree_visible_child_count(self_: Subtree) -> u32 {
    if subtree_child_count(self_) > 0 {
        (*self_.ptr).data.children.visible_child_count
    } else {
        0
    }
}

#[inline]
pub const unsafe fn subtree_named_child_count(self_: Subtree) -> u32 {
    if subtree_child_count(self_) > 0 {
        (*self_.ptr).data.children.named_child_count
    } else {
        0
    }
}

// --- #29: error_cost ---

#[inline]
pub const unsafe fn subtree_error_cost(self_: Subtree) -> u32 {
    if subtree_missing(self_) {
        ERROR_COST_PER_MISSING_TREE + ERROR_COST_PER_RECOVERY
    } else if self_.data.is_inline() {
        0
    } else {
        (*self_.ptr).error_cost
    }
}

// --- #30: dynamic_precedence, production_id ---

#[inline]
pub const unsafe fn subtree_dynamic_precedence(self_: Subtree) -> i32 {
    if self_.data.is_inline() || (*self_.ptr).child_count == 0 {
        0
    } else {
        (*self_.ptr).data.children.dynamic_precedence
    }
}

#[inline]
pub const unsafe fn subtree_production_id(self_: Subtree) -> u16 {
    if subtree_child_count(self_) > 0 {
        (*self_.ptr).data.children.production_id
    } else {
        0
    }
}

// --- #31: fragile/external/depends_on_column accessors ---

#[inline]
pub const unsafe fn subtree_fragile_left(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.ptr).fragile_left()
    }
}

#[inline]
pub const unsafe fn subtree_fragile_right(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.ptr).fragile_right()
    }
}

#[inline]
pub const unsafe fn subtree_has_external_tokens(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.ptr).has_external_tokens()
    }
}

#[inline]
pub const unsafe fn subtree_has_external_scanner_state_change(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.ptr).has_external_scanner_state_change()
    }
}

#[inline]
pub const unsafe fn subtree_depends_on_column(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.ptr).depends_on_column()
    }
}

#[inline]
pub const unsafe fn subtree_is_fragile(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.ptr).fragile_left() || (*self_.ptr).fragile_right()
    }
}

#[inline]
pub unsafe fn subtree_is_error(self_: Subtree) -> bool {
    subtree_symbol(self_) == ts_builtin_sym_error
}

#[inline]
pub unsafe fn subtree_is_eof(self_: Subtree) -> bool {
    subtree_symbol(self_) == ts_builtin_sym_end
}

// --- #32: from_mut, to_mut_unsafe ---

#[inline]
pub const fn subtree_from_mut(self_: MutableSubtree) -> Subtree {
    Subtree {
        data: unsafe { self_.data },
    }
}

#[inline]
pub const fn subtree_to_mut_unsafe(self_: Subtree) -> MutableSubtree {
    MutableSubtree {
        data: unsafe { self_.data },
    }
}

// ===========================================================================
// Subtree private helpers
// ===========================================================================

// --- #33: can_inline, set_has_changes ---

#[inline]
fn subtree_can_inline(padding: Length, size: Length, lookahead_bytes: u32) -> bool {
    padding.bytes < u32::from(TS_MAX_INLINE_TREE_LENGTH)
        && padding.extent.row < 16
        && padding.extent.column < u32::from(TS_MAX_INLINE_TREE_LENGTH)
        && size.bytes < u32::from(TS_MAX_INLINE_TREE_LENGTH)
        && size.extent.row == 0
        && size.extent.column < u32::from(TS_MAX_INLINE_TREE_LENGTH)
        && lookahead_bytes < 16
}

unsafe fn subtree_set_has_changes(self_: &mut MutableSubtree) {
    if self_.data.is_inline() {
        self_.data.set_has_changes(true);
    } else {
        (*self_.ptr).set_has_changes(true);
    }
}

// ===========================================================================
// Subtree construction functions (from subtree.c)
// ===========================================================================

// --- #34: new_leaf ---

#[allow(clippy::too_many_arguments)]
/// Create a leaf subtree.
///
/// Small leaves are packed directly into the `Subtree` word when the symbol,
/// padding, size, and lookahead byte counts fit the inline limits. Larger leaves
/// or leaves carrying external scanner state use `SubtreeHeapData` from the
/// parser's subtree pool.
pub unsafe fn subtree_new_leaf(
    pool: &mut SubtreePool,
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
    let metadata = ts_language_symbol_metadata(language, symbol);
    let extra = symbol == ts_builtin_sym_end;

    let is_inline = symbol <= TSSymbol::from(u8::MAX)
        && !has_external_tokens
        && subtree_can_inline(padding, size, lookahead_bytes);

    if is_inline {
        Subtree {
            data: SubtreeInlineData {
                flags: INLINE_IS_INLINE
                    | if metadata.visible { INLINE_VISIBLE } else { 0 }
                    | if metadata.named { INLINE_NAMED } else { 0 }
                    | if extra { INLINE_EXTRA } else { 0 }
                    | if is_keyword { INLINE_IS_KEYWORD } else { 0 },
                symbol: u8::try_from(symbol).expect("inline subtree symbol fits in u8"),
                parse_state,
                padding_columns: u8::try_from(padding.extent.column)
                    .expect("inline subtree padding column fits in u8"),
                rows_and_lookahead: (u8::try_from(padding.extent.row)
                    .expect("inline subtree padding row fits in u8")
                    & 0x0F)
                    | ((u8::try_from(lookahead_bytes)
                        .expect("inline subtree lookahead byte count fits in u8")
                        & 0x0F)
                        << 4),
                padding_bytes: u8::try_from(padding.bytes)
                    .expect("inline subtree padding byte count fits in u8"),
                size_bytes: u8::try_from(size.bytes)
                    .expect("inline subtree size byte count fits in u8"),
            },
        }
    } else {
        let data = subtree_pool_allocate(pool);
        *data = SubtreeHeapData {
            ref_count: 1,
            padding,
            size,
            lookahead_bytes,
            error_cost: 0,
            child_count: 0,
            symbol,
            parse_state,
            flags: SubtreeHeapData::make_flags(
                metadata.visible,
                metadata.named,
                extra,
                false,
                false,
                false,
                has_external_tokens,
                false,
                depends_on_column,
                false,
                is_keyword,
            ),
            data: SubtreeHeapDataContent {
                children: SubtreeChildrenData {
                    visible_child_count: 0,
                    named_child_count: 0,
                    visible_descendant_count: 0,
                    dynamic_precedence: 0,
                    repeat_depth: 0,
                    production_id: 0,
                    first_leaf: FirstLeaf {
                        symbol: 0,
                        parse_state: 0,
                    },
                },
            },
        };
        Subtree { ptr: data }
    }
}

// --- #35: new_error ---

/// Create an error leaf for skipped input.
///
/// Error leaves are marked fragile on both sides so later incremental parsing
/// does not over-trust their boundaries.
pub unsafe fn subtree_new_error(
    pool: &mut SubtreePool,
    lookahead_char: i32,
    padding: Length,
    size: Length,
    bytes_scanned: u32,
    parse_state: TSStateId,
    language: *const TSLanguage,
) -> Subtree {
    let result = subtree_new_leaf(
        pool,
        ts_builtin_sym_error,
        padding,
        size,
        bytes_scanned,
        parse_state,
        false,
        false,
        false,
        language,
    );
    let data = result.ptr.cast_mut();
    (*data).set_fragile_left(true);
    (*data).set_fragile_right(true);
    (*data).data.lookahead_char = lookahead_char;
    result
}

// --- #36: clone ---

pub unsafe fn subtree_clone(self_: Subtree) -> MutableSubtree {
    let data = subtree_data_ref(self_);
    let alloc_size = subtree_alloc_size(data.child_count);
    let new_children = malloc(alloc_size).cast::<Subtree>();
    let old_children = subtree_children(self_);
    ptr::copy_nonoverlapping(
        old_children.cast::<u8>(),
        new_children.cast::<u8>(),
        alloc_size,
    );
    let result = new_children
        .add(data.child_count as usize)
        .cast::<SubtreeHeapData>();
    if data.child_count > 0 {
        for i in 0..data.child_count {
            subtree_retain(*new_children.add(i as usize));
        }
    } else if data.has_external_tokens() {
        (*result).data.external_scanner_state = std::mem::ManuallyDrop::new(
            external_scanner_state_copy(&data.data.external_scanner_state),
        );
    }
    (*result).ref_count = 1;
    (*result).set_arena_owned(false);
    MutableSubtree { ptr: result }
}

// --- #37: new_node ---

/// Create a heap internal node by moving child storage into the node allocation.
///
/// The child array is resized so the `SubtreeHeapData` header can live directly
/// after the child slice, matching the C memory layout:
/// `[child_0, child_1, ... child_n][SubtreeHeapData]`.
pub unsafe fn subtree_new_node(
    symbol: TSSymbol,
    children: *mut SubtreeArray,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    let metadata = ts_language_symbol_metadata(language, symbol);
    let fragile = symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat;

    // Allocate the node's data at the end of the array of children.
    let new_byte_size = subtree_alloc_size((*children).size);
    if ((*children).capacity as usize) * std::mem::size_of::<Subtree>() < new_byte_size {
        (*children).contents =
            realloc((*children).contents.cast::<c_void>(), new_byte_size).cast::<Subtree>();
        (*children).capacity = (new_byte_size / std::mem::size_of::<Subtree>()) as u32;
    }
    let data = (*children)
        .contents
        .add((*children).size as usize)
        .cast::<SubtreeHeapData>();

    *data = SubtreeHeapData {
        ref_count: 1,
        padding: length_zero(),
        size: length_zero(),
        lookahead_bytes: 0,
        error_cost: 0,
        child_count: (*children).size,
        symbol,
        parse_state: 0,
        flags: SubtreeHeapData::make_flags(
            metadata.visible,
            metadata.named,
            false,
            fragile,
            fragile,
            false,
            false,
            false,
            false,
            false,
            false,
        ),
        data: SubtreeHeapDataContent {
            children: SubtreeChildrenData {
                visible_child_count: 0,
                named_child_count: 0,
                visible_descendant_count: 0,
                dynamic_precedence: 0,
                repeat_depth: 0,
                production_id: production_id as u16,
                first_leaf: FirstLeaf {
                    symbol: 0,
                    parse_state: 0,
                },
            },
        },
    };
    let result = MutableSubtree { ptr: data };
    subtree_summarize_children(result, language);
    result
}

/// Create an arena-owned internal node.
///
/// This has the same memory layout as `subtree_new_node`, but allocation
/// comes from the returned tree's arena instead of the transient subtree pool.
pub unsafe fn subtree_new_node_in_arena(
    arena: *mut TreeArena,
    symbol: TSSymbol,
    children: *const Subtree,
    child_count: u32,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    let metadata = ts_language_symbol_metadata(language, symbol);
    let fragile = symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat;
    let byte_size = subtree_alloc_size(child_count);
    let allocation = tree_arena_alloc(arena, byte_size, std::mem::align_of::<SubtreeHeapData>())
        .cast::<Subtree>();

    if child_count > 0 {
        ptr::copy_nonoverlapping(children, allocation, child_count as usize);
    }

    let data = allocation
        .add(child_count as usize)
        .cast::<SubtreeHeapData>();
    *data = SubtreeHeapData {
        ref_count: 1,
        padding: length_zero(),
        size: length_zero(),
        lookahead_bytes: 0,
        error_cost: 0,
        child_count,
        symbol,
        parse_state: 0,
        flags: SubtreeHeapData::make_flags(
            metadata.visible,
            metadata.named,
            false,
            fragile,
            fragile,
            false,
            false,
            false,
            false,
            false,
            false,
        ) | HEAP_ARENA_OWNED,
        data: SubtreeHeapDataContent {
            children: SubtreeChildrenData {
                visible_child_count: 0,
                named_child_count: 0,
                visible_descendant_count: 0,
                dynamic_precedence: 0,
                repeat_depth: 0,
                production_id: production_id as u16,
                first_leaf: FirstLeaf {
                    symbol: 0,
                    parse_state: 0,
                },
            },
        },
    };

    let result = MutableSubtree { ptr: data };
    subtree_summarize_children(result, language);
    result
}

// --- #38: new_error_node ---

pub unsafe fn subtree_new_error_node(
    children: *mut SubtreeArray,
    extra: bool,
    language: *const TSLanguage,
) -> Subtree {
    let result = subtree_new_node(ts_builtin_sym_error, children, 0, language);
    (*result.ptr).set_extra(extra);
    subtree_from_mut(result)
}

// --- #39: new_missing_leaf ---

pub unsafe fn subtree_new_missing_leaf(
    pool: &mut SubtreePool,
    symbol: TSSymbol,
    padding: Length,
    lookahead_bytes: u32,
    language: *const TSLanguage,
) -> Subtree {
    let mut result = subtree_new_leaf(
        pool,
        symbol,
        padding,
        length_zero(),
        lookahead_bytes,
        0,
        false,
        false,
        false,
        language,
    );
    if result.data.is_inline() {
        result.data.set_is_missing(true);
    } else {
        (*result.ptr.cast_mut()).set_is_missing(true);
    }
    result
}

// ===========================================================================
// Subtree mutation / ownership functions
// ===========================================================================

// --- #40: set_symbol ---

pub unsafe fn subtree_set_symbol(
    self_: &mut MutableSubtree,
    symbol: TSSymbol,
    language: *const TSLanguage,
) {
    let metadata = ts_language_symbol_metadata(language, symbol);
    if self_.data.is_inline() {
        debug_assert!(symbol < TSSymbol::from(u8::MAX));
        self_.data.symbol = symbol as u8;
        self_.data.set_named(metadata.named);
        self_.data.set_visible(metadata.visible);
    } else {
        let data = mutable_subtree_data_mut(*self_);
        data.symbol = symbol;
        data.set_named(metadata.named);
        data.set_visible(metadata.visible);
    }
}

// --- #41: make_mut ---

pub unsafe fn subtree_make_mut(pool: &mut SubtreePool, self_: Subtree) -> MutableSubtree {
    if self_.data.is_inline() {
        return MutableSubtree { data: self_.data };
    }
    if (*self_.ptr).ref_count == 1 {
        return subtree_to_mut_unsafe(self_);
    }
    let result = subtree_clone(self_);
    subtree_release(pool, self_);
    result
}

// --- #42: retain ---

pub unsafe fn subtree_retain(self_: Subtree) {
    if self_.data.is_inline() {
        return;
    }
    debug_assert!((*self_.ptr).ref_count > 0);
    let ref_count = ptr::addr_of!((*self_.ptr).ref_count).cast::<AtomicU32>();
    let prev = (*ref_count).fetch_add(1, Ordering::SeqCst);
    debug_assert!(prev.wrapping_add(1) != 0);
}

// --- #43: release ---

pub unsafe fn subtree_release(pool: &mut SubtreePool, self_: Subtree) {
    if self_.data.is_inline() {
        return;
    }
    pool.tree_stack.size = 0;

    debug_assert!((*self_.ptr).ref_count > 0);
    let ref_count = ptr::addr_of!((*self_.ptr).ref_count).cast::<AtomicU32>();
    if (*ref_count).fetch_sub(1, Ordering::SeqCst) == 1 {
        mutable_array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(self_));
    }

    while pool.tree_stack.size > 0 {
        let tree = mutable_array_pop(&mut pool.tree_stack);
        if (*tree.ptr).child_count > 0 {
            let children = subtree_children_slice(subtree_from_mut(tree));
            for child in children {
                let child = *child;
                if child.data.is_inline() {
                    continue;
                }
                debug_assert!((*child.ptr).ref_count > 0);
                let child_ref = ptr::addr_of!((*child.ptr).ref_count).cast::<AtomicU32>();
                if (*child_ref).fetch_sub(1, Ordering::SeqCst) == 1 {
                    mutable_array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(child));
                }
            }
            if !(*tree.ptr).arena_owned() {
                free(children.as_ptr().cast_mut().cast::<c_void>());
            }
        } else {
            if (*tree.ptr).has_external_tokens() {
                let external_scanner_state =
                    ptr::addr_of_mut!((*tree.ptr).data.external_scanner_state)
                        .cast::<ExternalScannerState>();
                external_scanner_state_delete(external_scanner_state_mut(external_scanner_state));
            }
            if !(*tree.ptr).arena_owned() {
                subtree_pool_free(pool, tree);
            }
        }
    }
}

// ===========================================================================
// Subtree tree-balancing / summarization
// ===========================================================================

pub unsafe fn subtree_compress(
    self_: MutableSubtree,
    count: u32,
    language: *const TSLanguage,
    stack: &mut MutableSubtreeArray,
) {
    let initial_stack_size = stack.size;

    let mut tree = self_;
    let symbol = (*tree.ptr).symbol;
    for _ in 0..count {
        if (*tree.ptr).ref_count > 1 || (*tree.ptr).child_count < 2 {
            break;
        }

        let child = subtree_to_mut_unsafe(mutable_subtree_child(tree, 0));
        if child.data.is_inline()
            || (*child.ptr).child_count < 2
            || (*child.ptr).ref_count > 1
            || (*child.ptr).symbol != symbol
        {
            break;
        }

        let grandchild = subtree_to_mut_unsafe(mutable_subtree_child(child, 0));
        if grandchild.data.is_inline()
            || (*grandchild.ptr).child_count < 2
            || (*grandchild.ptr).ref_count > 1
            || (*grandchild.ptr).symbol != symbol
        {
            break;
        }

        // Rotate: tree[0] = grandchild, child[0] = grandchild[last], grandchild[last] = child
        let gc_last = (*grandchild.ptr).child_count as usize - 1;
        *mutable_subtree_child_mut(tree, 0) = subtree_from_mut(grandchild);
        *mutable_subtree_child_mut(child, 0) = mutable_subtree_child(grandchild, gc_last);
        *mutable_subtree_child_mut(grandchild, gc_last) = subtree_from_mut(child);
        mutable_array_push(stack, tree);
        tree = grandchild;
    }

    while stack.size > initial_stack_size {
        tree = mutable_array_pop(stack);
        let child = subtree_to_mut_unsafe(mutable_subtree_child(tree, 0));
        let grandchild = subtree_to_mut_unsafe(mutable_subtree_child(
            child,
            (*child.ptr).child_count as usize - 1,
        ));
        subtree_summarize_children(grandchild, language);
        subtree_summarize_children(child, language);
        subtree_summarize_children(tree, language);
    }
}

pub unsafe fn subtree_summarize_children(self_: MutableSubtree, language: *const TSLanguage) {
    debug_assert!(!self_.data.is_inline());

    let data = mutable_subtree_data_mut(self_);
    data.data.children.named_child_count = 0;
    data.data.children.visible_child_count = 0;
    data.error_cost = 0;
    data.data.children.repeat_depth = 0;
    data.data.children.visible_descendant_count = 0;
    data.set_has_external_tokens(false);
    data.set_depends_on_column(false);
    data.set_has_external_scanner_state_change(false);
    data.data.children.dynamic_precedence = 0;

    let mut structural_index: u32 = 0;
    let alias_sequence =
        language_alias_sequence(language, u32::from(data.data.children.production_id));
    let mut lookahead_end_byte: u32 = 0;

    let children = subtree_children_slice(subtree_from_mut(self_));
    for (i, child) in children.iter().copied().enumerate() {
        let i = i as u32;

        if data.size.extent.row == 0 && subtree_depends_on_column(child) {
            data.set_depends_on_column(true);
        }

        if subtree_has_external_scanner_state_change(child) {
            data.set_has_external_scanner_state_change(true);
        }

        if i == 0 {
            data.padding = subtree_padding(child);
            data.size = subtree_size(child);
        } else {
            data.size = length_add(data.size, subtree_total_size(child));
        }

        let child_lookahead_end_byte =
            data.padding.bytes + data.size.bytes + subtree_lookahead_bytes(child);
        if child_lookahead_end_byte > lookahead_end_byte {
            lookahead_end_byte = child_lookahead_end_byte;
        }

        if subtree_symbol(child) != ts_builtin_sym_error_repeat {
            data.error_cost += subtree_error_cost(child);
        }

        let grandchild_count = subtree_child_count(child);
        if (data.symbol == ts_builtin_sym_error || data.symbol == ts_builtin_sym_error_repeat)
            && !subtree_extra(child)
            && !(subtree_is_error(child) && grandchild_count == 0)
        {
            if subtree_visible(child) {
                data.error_cost += ERROR_COST_PER_SKIPPED_TREE;
            } else if grandchild_count > 0 {
                data.error_cost +=
                    ERROR_COST_PER_SKIPPED_TREE * (*child.ptr).data.children.visible_child_count;
            }
        }

        data.data.children.dynamic_precedence += subtree_dynamic_precedence(child);
        data.data.children.visible_descendant_count += subtree_visible_descendant_count(child);

        if !subtree_extra(child)
            && subtree_symbol(child) != 0
            && !alias_sequence.is_null()
            && *alias_sequence.add(structural_index as usize) != 0
        {
            data.data.children.visible_descendant_count += 1;
            data.data.children.visible_child_count += 1;
            if ts_language_symbol_metadata(language, *alias_sequence.add(structural_index as usize))
                .named
            {
                data.data.children.named_child_count += 1;
            }
        } else if subtree_visible(child) {
            data.data.children.visible_descendant_count += 1;
            data.data.children.visible_child_count += 1;
            if subtree_named(child) {
                data.data.children.named_child_count += 1;
            }
        } else if grandchild_count > 0 {
            data.data.children.visible_child_count +=
                (*child.ptr).data.children.visible_child_count;
            data.data.children.named_child_count += (*child.ptr).data.children.named_child_count;
        }

        if subtree_has_external_tokens(child) {
            data.set_has_external_tokens(true);
        }

        if subtree_is_error(child) {
            data.set_fragile_left(true);
            data.set_fragile_right(true);
            data.parse_state = TS_TREE_STATE_NONE;
        }

        if !subtree_extra(child) {
            structural_index += 1;
        }
    }

    data.lookahead_bytes = lookahead_end_byte - data.size.bytes - data.padding.bytes;

    if data.symbol == ts_builtin_sym_error || data.symbol == ts_builtin_sym_error_repeat {
        data.error_cost += ERROR_COST_PER_RECOVERY
            + ERROR_COST_PER_SKIPPED_CHAR * data.size.bytes
            + ERROR_COST_PER_SKIPPED_LINE * data.size.extent.row;
    }

    if data.child_count > 0 {
        let first_child = *children.get_unchecked(0);
        let last_child = *children.get_unchecked(data.child_count as usize - 1);

        data.data.children.first_leaf.symbol = subtree_leaf_symbol(first_child);
        data.data.children.first_leaf.parse_state = subtree_leaf_parse_state(first_child);

        if subtree_fragile_left(first_child) {
            data.set_fragile_left(true);
        }
        if subtree_fragile_right(last_child) {
            data.set_fragile_right(true);
        }

        if data.child_count >= 2
            && !data.visible()
            && !data.named()
            && subtree_symbol(first_child) == data.symbol
        {
            if subtree_repeat_depth(first_child) > subtree_repeat_depth(last_child) {
                data.data.children.repeat_depth = (subtree_repeat_depth(first_child) + 1) as u16;
            } else {
                data.data.children.repeat_depth = (subtree_repeat_depth(last_child) + 1) as u16;
            }
        }
    }
}

// ===========================================================================
// Subtree comparison / query
// ===========================================================================

pub unsafe fn subtree_compare(left: Subtree, right: Subtree, pool: &mut SubtreePool) -> i32 {
    mutable_array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(left));
    mutable_array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(right));

    while pool.tree_stack.size > 0 {
        let right = subtree_from_mut(mutable_array_pop(&mut pool.tree_stack));
        let left = subtree_from_mut(mutable_array_pop(&mut pool.tree_stack));

        let mut result = 0i32;
        if subtree_symbol(left) < subtree_symbol(right) {
            result = -1;
        } else if subtree_symbol(right) < subtree_symbol(left) {
            result = 1;
        } else if subtree_child_count(left) < subtree_child_count(right) {
            result = -1;
        } else if subtree_child_count(right) < subtree_child_count(left) {
            result = 1;
        }
        if result != 0 {
            pool.tree_stack.size = 0;
            return result;
        }

        let count = subtree_child_count(left);
        let left_children = subtree_children_slice(left);
        let right_children = subtree_children_slice(right);
        let mut i = count;
        while i > 0 {
            i -= 1;
            let left_child = *left_children.get_unchecked(i as usize);
            let right_child = *right_children.get_unchecked(i as usize);
            mutable_array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(left_child));
            mutable_array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(right_child));
        }
    }

    0
}

pub unsafe fn subtree_edit(
    mut self_: Subtree,
    input_edit: &TSInputEdit,
    pool: &mut SubtreePool,
) -> Subtree {
    struct EditEntry {
        tree: *mut Subtree,
        edit: Edit,
    }

    let mut stack: Vec<EditEntry> = Vec::new();
    stack.push(EditEntry {
        tree: std::ptr::addr_of_mut!(self_),
        edit: Edit {
            start: Length {
                bytes: input_edit.start_byte,
                extent: input_edit.start_point,
            },
            old_end: Length {
                bytes: input_edit.old_end_byte,
                extent: input_edit.old_end_point,
            },
            new_end: Length {
                bytes: input_edit.new_end_byte,
                extent: input_edit.new_end_point,
            },
        },
    });

    while let Some(entry) = stack.pop() {
        let mut edit = entry.edit;
        let is_noop =
            edit.old_end.bytes == edit.start.bytes && edit.new_end.bytes == edit.start.bytes;
        let is_pure_insertion = edit.old_end.bytes == edit.start.bytes;
        let parent_depends_on_column = subtree_depends_on_column(*entry.tree);
        let column_shifted = edit.new_end.extent.column != edit.old_end.extent.column;

        let mut size = subtree_size(*entry.tree);
        let mut padding = subtree_padding(*entry.tree);
        let total_size = length_add(padding, size);
        let lookahead_bytes = subtree_lookahead_bytes(*entry.tree);
        let end_byte = total_size.bytes + lookahead_bytes;
        if edit.start.bytes > end_byte || (is_noop && edit.start.bytes == end_byte) {
            continue;
        }

        // Edit is entirely within the space before this subtree
        if edit.old_end.bytes <= padding.bytes {
            padding = length_add(edit.new_end, length_sub(padding, edit.old_end));
        }
        // Edit starts before and extends into this subtree
        else if edit.start.bytes < padding.bytes {
            size = length_saturating_sub(size, length_sub(edit.old_end, padding));
            padding = edit.new_end;
        }
        // Edit is within this subtree
        else if edit.start.bytes < total_size.bytes
            || (edit.start.bytes == total_size.bytes && is_pure_insertion)
        {
            size = length_add(
                length_sub(edit.new_end, padding),
                length_saturating_sub(total_size, edit.old_end),
            );
        }

        let mut result = subtree_make_mut(pool, *entry.tree);

        if result.data.is_inline() {
            if subtree_can_inline(padding, size, lookahead_bytes) {
                result.data.padding_bytes = padding.bytes as u8;
                result.data.set_padding_rows(padding.extent.row as u8);
                result.data.padding_columns = padding.extent.column as u8;
                result.data.size_bytes = size.bytes as u8;
            } else {
                // Promote inline node to heap
                let data = subtree_pool_allocate(pool);
                *data = SubtreeHeapData {
                    ref_count: 1,
                    padding,
                    size,
                    lookahead_bytes,
                    error_cost: 0,
                    child_count: 0,
                    symbol: TSSymbol::from(result.data.symbol),
                    parse_state: result.data.parse_state,
                    flags: SubtreeHeapData::make_flags(
                        result.data.visible(),
                        result.data.named(),
                        result.data.extra(),
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        result.data.is_missing(),
                        result.data.is_keyword(),
                    ),
                    data: SubtreeHeapDataContent { lookahead_char: 0 },
                };
                result.ptr = data;
            }
        } else {
            (*result.ptr).padding = padding;
            (*result.ptr).size = size;
        }

        subtree_set_has_changes(&mut result);
        *entry.tree = subtree_from_mut(result);

        let mut child_right = length_zero();
        let n = subtree_child_count(*entry.tree);
        let children = subtree_children_slice(*entry.tree);
        for i in 0..n {
            let child = children.get_unchecked(i as usize);
            let child_size = subtree_total_size(*child);
            let child_left = child_right;
            child_right = length_add(child_left, child_size);

            // If this child ends before the edit, it is not affected.
            if child_right.bytes + subtree_lookahead_bytes(*child) < edit.start.bytes {
                continue;
            }

            // Keep editing child nodes until a node is reached that starts after the edit.
            if ((child_left.bytes > edit.old_end.bytes)
                || (child_left.bytes == edit.old_end.bytes && child_size.bytes > 0 && i > 0))
                && (!parent_depends_on_column || child_left.extent.row > padding.extent.row)
                && (!subtree_depends_on_column(*child)
                    || !column_shifted
                    || child_left.extent.row > edit.old_end.extent.row)
            {
                break;
            }

            // Transform edit into the child's coordinate space.
            let mut child_edit = Edit {
                start: length_saturating_sub(edit.start, child_left),
                old_end: length_saturating_sub(edit.old_end, child_left),
                new_end: length_saturating_sub(edit.new_end, child_left),
            };

            // Interpret all inserted text as applying to the *first* child that touches the edit.
            if child_right.bytes > edit.start.bytes
                || (child_right.bytes == edit.start.bytes && is_pure_insertion)
            {
                edit.new_end = edit.start;
            } else {
                child_edit.old_end = child_edit.start;
                child_edit.new_end = child_edit.start;
            }

            stack.push(EditEntry {
                tree: ptr::from_ref(child).cast_mut(),
                edit: child_edit,
            });
        }
    }

    self_
}

pub unsafe fn subtree_last_external_token(mut tree: Subtree) -> Subtree {
    if !subtree_has_external_tokens(tree) {
        return NULL_SUBTREE;
    }
    loop {
        let data = subtree_data_ref(tree);
        if data.child_count == 0 {
            break;
        }
        let children = subtree_children_slice(tree);
        let mut i = data.child_count as usize;
        while i > 0 {
            i -= 1;
            let child = *children.get_unchecked(i);
            if subtree_has_external_tokens(child) {
                tree = child;
                break;
            }
        }
    }
    tree
}

pub unsafe fn subtree_external_scanner_state(self_: &Subtree) -> &ExternalScannerState {
    if self_.ptr.is_null() || self_.data.is_inline() {
        return &EMPTY_EXTERNAL_SCANNER_STATE;
    }

    let data = subtree_data_ref(*self_);
    if data.has_external_tokens() && data.child_count == 0 {
        &data.data.external_scanner_state
    } else {
        &EMPTY_EXTERNAL_SCANNER_STATE
    }
}

pub unsafe fn subtree_external_scanner_state_eq(self_: &Subtree, other: &Subtree) -> bool {
    let state_self = subtree_external_scanner_state(self_);
    let state_other = subtree_external_scanner_state(other);
    external_scanner_state_eq(
        state_self,
        external_scanner_state_data(state_other),
        state_other.length,
    )
}

// ===========================================================================
// Subtree string / debug output
// ===========================================================================

extern "C" {
    fn snprintf(s: *mut i8, n: usize, format: *const i8, ...) -> i32;
    fn fprintf(f: *mut c_void, format: *const i8, ...) -> i32;
    fn fputc(c: i32, f: *mut c_void) -> i32;
    fn fputs(s: *const i8, f: *mut c_void) -> i32;
}

static ROOT_FIELD: &[u8; 9] = b"__ROOT__\0";

/// Rust re-implementation of the static inline `language_field_map` from `language.h`.
unsafe fn language_field_map(
    language: *const TSLanguage,
    production_id: u32,
    start: *mut *const TSFieldMapEntry,
    end: *mut *const TSFieldMapEntry,
) {
    let lang = language.cast::<TSLanguageData>();
    if (*lang).field_count == 0 {
        *start = ptr::null();
        *end = ptr::null();
        return;
    }
    let slice = *(*lang).field_map_slices.add(production_id as usize);
    *start = (*lang).field_map_entries.add(slice.index as usize);
    *end = (*lang)
        .field_map_entries
        .add(slice.index as usize + slice.length as usize);
}

/// Rust re-implementation of the static inline `language_write_symbol_as_dot_string`.
unsafe fn language_write_symbol_as_dot_string(
    language: *const TSLanguage,
    f: *mut c_void,
    symbol: TSSymbol,
) {
    let name = ts_language_symbol_name(language, symbol);
    let mut chr = name;
    while *chr != 0 {
        match *chr as u8 {
            b'"' | b'\\' => {
                fputc(i32::from(b'\\'), f);
                fputc(i32::from(*chr), f);
            }
            b'\n' => {
                fputs(c"\\n".as_ptr().cast::<i8>(), f);
            }
            b'\t' => {
                fputs(c"\\t".as_ptr().cast::<i8>(), f);
            }
            _ => {
                fputc(i32::from(*chr), f);
            }
        }
        chr = chr.add(1);
    }
}

unsafe fn subtree__write_char_to_string(s: *mut i8, n: usize, chr: i32) -> usize {
    if chr == -1 {
        snprintf(s, n, c"INVALID".as_ptr().cast::<i8>()) as usize
    } else if chr == 0 {
        snprintf(s, n, c"'\\0'".as_ptr().cast::<i8>()) as usize
    } else if chr == i32::from(b'\n') {
        snprintf(s, n, c"'\\n'".as_ptr().cast::<i8>()) as usize
    } else if chr == i32::from(b'\t') {
        snprintf(s, n, c"'\\t'".as_ptr().cast::<i8>()) as usize
    } else if chr == i32::from(b'\r') {
        snprintf(s, n, c"'\\r'".as_ptr().cast::<i8>()) as usize
    } else if (0x20..0x7F).contains(&chr) {
        snprintf(s, n, c"'%c'".as_ptr().cast::<i8>(), chr) as usize
    } else {
        snprintf(s, n, c"%d".as_ptr().cast::<i8>(), chr) as usize
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn subtree__write_to_string(
    self_: Subtree,
    string: *mut i8,
    limit: usize,
    language: *const TSLanguage,
    include_all: bool,
    alias_symbol: TSSymbol,
    alias_is_named: bool,
    field_name: *const i8,
) -> usize {
    if self_.ptr.is_null() {
        return snprintf(string, limit, c"(NULL)".as_ptr().cast::<i8>()) as usize;
    }

    let mut cursor = string;
    let mut string_measuring = string;
    let writer: *mut *mut i8 = if limit > 1 {
        &mut cursor
    } else {
        &mut string_measuring
    };
    let is_root = field_name == ROOT_FIELD.as_ptr().cast::<i8>();
    let is_visible = include_all
        || subtree_missing(self_)
        || (if alias_symbol != 0 {
            alias_is_named
        } else {
            subtree_visible(self_) && subtree_named(self_)
        });

    if is_visible {
        if !is_root {
            cursor = cursor.add(snprintf(*writer, limit, c" ".as_ptr().cast::<i8>()) as usize);
            if !field_name.is_null() {
                cursor =
                    cursor.add(
                        snprintf(*writer, limit, c"%s: ".as_ptr().cast::<i8>(), field_name)
                            as usize,
                    );
            }
        }

        if subtree_is_error(self_) && subtree_child_count(self_) == 0 && (*self_.ptr).size.bytes > 0
        {
            cursor = cursor
                .add(snprintf(*writer, limit, c"(UNEXPECTED ".as_ptr().cast::<i8>()) as usize);
            cursor = cursor.add(subtree__write_char_to_string(
                *writer,
                limit,
                (*self_.ptr).data.lookahead_char,
            ));
        } else {
            let symbol = if alias_symbol != 0 {
                alias_symbol
            } else {
                subtree_symbol(self_)
            };
            let symbol_name = ts_language_symbol_name(language, symbol);
            if subtree_missing(self_) {
                cursor = cursor
                    .add(snprintf(*writer, limit, c"(MISSING ".as_ptr().cast::<i8>()) as usize);
                if alias_is_named || subtree_named(self_) {
                    cursor = cursor.add(snprintf(
                        *writer,
                        limit,
                        c"%s".as_ptr().cast::<i8>(),
                        symbol_name,
                    ) as usize);
                } else {
                    cursor = cursor.add(snprintf(
                        *writer,
                        limit,
                        c"\"%s\"".as_ptr().cast::<i8>(),
                        symbol_name,
                    ) as usize);
                }
            } else {
                cursor =
                    cursor.add(
                        snprintf(*writer, limit, c"(%s".as_ptr().cast::<i8>(), symbol_name)
                            as usize,
                    );
            }
        }
    } else if is_root {
        let symbol = if alias_symbol != 0 {
            alias_symbol
        } else {
            subtree_symbol(self_)
        };
        let symbol_name = ts_language_symbol_name(language, symbol);
        if subtree_child_count(self_) > 0 {
            cursor = cursor
                .add(snprintf(*writer, limit, c"(%s".as_ptr().cast::<i8>(), symbol_name) as usize);
        } else if subtree_named(self_) {
            cursor = cursor
                .add(snprintf(*writer, limit, c"(%s)".as_ptr().cast::<i8>(), symbol_name) as usize);
        } else {
            cursor = cursor.add(snprintf(
                *writer,
                limit,
                c"(\"%s\")".as_ptr().cast::<i8>(),
                symbol_name,
            ) as usize);
        }
    }

    if subtree_child_count(self_) > 0 {
        let alias_sequence = language_alias_sequence(
            language,
            u32::from((*self_.ptr).data.children.production_id),
        );
        let mut field_map: *const TSFieldMapEntry = ptr::null();
        let mut field_map_end: *const TSFieldMapEntry = ptr::null();
        language_field_map(
            language,
            u32::from((*self_.ptr).data.children.production_id),
            &mut field_map,
            &mut field_map_end,
        );

        let mut structural_child_index: u32 = 0;
        for child in subtree_children_slice(self_) {
            let child = *child;
            if subtree_extra(child) {
                cursor = cursor.add(subtree__write_to_string(
                    child,
                    *writer,
                    limit,
                    language,
                    include_all,
                    0,
                    false,
                    ptr::null(),
                ));
            } else {
                let subtree_alias_symbol = if !alias_sequence.is_null() {
                    *alias_sequence.add(structural_child_index as usize)
                } else {
                    0
                };
                let subtree_alias_is_named = if subtree_alias_symbol != 0 {
                    ts_language_symbol_metadata(language, subtree_alias_symbol).named
                } else {
                    false
                };

                let mut child_field_name: *const i8 =
                    if is_visible { ptr::null() } else { field_name };
                let mut map = field_map;
                while map < field_map_end {
                    if !(*map).inherited && (*map).child_index == structural_child_index as u8 {
                        let lang = language.cast::<TSLanguageData>();
                        child_field_name = *(*lang).field_names.add((*map).field_id as usize);
                        break;
                    }
                    map = map.add(1);
                }

                cursor = cursor.add(subtree__write_to_string(
                    child,
                    *writer,
                    limit,
                    language,
                    include_all,
                    subtree_alias_symbol,
                    subtree_alias_is_named,
                    child_field_name,
                ));
                structural_child_index += 1;
            }
        }
    }

    if is_visible {
        cursor = cursor.add(snprintf(*writer, limit, c")".as_ptr().cast::<i8>()) as usize);
    }

    cursor as usize - string as usize
}

pub unsafe fn subtree_string(
    self_: Subtree,
    alias_symbol: TSSymbol,
    alias_is_named: bool,
    language: *const TSLanguage,
    include_all: bool,
) -> *mut i8 {
    let mut scratch_string: [i8; 1] = [0];
    let size = subtree__write_to_string(
        self_,
        scratch_string.as_mut_ptr(),
        1,
        language,
        include_all,
        alias_symbol,
        alias_is_named,
        ROOT_FIELD.as_ptr().cast::<i8>(),
    ) + 1;
    let result = malloc(size).cast::<i8>();
    subtree__write_to_string(
        self_,
        result,
        size,
        language,
        include_all,
        alias_symbol,
        alias_is_named,
        ROOT_FIELD.as_ptr().cast::<i8>(),
    );
    result
}

unsafe fn subtree__print_dot_graph(
    self_: *const Subtree,
    start_offset: u32,
    language: *const TSLanguage,
    alias_symbol: TSSymbol,
    f: *mut c_void,
) {
    let tree = *self_;
    let subtree_symbol = subtree_symbol(tree);
    let symbol = if alias_symbol != 0 {
        alias_symbol
    } else {
        subtree_symbol
    };
    let end_offset = start_offset + subtree_total_bytes(tree);
    fprintf(
        f,
        c"tree_%p [label=\"".as_ptr().cast::<i8>(),
        self_.cast::<c_void>(),
    );
    language_write_symbol_as_dot_string(language, f, symbol);
    fprintf(f, c"\"".as_ptr().cast::<i8>());

    if subtree_child_count(tree) == 0 {
        fprintf(f, c", shape=plaintext".as_ptr().cast::<i8>());
    }
    if subtree_extra(tree) {
        fprintf(f, c", fontcolor=gray".as_ptr().cast::<i8>());
    }
    if subtree_has_changes(tree) {
        fprintf(f, c", color=green, penwidth=2".as_ptr().cast::<i8>());
    }

    fprintf(
        f,
        c", tooltip=\"range: %u - %u\nstate: %d\nerror-cost: %u\nhas-changes: %u\ndepends-on-column: %u\ndescendant-count: %u\nrepeat-depth: %u\nlookahead-bytes: %u".as_ptr().cast::<i8>(),
        start_offset,
        end_offset,
        i32::from(subtree_parse_state(tree)),
        subtree_error_cost(tree),
        u32::from(subtree_has_changes(tree)),
        u32::from(subtree_depends_on_column(tree)),
        subtree_visible_descendant_count(tree),
        subtree_repeat_depth(tree),
        subtree_lookahead_bytes(tree),
    );

    if subtree_is_error(tree)
        && subtree_child_count(tree) == 0
        && (*tree.ptr).data.lookahead_char != 0
    {
        fprintf(
            f,
            c"\ncharacter: '%c'".as_ptr().cast::<i8>(),
            (*tree.ptr).data.lookahead_char,
        );
    }

    fprintf(f, c"\"]\n".as_ptr().cast::<i8>());

    let mut child_start_offset = start_offset;
    let lang = language.cast::<TSLanguageData>();
    let mut child_info_offset =
        u32::from((*lang).max_alias_sequence_length) * u32::from(subtree_production_id(tree));
    for (i, child) in subtree_children_slice(tree).iter().enumerate() {
        let child_ptr = ptr::from_ref(child);
        let mut subtree_alias_symbol: TSSymbol = 0;
        if !subtree_extra(*child) && child_info_offset != 0 {
            subtree_alias_symbol = *(*lang).alias_sequences.add(child_info_offset as usize);
            child_info_offset += 1;
        }
        subtree__print_dot_graph(
            child_ptr,
            child_start_offset,
            language,
            subtree_alias_symbol,
            f,
        );
        fprintf(
            f,
            c"tree_%p -> tree_%p [tooltip=%u]\n".as_ptr().cast::<i8>(),
            self_.cast::<c_void>(),
            child_ptr.cast::<c_void>(),
            i,
        );
        child_start_offset += subtree_total_bytes(*child);
    }
}

pub unsafe fn subtree_print_dot_graph(self_: Subtree, language: *const TSLanguage, f: *mut c_void) {
    fprintf(f, c"digraph tree {\n".as_ptr().cast::<i8>());
    fprintf(f, c"edge [arrowhead=none]\n".as_ptr().cast::<i8>());
    subtree__print_dot_graph(std::ptr::addr_of!(self_), 0, language, 0, f);
    fprintf(f, c"}\n".as_ptr().cast::<i8>());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_node_owns_child_array_until_release() {
        unsafe {
            let mut pool = subtree_pool_new(4);
            let child1 = subtree_new_error(
                &mut pool,
                b'a' as i32,
                length_zero(),
                length_zero(),
                0,
                0,
                ptr::null(),
            );
            let child2 = subtree_new_error(
                &mut pool,
                b'b' as i32,
                length_zero(),
                length_zero(),
                0,
                0,
                ptr::null(),
            );

            let mut children = subtree_array_new();
            array_push_subtree(&mut children, child1);
            array_push_subtree(&mut children, child2);

            let parent =
                subtree_new_node(ts_builtin_sym_error_repeat, &mut children, 0, ptr::null());
            let parent_tree = subtree_from_mut(parent);

            assert_eq!(subtree_child_count(parent_tree), 2);
            assert_eq!(subtree_children_slice(parent_tree).len(), 2);
            assert_eq!(
                subtree_symbol(subtree_children_slice(parent_tree)[0]),
                ts_builtin_sym_error
            );
            assert_eq!(
                subtree_symbol(subtree_children_slice(parent_tree)[1]),
                ts_builtin_sym_error
            );

            subtree_release(&mut pool, parent_tree);
            subtree_pool_delete(&mut pool);
        }
    }
}
