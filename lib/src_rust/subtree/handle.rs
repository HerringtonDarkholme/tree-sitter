//! Compact subtree handles and their ownership operations.
//!
//! [`Subtree`] and [`MutableSubtree`] are four-byte arena indexes. Compact leaf
//! records retain the former packed inline layout in the arena; full leaves
//! and internal nodes use [`SubtreeHeapData`]. The low index bit tags compact
//! leaf records, while the remaining bits are a physical byte index in the
//! owning [`SubtreeArena`](super::SubtreeArena).
//!
//! A copied handle is not automatically retained. Callers use `retain` when a
//! new owner is created and `release` when that owner is removed.

use core::{cell::Cell, ptr::NonNull, sync::atomic::AtomicBool};

use crate::ffi::{TSLanguage, TSPoint, TSStateId, TSSymbol};

use super::super::error_costs::{ERROR_COST_PER_MISSING_TREE, ERROR_COST_PER_RECOVERY};
use super::super::language::ts_language_symbol_metadata;
use super::super::length::{length_add, length_zero, Length};
use super::data::{
    ExternalScannerState, SubtreeHeapData, SubtreeHeapDataContent, SubtreeInlineData,
};
use super::storage::{
    subtree_arena_data, subtree_arena_is_published, subtree_clone_allocation,
    subtree_pool_allocate_inline,
};
use super::subtree_child_storage_size;
use super::TS_TREE_STATE_NONE;
use super::{SubtreeArena, SubtreePool, TS_SUBTREE_SLAB_CAPACITY};
use super::{EMPTY_EXTERNAL_SCANNER_STATE, TS_BUILTIN_SYM_END, TS_BUILTIN_SYM_ERROR};

// Subtree / MutableSubtree
// ---------------------------------------------------------------------------

/// Compact syntax-tree handle using a tagged physical arena byte index.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Subtree {
    index: u32,
}

/// Handle used when subtree mutation may be required.
///
/// The parser-private and published sharing markers mean this wrapper does not
/// itself prove uniqueness; callers establish uniqueness before mutation.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct MutableSubtree {
    index: u32,
}

// SAFETY: Heap subtrees are immutable while shared. Parser-private sharing
// markers are touched only before publication; published sharing uses an
// atomic marker. Mutable access follows the corresponding uniqueness check.
unsafe impl Send for Subtree {}
unsafe impl Sync for Subtree {}
unsafe impl Send for MutableSubtree {}

impl Subtree {
    pub(super) unsafe fn from_inline(
        arena: *mut SubtreeArena,
        ptr: NonNull<SubtreeInlineData>,
    ) -> Self {
        debug_assert!(ptr.as_ref().is_inline());
        let index = Self::index_from_ptr(arena, ptr.cast());
        debug_assert_eq!(index & 1, 0);
        Self { index: index | 1 }
    }

    pub(super) unsafe fn from_heap(
        arena: *mut SubtreeArena,
        ptr: NonNull<SubtreeHeapData>,
    ) -> Self {
        debug_assert!(!arena.is_null());
        let index = Self::index_from_ptr(arena, ptr.cast());
        debug_assert_eq!(index & 1, 0);
        Self { index }
    }

    unsafe fn index_from_ptr(arena: *mut SubtreeArena, ptr: NonNull<u8>) -> u32 {
        debug_assert!(!arena.is_null());
        let base = subtree_arena_data(arena) as usize;
        let address = ptr.as_ptr() as usize;
        debug_assert!(address >= base);
        let byte_index = address - base;
        debug_assert!(byte_index < TS_SUBTREE_SLAB_CAPACITY);
        debug_assert_ne!(byte_index, 0);
        u32::try_from(byte_index).expect("subtree arena index fits in u32")
    }

    const fn from_bits(bits: u32) -> Self {
        Self { index: bits }
    }

    const fn bits(self) -> u32 {
        self.index
    }

    pub(super) const fn heap_index(self) -> usize {
        (self.bits() & !1) as usize
    }

    #[inline]
    #[allow(clippy::cast_ptr_alignment)]
    const unsafe fn inline_ptr(self, arena: *mut SubtreeArena) -> NonNull<SubtreeInlineData> {
        debug_assert!(self.is_inline());
        NonNull::new_unchecked(
            subtree_arena_data(arena)
                .add(self.heap_index())
                .cast::<SubtreeInlineData>(),
        )
    }

    #[allow(clippy::cast_ptr_alignment)]
    const unsafe fn heap_ptr(self, arena: *mut SubtreeArena) -> NonNull<SubtreeHeapData> {
        debug_assert!(!self.is_inline());
        debug_assert!(!self.is_null());
        NonNull::new_unchecked(
            subtree_arena_data(arena)
                .add(self.heap_index())
                .cast::<SubtreeHeapData>(),
        )
    }

    /// Whether this handle is the null subtree sentinel.
    pub const fn is_null(self) -> bool {
        self.bits() == 0
    }

    /// Whether this index addresses a compact leaf record.
    pub const fn is_inline(self) -> bool {
        self.bits() & 1 != 0
    }

    pub(super) const unsafe fn inline_data(
        self,
        arena: *mut SubtreeArena,
    ) -> Option<SubtreeInlineData> {
        if self.is_inline() {
            Some(*self.inline_ptr(arena).as_ptr())
        } else {
            None
        }
    }

    /// Borrow the full heap node represented by this non-compact handle.
    pub const unsafe fn heap_data(&self, arena: *mut SubtreeArena) -> &SubtreeHeapData {
        self.heap_ptr(arena).as_ref()
    }

    #[inline(always)]
    pub unsafe fn shared(self, arena: *mut SubtreeArena) -> bool {
        if subtree_arena_is_published(arena) {
            self.heap_data(arena).published_shared()
        } else {
            self.heap_data(arena).parser_shared()
        }
    }

    #[inline]
    pub unsafe fn symbol(self, arena: *mut SubtreeArena) -> TSSymbol {
        if let Some(data) = self.inline_data(arena) {
            TSSymbol::from(data.symbol)
        } else {
            self.heap_data(arena).symbol
        }
    }

    #[inline]
    pub const unsafe fn visible(self, arena: *mut SubtreeArena) -> bool {
        if let Some(data) = self.inline_data(arena) {
            data.visible()
        } else if self.is_null() {
            false
        } else {
            self.heap_data(arena).visible()
        }
    }

    #[inline]
    pub const unsafe fn named(self, arena: *mut SubtreeArena) -> bool {
        if let Some(data) = self.inline_data(arena) {
            data.named()
        } else if self.is_null() {
            false
        } else {
            self.heap_data(arena).named()
        }
    }

    #[inline]
    pub const unsafe fn extra(self, arena: *mut SubtreeArena) -> bool {
        if let Some(data) = self.inline_data(arena) {
            data.extra()
        } else if self.is_null() {
            false
        } else {
            self.heap_data(arena).extra()
        }
    }

    #[inline]
    pub const unsafe fn has_changes(self, arena: *mut SubtreeArena) -> bool {
        if let Some(data) = self.inline_data(arena) {
            data.has_changes()
        } else if self.is_null() {
            false
        } else {
            self.heap_data(arena).has_changes()
        }
    }

    #[inline]
    pub const unsafe fn missing(self, arena: *mut SubtreeArena) -> bool {
        if let Some(data) = self.inline_data(arena) {
            data.is_missing()
        } else if self.is_null() {
            false
        } else {
            self.heap_data(arena).is_missing()
        }
    }

    #[inline]
    pub const unsafe fn is_keyword(self, arena: *mut SubtreeArena) -> bool {
        if let Some(data) = self.inline_data(arena) {
            data.is_keyword()
        } else if self.is_null() {
            false
        } else {
            self.heap_data(arena).is_keyword()
        }
    }

    #[inline]
    pub const unsafe fn parse_state(self, arena: *mut SubtreeArena) -> TSStateId {
        if let Some(data) = self.inline_data(arena) {
            data.parse_state
        } else if self.is_null() {
            TS_TREE_STATE_NONE
        } else {
            self.heap_data(arena).parse_state
        }
    }

    #[inline]
    pub unsafe fn lookahead_bytes(self, arena: *mut SubtreeArena) -> u32 {
        if let Some(data) = self.inline_data(arena) {
            u32::from(data.lookahead_bytes())
        } else if self.is_null() {
            0
        } else {
            self.heap_data(arena).lookahead_bytes
        }
    }

    #[inline]
    #[allow(clippy::cast_ptr_alignment)]
    pub(super) const unsafe fn children_ptr(&self, arena: *mut SubtreeArena) -> NonNull<Self> {
        debug_assert!(!self.is_inline() && !self.is_null());
        debug_assert!(self.heap_data(arena).child_count > 0);
        let child_count = self.heap_data(arena).child_count;
        NonNull::new_unchecked(
            self.heap_ptr(arena)
                .cast::<u8>()
                .as_ptr()
                .sub(subtree_child_storage_size(child_count))
                .cast::<Self>(),
        )
    }

    #[inline]
    pub unsafe fn child(&self, arena: *mut SubtreeArena, index: u32) -> &Self {
        self.children(arena).get_unchecked(index as usize)
    }

    pub const unsafe fn children(&self, arena: *mut SubtreeArena) -> &[Self] {
        let count = self.child_count(arena) as usize;
        if count == 0 {
            &[]
        } else {
            core::slice::from_raw_parts(self.children_ptr(arena).as_ptr(), count)
        }
    }

    #[inline]
    pub unsafe fn padding(self, arena: *mut SubtreeArena) -> Length {
        if let Some(data) = self.inline_data(arena) {
            Length {
                bytes: u32::from(data.padding_bytes),
                extent: TSPoint {
                    row: u32::from(data.padding_rows()),
                    column: u32::from(data.padding_columns),
                },
            }
        } else if self.is_null() {
            length_zero()
        } else {
            self.heap_data(arena).padding
        }
    }

    #[inline]
    pub unsafe fn size(self, arena: *mut SubtreeArena) -> Length {
        if let Some(data) = self.inline_data(arena) {
            Length {
                bytes: u32::from(data.size_bytes),
                extent: TSPoint {
                    row: 0,
                    column: u32::from(data.size_bytes),
                },
            }
        } else if self.is_null() {
            length_zero()
        } else {
            self.heap_data(arena).size
        }
    }

    #[inline]
    pub unsafe fn total_size(self, arena: *mut SubtreeArena) -> Length {
        length_add(self.padding(arena), self.size(arena))
    }

    #[inline]
    pub unsafe fn total_bytes(self, arena: *mut SubtreeArena) -> u32 {
        self.total_size(arena).bytes
    }

    #[inline]
    pub const unsafe fn child_count(self, arena: *mut SubtreeArena) -> u32 {
        if self.is_inline() || self.is_null() {
            0
        } else {
            self.heap_data(arena).child_count
        }
    }

    #[inline]
    pub unsafe fn repeat_depth(self, arena: *mut SubtreeArena) -> u32 {
        if self.is_inline() || self.is_null() || self.heap_data(arena).child_count == 0 {
            0
        } else {
            u32::from(self.heap_data(arena).children().repeat_depth)
        }
    }

    #[inline]
    pub const unsafe fn visible_descendant_count(self, arena: *mut SubtreeArena) -> u32 {
        if self.is_inline() || self.is_null() || self.heap_data(arena).child_count == 0 {
            0
        } else {
            self.heap_data(arena).children().visible_descendant_count
        }
    }

    #[inline]
    pub const unsafe fn visible_child_count(self, arena: *mut SubtreeArena) -> u32 {
        if self.child_count(arena) > 0 {
            self.heap_data(arena).children().visible_child_count
        } else {
            0
        }
    }

    #[inline]
    pub const unsafe fn error_cost(self, arena: *mut SubtreeArena) -> u32 {
        if self.missing(arena) {
            ERROR_COST_PER_MISSING_TREE + ERROR_COST_PER_RECOVERY
        } else if self.is_inline() || self.is_null() {
            0
        } else {
            self.heap_data(arena).error_cost
        }
    }

    #[inline]
    pub const unsafe fn dynamic_precedence(self, arena: *mut SubtreeArena) -> i32 {
        if self.is_inline() || self.is_null() || self.heap_data(arena).child_count == 0 {
            0
        } else {
            self.heap_data(arena).children().dynamic_precedence
        }
    }

    #[inline]
    pub const unsafe fn production_id(self, arena: *mut SubtreeArena) -> u16 {
        if self.child_count(arena) > 0 {
            self.heap_data(arena).children().production_id
        } else {
            0
        }
    }

    #[inline]
    pub const unsafe fn has_external_tokens(self, arena: *mut SubtreeArena) -> bool {
        !self.is_inline() && !self.is_null() && self.heap_data(arena).has_external_tokens()
    }

    #[inline]
    pub const unsafe fn has_external_scanner_state_change(self, arena: *mut SubtreeArena) -> bool {
        !self.is_inline()
            && !self.is_null()
            && self.heap_data(arena).has_external_scanner_state_change()
    }

    #[inline]
    pub const unsafe fn depends_on_column(self, arena: *mut SubtreeArena) -> bool {
        !self.is_inline() && !self.is_null() && self.heap_data(arena).depends_on_column()
    }

    #[inline]
    pub unsafe fn is_error(self, arena: *mut SubtreeArena) -> bool {
        self.symbol(arena) == TS_BUILTIN_SYM_ERROR
    }

    #[inline]
    pub unsafe fn is_eof(self, arena: *mut SubtreeArena) -> bool {
        self.symbol(arena) == TS_BUILTIN_SYM_END
    }

    #[inline]
    pub const fn into_mut(self) -> MutableSubtree {
        MutableSubtree::from_bits(self.bits())
    }

    pub unsafe fn clone_mut(self, pool: &mut SubtreePool) -> MutableSubtree {
        let arena = pool.arena();
        if let Some(data) = self.inline_data(arena) {
            let result = subtree_pool_allocate_inline(pool);
            result.as_ptr().write(data);
            return MutableSubtree::from_inline(pool.arena(), result);
        }
        let data = self.heap_data(arena);
        let result = subtree_clone_allocation(pool, self);
        let content = match &data.data {
            SubtreeHeapDataContent::Children(children) => {
                SubtreeHeapDataContent::Children(*children)
            }
            SubtreeHeapDataContent::ExternalScannerState(state) => {
                SubtreeHeapDataContent::ExternalScannerState(state.copy(pool.arena()))
            }
            SubtreeHeapDataContent::LookaheadChar(character) => {
                SubtreeHeapDataContent::LookaheadChar(*character)
            }
        };
        let result_data = SubtreeHeapData {
            parser_shared: Cell::new(false),
            published_shared: AtomicBool::new(false),
            parser_visited: Cell::new(false),
            padding: data.padding,
            size: data.size,
            lookahead_bytes: data.lookahead_bytes,
            error_cost: data.error_cost,
            child_count: data.child_count,
            symbol: data.symbol,
            parse_state: data.parse_state,
            flags: data.flags,
            data: content,
        };
        result.as_ptr().write(result_data);
        MutableSubtree::from_heap(arena, result)
    }

    pub unsafe fn make_mut(self, pool: &mut SubtreePool) -> MutableSubtree {
        if self.is_null() {
            return self.into_mut();
        }
        // Compact leaves have value semantics in the old hybrid handle. They
        // carry no refcount in Candidate D, so clone their eight-byte record
        // before any mutation to preserve copy-on-write behavior.
        if self.is_inline() {
            return self.clone_mut(pool);
        }
        let arena = pool.arena();
        if !self.shared(arena) {
            return self.into_mut();
        }
        self.clone_mut(pool)
    }

    pub unsafe fn retain(self, arena: *mut SubtreeArena) {
        if self.is_inline() || self.is_null() {
            return;
        }
        if subtree_arena_is_published(arena) {
            self.heap_data(arena).mark_published_shared();
        } else {
            self.heap_data(arena).mark_parser_shared();
        }
    }

    pub const unsafe fn release(self, _pool: &mut SubtreePool) {}

    pub unsafe fn last_external_token(self, arena: *mut SubtreeArena) -> Self {
        let mut tree = self;
        if !tree.has_external_tokens(arena) {
            return NULL_SUBTREE;
        }
        loop {
            let data = tree.heap_data(arena);
            if data.child_count == 0 {
                return tree;
            }
            for &child in tree.children(arena).iter().rev() {
                if child.has_external_tokens(arena) {
                    tree = child;
                    break;
                }
            }
        }
    }

    pub unsafe fn external_scanner_state(&self, arena: *mut SubtreeArena) -> &ExternalScannerState {
        if self.is_null() || self.is_inline() {
            return &EMPTY_EXTERNAL_SCANNER_STATE;
        }
        let data = self.heap_data(arena);
        if data.has_external_tokens() && data.child_count == 0 {
            data.external_scanner_state()
        } else {
            &EMPTY_EXTERNAL_SCANNER_STATE
        }
    }

    pub unsafe fn has_same_external_scanner_state(
        self,
        other: Self,
        arena: *mut SubtreeArena,
        other_arena: *mut SubtreeArena,
    ) -> bool {
        self.external_scanner_state(arena).as_bytes()
            == other.external_scanner_state(other_arena).as_bytes()
    }
}

impl MutableSubtree {
    pub(super) unsafe fn from_inline(
        arena: *mut SubtreeArena,
        ptr: NonNull<SubtreeInlineData>,
    ) -> Self {
        Self::from_bits(Subtree::from_inline(arena, ptr).bits())
    }

    pub(super) unsafe fn from_heap(
        arena: *mut SubtreeArena,
        ptr: NonNull<SubtreeHeapData>,
    ) -> Self {
        let immutable = Subtree::from_heap(arena, ptr);
        Self::from_bits(immutable.bits())
    }

    const fn from_bits(bits: u32) -> Self {
        Self { index: bits }
    }

    const fn bits(self) -> u32 {
        self.index
    }

    pub(super) const unsafe fn heap_ptr(
        self,
        arena: *mut SubtreeArena,
    ) -> NonNull<SubtreeHeapData> {
        debug_assert!(!self.is_inline());
        self.into_immutable().heap_ptr(arena)
    }

    pub const fn is_inline(self) -> bool {
        self.bits() & 1 != 0
    }

    pub(super) const unsafe fn inline_data(
        self,
        arena: *mut SubtreeArena,
    ) -> Option<SubtreeInlineData> {
        self.into_immutable().inline_data(arena)
    }

    pub(super) unsafe fn inline_data_mut(
        &mut self,
        arena: *mut SubtreeArena,
    ) -> Option<&mut SubtreeInlineData> {
        if self.is_inline() {
            Some(self.into_immutable().inline_ptr(arena).as_mut())
        } else {
            None
        }
    }

    /// Borrow the heap node represented by this mutable handle.
    pub const unsafe fn heap_data(&self, arena: *mut SubtreeArena) -> &SubtreeHeapData {
        self.heap_ptr(arena).as_ref()
    }

    /// Mutably borrow the heap node represented by this handle.
    pub unsafe fn heap_data_mut(&mut self, arena: *mut SubtreeArena) -> &mut SubtreeHeapData {
        self.heap_ptr(arena).as_mut()
    }

    /// Mutably borrow this internal node's children.
    pub unsafe fn children_mut(&mut self, arena: *mut SubtreeArena) -> &mut [Subtree] {
        let count = self.heap_data(arena).child_count as usize;
        if count == 0 {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(
                self.into_immutable().children_ptr(arena).as_ptr(),
                count,
            )
        }
    }

    #[inline]
    pub(super) unsafe fn child(self, arena: *mut SubtreeArena, index: usize) -> Subtree {
        *self.into_immutable().children(arena).get_unchecked(index)
    }

    #[inline]
    pub(super) unsafe fn child_mut(
        &mut self,
        arena: *mut SubtreeArena,
        index: usize,
    ) -> &mut Subtree {
        self.children_mut(arena).get_unchecked_mut(index)
    }

    #[inline]
    pub unsafe fn set_extra(&mut self, arena: *mut SubtreeArena, is_extra: bool) {
        if let Some(data) = self.inline_data_mut(arena) {
            data.set_extra(is_extra);
        } else {
            self.heap_data_mut(arena).set_extra(is_extra);
        }
    }

    #[inline]
    pub const fn into_immutable(self) -> Subtree {
        Subtree::from_bits(self.bits())
    }

    pub unsafe fn set_symbol(
        &mut self,
        arena: *mut SubtreeArena,
        symbol: TSSymbol,
        language: *const TSLanguage,
    ) {
        let metadata = ts_language_symbol_metadata(language, symbol);
        if let Some(data) = self.inline_data_mut(arena) {
            debug_assert!(symbol < TSSymbol::from(u8::MAX));
            data.symbol = symbol as u8;
            data.set_named(metadata.named);
            data.set_visible(metadata.visible);
        } else {
            let data = self.heap_data_mut(arena);
            data.symbol = symbol;
            data.set_named(metadata.named);
            data.set_visible(metadata.visible);
        }
    }

    pub unsafe fn set_external_scanner_state(mut self, arena: *mut SubtreeArena, bytes: &[u8]) {
        let data = self.heap_data_mut(arena);
        debug_assert_eq!(data.child_count, 0);
        debug_assert!(data.has_external_tokens());
        data.data = SubtreeHeapDataContent::ExternalScannerState(ExternalScannerState::from_bytes(
            arena, bytes,
        ));
    }
}

pub const NULL_SUBTREE: Subtree = Subtree::from_bits(0);

// Compact leaf records retain the former eight-byte packed layout, while every
// parser-facing reference is a four-byte tagged arena index.
const _: () = assert!(core::mem::size_of::<SubtreeInlineData>() == 8);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, flags) == 0);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, symbol) == 1);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, parse_state) == 2);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, padding_columns) == 4);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, rows_and_lookahead) == 5);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, padding_bytes) == 6);
const _: () = assert!(core::mem::offset_of!(SubtreeInlineData, size_bytes) == 7);
const _: () = assert!(core::mem::size_of::<Subtree>() == 4);
const _: () = assert!(core::mem::size_of::<MutableSubtree>() == 4);
