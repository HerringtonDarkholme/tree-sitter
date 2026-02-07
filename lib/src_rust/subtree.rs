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

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSMapSlice {
    pub index: u16,
    pub length: u16,
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

// SAFETY: Only used in a read-only static (EMPTY_EXTERNAL_SCANNER_STATE).
unsafe impl Sync for ExternalScannerState {}

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
///
/// Little-endian layout (matches the C struct bitfields):
///   byte 0: is_inline:1, visible:1, named:1, extra:1, has_changes:1, is_missing:1, is_keyword:1, unused:1
///   byte 1: symbol
///   bytes 2-3: parse_state (u16 LE)
///   byte 4: padding_columns
///   byte 5: padding_rows:4 (low), lookahead_bytes:4 (high)
///   byte 6: padding_bytes
///   byte 7: size_bytes
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SubtreeInlineData {
    /// Byte 0: packed bitfields (is_inline, visible, named, extra, has_changes, is_missing, is_keyword)
    pub flags: u8,
    pub symbol: u8,
    pub parse_state: u16,
    pub padding_columns: u8,
    /// Low 4 bits = padding_rows, high 4 bits = lookahead_bytes
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
    pub fn is_inline(&self) -> bool { self.flags & INLINE_IS_INLINE != 0 }
    #[inline(always)]
    pub fn visible(&self) -> bool { self.flags & INLINE_VISIBLE != 0 }
    #[inline(always)]
    pub fn named(&self) -> bool { self.flags & INLINE_NAMED != 0 }
    #[inline(always)]
    pub fn extra(&self) -> bool { self.flags & INLINE_EXTRA != 0 }
    #[inline(always)]
    pub fn has_changes(&self) -> bool { self.flags & INLINE_HAS_CHANGES != 0 }
    #[inline(always)]
    pub fn is_missing(&self) -> bool { self.flags & INLINE_IS_MISSING != 0 }
    #[inline(always)]
    pub fn is_keyword(&self) -> bool { self.flags & INLINE_IS_KEYWORD != 0 }
    #[inline(always)]
    pub fn padding_rows(&self) -> u8 { self.rows_and_lookahead & 0x0F }
    #[inline(always)]
    pub fn lookahead_bytes(&self) -> u8 { (self.rows_and_lookahead >> 4) & 0x0F }

    #[inline(always)]
    pub fn set_is_inline(&mut self, v: bool) { if v { self.flags |= INLINE_IS_INLINE } else { self.flags &= !INLINE_IS_INLINE } }
    #[inline(always)]
    pub fn set_visible(&mut self, v: bool) { if v { self.flags |= INLINE_VISIBLE } else { self.flags &= !INLINE_VISIBLE } }
    #[inline(always)]
    pub fn set_named(&mut self, v: bool) { if v { self.flags |= INLINE_NAMED } else { self.flags &= !INLINE_NAMED } }
    #[inline(always)]
    pub fn set_extra(&mut self, v: bool) { if v { self.flags |= INLINE_EXTRA } else { self.flags &= !INLINE_EXTRA } }
    #[inline(always)]
    pub fn set_has_changes(&mut self, v: bool) { if v { self.flags |= INLINE_HAS_CHANGES } else { self.flags &= !INLINE_HAS_CHANGES } }
    #[inline(always)]
    pub fn set_is_missing(&mut self, v: bool) { if v { self.flags |= INLINE_IS_MISSING } else { self.flags &= !INLINE_IS_MISSING } }
    #[inline(always)]
    pub fn set_is_keyword(&mut self, v: bool) { if v { self.flags |= INLINE_IS_KEYWORD } else { self.flags &= !INLINE_IS_KEYWORD } }
    #[inline(always)]
    pub fn set_padding_rows(&mut self, v: u8) { self.rows_and_lookahead = (self.rows_and_lookahead & 0xF0) | (v & 0x0F) }
    #[inline(always)]
    pub fn set_lookahead_bytes(&mut self, v: u8) { self.rows_and_lookahead = (self.rows_and_lookahead & 0x0F) | ((v & 0x0F) << 4) }
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

    /// Packed bitfield flags (11 bits used, matches C bitfield layout)
    /// bit 0: visible, bit 1: named, bit 2: extra, bit 3: fragile_left,
    /// bit 4: fragile_right, bit 5: has_changes, bit 6: has_external_tokens,
    /// bit 7: has_external_scanner_state_change, bit 8: depends_on_column,
    /// bit 9: is_missing, bit 10: is_keyword
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

impl SubtreeHeapData {
    #[inline(always)] pub fn visible(&self) -> bool { self.flags & HEAP_VISIBLE != 0 }
    #[inline(always)] pub fn named(&self) -> bool { self.flags & HEAP_NAMED != 0 }
    #[inline(always)] pub fn extra(&self) -> bool { self.flags & HEAP_EXTRA != 0 }
    #[inline(always)] pub fn fragile_left(&self) -> bool { self.flags & HEAP_FRAGILE_LEFT != 0 }
    #[inline(always)] pub fn fragile_right(&self) -> bool { self.flags & HEAP_FRAGILE_RIGHT != 0 }
    #[inline(always)] pub fn has_changes(&self) -> bool { self.flags & HEAP_HAS_CHANGES != 0 }
    #[inline(always)] pub fn has_external_tokens(&self) -> bool { self.flags & HEAP_HAS_EXTERNAL_TOKENS != 0 }
    #[inline(always)] pub fn has_external_scanner_state_change(&self) -> bool { self.flags & HEAP_HAS_EXTERNAL_SCANNER_STATE_CHANGE != 0 }
    #[inline(always)] pub fn depends_on_column(&self) -> bool { self.flags & HEAP_DEPENDS_ON_COLUMN != 0 }
    #[inline(always)] pub fn is_missing(&self) -> bool { self.flags & HEAP_IS_MISSING != 0 }
    #[inline(always)] pub fn is_keyword(&self) -> bool { self.flags & HEAP_IS_KEYWORD != 0 }

    #[inline(always)] pub fn set_visible(&mut self, v: bool) { if v { self.flags |= HEAP_VISIBLE } else { self.flags &= !HEAP_VISIBLE } }
    #[inline(always)] pub fn set_named(&mut self, v: bool) { if v { self.flags |= HEAP_NAMED } else { self.flags &= !HEAP_NAMED } }
    #[inline(always)] pub fn set_extra(&mut self, v: bool) { if v { self.flags |= HEAP_EXTRA } else { self.flags &= !HEAP_EXTRA } }
    #[inline(always)] pub fn set_fragile_left(&mut self, v: bool) { if v { self.flags |= HEAP_FRAGILE_LEFT } else { self.flags &= !HEAP_FRAGILE_LEFT } }
    #[inline(always)] pub fn set_fragile_right(&mut self, v: bool) { if v { self.flags |= HEAP_FRAGILE_RIGHT } else { self.flags &= !HEAP_FRAGILE_RIGHT } }
    #[inline(always)] pub fn set_has_changes(&mut self, v: bool) { if v { self.flags |= HEAP_HAS_CHANGES } else { self.flags &= !HEAP_HAS_CHANGES } }
    #[inline(always)] pub fn set_has_external_tokens(&mut self, v: bool) { if v { self.flags |= HEAP_HAS_EXTERNAL_TOKENS } else { self.flags &= !HEAP_HAS_EXTERNAL_TOKENS } }
    #[inline(always)] pub fn set_has_external_scanner_state_change(&mut self, v: bool) { if v { self.flags |= HEAP_HAS_EXTERNAL_SCANNER_STATE_CHANGE } else { self.flags &= !HEAP_HAS_EXTERNAL_SCANNER_STATE_CHANGE } }
    #[inline(always)] pub fn set_depends_on_column(&mut self, v: bool) { if v { self.flags |= HEAP_DEPENDS_ON_COLUMN } else { self.flags &= !HEAP_DEPENDS_ON_COLUMN } }
    #[inline(always)] pub fn set_is_missing(&mut self, v: bool) { if v { self.flags |= HEAP_IS_MISSING } else { self.flags &= !HEAP_IS_MISSING } }
    #[inline(always)] pub fn set_is_keyword(&mut self, v: bool) { if v { self.flags |= HEAP_IS_KEYWORD } else { self.flags &= !HEAP_IS_KEYWORD } }

    /// Build flags from individual booleans (for struct initialization)
    #[inline]
    pub fn make_flags(
        visible: bool, named: bool, extra: bool,
        fragile_left: bool, fragile_right: bool,
        has_changes: bool, has_external_tokens: bool,
        has_external_scanner_state_change: bool,
        depends_on_column: bool, is_missing: bool, is_keyword: bool,
    ) -> u16 {
        (visible as u16) << 0
        | (named as u16) << 1
        | (extra as u16) << 2
        | (fragile_left as u16) << 3
        | (fragile_right as u16) << 4
        | (has_changes as u16) << 5
        | (has_external_tokens as u16) << 6
        | (has_external_scanner_state_change as u16) << 7
        | (depends_on_column as u16) << 8
        | (is_missing as u16) << 9
        | (is_keyword as u16) << 10
    }
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
    #[link_name = "memcmp"]
    fn libc_memcmp(s1: *const c_void, s2: *const c_void, n: usize) -> i32;

    fn ts_language_symbol_metadata(language: *const TSLanguage, symbol: TSSymbol)
        -> TSSymbolMetadata;
    fn ts_language_symbol_name(
        language: *const TSLanguage,
        symbol: TSSymbol,
    ) -> *const i8;
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

/// Rust re-implementation of the static inline ts_language_alias_sequence from language.h.
#[inline]
unsafe fn language_alias_sequence(
    language: *const TSLanguage,
    production_id: u32,
) -> *const TSSymbol {
    let lang = language as *const TSLanguageData;
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

pub unsafe fn ts_external_scanner_state_init(
    self_: *mut ExternalScannerState,
    data: *const u8,
    length: u32,
) {
    (*self_).length = length;
    if length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        (*self_).data.long_data = ts_malloc(length as usize) as *mut u8;
        ptr::copy_nonoverlapping(data, (*self_).data.long_data, length as usize);
    } else {
        ptr::copy_nonoverlapping(data, (*self_).data.short_data.as_mut_ptr(), length as usize);
    }
}

pub unsafe fn ts_external_scanner_state_copy(
    self_: *const ExternalScannerState,
) -> ExternalScannerState {
    let mut result = ExternalScannerState {
        data: ExternalScannerStateData { short_data: (*self_).data.short_data },
        length: (*self_).length,
    };
    if (*self_).length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        result.data.long_data = ts_malloc((*self_).length as usize) as *mut u8;
        ptr::copy_nonoverlapping(
            (*self_).data.long_data,
            result.data.long_data,
            (*self_).length as usize,
        );
    }
    result
}

pub unsafe fn ts_external_scanner_state_delete(self_: *mut ExternalScannerState) {
    if (*self_).length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        ts_free((*self_).data.long_data as *mut c_void);
    }
}

pub unsafe fn ts_external_scanner_state_data(
    self_: *const ExternalScannerState,
) -> *const u8 {
    if (*self_).length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        (*self_).data.long_data
    } else {
        (*self_).data.short_data.as_ptr()
    }
}

pub unsafe fn ts_external_scanner_state_eq(
    self_: *const ExternalScannerState,
    buffer: *const u8,
    length: u32,
) -> bool {
    (*self_).length == length
        && libc_memcmp(
            ts_external_scanner_state_data(self_) as *const c_void,
            buffer as *const c_void,
            length as usize,
        ) == 0
}

// ===========================================================================
// SubtreeArray helpers (replaces array.h macros)
// ===========================================================================

/// Grow array capacity if needed to fit `count` more elements.
unsafe fn array_grow(arr: *mut SubtreeArray, count: u32) {
    let new_size = (*arr).size + count;
    if new_size > (*arr).capacity {
        let mut new_capacity = (*arr).capacity * 2;
        if new_capacity < 8 {
            new_capacity = 8;
        }
        if new_capacity < new_size {
            new_capacity = new_size;
        }
        (*arr).contents = ts_realloc(
            (*arr).contents as *mut c_void,
            new_capacity as usize * std::mem::size_of::<Subtree>(),
        ) as *mut Subtree;
        (*arr).capacity = new_capacity;
    }
}

/// Push a subtree onto the end of the array.
unsafe fn array_push_subtree(arr: *mut SubtreeArray, element: Subtree) {
    array_grow(arr, 1);
    *(*arr).contents.add((*arr).size as usize) = element;
    (*arr).size += 1;
}

// ===========================================================================
// SubtreeArray functions
// ===========================================================================

pub unsafe fn ts_subtree_array_copy(self_: SubtreeArray, dest: *mut SubtreeArray) {
    (*dest).size = self_.size;
    (*dest).capacity = self_.capacity;
    (*dest).contents = self_.contents;
    if self_.capacity > 0 {
        (*dest).contents =
            ts_calloc(self_.capacity as usize, std::mem::size_of::<Subtree>()) as *mut Subtree;
        ptr::copy_nonoverlapping(self_.contents, (*dest).contents, self_.size as usize);
        for i in 0..self_.size {
            ts_subtree_retain(*(*dest).contents.add(i as usize));
        }
    }
}

pub unsafe fn ts_subtree_array_clear(pool: *mut SubtreePool, self_: *mut SubtreeArray) {
    for i in 0..(*self_).size {
        ts_subtree_release(pool, *(*self_).contents.add(i as usize));
    }
    (*self_).size = 0;
}

pub unsafe fn ts_subtree_array_delete(pool: *mut SubtreePool, self_: *mut SubtreeArray) {
    ts_subtree_array_clear(pool, self_);
    if !(*self_).contents.is_null() {
        ts_free((*self_).contents as *mut c_void);
    }
    (*self_).contents = ptr::null_mut();
    (*self_).size = 0;
    (*self_).capacity = 0;
}

pub unsafe fn ts_subtree_array_remove_trailing_extras(
    self_: *mut SubtreeArray,
    destination: *mut SubtreeArray,
) {
    (*destination).size = 0;
    while (*self_).size > 0 {
        let last = *(*self_).contents.add((*self_).size as usize - 1);
        if ts_subtree_extra(last) {
            (*self_).size -= 1;
            array_push_subtree(destination, last);
        } else {
            break;
        }
    }
    ts_subtree_array_reverse(destination);
}

pub unsafe fn ts_subtree_array_reverse(self_: *mut SubtreeArray) {
    let limit = (*self_).size / 2;
    for i in 0..limit {
        let reverse_index = (*self_).size as usize - 1 - i as usize;
        let a = (*self_).contents.add(i as usize);
        let b = (*self_).contents.add(reverse_index);
        ptr::swap(a, b);
    }
}

// ===========================================================================
// MutableSubtreeArray helpers
// ===========================================================================

unsafe fn mutable_array_grow(arr: *mut MutableSubtreeArray, count: u32) {
    let new_size = (*arr).size + count;
    if new_size > (*arr).capacity {
        let mut new_capacity = (*arr).capacity * 2;
        if new_capacity < 8 {
            new_capacity = 8;
        }
        if new_capacity < new_size {
            new_capacity = new_size;
        }
        (*arr).contents = ts_realloc(
            (*arr).contents as *mut c_void,
            new_capacity as usize * std::mem::size_of::<MutableSubtree>(),
        ) as *mut MutableSubtree;
        (*arr).capacity = new_capacity;
    }
}

pub(crate) unsafe fn mutable_array_push(arr: *mut MutableSubtreeArray, element: MutableSubtree) {
    mutable_array_grow(arr, 1);
    *(*arr).contents.add((*arr).size as usize) = element;
    (*arr).size += 1;
}

unsafe fn mutable_array_pop(arr: *mut MutableSubtreeArray) -> MutableSubtree {
    (*arr).size -= 1;
    *(*arr).contents.add((*arr).size as usize)
}

unsafe fn mutable_array_delete(arr: *mut MutableSubtreeArray) {
    if !(*arr).contents.is_null() {
        ts_free((*arr).contents as *mut c_void);
    }
    (*arr).contents = ptr::null_mut();
    (*arr).size = 0;
    (*arr).capacity = 0;
}

fn mutable_array_new() -> MutableSubtreeArray {
    MutableSubtreeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    }
}

unsafe fn mutable_array_reserve(arr: *mut MutableSubtreeArray, new_capacity: u32) {
    if new_capacity > (*arr).capacity {
        (*arr).contents = ts_realloc(
            (*arr).contents as *mut c_void,
            new_capacity as usize * std::mem::size_of::<MutableSubtree>(),
        ) as *mut MutableSubtree;
        (*arr).capacity = new_capacity;
    }
}

// ===========================================================================
// SubtreePool functions
// ===========================================================================

pub unsafe fn ts_subtree_pool_new(capacity: u32) -> SubtreePool {
    let mut pool = SubtreePool {
        free_trees: mutable_array_new(),
        tree_stack: mutable_array_new(),
    };
    mutable_array_reserve(&mut pool.free_trees, capacity);
    pool
}

pub unsafe fn ts_subtree_pool_delete(self_: *mut SubtreePool) {
    if !(*self_).free_trees.contents.is_null() {
        for i in 0..(*self_).free_trees.size {
            let tree = *(*self_).free_trees.contents.add(i as usize);
            ts_free(tree.ptr as *mut c_void);
        }
        mutable_array_delete(&mut (*self_).free_trees);
    }
    if !(*self_).tree_stack.contents.is_null() {
        mutable_array_delete(&mut (*self_).tree_stack);
    }
}

unsafe fn ts_subtree_pool_allocate(self_: *mut SubtreePool) -> *mut SubtreeHeapData {
    if (*self_).free_trees.size > 0 {
        mutable_array_pop(&mut (*self_).free_trees).ptr
    } else {
        ts_malloc(std::mem::size_of::<SubtreeHeapData>()) as *mut SubtreeHeapData
    }
}

unsafe fn ts_subtree_pool_free(self_: *mut SubtreePool, tree: *mut SubtreeHeapData) {
    if (*self_).free_trees.capacity > 0
        && (*self_).free_trees.size + 1 <= TS_MAX_TREE_POOL_SIZE
    {
        mutable_array_push(
            &mut (*self_).free_trees,
            MutableSubtree { ptr: tree },
        );
    } else {
        ts_free(tree as *mut c_void);
    }
}

// ===========================================================================
// Subtree inline helpers (from subtree.h static inline functions)
// ===========================================================================

#[inline]
pub unsafe fn ts_subtree_symbol(self_: Subtree) -> TSSymbol {
    if self_.data.is_inline() {
        self_.data.symbol as TSSymbol
    } else {
        (*self_.ptr).symbol
    }
}

#[inline]
pub unsafe fn ts_subtree_visible(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.visible()
    } else {
        (*self_.ptr).visible()
    }
}

#[inline]
pub unsafe fn ts_subtree_named(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.named()
    } else {
        (*self_.ptr).named()
    }
}

#[inline]
pub unsafe fn ts_subtree_extra(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.extra()
    } else {
        (*self_.ptr).extra()
    }
}

#[inline]
pub unsafe fn ts_subtree_has_changes(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.has_changes()
    } else {
        (*self_.ptr).has_changes()
    }
}

#[inline]
pub unsafe fn ts_subtree_missing(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.is_missing()
    } else {
        (*self_.ptr).is_missing()
    }
}

#[inline]
pub unsafe fn ts_subtree_is_keyword(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.is_keyword()
    } else {
        (*self_.ptr).is_keyword()
    }
}

#[inline]
pub unsafe fn ts_subtree_parse_state(self_: Subtree) -> TSStateId {
    if self_.data.is_inline() {
        self_.data.parse_state
    } else {
        (*self_.ptr).parse_state
    }
}

#[inline]
pub unsafe fn ts_subtree_lookahead_bytes(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        self_.data.lookahead_bytes() as u32
    } else {
        (*self_.ptr).lookahead_bytes
    }
}

#[inline]
pub fn ts_subtree_alloc_size(child_count: u32) -> usize {
    child_count as usize * std::mem::size_of::<Subtree>()
        + std::mem::size_of::<SubtreeHeapData>()
}

#[inline]
pub unsafe fn ts_subtree_children(self_: Subtree) -> *mut Subtree {
    if self_.data.is_inline() {
        ptr::null_mut()
    } else {
        (self_.ptr as *mut Subtree).sub((*self_.ptr).child_count as usize)
    }
}

#[inline]
pub unsafe fn ts_subtree_set_extra(self_: *mut MutableSubtree, is_extra: bool) {
    if (*self_).data.is_inline() {
        (*self_).data.set_extra(is_extra);
    } else {
        (*(*self_).ptr).set_extra(is_extra);
    }
}

// --- #25: leaf_symbol, leaf_parse_state ---

#[inline]
pub unsafe fn ts_subtree_leaf_symbol(self_: Subtree) -> TSSymbol {
    if self_.data.is_inline() {
        return self_.data.symbol as TSSymbol;
    }
    if (*self_.ptr).child_count == 0 {
        return (*self_.ptr).symbol;
    }
    (*self_.ptr).data.children.first_leaf.symbol
}

#[inline]
pub unsafe fn ts_subtree_leaf_parse_state(self_: Subtree) -> TSStateId {
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
pub unsafe fn ts_subtree_padding(self_: Subtree) -> Length {
    if self_.data.is_inline() {
        Length {
            bytes: self_.data.padding_bytes as u32,
            extent: TSPoint {
                row: self_.data.padding_rows() as u32,
                column: self_.data.padding_columns as u32,
            },
        }
    } else {
        (*self_.ptr).padding
    }
}

#[inline]
pub unsafe fn ts_subtree_size(self_: Subtree) -> Length {
    if self_.data.is_inline() {
        Length {
            bytes: self_.data.size_bytes as u32,
            extent: TSPoint {
                row: 0,
                column: self_.data.size_bytes as u32,
            },
        }
    } else {
        (*self_.ptr).size
    }
}

#[inline]
pub unsafe fn ts_subtree_total_size(self_: Subtree) -> Length {
    length_add(ts_subtree_padding(self_), ts_subtree_size(self_))
}

#[inline]
pub unsafe fn ts_subtree_total_bytes(self_: Subtree) -> u32 {
    ts_subtree_total_size(self_).bytes
}

// --- #27: child_count, repeat_depth, is_repetition ---

#[inline]
pub unsafe fn ts_subtree_child_count(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        0
    } else {
        (*self_.ptr).child_count
    }
}

#[inline]
pub unsafe fn ts_subtree_repeat_depth(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        0
    } else {
        (*self_.ptr).data.children.repeat_depth as u32
    }
}

#[inline]
pub unsafe fn ts_subtree_is_repetition(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        0
    } else {
        (!(*self_.ptr).named() && !(*self_.ptr).visible() && (*self_.ptr).child_count != 0) as u32
    }
}

// --- #28: visible_descendant_count, visible_child_count ---

#[inline]
pub unsafe fn ts_subtree_visible_descendant_count(self_: Subtree) -> u32 {
    if self_.data.is_inline() || (*self_.ptr).child_count == 0 {
        0
    } else {
        (*self_.ptr).data.children.visible_descendant_count
    }
}

#[inline]
pub unsafe fn ts_subtree_visible_child_count(self_: Subtree) -> u32 {
    if ts_subtree_child_count(self_) > 0 {
        (*self_.ptr).data.children.visible_child_count
    } else {
        0
    }
}

// --- #29: error_cost ---

#[inline]
pub unsafe fn ts_subtree_error_cost(self_: Subtree) -> u32 {
    if ts_subtree_missing(self_) {
        ERROR_COST_PER_MISSING_TREE + ERROR_COST_PER_RECOVERY
    } else if self_.data.is_inline() {
        0
    } else {
        (*self_.ptr).error_cost
    }
}

// --- #30: dynamic_precedence, production_id ---

#[inline]
pub unsafe fn ts_subtree_dynamic_precedence(self_: Subtree) -> i32 {
    if self_.data.is_inline() || (*self_.ptr).child_count == 0 {
        0
    } else {
        (*self_.ptr).data.children.dynamic_precedence
    }
}

#[inline]
pub unsafe fn ts_subtree_production_id(self_: Subtree) -> u16 {
    if ts_subtree_child_count(self_) > 0 {
        (*self_.ptr).data.children.production_id
    } else {
        0
    }
}

// --- #31: fragile/external/depends_on_column accessors ---

#[inline]
pub unsafe fn ts_subtree_fragile_left(self_: Subtree) -> bool {
    if self_.data.is_inline() { false } else { (*self_.ptr).fragile_left() }
}

#[inline]
pub unsafe fn ts_subtree_fragile_right(self_: Subtree) -> bool {
    if self_.data.is_inline() { false } else { (*self_.ptr).fragile_right() }
}

#[inline]
pub unsafe fn ts_subtree_has_external_tokens(self_: Subtree) -> bool {
    if self_.data.is_inline() { false } else { (*self_.ptr).has_external_tokens() }
}

#[inline]
pub unsafe fn ts_subtree_has_external_scanner_state_change(self_: Subtree) -> bool {
    if self_.data.is_inline() { false } else { (*self_.ptr).has_external_scanner_state_change() }
}

#[inline]
pub unsafe fn ts_subtree_depends_on_column(self_: Subtree) -> bool {
    if self_.data.is_inline() { false } else { (*self_.ptr).depends_on_column() }
}

#[inline]
pub unsafe fn ts_subtree_is_fragile(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.ptr).fragile_left() || (*self_.ptr).fragile_right()
    }
}

#[inline]
pub unsafe fn ts_subtree_is_error(self_: Subtree) -> bool {
    ts_subtree_symbol(self_) == ts_builtin_sym_error
}

#[inline]
pub unsafe fn ts_subtree_is_eof(self_: Subtree) -> bool {
    ts_subtree_symbol(self_) == ts_builtin_sym_end
}

// --- #32: from_mut, to_mut_unsafe ---

#[inline]
pub fn ts_subtree_from_mut(self_: MutableSubtree) -> Subtree {
    Subtree { data: unsafe { self_.data } }
}

#[inline]
pub fn ts_subtree_to_mut_unsafe(self_: Subtree) -> MutableSubtree {
    MutableSubtree { data: unsafe { self_.data } }
}

// ===========================================================================
// Subtree private helpers
// ===========================================================================

// --- #33: can_inline, set_has_changes ---

#[inline]
fn ts_subtree_can_inline(padding: Length, size: Length, lookahead_bytes: u32) -> bool {
    padding.bytes < TS_MAX_INLINE_TREE_LENGTH as u32
        && padding.extent.row < 16
        && padding.extent.column < TS_MAX_INLINE_TREE_LENGTH as u32
        && size.bytes < TS_MAX_INLINE_TREE_LENGTH as u32
        && size.extent.row == 0
        && size.extent.column < TS_MAX_INLINE_TREE_LENGTH as u32
        && lookahead_bytes < 16
}

unsafe fn ts_subtree_set_has_changes(self_: *mut MutableSubtree) {
    if (*self_).data.is_inline() {
        (*self_).data.set_has_changes(true);
    } else {
        (*(*self_).ptr).set_has_changes(true);
    }
}

// ===========================================================================
// Subtree construction functions (from subtree.c)
// ===========================================================================

// --- #34: new_leaf ---

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
    let metadata = ts_language_symbol_metadata(language, symbol);
    let extra = symbol == ts_builtin_sym_end;

    let is_inline = symbol <= u8::MAX as TSSymbol
        && !has_external_tokens
        && ts_subtree_can_inline(padding, size, lookahead_bytes);

    if is_inline {
        Subtree {
            data: SubtreeInlineData {
                flags: INLINE_IS_INLINE
                    | if metadata.visible { INLINE_VISIBLE } else { 0 }
                    | if metadata.named { INLINE_NAMED } else { 0 }
                    | if extra { INLINE_EXTRA } else { 0 }
                    | if is_keyword { INLINE_IS_KEYWORD } else { 0 },
                symbol: symbol as u8,
                parse_state,
                padding_columns: padding.extent.column as u8,
                rows_and_lookahead: (padding.extent.row as u8 & 0x0F)
                    | ((lookahead_bytes as u8 & 0x0F) << 4),
                padding_bytes: padding.bytes as u8,
                size_bytes: size.bytes as u8,
            },
        }
    } else {
        let data = ts_subtree_pool_allocate(pool);
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
                metadata.visible, metadata.named, extra,
                false, false, false, has_external_tokens,
                false, depends_on_column, false, is_keyword,
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

pub unsafe fn ts_subtree_new_error(
    pool: *mut SubtreePool,
    lookahead_char: i32,
    padding: Length,
    size: Length,
    bytes_scanned: u32,
    parse_state: TSStateId,
    language: *const TSLanguage,
) -> Subtree {
    let result = ts_subtree_new_leaf(
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
    let data = result.ptr as *mut SubtreeHeapData;
    (*data).set_fragile_left(true);
    (*data).set_fragile_right(true);
    (*data).data.lookahead_char = lookahead_char;
    result
}

// --- #36: clone ---

pub unsafe fn ts_subtree_clone(self_: Subtree) -> MutableSubtree {
    let alloc_size = ts_subtree_alloc_size((*self_.ptr).child_count);
    let new_children = ts_malloc(alloc_size) as *mut Subtree;
    let old_children = ts_subtree_children(self_);
    ptr::copy_nonoverlapping(old_children as *const u8, new_children as *mut u8, alloc_size);
    let result = (new_children as *mut u8).add((*self_.ptr).child_count as usize * std::mem::size_of::<Subtree>()) as *mut SubtreeHeapData;
    if (*self_.ptr).child_count > 0 {
        for i in 0..(*self_.ptr).child_count {
            ts_subtree_retain(*new_children.add(i as usize));
        }
    } else if (*self_.ptr).has_external_tokens() {
        (*result).data.external_scanner_state = std::mem::ManuallyDrop::new(
            ts_external_scanner_state_copy(
                &*(*self_.ptr).data.external_scanner_state,
            )
        );
    }
    (*result).ref_count = 1;
    MutableSubtree { ptr: result }
}

// --- #37: new_node ---

pub unsafe fn ts_subtree_new_node(
    symbol: TSSymbol,
    children: *mut SubtreeArray,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    let metadata = ts_language_symbol_metadata(language, symbol);
    let fragile = symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat;

    // Allocate the node's data at the end of the array of children.
    let new_byte_size = ts_subtree_alloc_size((*children).size);
    if ((*children).capacity as usize) * std::mem::size_of::<Subtree>() < new_byte_size {
        (*children).contents =
            ts_realloc((*children).contents as *mut c_void, new_byte_size) as *mut Subtree;
        (*children).capacity = (new_byte_size / std::mem::size_of::<Subtree>()) as u32;
    }
    let data = (*children).contents.add((*children).size as usize) as *mut SubtreeHeapData;

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
            metadata.visible, metadata.named, false,
            fragile, fragile, false, false,
            false, false, false, false,
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
    ts_subtree_summarize_children(result, language);
    result
}

// --- #38: new_error_node ---

pub unsafe fn ts_subtree_new_error_node(
    children: *mut SubtreeArray,
    extra: bool,
    language: *const TSLanguage,
) -> Subtree {
    let result = ts_subtree_new_node(ts_builtin_sym_error, children, 0, language);
    (*result.ptr).set_extra(extra);
    ts_subtree_from_mut(result)
}

// --- #39: new_missing_leaf ---

pub unsafe fn ts_subtree_new_missing_leaf(
    pool: *mut SubtreePool,
    symbol: TSSymbol,
    padding: Length,
    lookahead_bytes: u32,
    language: *const TSLanguage,
) -> Subtree {
    let mut result = ts_subtree_new_leaf(
        pool, symbol, padding, length_zero(), lookahead_bytes, 0, false, false, false, language,
    );
    if result.data.is_inline() {
        result.data.set_is_missing(true);
    } else {
        (*(result.ptr as *mut SubtreeHeapData)).set_is_missing(true);
    }
    result
}

// ===========================================================================
// Subtree mutation / ownership functions
// ===========================================================================

// --- #40: set_symbol ---

pub unsafe fn ts_subtree_set_symbol(
    self_: *mut MutableSubtree,
    symbol: TSSymbol,
    language: *const TSLanguage,
) {
    let metadata = ts_language_symbol_metadata(language, symbol);
    if (*self_).data.is_inline() {
        debug_assert!(symbol < u8::MAX as TSSymbol);
        (*self_).data.symbol = symbol as u8;
        (*self_).data.set_named(metadata.named);
        (*self_).data.set_visible(metadata.visible);
    } else {
        (*(*self_).ptr).symbol = symbol;
        (*(*self_).ptr).set_named(metadata.named);
        (*(*self_).ptr).set_visible(metadata.visible);
    }
}

// --- #41: make_mut ---

pub unsafe fn ts_subtree_make_mut(pool: *mut SubtreePool, self_: Subtree) -> MutableSubtree {
    if self_.data.is_inline() {
        return MutableSubtree { data: self_.data };
    }
    if (*self_.ptr).ref_count == 1 {
        return ts_subtree_to_mut_unsafe(self_);
    }
    let result = ts_subtree_clone(self_);
    ts_subtree_release(pool, self_);
    result
}

// --- #42: retain ---

pub unsafe fn ts_subtree_retain(self_: Subtree) {
    if self_.data.is_inline() {
        return;
    }
    debug_assert!((*self_.ptr).ref_count > 0);
    let ref_count = &(*self_.ptr).ref_count as *const u32 as *const std::sync::atomic::AtomicU32;
    let prev = (*ref_count).fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    debug_assert!(prev.wrapping_add(1) != 0);
}

// --- #43: release ---

pub unsafe fn ts_subtree_release(pool: *mut SubtreePool, self_: Subtree) {
    if self_.data.is_inline() {
        return;
    }
    (*pool).tree_stack.size = 0;

    debug_assert!((*self_.ptr).ref_count > 0);
    let ref_count = &(*self_.ptr).ref_count as *const u32 as *const std::sync::atomic::AtomicU32;
    if (*ref_count).fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
        mutable_array_push(&mut (*pool).tree_stack, ts_subtree_to_mut_unsafe(self_));
    }

    while (*pool).tree_stack.size > 0 {
        let tree = mutable_array_pop(&mut (*pool).tree_stack);
        if (*tree.ptr).child_count > 0 {
            let children = ts_subtree_children(ts_subtree_from_mut(tree));
            for i in 0..(*tree.ptr).child_count {
                let child = *children.add(i as usize);
                if child.data.is_inline() {
                    continue;
                }
                debug_assert!((*child.ptr).ref_count > 0);
                let child_ref = &(*child.ptr).ref_count as *const u32
                    as *const std::sync::atomic::AtomicU32;
                if (*child_ref).fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                    mutable_array_push(
                        &mut (*pool).tree_stack,
                        ts_subtree_to_mut_unsafe(child),
                    );
                }
            }
            ts_free(children as *mut c_void);
        } else {
            if (*tree.ptr).has_external_tokens() {
                ts_external_scanner_state_delete(
                    &mut (*tree.ptr).data.external_scanner_state as *mut std::mem::ManuallyDrop<ExternalScannerState>
                        as *mut ExternalScannerState,
                );
            }
            ts_subtree_pool_free(pool, tree.ptr);
        }
    }
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
    let initial_stack_size = (*stack).size;

    let mut tree = self_;
    let symbol = (*tree.ptr).symbol;
    for _ in 0..count {
        if (*tree.ptr).ref_count > 1 || (*tree.ptr).child_count < 2 {
            break;
        }

        let child = ts_subtree_to_mut_unsafe(
            *ts_subtree_children(ts_subtree_from_mut(tree)).add(0),
        );
        if child.data.is_inline()
            || (*child.ptr).child_count < 2
            || (*child.ptr).ref_count > 1
            || (*child.ptr).symbol != symbol
        {
            break;
        }

        let grandchild = ts_subtree_to_mut_unsafe(
            *ts_subtree_children(ts_subtree_from_mut(child)).add(0),
        );
        if grandchild.data.is_inline()
            || (*grandchild.ptr).child_count < 2
            || (*grandchild.ptr).ref_count > 1
            || (*grandchild.ptr).symbol != symbol
        {
            break;
        }

        // Rotate: tree[0] = grandchild, child[0] = grandchild[last], grandchild[last] = child
        let gc_last = (*grandchild.ptr).child_count as usize - 1;
        *ts_subtree_children(ts_subtree_from_mut(tree)).add(0) =
            ts_subtree_from_mut(grandchild);
        *ts_subtree_children(ts_subtree_from_mut(child)).add(0) =
            *ts_subtree_children(ts_subtree_from_mut(grandchild)).add(gc_last);
        *ts_subtree_children(ts_subtree_from_mut(grandchild)).add(gc_last) =
            ts_subtree_from_mut(child);
        mutable_array_push(stack, tree);
        tree = grandchild;
    }

    while (*stack).size > initial_stack_size {
        tree = mutable_array_pop(stack);
        let child = ts_subtree_to_mut_unsafe(
            *ts_subtree_children(ts_subtree_from_mut(tree)).add(0),
        );
        let grandchild = ts_subtree_to_mut_unsafe(
            *ts_subtree_children(ts_subtree_from_mut(child))
                .add((*child.ptr).child_count as usize - 1),
        );
        ts_subtree_summarize_children(grandchild, language);
        ts_subtree_summarize_children(child, language);
        ts_subtree_summarize_children(tree, language);
    }
}

pub unsafe fn ts_subtree_summarize_children(
    self_: MutableSubtree,
    language: *const TSLanguage,
) {
    debug_assert!(!self_.data.is_inline());

    (*self_.ptr).data.children.named_child_count = 0;
    (*self_.ptr).data.children.visible_child_count = 0;
    (*self_.ptr).error_cost = 0;
    (*self_.ptr).data.children.repeat_depth = 0;
    (*self_.ptr).data.children.visible_descendant_count = 0;
    (*self_.ptr).set_has_external_tokens(false);
    (*self_.ptr).set_depends_on_column(false);
    (*self_.ptr).set_has_external_scanner_state_change(false);
    (*self_.ptr).data.children.dynamic_precedence = 0;

    let mut structural_index: u32 = 0;
    let alias_sequence =
        language_alias_sequence(language, (*self_.ptr).data.children.production_id as u32);
    let mut lookahead_end_byte: u32 = 0;

    let children = ts_subtree_children(ts_subtree_from_mut(self_));
    for i in 0..(*self_.ptr).child_count {
        let child = *children.add(i as usize);

        if (*self_.ptr).size.extent.row == 0 && ts_subtree_depends_on_column(child) {
            (*self_.ptr).set_depends_on_column(true);
        }

        if ts_subtree_has_external_scanner_state_change(child) {
            (*self_.ptr).set_has_external_scanner_state_change(true);
        }

        if i == 0 {
            (*self_.ptr).padding = ts_subtree_padding(child);
            (*self_.ptr).size = ts_subtree_size(child);
        } else {
            (*self_.ptr).size = length_add((*self_.ptr).size, ts_subtree_total_size(child));
        }

        let child_lookahead_end_byte = (*self_.ptr).padding.bytes
            + (*self_.ptr).size.bytes
            + ts_subtree_lookahead_bytes(child);
        if child_lookahead_end_byte > lookahead_end_byte {
            lookahead_end_byte = child_lookahead_end_byte;
        }

        if ts_subtree_symbol(child) != ts_builtin_sym_error_repeat {
            (*self_.ptr).error_cost += ts_subtree_error_cost(child);
        }

        let grandchild_count = ts_subtree_child_count(child);
        if (*self_.ptr).symbol == ts_builtin_sym_error
            || (*self_.ptr).symbol == ts_builtin_sym_error_repeat
        {
            if !ts_subtree_extra(child)
                && !(ts_subtree_is_error(child) && grandchild_count == 0)
            {
                if ts_subtree_visible(child) {
                    (*self_.ptr).error_cost += ERROR_COST_PER_SKIPPED_TREE;
                } else if grandchild_count > 0 {
                    (*self_.ptr).error_cost += ERROR_COST_PER_SKIPPED_TREE
                        * (*child.ptr).data.children.visible_child_count;
                }
            }
        }

        (*self_.ptr).data.children.dynamic_precedence +=
            ts_subtree_dynamic_precedence(child);
        (*self_.ptr).data.children.visible_descendant_count +=
            ts_subtree_visible_descendant_count(child);

        if !ts_subtree_extra(child)
            && ts_subtree_symbol(child) != 0
            && !alias_sequence.is_null()
            && *alias_sequence.add(structural_index as usize) != 0
        {
            (*self_.ptr).data.children.visible_descendant_count += 1;
            (*self_.ptr).data.children.visible_child_count += 1;
            if ts_language_symbol_metadata(
                language,
                *alias_sequence.add(structural_index as usize),
            )
            .named
            {
                (*self_.ptr).data.children.named_child_count += 1;
            }
        } else if ts_subtree_visible(child) {
            (*self_.ptr).data.children.visible_descendant_count += 1;
            (*self_.ptr).data.children.visible_child_count += 1;
            if ts_subtree_named(child) {
                (*self_.ptr).data.children.named_child_count += 1;
            }
        } else if grandchild_count > 0 {
            (*self_.ptr).data.children.visible_child_count +=
                (*child.ptr).data.children.visible_child_count;
            (*self_.ptr).data.children.named_child_count +=
                (*child.ptr).data.children.named_child_count;
        }

        if ts_subtree_has_external_tokens(child) {
            (*self_.ptr).set_has_external_tokens(true);
        }

        if ts_subtree_is_error(child) {
            (*self_.ptr).set_fragile_left(true);
            (*self_.ptr).set_fragile_right(true);
            (*self_.ptr).parse_state = TS_TREE_STATE_NONE;
        }

        if !ts_subtree_extra(child) {
            structural_index += 1;
        }
    }

    (*self_.ptr).lookahead_bytes =
        lookahead_end_byte - (*self_.ptr).size.bytes - (*self_.ptr).padding.bytes;

    if (*self_.ptr).symbol == ts_builtin_sym_error
        || (*self_.ptr).symbol == ts_builtin_sym_error_repeat
    {
        (*self_.ptr).error_cost += ERROR_COST_PER_RECOVERY
            + ERROR_COST_PER_SKIPPED_CHAR * (*self_.ptr).size.bytes
            + ERROR_COST_PER_SKIPPED_LINE * (*self_.ptr).size.extent.row;
    }

    if (*self_.ptr).child_count > 0 {
        let first_child = *children.add(0);
        let last_child = *children.add((*self_.ptr).child_count as usize - 1);

        (*self_.ptr).data.children.first_leaf.symbol = ts_subtree_leaf_symbol(first_child);
        (*self_.ptr).data.children.first_leaf.parse_state =
            ts_subtree_leaf_parse_state(first_child);

        if ts_subtree_fragile_left(first_child) {
            (*self_.ptr).set_fragile_left(true);
        }
        if ts_subtree_fragile_right(last_child) {
            (*self_.ptr).set_fragile_right(true);
        }

        if (*self_.ptr).child_count >= 2
            && !(*self_.ptr).visible()
            && !(*self_.ptr).named()
            && ts_subtree_symbol(first_child) == (*self_.ptr).symbol
        {
            if ts_subtree_repeat_depth(first_child) > ts_subtree_repeat_depth(last_child) {
                (*self_.ptr).data.children.repeat_depth =
                    (ts_subtree_repeat_depth(first_child) + 1) as u16;
            } else {
                (*self_.ptr).data.children.repeat_depth =
                    (ts_subtree_repeat_depth(last_child) + 1) as u16;
            }
        }
    }
}

// ===========================================================================
// Subtree comparison / query
// ===========================================================================

pub unsafe fn ts_subtree_compare(
    left: Subtree,
    right: Subtree,
    pool: *mut SubtreePool,
) -> i32 {
    mutable_array_push(&mut (*pool).tree_stack, ts_subtree_to_mut_unsafe(left));
    mutable_array_push(&mut (*pool).tree_stack, ts_subtree_to_mut_unsafe(right));

    while (*pool).tree_stack.size > 0 {
        let right = ts_subtree_from_mut(mutable_array_pop(&mut (*pool).tree_stack));
        let left = ts_subtree_from_mut(mutable_array_pop(&mut (*pool).tree_stack));

        let mut result = 0i32;
        if ts_subtree_symbol(left) < ts_subtree_symbol(right) {
            result = -1;
        } else if ts_subtree_symbol(right) < ts_subtree_symbol(left) {
            result = 1;
        } else if ts_subtree_child_count(left) < ts_subtree_child_count(right) {
            result = -1;
        } else if ts_subtree_child_count(right) < ts_subtree_child_count(left) {
            result = 1;
        }
        if result != 0 {
            (*pool).tree_stack.size = 0;
            return result;
        }

        let count = ts_subtree_child_count(left);
        let mut i = count;
        while i > 0 {
            i -= 1;
            let left_child = *ts_subtree_children(left).add(i as usize);
            let right_child = *ts_subtree_children(right).add(i as usize);
            mutable_array_push(
                &mut (*pool).tree_stack,
                ts_subtree_to_mut_unsafe(left_child),
            );
            mutable_array_push(
                &mut (*pool).tree_stack,
                ts_subtree_to_mut_unsafe(right_child),
            );
        }
    }

    0
}

pub unsafe fn ts_subtree_edit(
    mut self_: Subtree,
    input_edit: *const TSInputEdit,
    pool: *mut SubtreePool,
) -> Subtree {
    struct EditEntry {
        tree: *mut Subtree,
        edit: Edit,
    }

    let mut stack: Vec<EditEntry> = Vec::new();
    stack.push(EditEntry {
        tree: &mut self_ as *mut Subtree,
        edit: Edit {
            start: Length {
                bytes: (*input_edit).start_byte,
                extent: (*input_edit).start_point,
            },
            old_end: Length {
                bytes: (*input_edit).old_end_byte,
                extent: (*input_edit).old_end_point,
            },
            new_end: Length {
                bytes: (*input_edit).new_end_byte,
                extent: (*input_edit).new_end_point,
            },
        },
    });

    while let Some(entry) = stack.pop() {
        let mut edit = entry.edit;
        let is_noop =
            edit.old_end.bytes == edit.start.bytes && edit.new_end.bytes == edit.start.bytes;
        let is_pure_insertion = edit.old_end.bytes == edit.start.bytes;
        let parent_depends_on_column = ts_subtree_depends_on_column(*entry.tree);
        let column_shifted = edit.new_end.extent.column != edit.old_end.extent.column;

        let mut size = ts_subtree_size(*entry.tree);
        let mut padding = ts_subtree_padding(*entry.tree);
        let total_size = length_add(padding, size);
        let lookahead_bytes = ts_subtree_lookahead_bytes(*entry.tree);
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

        let mut result = ts_subtree_make_mut(pool, *entry.tree);

        if result.data.is_inline() {
            if ts_subtree_can_inline(padding, size, lookahead_bytes) {
                result.data.padding_bytes = padding.bytes as u8;
                result.data.set_padding_rows(padding.extent.row as u8);
                result.data.padding_columns = padding.extent.column as u8;
                result.data.size_bytes = size.bytes as u8;
            } else {
                // Promote inline node to heap
                let data = ts_subtree_pool_allocate(pool);
                *data = SubtreeHeapData {
                    ref_count: 1,
                    padding,
                    size,
                    lookahead_bytes,
                    error_cost: 0,
                    child_count: 0,
                    symbol: result.data.symbol as TSSymbol,
                    parse_state: result.data.parse_state,
                    flags: SubtreeHeapData::make_flags(
                        result.data.visible(), result.data.named(), result.data.extra(),
                        false, false, false, false,
                        false, false, result.data.is_missing(), result.data.is_keyword(),
                    ),
                    data: SubtreeHeapDataContent { lookahead_char: 0 },
                };
                result.ptr = data;
            }
        } else {
            (*result.ptr).padding = padding;
            (*result.ptr).size = size;
        }

        ts_subtree_set_has_changes(&mut result);
        *entry.tree = ts_subtree_from_mut(result);

        let mut child_right = length_zero();
        let n = ts_subtree_child_count(*entry.tree);
        for i in 0..n {
            let child = ts_subtree_children(*entry.tree).add(i as usize);
            let child_size = ts_subtree_total_size(*child);
            let child_left = child_right;
            child_right = length_add(child_left, child_size);

            // If this child ends before the edit, it is not affected.
            if child_right.bytes + ts_subtree_lookahead_bytes(*child) < edit.start.bytes {
                continue;
            }

            // Keep editing child nodes until a node is reached that starts after the edit.
            if ((child_left.bytes > edit.old_end.bytes)
                || (child_left.bytes == edit.old_end.bytes && child_size.bytes > 0 && i > 0))
                && (!parent_depends_on_column || child_left.extent.row > padding.extent.row)
                && (!ts_subtree_depends_on_column(*child)
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
                tree: child,
                edit: child_edit,
            });
        }
    }

    self_
}

pub unsafe fn ts_subtree_last_external_token(mut tree: Subtree) -> Subtree {
    if !ts_subtree_has_external_tokens(tree) {
        return NULL_SUBTREE;
    }
    while (*tree.ptr).child_count > 0 {
        let children = ts_subtree_children(tree);
        let mut i = (*tree.ptr).child_count as usize;
        while i > 0 {
            i -= 1;
            let child = *children.add(i);
            if ts_subtree_has_external_tokens(child) {
                tree = child;
                break;
            }
        }
    }
    tree
}

pub unsafe fn ts_subtree_external_scanner_state(
    self_: Subtree,
) -> *const ExternalScannerState {
    if !self_.ptr.is_null()
        && !self_.data.is_inline()
        && (*self_.ptr).has_external_tokens()
        && (*self_.ptr).child_count == 0
    {
        &*(*self_.ptr).data.external_scanner_state as *const ExternalScannerState
    } else {
        &EMPTY_EXTERNAL_SCANNER_STATE
    }
}

pub unsafe fn ts_subtree_external_scanner_state_eq(self_: Subtree, other: Subtree) -> bool {
    let state_self = ts_subtree_external_scanner_state(self_);
    let state_other = ts_subtree_external_scanner_state(other);
    ts_external_scanner_state_eq(
        state_self,
        ts_external_scanner_state_data(state_other),
        (*state_other).length,
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

/// Rust re-implementation of the static inline ts_language_field_map from language.h.
unsafe fn language_field_map(
    language: *const TSLanguage,
    production_id: u32,
    start: *mut *const TSFieldMapEntry,
    end: *mut *const TSFieldMapEntry,
) {
    let lang = language as *const TSLanguageData;
    if (*lang).field_count == 0 {
        *start = ptr::null();
        *end = ptr::null();
        return;
    }
    let slice = *(*lang).field_map_slices.add(production_id as usize);
    *start = (*lang).field_map_entries.add(slice.index as usize);
    *end = (*lang).field_map_entries.add(slice.index as usize + slice.length as usize);
}

/// Rust re-implementation of the static inline ts_language_write_symbol_as_dot_string.
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
                fputc(b'\\' as i32, f);
                fputc(*chr as i32, f);
            }
            b'\n' => {
                fputs(b"\\n\0".as_ptr() as *const i8, f);
            }
            b'\t' => {
                fputs(b"\\t\0".as_ptr() as *const i8, f);
            }
            _ => {
                fputc(*chr as i32, f);
            }
        }
        chr = chr.add(1);
    }
}

unsafe fn ts_subtree__write_char_to_string(s: *mut i8, n: usize, chr: i32) -> usize {
    if chr == -1 {
        snprintf(s, n, b"INVALID\0".as_ptr() as *const i8) as usize
    } else if chr == 0 {
        snprintf(s, n, b"'\\0'\0".as_ptr() as *const i8) as usize
    } else if chr == b'\n' as i32 {
        snprintf(s, n, b"'\\n'\0".as_ptr() as *const i8) as usize
    } else if chr == b'\t' as i32 {
        snprintf(s, n, b"'\\t'\0".as_ptr() as *const i8) as usize
    } else if chr == b'\r' as i32 {
        snprintf(s, n, b"'\\r'\0".as_ptr() as *const i8) as usize
    } else if chr >= 0x20 && chr < 0x7F {
        snprintf(s, n, b"'%c'\0".as_ptr() as *const i8, chr) as usize
    } else {
        snprintf(s, n, b"%d\0".as_ptr() as *const i8, chr) as usize
    }
}

unsafe fn ts_subtree__write_to_string(
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
        return snprintf(string, limit, b"(NULL)\0".as_ptr() as *const i8) as usize;
    }

    let mut cursor = string;
    let mut string_measuring = string;
    let writer: *mut *mut i8 = if limit > 1 {
        &mut cursor
    } else {
        &mut string_measuring
    };
    let is_root = field_name == ROOT_FIELD.as_ptr() as *const i8;
    let is_visible = include_all
        || ts_subtree_missing(self_)
        || (if alias_symbol != 0 {
            alias_is_named
        } else {
            ts_subtree_visible(self_) && ts_subtree_named(self_)
        });

    if is_visible {
        if !is_root {
            cursor = cursor.add(
                snprintf(*writer, limit, b" \0".as_ptr() as *const i8) as usize,
            );
            if !field_name.is_null() {
                cursor = cursor.add(
                    snprintf(*writer, limit, b"%s: \0".as_ptr() as *const i8, field_name)
                        as usize,
                );
            }
        }

        if ts_subtree_is_error(self_)
            && ts_subtree_child_count(self_) == 0
            && (*self_.ptr).size.bytes > 0
        {
            cursor = cursor.add(
                snprintf(*writer, limit, b"(UNEXPECTED \0".as_ptr() as *const i8) as usize,
            );
            cursor = cursor.add(ts_subtree__write_char_to_string(
                *writer,
                limit,
                (*self_.ptr).data.lookahead_char,
            ));
        } else {
            let symbol = if alias_symbol != 0 {
                alias_symbol
            } else {
                ts_subtree_symbol(self_)
            };
            let symbol_name = ts_language_symbol_name(language, symbol);
            if ts_subtree_missing(self_) {
                cursor = cursor.add(
                    snprintf(*writer, limit, b"(MISSING \0".as_ptr() as *const i8) as usize,
                );
                if alias_is_named || ts_subtree_named(self_) {
                    cursor = cursor.add(
                        snprintf(*writer, limit, b"%s\0".as_ptr() as *const i8, symbol_name)
                            as usize,
                    );
                } else {
                    cursor = cursor.add(
                        snprintf(
                            *writer,
                            limit,
                            b"\"%s\"\0".as_ptr() as *const i8,
                            symbol_name,
                        ) as usize,
                    );
                }
            } else {
                cursor = cursor.add(
                    snprintf(*writer, limit, b"(%s\0".as_ptr() as *const i8, symbol_name)
                        as usize,
                );
            }
        }
    } else if is_root {
        let symbol = if alias_symbol != 0 {
            alias_symbol
        } else {
            ts_subtree_symbol(self_)
        };
        let symbol_name = ts_language_symbol_name(language, symbol);
        if ts_subtree_child_count(self_) > 0 {
            cursor = cursor.add(
                snprintf(*writer, limit, b"(%s\0".as_ptr() as *const i8, symbol_name) as usize,
            );
        } else if ts_subtree_named(self_) {
            cursor = cursor.add(
                snprintf(*writer, limit, b"(%s)\0".as_ptr() as *const i8, symbol_name)
                    as usize,
            );
        } else {
            cursor = cursor.add(
                snprintf(
                    *writer,
                    limit,
                    b"(\"%s\")\0".as_ptr() as *const i8,
                    symbol_name,
                ) as usize,
            );
        }
    }

    if ts_subtree_child_count(self_) > 0 {
        let alias_sequence = language_alias_sequence(
            language,
            (*self_.ptr).data.children.production_id as u32,
        );
        let mut field_map: *const TSFieldMapEntry = ptr::null();
        let mut field_map_end: *const TSFieldMapEntry = ptr::null();
        language_field_map(
            language,
            (*self_.ptr).data.children.production_id as u32,
            &mut field_map,
            &mut field_map_end,
        );

        let mut structural_child_index: u32 = 0;
        for i in 0..(*self_.ptr).child_count {
            let child = *ts_subtree_children(self_).add(i as usize);
            if ts_subtree_extra(child) {
                cursor = cursor.add(ts_subtree__write_to_string(
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
                    if !(*map).inherited
                        && (*map).child_index == structural_child_index as u8
                    {
                        let lang = language as *const TSLanguageData;
                        child_field_name =
                            *(*lang).field_names.add((*map).field_id as usize);
                        break;
                    }
                    map = map.add(1);
                }

                cursor = cursor.add(ts_subtree__write_to_string(
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
        cursor = cursor.add(
            snprintf(*writer, limit, b")\0".as_ptr() as *const i8) as usize,
        );
    }

    cursor as usize - string as usize
}

pub unsafe fn ts_subtree_string(
    self_: Subtree,
    alias_symbol: TSSymbol,
    alias_is_named: bool,
    language: *const TSLanguage,
    include_all: bool,
) -> *mut i8 {
    let mut scratch_string: [i8; 1] = [0];
    let size = ts_subtree__write_to_string(
        self_,
        scratch_string.as_mut_ptr(),
        1,
        language,
        include_all,
        alias_symbol,
        alias_is_named,
        ROOT_FIELD.as_ptr() as *const i8,
    ) + 1;
    let result = ts_malloc(size) as *mut i8;
    ts_subtree__write_to_string(
        self_,
        result,
        size,
        language,
        include_all,
        alias_symbol,
        alias_is_named,
        ROOT_FIELD.as_ptr() as *const i8,
    );
    result
}

unsafe fn ts_subtree__print_dot_graph(
    self_: *const Subtree,
    start_offset: u32,
    language: *const TSLanguage,
    alias_symbol: TSSymbol,
    f: *mut c_void,
) {
    let subtree_symbol = ts_subtree_symbol(*self_);
    let symbol = if alias_symbol != 0 { alias_symbol } else { subtree_symbol };
    let end_offset = start_offset + ts_subtree_total_bytes(*self_);
    fprintf(
        f,
        b"tree_%p [label=\"\0".as_ptr() as *const i8,
        self_ as *const c_void,
    );
    language_write_symbol_as_dot_string(language, f, symbol);
    fprintf(f, b"\"\0".as_ptr() as *const i8);

    if ts_subtree_child_count(*self_) == 0 {
        fprintf(f, b", shape=plaintext\0".as_ptr() as *const i8);
    }
    if ts_subtree_extra(*self_) {
        fprintf(f, b", fontcolor=gray\0".as_ptr() as *const i8);
    }
    if ts_subtree_has_changes(*self_) {
        fprintf(f, b", color=green, penwidth=2\0".as_ptr() as *const i8);
    }

    fprintf(
        f,
        b", tooltip=\"range: %u - %u\nstate: %d\nerror-cost: %u\nhas-changes: %u\ndepends-on-column: %u\ndescendant-count: %u\nrepeat-depth: %u\nlookahead-bytes: %u\0".as_ptr() as *const i8,
        start_offset,
        end_offset,
        ts_subtree_parse_state(*self_) as i32,
        ts_subtree_error_cost(*self_),
        ts_subtree_has_changes(*self_) as u32,
        ts_subtree_depends_on_column(*self_) as u32,
        ts_subtree_visible_descendant_count(*self_),
        ts_subtree_repeat_depth(*self_),
        ts_subtree_lookahead_bytes(*self_),
    );

    if ts_subtree_is_error(*self_)
        && ts_subtree_child_count(*self_) == 0
        && (*(*self_).ptr).data.lookahead_char != 0
    {
        fprintf(
            f,
            b"\ncharacter: '%c'\0".as_ptr() as *const i8,
            (*(*self_).ptr).data.lookahead_char,
        );
    }

    fprintf(f, b"\"]\n\0".as_ptr() as *const i8);

    let mut child_start_offset = start_offset;
    let lang = language as *const TSLanguageData;
    let mut child_info_offset =
        (*lang).max_alias_sequence_length as u32 * ts_subtree_production_id(*self_) as u32;
    let n = ts_subtree_child_count(*self_);
    for i in 0..n {
        let child = ts_subtree_children(*self_).add(i as usize) as *const Subtree;
        let mut subtree_alias_symbol: TSSymbol = 0;
        if !ts_subtree_extra(*child) && child_info_offset != 0 {
            subtree_alias_symbol = *(*lang).alias_sequences.add(child_info_offset as usize);
            child_info_offset += 1;
        }
        ts_subtree__print_dot_graph(
            child,
            child_start_offset,
            language,
            subtree_alias_symbol,
            f,
        );
        fprintf(
            f,
            b"tree_%p -> tree_%p [tooltip=%u]\n\0".as_ptr() as *const i8,
            self_ as *const c_void,
            child as *const c_void,
            i,
        );
        child_start_offset += ts_subtree_total_bytes(*child);
    }
}

pub unsafe fn ts_subtree_print_dot_graph(
    self_: Subtree,
    language: *const TSLanguage,
    f: *mut c_void,
) {
    fprintf(f, b"digraph tree {\n\0".as_ptr() as *const i8);
    fprintf(f, b"edge [arrowhead=none]\n\0".as_ptr() as *const i8);
    ts_subtree__print_dot_graph(&self_ as *const Subtree, 0, language, 0, f);
    fprintf(f, b"}\n\0".as_ptr() as *const i8);
}
