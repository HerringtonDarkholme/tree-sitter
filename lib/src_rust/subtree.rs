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
    language_alias_sequence, language_field_map, language_full,
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
    /// Inline or heap state bytes, selected by `length`.
    data: ExternalScannerStateData,
    /// Serialized byte count.
    pub length: u32,
}

// SAFETY: Only used in a read-only static (EMPTY_EXTERNAL_SCANNER_STATE).
unsafe impl Sync for ExternalScannerState {}

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
    /// bit 9: `is_missing`, bit 10: `is_keyword`, bit 11: `arena_owned`
    pub flags: u16,

    // Anonymous union: children-info / external_scanner_state / lookahead_char
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
const HEAP_ARENA_OWNED: u16 = 1 << 11;

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
    pub const fn arena_owned(&self) -> bool {
        self.flags & HEAP_ARENA_OWNED != 0
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
    #[inline(always)]
    pub fn set_arena_owned(&mut self, value: bool) {
        set_u16_flag(&mut self.flags, HEAP_ARENA_OWNED, value);
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

pub union SubtreeHeapDataContent {
    /// Aggregate child metadata for internal nodes.
    pub children: SubtreeChildrenData,
    /// Serialized scanner state for external-token leaves.
    pub external_scanner_state: core::mem::ManuallyDrop<ExternalScannerState>,
    /// First skipped character for error leaves.
    pub lookahead_char: i32,
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

// ---------------------------------------------------------------------------
// Subtree / MutableSubtree — the core union types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub union Subtree {
    /// Inline representation when `data.is_inline()` is set.
    pub data: SubtreeInlineData,
    /// Heap representation otherwise.
    pub ptr: *const SubtreeHeapData,
}

#[derive(Clone, Copy)]
pub union MutableSubtree {
    /// Inline representation when `data.is_inline()` is set.
    pub data: SubtreeInlineData,
    /// Mutable heap representation otherwise.
    pub ptr: *mut SubtreeHeapData,
}

pub const NULL_SUBTREE: Subtree = Subtree { ptr: ptr::null() };

// Compile-time layout assertions for the internal tagged pointer/inline-data
// overlap. The unions use Rust layout, but both representations must remain
// exactly one pointer wide with the tag bit in the first byte.
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

/// Arena for tree-owned internal nodes.
///
/// Parser reductions can allocate accepted internal nodes in this arena. The
/// returned `TSTree` retains the arena, so copying a tree only bumps the arena
/// refcount instead of cloning every internal node.
pub struct TreeArena {
    /// Shared ownership count across copied trees.
    ref_count: AtomicU32,
    /// Singly linked list of allocated pages.
    pages: Option<NonNull<TreeArenaPage>>,
    /// Page currently used for bump allocation.
    current_page: Option<NonNull<TreeArenaPage>>,
}

struct TreeArenaPage {
    /// Next older page in the arena list.
    next: Option<NonNull<Self>>,
    /// Bump allocation buffer.
    contents: NonNull<u8>,
    /// Bytes currently used in `contents`.
    size: usize,
    /// Allocated byte capacity.
    capacity: usize,
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
    tree: *mut Subtree,
    edit: Edit,
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

// External scanner state

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

// Tree arena

const TREE_ARENA_PAGE_SIZE: usize = 16 * 1024;

const fn align_up(value: usize, alignment: usize) -> usize {
    debug_assert!(alignment.is_power_of_two());
    (value + alignment - 1) & !(alignment - 1)
}

pub unsafe fn tree_arena_new() -> NonNull<TreeArena> {
    let arena = NonNull::new_unchecked(malloc(core::mem::size_of::<TreeArena>()).cast());
    ptr::write(
        arena.as_ptr(),
        TreeArena {
            ref_count: AtomicU32::new(1),
            pages: None,
            current_page: None,
        },
    );
    arena
}

pub unsafe fn tree_arena_retain(arena: NonNull<TreeArena>) {
    let prev = (*arena.as_ptr()).ref_count.fetch_add(1, Ordering::SeqCst);
    debug_assert!(prev.wrapping_add(1) != 0);
}

pub unsafe fn tree_arena_release(arena: NonNull<TreeArena>) {
    if (*arena.as_ptr()).ref_count.fetch_sub(1, Ordering::SeqCst) != 1 {
        return;
    }

    let mut page = (*arena.as_ptr()).pages;
    while let Some(current) = page {
        let next = (*current.as_ptr()).next;
        free((*current.as_ptr()).contents.as_ptr().cast::<c_void>());
        free(current.as_ptr().cast::<c_void>());
        page = next;
    }
    free(arena.as_ptr().cast::<c_void>());
}

/// Try to satisfy an arena allocation from the current bump page.
unsafe fn tree_arena_try_current_page(
    arena: &mut TreeArena,
    size: usize,
    alignment: usize,
) -> *mut c_void {
    if let Some(mut current_page) = arena.current_page {
        let page = current_page.as_mut();
        let offset = align_up(page.size, alignment);
        if offset + size <= page.capacity {
            page.size = offset + size;
            return page.contents.as_ptr().add(offset).cast::<c_void>();
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
    let page = NonNull::new_unchecked(malloc(core::mem::size_of::<TreeArenaPage>()).cast());
    let contents = NonNull::new_unchecked(malloc(capacity).cast::<u8>());
    ptr::write(
        page.as_ptr(),
        TreeArenaPage {
            next: arena.pages,
            contents,
            size,
            capacity,
        },
    );
    arena.pages = Some(page);
    arena.current_page = Some(page);
    contents.as_ptr().cast::<c_void>()
}

/// Allocate bytes from the tree arena.
///
/// Internal nodes are stored as `[Subtree children...][SubtreeHeapData]`. The
/// arena uses page-sized bump allocation because accepted trees free all arena
/// nodes together when the last copied `TSTree` is deleted.
unsafe fn tree_arena_alloc(arena: &mut TreeArena, size: usize, alignment: usize) -> *mut c_void {
    let result = tree_arena_try_current_page(arena, size, alignment);
    if !result.is_null() {
        return result;
    }

    tree_arena_alloc_new_page(arena, size, alignment)
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
            free(tree.ptr.cast::<c_void>());
        }
        array_delete(&mut self_.free_trees);
    }
    if !self_.tree_stack.contents.is_null() {
        array_delete(&mut self_.tree_stack);
    }
}

unsafe fn subtree_pool_allocate(self_: &mut SubtreePool) -> *mut SubtreeHeapData {
    if self_.free_trees.size > 0 {
        array_pop(&mut self_.free_trees).ptr
    } else {
        malloc(core::mem::size_of::<SubtreeHeapData>()).cast::<SubtreeHeapData>()
    }
}

unsafe fn subtree_pool_free(self_: &mut SubtreePool, tree: MutableSubtree) {
    if self_.free_trees.capacity > 0 && self_.free_trees.size < TS_MAX_TREE_POOL_SIZE {
        array_push(&mut self_.free_trees, tree);
    } else {
        free(tree.ptr.cast::<c_void>());
    }
}

// Subtree accessors

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
    child_count as usize * core::mem::size_of::<Subtree>() + core::mem::size_of::<SubtreeHeapData>()
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
    let count = (*self_.ptr).child_count as usize;
    if count == 0 {
        &mut []
    } else {
        core::slice::from_raw_parts_mut(subtree_children(subtree_from_mut(self_)), count)
    }
}

#[inline]
unsafe fn mutable_subtree_data_mut<'a>(self_: MutableSubtree) -> &'a mut SubtreeHeapData {
    ptr_mut(self_.ptr)
}

#[inline]
unsafe fn subtree_data_ref<'a>(self_: Subtree) -> &'a SubtreeHeapData {
    ptr_ref(self_.ptr)
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

// Child and repetition metadata

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

// Visible-child metadata

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

// Error cost

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

// Parse metadata

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
        (*self_.ptr).set_has_changes(true);
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
            data: SubtreeHeapDataContent {
                children: SubtreeChildrenData {
                    visible_child_count: 0,
                    named_child_count: 0,
                    visible_descendant_count: 0,
                    dynamic_precedence: 0,
                    repeat_depth: 0,
                    production_id: 0,
                },
            },
        };
        Subtree { ptr: data }
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
    let data = result.ptr.cast_mut();
    (*data).data.lookahead_char = lookahead_char;
    result
}

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
        (*result).data.external_scanner_state = core::mem::ManuallyDrop::new(
            external_scanner_state_copy(&data.data.external_scanner_state),
        );
    }
    (*result).ref_count = 1;
    (*result).set_arena_owned(false);
    MutableSubtree { ptr: result }
}

unsafe fn subtree_init_node_data(
    data: *mut SubtreeHeapData,
    symbol: TSSymbol,
    child_count: u32,
    production_id: u32,
    language: *const TSLanguage,
    extra_flags: u16,
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
        ) | extra_flags,
        data: SubtreeHeapDataContent {
            children: SubtreeChildrenData {
                visible_child_count: 0,
                named_child_count: 0,
                visible_descendant_count: 0,
                dynamic_precedence: 0,
                repeat_depth: 0,
                production_id: production_id as u16,
            },
        },
    };
    MutableSubtree { ptr: data }
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

    let result = subtree_init_node_data(data, symbol, (*children).size, production_id, language, 0);
    subtree_summarize_children(result, language);
    result
}

/// Create an arena-owned internal node.
///
/// This has the same memory layout as `subtree_new_node`, but allocation
/// comes from the returned tree's arena instead of the transient subtree pool.
pub unsafe fn subtree_new_node_in_arena(
    arena: &mut TreeArena,
    symbol: TSSymbol,
    children: *const Subtree,
    child_count: u32,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    let byte_size = subtree_alloc_size(child_count);
    let allocation = tree_arena_alloc(arena, byte_size, core::mem::align_of::<SubtreeHeapData>())
        .cast::<Subtree>();

    if child_count > 0 {
        ptr::copy_nonoverlapping(children, allocation, child_count as usize);
    }

    let data = allocation
        .add(child_count as usize)
        .cast::<SubtreeHeapData>();
    let result = subtree_init_node_data(
        data,
        symbol,
        child_count,
        production_id,
        language,
        HEAP_ARENA_OWNED,
    );
    subtree_summarize_children(result, language);
    result
}

pub unsafe fn subtree_new_error_node(
    children: *mut SubtreeArray,
    extra: bool,
    language: *const TSLanguage,
) -> Subtree {
    let result = subtree_new_node(TS_BUILTIN_SYM_ERROR, children, 0, language);
    (*result.ptr).set_extra(extra);
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
        (*result.ptr.cast_mut()).set_is_missing(true);
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
    if (*self_.ptr).ref_count == 1 {
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
    debug_assert!((*self_.ptr).ref_count > 0);
    let ref_count = ptr::addr_of!((*self_.ptr).ref_count).cast::<AtomicU32>();
    let prev = (*ref_count).fetch_add(1, Ordering::SeqCst);
    debug_assert!(prev.wrapping_add(1) != 0);
}

pub unsafe fn subtree_release(pool: &mut SubtreePool, self_: Subtree) {
    if self_.data.is_inline() {
        return;
    }
    pool.tree_stack.size = 0;

    debug_assert!((*self_.ptr).ref_count > 0);
    let ref_count = ptr::addr_of!((*self_.ptr).ref_count).cast::<AtomicU32>();
    if (*ref_count).fetch_sub(1, Ordering::SeqCst) == 1 {
        array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(self_));
    }

    while pool.tree_stack.size > 0 {
        let tree = array_pop(&mut pool.tree_stack);
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
                    array_push(&mut pool.tree_stack, subtree_to_mut_unsafe(child));
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
                external_scanner_state_delete(ptr_mut(external_scanner_state));
            }
            if !(*tree.ptr).arena_owned() {
                subtree_pool_free(pool, tree);
            }
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
        array_push(stack, tree);
        tree = grandchild;
    }

    while stack.size > initial_stack_size {
        tree = array_pop(stack);
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
                data.data.children.repeat_depth = (subtree_repeat_depth(first_child) + 1) as u16;
            } else {
                data.data.children.repeat_depth = (subtree_repeat_depth(last_child) + 1) as u16;
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
