use core::ffi::c_void;
use core::{
    ptr,
    ptr::NonNull,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::ffi::{TSInputEdit, TSLanguage, TSPoint, TSStateId, TSSymbol};

use super::alloc::{calloc, free, malloc, realloc};
use super::error_costs::{
    ERROR_COST_PER_MISSING_TREE, ERROR_COST_PER_RECOVERY, ERROR_COST_PER_SKIPPED_CHAR,
    ERROR_COST_PER_SKIPPED_LINE, ERROR_COST_PER_SKIPPED_TREE,
};
use super::language::{
    language_alias_sequence_slice, language_field_map_slice, language_full,
    language_write_symbol_as_dot_string, ts_language_symbol_metadata, ts_language_symbol_name,
};
use super::length::{length_add, length_saturating_sub, length_sub, length_zero, Length};
use super::utils::{array_delete, array_new, array_pop, array_push, array_reserve, Array};
use super::utils::{ptr_mut, ptr_ref};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const TS_TREE_STATE_NONE: TSStateId = u16::MAX;
const TS_MAX_INLINE_TREE_LENGTH: u8 = u8::MAX;
const TS_MAX_TREE_POOL_SIZE: u32 = 32;

pub const TS_BUILTIN_SYM_ERROR: TSSymbol = u16::MAX;
pub const TS_BUILTIN_SYM_END: TSSymbol = 0;
pub const TS_BUILTIN_SYM_ERROR_REPEAT: TSSymbol = TS_BUILTIN_SYM_ERROR - 1;

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

pub struct ExternalScannerState {
    /// Owned serialized scanner bytes.
    data: ExternalScannerStateData,
    /// Serialized byte count.
    pub length: u32,
}

// SAFETY: Scanner state is immutable after it is stored in a subtree. The heap
// variant owns its allocation and exposes it only as read-only bytes.
unsafe impl Sync for ExternalScannerState {}

enum ExternalScannerStateData {
    Inline([u8; EXTERNAL_SCANNER_STATE_INLINE_SIZE]),
    Heap(NonNull<u8>),
}

// ---------------------------------------------------------------------------
// SubtreeInlineData — bitfield-packed inline node
// ---------------------------------------------------------------------------

/// Compact inline representation of a subtree (fits in a pointer-sized word).
///
/// The C runtime stores this as bitfields inside one arm of the `Subtree`
/// union. Rust has no C-compatible bitfields, so this mirrors the byte layout
/// explicitly and exposes accessors for the individual flags. The `is_inline`
/// bit overlaps with the LSB of a pointer, distinguishing inline nodes from
/// heap-allocated ones.
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

#[inline(always)]
fn set_u8_flag(flags: &mut u8, mask: u8, value: bool) {
    if value {
        *flags |= mask;
    } else {
        *flags &= !mask;
    }
}

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
    pub fn set_visible(&mut self, value: bool) {
        set_u8_flag(&mut self.flags, INLINE_VISIBLE, value);
    }
    #[inline(always)]
    pub fn set_named(&mut self, value: bool) {
        set_u8_flag(&mut self.flags, INLINE_NAMED, value);
    }
    #[inline(always)]
    pub fn set_extra(&mut self, value: bool) {
        set_u8_flag(&mut self.flags, INLINE_EXTRA, value);
    }
    #[inline(always)]
    pub fn set_has_changes(&mut self, value: bool) {
        set_u8_flag(&mut self.flags, INLINE_HAS_CHANGES, value);
    }
    #[inline(always)]
    pub fn set_is_missing(&mut self, value: bool) {
        set_u8_flag(&mut self.flags, INLINE_IS_MISSING, value);
    }
    #[inline(always)]
    pub fn set_padding_rows(&mut self, v: u8) {
        self.rows_and_lookahead = (self.rows_and_lookahead & 0xF0) | (v & 0x0F);
    }
}

// ---------------------------------------------------------------------------
// SubtreeHeapData — heap-allocated node
// ---------------------------------------------------------------------------

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

    /// Packed bitfield flags.
    ///
    /// Stored as one word so flag access remains compact and explicit.
    /// bit 0: `visible`, bit 1: `named`, bit 2: `extra`, bits 3-4: unused,
    /// bit 5: `has_changes`, bit 6: `has_external_tokens`,
    /// bit 7: `has_external_scanner_state_change`, bit 8: `depends_on_column`,
    /// bit 9: `is_missing`, bit 10: `is_keyword`
    pub flags: u16,

    /// Payload selected explicitly by node kind.
    pub data: SubtreeHeapDataContent,
}

// Bit positions in SubtreeHeapData.flags
const HEAP_VISIBLE: u16 = 1 << 0;
const HEAP_NAMED: u16 = 1 << 1;
const HEAP_EXTRA: u16 = 1 << 2;
const HEAP_HAS_CHANGES: u16 = 1 << 5;
const HEAP_HAS_EXTERNAL_TOKENS: u16 = 1 << 6;
const HEAP_HAS_EXTERNAL_SCANNER_STATE_CHANGE: u16 = 1 << 7;
const HEAP_DEPENDS_ON_COLUMN: u16 = 1 << 8;
const HEAP_IS_MISSING: u16 = 1 << 9;
const HEAP_IS_KEYWORD: u16 = 1 << 10;

#[inline(always)]
fn set_u16_flag(flags: &mut u16, mask: u16, value: bool) {
    if value {
        *flags |= mask;
    } else {
        *flags &= !mask;
    }
}

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
    pub fn set_visible(&mut self, value: bool) {
        set_u16_flag(&mut self.flags, HEAP_VISIBLE, value);
    }
    #[inline(always)]
    pub fn set_named(&mut self, value: bool) {
        set_u16_flag(&mut self.flags, HEAP_NAMED, value);
    }
    #[inline(always)]
    pub fn set_extra(&mut self, value: bool) {
        set_u16_flag(&mut self.flags, HEAP_EXTRA, value);
    }
    #[inline(always)]
    pub fn set_has_changes(&mut self, value: bool) {
        set_u16_flag(&mut self.flags, HEAP_HAS_CHANGES, value);
    }
    #[inline(always)]
    pub fn set_has_external_tokens(&mut self, value: bool) {
        set_u16_flag(&mut self.flags, HEAP_HAS_EXTERNAL_TOKENS, value);
    }
    #[inline(always)]
    pub fn set_has_external_scanner_state_change(&mut self, value: bool) {
        set_u16_flag(
            &mut self.flags,
            HEAP_HAS_EXTERNAL_SCANNER_STATE_CHANGE,
            value,
        );
    }
    #[inline(always)]
    pub fn set_depends_on_column(&mut self, value: bool) {
        set_u16_flag(&mut self.flags, HEAP_DEPENDS_ON_COLUMN, value);
    }
    #[inline(always)]
    pub fn set_is_missing(&mut self, value: bool) {
        set_u16_flag(&mut self.flags, HEAP_IS_MISSING, value);
    }
    /// Build flags from individual booleans (for struct initialization)
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn make_flags(
        visible: bool,
        named: bool,
        extra: bool,
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
            | u16::from(has_changes) << 5
            | u16::from(has_external_tokens) << 6
            | u16::from(has_external_scanner_state_change) << 7
            | u16::from(depends_on_column) << 8
            | u16::from(is_missing) << 9
            | u16::from(is_keyword) << 10
    }
}

pub enum SubtreeHeapDataContent {
    Children(SubtreeChildrenData),
    ExternalScannerState(ExternalScannerState),
    LookaheadChar(i32),
}

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
}

impl SubtreeHeapData {
    pub const fn children(&self) -> &SubtreeChildrenData {
        let SubtreeHeapDataContent::Children(children) = &self.data else {
            panic!("internal subtree must contain child metadata")
        };
        children
    }

    pub fn children_mut(&mut self) -> &mut SubtreeChildrenData {
        let SubtreeHeapDataContent::Children(children) = &mut self.data else {
            panic!("internal subtree must contain child metadata")
        };
        children
    }

    pub const fn external_scanner_state(&self) -> &ExternalScannerState {
        let SubtreeHeapDataContent::ExternalScannerState(state) = &self.data else {
            panic!("external-token leaf must contain scanner state")
        };
        state
    }

    pub fn external_scanner_state_mut(&mut self) -> &mut ExternalScannerState {
        let SubtreeHeapDataContent::ExternalScannerState(state) = &mut self.data else {
            panic!("external-token leaf must contain scanner state")
        };
        state
    }

    pub const fn lookahead_char(&self) -> i32 {
        let SubtreeHeapDataContent::LookaheadChar(character) = &self.data else {
            panic!("error leaf must contain its lookahead character")
        };
        *character
    }
}

// ---------------------------------------------------------------------------
// Subtree / MutableSubtree — compact tagged node handles
// ---------------------------------------------------------------------------

#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct Subtree {
    /// Inline bytes, or the byte representation of a heap pointer.
    pub data: SubtreeInlineData,
}

#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct MutableSubtree {
    /// Inline bytes, or the byte representation of a mutable heap pointer.
    pub data: SubtreeInlineData,
}

impl Subtree {
    pub const fn from_heap(ptr: *const SubtreeHeapData) -> Self {
        Self {
            data: unsafe { core::mem::transmute::<*const SubtreeHeapData, SubtreeInlineData>(ptr) },
        }
    }

    pub const fn heap_ptr(self) -> *const SubtreeHeapData {
        unsafe { core::mem::transmute::<SubtreeInlineData, *const SubtreeHeapData>(self.data) }
    }

    /// Whether this handle is the null subtree sentinel.
    pub fn is_null(self) -> bool {
        self.heap_ptr().is_null()
    }

    /// Borrow the heap node represented by this non-inline handle.
    pub const unsafe fn heap_data<'a>(self) -> &'a SubtreeHeapData {
        &*self.heap_ptr()
    }
}

impl MutableSubtree {
    pub const fn from_heap(ptr: *mut SubtreeHeapData) -> Self {
        Self {
            data: unsafe { core::mem::transmute::<*mut SubtreeHeapData, SubtreeInlineData>(ptr) },
        }
    }

    pub const fn heap_ptr(self) -> *mut SubtreeHeapData {
        unsafe { core::mem::transmute::<SubtreeInlineData, *mut SubtreeHeapData>(self.data) }
    }
}

pub const NULL_SUBTREE: Subtree = Subtree::from_heap(ptr::null());

// Compile-time layout assertions for the internal tagged pointer/inline-data
// representation. Both handles remain one word wide, with the tag bit in the
// first byte.
const _: () = assert!(core::mem::size_of::<SubtreeInlineData>() == 8);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, flags) == 0);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, symbol) == 1);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, parse_state) == 2);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, padding_columns) == 4);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, rows_and_lookahead) == 5);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, padding_bytes) == 6);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, size_bytes) == 7);
const _: () = assert!(core::mem::size_of::<Subtree>() == 8);
const _: () = assert!(core::mem::size_of::<MutableSubtree>() == 8);

pub type SubtreeArray = Array<Subtree>;
pub type MutableSubtreeArray = Array<MutableSubtree>;

// ---------------------------------------------------------------------------
// SubtreePool
// ---------------------------------------------------------------------------

pub struct SubtreePool {
    /// Free list of heap subtree allocations.
    free_trees: MutableSubtreeArray,
    /// Scratch stack used by iterative release/compress operations.
    pub(super) tree_stack: MutableSubtreeArray,
}

// ---------------------------------------------------------------------------
// Internal helper: Edit (local to subtree edit logic)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Edit {
    /// Edited range start in old coordinates.
    start: Length,
    /// Edited range end in old coordinates.
    old_end: Length,
    /// Edited range end in new coordinates.
    new_end: Length,
}

struct EditEntry {
    tree: NonNull<Subtree>,
    edit: Edit,
}

// ---------------------------------------------------------------------------
// Static data
// ---------------------------------------------------------------------------

static EMPTY_EXTERNAL_SCANNER_STATE: ExternalScannerState = ExternalScannerState {
    data: ExternalScannerStateData::Inline([0; EXTERNAL_SCANNER_STATE_INLINE_SIZE]),
    length: 0,
};

// External scanner state

pub unsafe fn external_scanner_state_new(data: *const u8, length: u32) -> ExternalScannerState {
    let state = if length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        let bytes = NonNull::new_unchecked(malloc(length as usize).cast::<u8>());
        ptr::copy_nonoverlapping(data, bytes.as_ptr(), length as usize);
        ExternalScannerStateData::Heap(bytes)
    } else {
        let mut bytes = [0; EXTERNAL_SCANNER_STATE_INLINE_SIZE];
        ptr::copy_nonoverlapping(data, bytes.as_mut_ptr(), length as usize);
        ExternalScannerStateData::Inline(bytes)
    };
    ExternalScannerState {
        data: state,
        length,
    }
}

pub unsafe fn external_scanner_state_copy(self_: &ExternalScannerState) -> ExternalScannerState {
    let data = match &self_.data {
        ExternalScannerStateData::Inline(bytes) => ExternalScannerStateData::Inline(*bytes),
        ExternalScannerStateData::Heap(bytes) => {
            let copy = NonNull::new_unchecked(malloc(self_.length as usize).cast::<u8>());
            ptr::copy_nonoverlapping(bytes.as_ptr(), copy.as_ptr(), self_.length as usize);
            ExternalScannerStateData::Heap(copy)
        }
    };
    ExternalScannerState {
        data,
        length: self_.length,
    }
}

pub unsafe fn external_scanner_state_delete(self_: &mut ExternalScannerState) {
    if let ExternalScannerStateData::Heap(bytes) = self_.data {
        free(bytes.as_ptr().cast::<c_void>());
    }
}

pub const fn external_scanner_state_data(self_: &ExternalScannerState) -> *const u8 {
    match &self_.data {
        ExternalScannerStateData::Inline(bytes) => bytes.as_ptr(),
        ExternalScannerStateData::Heap(bytes) => bytes.as_ptr(),
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
    core::slice::from_raw_parts(external_scanner_state_data(self_), length)
        == core::slice::from_raw_parts(buffer, length)
}

// Subtree arrays

pub unsafe fn subtree_array_copy(self_: &SubtreeArray, dest: &mut SubtreeArray) {
    dest.size = self_.size;
    dest.capacity = self_.capacity;
    dest.contents = self_.contents;
    if self_.capacity > 0 {
        dest.contents =
            calloc(self_.capacity as usize, core::mem::size_of::<Subtree>()).cast::<Subtree>();
        if self_.size > 0 {
            let source = core::slice::from_raw_parts(self_.contents, self_.size as usize);
            let destination = core::slice::from_raw_parts_mut(dest.contents, self_.size as usize);
            destination.copy_from_slice(source);
            for tree in destination {
                subtree_retain(*tree);
            }
        }
    }
}

pub unsafe fn subtree_array_clear(pool: &mut SubtreePool, self_: &mut SubtreeArray) {
    if self_.size > 0 {
        let trees = core::slice::from_raw_parts(self_.contents, self_.size as usize);
        for tree in trees {
            subtree_release(pool, *tree);
        }
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
            array_push(destination, last);
        } else {
            break;
        }
    }
    subtree_array_reverse(destination);
}

pub unsafe fn subtree_array_reverse(self_: &mut SubtreeArray) {
    if self_.size > 0 {
        let trees = core::slice::from_raw_parts_mut(self_.contents, self_.size as usize);
        trees.reverse();
    }
}

// Subtree pool

pub unsafe fn subtree_pool_new(capacity: u32) -> SubtreePool {
    let mut pool = SubtreePool {
        free_trees: array_new(),
        tree_stack: array_new(),
    };
    array_reserve(&mut pool.free_trees, capacity);
    pool
}

pub unsafe fn subtree_pool_delete(self_: &mut SubtreePool) {
    if !self_.free_trees.contents.is_null() {
        for i in 0..self_.free_trees.size {
            let tree = *self_.free_trees.contents.add(i as usize);
            free(tree.heap_ptr().cast::<c_void>());
        }
        array_delete(&mut self_.free_trees);
    }
    if !self_.tree_stack.contents.is_null() {
        array_delete(&mut self_.tree_stack);
    }
}

unsafe fn subtree_pool_allocate(self_: &mut SubtreePool) -> *mut SubtreeHeapData {
    if self_.free_trees.size > 0 {
        array_pop(&mut self_.free_trees).heap_ptr()
    } else {
        malloc(core::mem::size_of::<SubtreeHeapData>()).cast::<SubtreeHeapData>()
    }
}

unsafe fn subtree_pool_free(self_: &mut SubtreePool, tree: MutableSubtree) {
    if self_.free_trees.capacity > 0 && self_.free_trees.size < TS_MAX_TREE_POOL_SIZE {
        array_push(&mut self_.free_trees, tree);
    } else {
        free(tree.heap_ptr().cast::<c_void>());
    }
}

// Subtree accessors

#[inline]
pub unsafe fn subtree_symbol(self_: Subtree) -> TSSymbol {
    if self_.data.is_inline() {
        TSSymbol::from(self_.data.symbol)
    } else {
        (*self_.heap_ptr()).symbol
    }
}

#[inline]
pub const unsafe fn subtree_visible(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.visible()
    } else {
        (*self_.heap_ptr()).visible()
    }
}

#[inline]
pub const unsafe fn subtree_named(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.named()
    } else {
        (*self_.heap_ptr()).named()
    }
}

#[inline]
pub const unsafe fn subtree_extra(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.extra()
    } else {
        (*self_.heap_ptr()).extra()
    }
}

#[inline]
pub const unsafe fn subtree_has_changes(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.has_changes()
    } else {
        (*self_.heap_ptr()).has_changes()
    }
}

#[inline]
pub const unsafe fn subtree_missing(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.is_missing()
    } else {
        (*self_.heap_ptr()).is_missing()
    }
}

#[inline]
pub const unsafe fn subtree_is_keyword(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        self_.data.is_keyword()
    } else {
        (*self_.heap_ptr()).is_keyword()
    }
}

#[inline]
pub const unsafe fn subtree_parse_state(self_: Subtree) -> TSStateId {
    if self_.data.is_inline() {
        self_.data.parse_state
    } else {
        (*self_.heap_ptr()).parse_state
    }
}

#[inline]
pub unsafe fn subtree_lookahead_bytes(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        u32::from(self_.data.lookahead_bytes())
    } else {
        (*self_.heap_ptr()).lookahead_bytes
    }
}

#[inline]
pub const fn subtree_alloc_size(child_count: u32) -> usize {
    child_count as usize * core::mem::size_of::<Subtree>() + core::mem::size_of::<SubtreeHeapData>()
}

#[inline]
pub const unsafe fn subtree_children(self_: Subtree) -> *mut Subtree {
    if self_.data.is_inline() {
        ptr::null_mut()
    } else {
        self_
            .heap_ptr()
            .cast_mut()
            .cast::<Subtree>()
            .sub((*self_.heap_ptr()).child_count as usize)
    }
}

#[inline]
pub unsafe fn subtree_child<'a>(self_: Subtree, index: u32) -> &'a Subtree {
    subtree_children_slice(self_).get_unchecked(index as usize)
}

pub const unsafe fn subtree_children_slice<'a>(self_: Subtree) -> &'a [Subtree] {
    let count = subtree_child_count(self_) as usize;
    if count == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(subtree_children(self_), count)
    }
}

#[inline]
unsafe fn mutable_subtree_children<'a>(self_: MutableSubtree) -> &'a mut [Subtree] {
    let count = (*self_.heap_ptr()).child_count as usize;
    if count == 0 {
        &mut []
    } else {
        core::slice::from_raw_parts_mut(subtree_children(subtree_from_mut(self_)), count)
    }
}

#[inline]
unsafe fn mutable_subtree_data_mut<'a>(self_: MutableSubtree) -> &'a mut SubtreeHeapData {
    ptr_mut(self_.heap_ptr())
}

#[inline]
unsafe fn subtree_data_ref<'a>(self_: Subtree) -> &'a SubtreeHeapData {
    ptr_ref(self_.heap_ptr())
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
        (*self_.heap_ptr()).set_extra(is_extra);
    }
}

// Source spans

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
        (*self_.heap_ptr()).padding
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
        (*self_.heap_ptr()).size
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

// Child and repetition metadata

#[inline]
pub const unsafe fn subtree_child_count(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        0
    } else {
        (*self_.heap_ptr()).child_count
    }
}

#[inline]
pub unsafe fn subtree_repeat_depth(self_: Subtree) -> u32 {
    if self_.data.is_inline() || (*self_.heap_ptr()).child_count == 0 {
        0
    } else {
        u32::from((*self_.heap_ptr()).children().repeat_depth)
    }
}

#[inline]
pub unsafe fn subtree_is_repetition(self_: Subtree) -> u32 {
    if self_.data.is_inline() {
        0
    } else {
        u32::from(
            !(*self_.heap_ptr()).named()
                && !(*self_.heap_ptr()).visible()
                && (*self_.heap_ptr()).child_count != 0,
        )
    }
}

// Visible-child metadata

#[inline]
pub const unsafe fn subtree_visible_descendant_count(self_: Subtree) -> u32 {
    if self_.data.is_inline() || (*self_.heap_ptr()).child_count == 0 {
        0
    } else {
        (*self_.heap_ptr()).children().visible_descendant_count
    }
}

#[inline]
pub const unsafe fn subtree_visible_child_count(self_: Subtree) -> u32 {
    if subtree_child_count(self_) > 0 {
        (*self_.heap_ptr()).children().visible_child_count
    } else {
        0
    }
}

// Error cost

#[inline]
pub const unsafe fn subtree_error_cost(self_: Subtree) -> u32 {
    if subtree_missing(self_) {
        ERROR_COST_PER_MISSING_TREE + ERROR_COST_PER_RECOVERY
    } else if self_.data.is_inline() {
        0
    } else {
        (*self_.heap_ptr()).error_cost
    }
}

// Parse metadata

#[inline]
pub const unsafe fn subtree_dynamic_precedence(self_: Subtree) -> i32 {
    if self_.data.is_inline() || (*self_.heap_ptr()).child_count == 0 {
        0
    } else {
        (*self_.heap_ptr()).children().dynamic_precedence
    }
}

#[inline]
pub const unsafe fn subtree_production_id(self_: Subtree) -> u16 {
    if subtree_child_count(self_) > 0 {
        (*self_.heap_ptr()).children().production_id
    } else {
        0
    }
}

#[inline]
pub const unsafe fn subtree_has_external_tokens(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.heap_ptr()).has_external_tokens()
    }
}

#[inline]
pub const unsafe fn subtree_has_external_scanner_state_change(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.heap_ptr()).has_external_scanner_state_change()
    }
}

#[inline]
pub const unsafe fn subtree_depends_on_column(self_: Subtree) -> bool {
    if self_.data.is_inline() {
        false
    } else {
        (*self_.heap_ptr()).depends_on_column()
    }
}

#[inline]
pub unsafe fn subtree_is_error(self_: Subtree) -> bool {
    subtree_symbol(self_) == TS_BUILTIN_SYM_ERROR
}

#[inline]
pub unsafe fn subtree_is_eof(self_: Subtree) -> bool {
    subtree_symbol(self_) == TS_BUILTIN_SYM_END
}

// Mutable/immutable representation conversion

#[inline]
pub const fn subtree_from_mut(self_: MutableSubtree) -> Subtree {
    Subtree { data: self_.data }
}

#[inline]
pub const fn subtree_to_mut_unsafe(self_: Subtree) -> MutableSubtree {
    MutableSubtree { data: self_.data }
}

// Subtree private helpers

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
        (*self_.heap_ptr()).set_has_changes(true);
    }
}

// Subtree construction

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
    let extra = symbol == TS_BUILTIN_SYM_END;

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
                has_external_tokens,
                false,
                depends_on_column,
                false,
                is_keyword,
            ),
            data: if has_external_tokens {
                SubtreeHeapDataContent::ExternalScannerState(ExternalScannerState {
                    data: ExternalScannerStateData::Inline([0; EXTERNAL_SCANNER_STATE_INLINE_SIZE]),
                    length: 0,
                })
            } else {
                SubtreeHeapDataContent::LookaheadChar(0)
            },
        };
        Subtree::from_heap(data)
    }
}

/// Create an error leaf for skipped input.
///
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
        TS_BUILTIN_SYM_ERROR,
        padding,
        size,
        bytes_scanned,
        parse_state,
        false,
        false,
        false,
        language,
    );
    (*result.heap_ptr().cast_mut()).data = SubtreeHeapDataContent::LookaheadChar(lookahead_char);
    result
}

pub unsafe fn subtree_clone(self_: Subtree) -> MutableSubtree {
    let data = subtree_data_ref(self_);
    let alloc_size = subtree_alloc_size(data.child_count);
    let new_children = malloc(alloc_size).cast::<Subtree>();
    let old_children = subtree_children(self_);
    ptr::copy_nonoverlapping(old_children, new_children, data.child_count as usize);
    let result = new_children
        .add(data.child_count as usize)
        .cast::<SubtreeHeapData>();
    if data.child_count > 0 {
        for i in 0..data.child_count {
            subtree_retain(*new_children.add(i as usize));
        }
    }
    let content = match &data.data {
        SubtreeHeapDataContent::Children(children) => SubtreeHeapDataContent::Children(*children),
        SubtreeHeapDataContent::ExternalScannerState(state) => {
            SubtreeHeapDataContent::ExternalScannerState(external_scanner_state_copy(state))
        }
        SubtreeHeapDataContent::LookaheadChar(character) => {
            SubtreeHeapDataContent::LookaheadChar(*character)
        }
    };
    ptr::write(
        result,
        SubtreeHeapData {
            ref_count: 1,
            padding: data.padding,
            size: data.size,
            lookahead_bytes: data.lookahead_bytes,
            error_cost: data.error_cost,
            child_count: data.child_count,
            symbol: data.symbol,
            parse_state: data.parse_state,
            flags: data.flags,
            data: content,
        },
    );
    MutableSubtree::from_heap(result)
}

unsafe fn subtree_init_node_data(
    data: *mut SubtreeHeapData,
    symbol: TSSymbol,
    child_count: u32,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    let metadata = ts_language_symbol_metadata(language, symbol);
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
            false,
            false,
            false,
            false,
            false,
            false,
        ),
        data: SubtreeHeapDataContent::Children(SubtreeChildrenData {
            visible_child_count: 0,
            named_child_count: 0,
            visible_descendant_count: 0,
            dynamic_precedence: 0,
            repeat_depth: 0,
            production_id: production_id as u16,
        }),
    };
    MutableSubtree::from_heap(data)
}

/// Create a heap internal node by moving child storage into the node allocation.
///
/// The child array is resized so the `SubtreeHeapData` header can live directly
/// after the child slice in one compact allocation:
/// `[child_0, child_1, ... child_n][SubtreeHeapData]`.
pub unsafe fn subtree_new_node(
    symbol: TSSymbol,
    children: *mut SubtreeArray,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    // Allocate the node's data at the end of the array of children.
    let new_byte_size = subtree_alloc_size((*children).size);
    if ((*children).capacity as usize) * core::mem::size_of::<Subtree>() < new_byte_size {
        (*children).contents =
            realloc((*children).contents.cast::<c_void>(), new_byte_size).cast::<Subtree>();
        (*children).capacity = (new_byte_size / core::mem::size_of::<Subtree>()) as u32;
    }
    let data = (*children)
        .contents
        .add((*children).size as usize)
        .cast::<SubtreeHeapData>();

    let result = subtree_init_node_data(data, symbol, (*children).size, production_id, language);
    subtree_summarize_children(result, language);
    result
}

pub unsafe fn subtree_new_error_node(
    children: *mut SubtreeArray,
    extra: bool,
    language: *const TSLanguage,
) -> Subtree {
    let result = subtree_new_node(TS_BUILTIN_SYM_ERROR, children, 0, language);
    (*result.heap_ptr()).set_extra(extra);
    subtree_from_mut(result)
}

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
        (*result.heap_ptr().cast_mut()).set_is_missing(true);
    }
    result
}

// Subtree mutation and ownership

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

pub unsafe fn subtree_make_mut(pool: &mut SubtreePool, self_: Subtree) -> MutableSubtree {
    if self_.data.is_inline() {
        return MutableSubtree { data: self_.data };
    }
    if (*self_.heap_ptr()).ref_count == 1 {
        return subtree_to_mut_unsafe(self_);
    }
    let result = subtree_clone(self_);
    subtree_release(pool, self_);
    result
}

pub unsafe fn subtree_retain(self_: Subtree) {
    if self_.data.is_inline() {
        return;
    }
    debug_assert!((*self_.heap_ptr()).ref_count > 0);
    let ref_count = ptr::addr_of!((*self_.heap_ptr()).ref_count).cast::<AtomicU32>();
    let prev = (*ref_count).fetch_add(1, Ordering::SeqCst);
    debug_assert!(prev.wrapping_add(1) != 0);
}

pub unsafe fn subtree_release(pool: &mut SubtreePool, self_: Subtree) {
    if self_.data.is_inline() {
        return;
    }
    pool.tree_stack.size = 0;

    debug_assert!((*self_.heap_ptr()).ref_count > 0);
    let ref_count = ptr::addr_of!((*self_.heap_ptr()).ref_count).cast::<AtomicU32>();
    if (*ref_count).fetch_sub(1, Ordering::SeqCst) == 1 {
        array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(self_));
    }

    while pool.tree_stack.size > 0 {
        let tree = array_pop(&mut pool.tree_stack);
        if (*tree.heap_ptr()).child_count > 0 {
            let children = subtree_children_slice(subtree_from_mut(tree));
            for child in children {
                let child = *child;
                if child.data.is_inline() {
                    continue;
                }
                debug_assert!((*child.heap_ptr()).ref_count > 0);
                let child_ref = ptr::addr_of!((*child.heap_ptr()).ref_count).cast::<AtomicU32>();
                if (*child_ref).fetch_sub(1, Ordering::SeqCst) == 1 {
                    array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(child));
                }
            }
            free(children.as_ptr().cast_mut().cast::<c_void>());
        } else {
            if (*tree.heap_ptr()).has_external_tokens() {
                external_scanner_state_delete((*tree.heap_ptr()).external_scanner_state_mut());
            }
            subtree_pool_free(pool, tree);
        }
    }
}

// Subtree balancing and summarization

pub unsafe fn subtree_compress(
    self_: MutableSubtree,
    count: u32,
    language: *const TSLanguage,
    stack: &mut MutableSubtreeArray,
) {
    let initial_stack_size = stack.size;

    let mut tree = self_;
    let symbol = (*tree.heap_ptr()).symbol;
    for _ in 0..count {
        if (*tree.heap_ptr()).ref_count > 1 || (*tree.heap_ptr()).child_count < 2 {
            break;
        }

        let child = subtree_to_mut_unsafe(mutable_subtree_child(tree, 0));
        if child.data.is_inline()
            || (*child.heap_ptr()).child_count < 2
            || (*child.heap_ptr()).ref_count > 1
            || (*child.heap_ptr()).symbol != symbol
        {
            break;
        }

        let grandchild = subtree_to_mut_unsafe(mutable_subtree_child(child, 0));
        if grandchild.data.is_inline()
            || (*grandchild.heap_ptr()).child_count < 2
            || (*grandchild.heap_ptr()).ref_count > 1
            || (*grandchild.heap_ptr()).symbol != symbol
        {
            break;
        }

        // Rotate: tree[0] = grandchild, child[0] = grandchild[last], grandchild[last] = child
        let gc_last = (*grandchild.heap_ptr()).child_count as usize - 1;
        *mutable_subtree_child_mut(tree, 0) = subtree_from_mut(grandchild);
        *mutable_subtree_child_mut(child, 0) = mutable_subtree_child(grandchild, gc_last);
        *mutable_subtree_child_mut(grandchild, gc_last) = subtree_from_mut(child);
        array_push(stack, tree);
        tree = grandchild;
    }

    while stack.size > initial_stack_size {
        tree = array_pop(stack);
        let child = subtree_to_mut_unsafe(mutable_subtree_child(tree, 0));
        let grandchild = subtree_to_mut_unsafe(mutable_subtree_child(
            child,
            (*child.heap_ptr()).child_count as usize - 1,
        ));
        subtree_summarize_children(grandchild, language);
        subtree_summarize_children(child, language);
        subtree_summarize_children(tree, language);
    }
}

pub unsafe fn subtree_summarize_children(self_: MutableSubtree, language: *const TSLanguage) {
    debug_assert!(!self_.data.is_inline());

    let data = mutable_subtree_data_mut(self_);
    data.children_mut().named_child_count = 0;
    data.children_mut().visible_child_count = 0;
    data.error_cost = 0;
    data.children_mut().repeat_depth = 0;
    data.children_mut().visible_descendant_count = 0;
    data.set_has_external_tokens(false);
    data.set_depends_on_column(false);
    data.set_has_external_scanner_state_change(false);
    data.children_mut().dynamic_precedence = 0;

    let mut structural_index: u32 = 0;
    let alias_sequence =
        language_alias_sequence_slice(language, u32::from(data.children().production_id));
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

        if subtree_symbol(child) != TS_BUILTIN_SYM_ERROR_REPEAT {
            data.error_cost += subtree_error_cost(child);
        }

        let grandchild_count = subtree_child_count(child);
        if (data.symbol == TS_BUILTIN_SYM_ERROR || data.symbol == TS_BUILTIN_SYM_ERROR_REPEAT)
            && !subtree_extra(child)
            && !(subtree_is_error(child) && grandchild_count == 0)
        {
            if subtree_visible(child) {
                data.error_cost += ERROR_COST_PER_SKIPPED_TREE;
            } else if grandchild_count > 0 {
                data.error_cost += ERROR_COST_PER_SKIPPED_TREE
                    * (*child.heap_ptr()).children().visible_child_count;
            }
        }

        data.children_mut().dynamic_precedence += subtree_dynamic_precedence(child);
        data.children_mut().visible_descendant_count += subtree_visible_descendant_count(child);

        let alias_symbol = alias_sequence
            .get(structural_index as usize)
            .copied()
            .unwrap_or(0);
        if !subtree_extra(child) && subtree_symbol(child) != 0 && alias_symbol != 0 {
            data.children_mut().visible_descendant_count += 1;
            data.children_mut().visible_child_count += 1;
            if ts_language_symbol_metadata(language, alias_symbol).named {
                data.children_mut().named_child_count += 1;
            }
        } else if subtree_visible(child) {
            data.children_mut().visible_descendant_count += 1;
            data.children_mut().visible_child_count += 1;
            if subtree_named(child) {
                data.children_mut().named_child_count += 1;
            }
        } else if grandchild_count > 0 {
            data.children_mut().visible_child_count +=
                (*child.heap_ptr()).children().visible_child_count;
            data.children_mut().named_child_count +=
                (*child.heap_ptr()).children().named_child_count;
        }

        if subtree_has_external_tokens(child) {
            data.set_has_external_tokens(true);
        }

        if subtree_is_error(child) {
            data.parse_state = TS_TREE_STATE_NONE;
        }

        if !subtree_extra(child) {
            structural_index += 1;
        }
    }

    data.lookahead_bytes = lookahead_end_byte - data.size.bytes - data.padding.bytes;

    if data.symbol == TS_BUILTIN_SYM_ERROR || data.symbol == TS_BUILTIN_SYM_ERROR_REPEAT {
        data.error_cost += ERROR_COST_PER_RECOVERY
            + ERROR_COST_PER_SKIPPED_CHAR * data.size.bytes
            + ERROR_COST_PER_SKIPPED_LINE * data.size.extent.row;
    }

    if data.child_count > 0 {
        let first_child = *children.get_unchecked(0);
        let last_child = *children.get_unchecked(data.child_count as usize - 1);

        if data.child_count >= 2
            && !data.visible()
            && !data.named()
            && subtree_symbol(first_child) == data.symbol
        {
            if subtree_repeat_depth(first_child) > subtree_repeat_depth(last_child) {
                data.children_mut().repeat_depth = (subtree_repeat_depth(first_child) + 1) as u16;
            } else {
                data.children_mut().repeat_depth = (subtree_repeat_depth(last_child) + 1) as u16;
            }
        }
    }
}

// Subtree comparison and editing

pub unsafe fn subtree_compare(left: Subtree, right: Subtree, pool: &mut SubtreePool) -> i32 {
    array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(left));
    array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(right));

    while pool.tree_stack.size > 0 {
        let right = subtree_from_mut(array_pop(&mut pool.tree_stack));
        let left = subtree_from_mut(array_pop(&mut pool.tree_stack));

        let left_symbol = subtree_symbol(left);
        let right_symbol = subtree_symbol(right);
        let left_child_count = subtree_child_count(left);
        let right_child_count = subtree_child_count(right);

        let mut result = 0i32;
        if left_symbol < right_symbol {
            result = -1;
        } else if right_symbol < left_symbol {
            result = 1;
        } else if left_child_count < right_child_count {
            result = -1;
        } else if right_child_count < left_child_count {
            result = 1;
        }
        if result != 0 {
            pool.tree_stack.size = 0;
            return result;
        }

        let left_children = subtree_children_slice(left);
        let right_children = subtree_children_slice(right);
        let mut i = left_child_count;
        while i > 0 {
            i -= 1;
            let left_child = *left_children.get_unchecked(i as usize);
            let right_child = *right_children.get_unchecked(i as usize);
            array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(left_child));
            array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(right_child));
        }
    }

    0
}

mod edit;
pub use edit::subtree_edit;

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
    if self_.heap_ptr().is_null() || self_.data.is_inline() {
        return &EMPTY_EXTERNAL_SCANNER_STATE;
    }

    let data = subtree_data_ref(*self_);
    if data.has_external_tokens() && data.child_count == 0 {
        data.external_scanner_state()
    } else {
        &EMPTY_EXTERNAL_SCANNER_STATE
    }
}

pub unsafe fn subtree_set_external_scanner_state(
    self_: MutableSubtree,
    bytes: *const u8,
    length: u32,
) {
    let data = mutable_subtree_data_mut(self_);
    debug_assert_eq!(data.child_count, 0);
    debug_assert!(data.has_external_tokens());
    data.data =
        SubtreeHeapDataContent::ExternalScannerState(external_scanner_state_new(bytes, length));
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

mod debug;
pub use debug::{subtree_print_dot_graph, subtree_string};

#[cfg(test)]
mod tests {
    use super::*;

    fn inline_leaf(size: u8) -> Subtree {
        Subtree {
            data: SubtreeInlineData {
                flags: INLINE_IS_INLINE | INLINE_VISIBLE | INLINE_NAMED,
                symbol: 1,
                parse_state: 1,
                padding_columns: 0,
                rows_and_lookahead: 0,
                padding_bytes: 0,
                size_bytes: size,
            },
        }
    }

    fn insertion(at: u32, size: u32) -> TSInputEdit {
        TSInputEdit {
            start_byte: at,
            old_end_byte: at,
            new_end_byte: at + size,
            start_point: TSPoint { row: 0, column: at },
            old_end_point: TSPoint { row: 0, column: at },
            new_end_point: TSPoint {
                row: 0,
                column: at + size,
            },
        }
    }

    #[test]
    fn edit_keeps_small_leaf_inline() {
        unsafe {
            let mut pool = subtree_pool_new(4);
            let tree = subtree_edit(inline_leaf(5), &insertion(2, 1), &mut pool);

            assert!(tree.data.is_inline());
            assert_eq!(subtree_size(tree).bytes, 6);
            assert!(subtree_has_changes(tree));

            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn edit_promotes_leaf_that_no_longer_fits_inline() {
        unsafe {
            let mut pool = subtree_pool_new(4);
            let tree = subtree_edit(inline_leaf(250), &insertion(2, 10), &mut pool);

            assert!(!tree.data.is_inline());
            assert_eq!(subtree_size(tree).bytes, 260);
            assert!(subtree_has_changes(tree));

            subtree_release(&mut pool, tree);
            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn edit_marks_the_affected_child() {
        unsafe {
            let mut pool = subtree_pool_new(4);
            let mut children = array_new();
            array_push(&mut children, inline_leaf(5));
            array_push(&mut children, inline_leaf(5));
            let parent = subtree_from_mut(subtree_new_node(
                TS_BUILTIN_SYM_ERROR,
                &mut children,
                0,
                ptr::null(),
            ));

            let tree = subtree_edit(parent, &insertion(2, 1), &mut pool);
            assert!(subtree_has_changes(tree));
            assert!(subtree_has_changes(*subtree_child(tree, 0)));
            assert!(!subtree_has_changes(*subtree_child(tree, 1)));

            subtree_release(&mut pool, tree);
            subtree_pool_delete(&mut pool);
        }
    }

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

            let mut children = array_new();
            array_push(&mut children, child1);
            array_push(&mut children, child2);

            let parent =
                subtree_new_node(TS_BUILTIN_SYM_ERROR_REPEAT, &mut children, 0, ptr::null());
            let parent_tree = subtree_from_mut(parent);

            assert_eq!(subtree_child_count(parent_tree), 2);
            assert_eq!(subtree_children_slice(parent_tree).len(), 2);
            assert_eq!(
                subtree_symbol(subtree_children_slice(parent_tree)[0]),
                TS_BUILTIN_SYM_ERROR
            );
            assert_eq!(
                subtree_symbol(subtree_children_slice(parent_tree)[1]),
                TS_BUILTIN_SYM_ERROR
            );

            subtree_release(&mut pool, parent_tree);
            subtree_pool_delete(&mut pool);
        }
    }
}
