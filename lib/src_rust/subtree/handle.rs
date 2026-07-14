//! Compact subtree handles and their ownership operations.
//!
//! [`Subtree`] and [`MutableSubtree`] match the C runtime's pointer-sized union:
//! the same word stores either packed inline data or a pointer to heap data.
//! This module is the boundary around that representation. It discriminates
//! union arms, exposes typed accessors, maintains intrusive reference counts,
//! and performs copy-on-write conversion to a mutable handle.
//!
//! A copied handle is not automatically retained. Callers use `retain` when a
//! new owner is created and `release` when that owner is removed.

use core::{
    ptr::NonNull,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::ffi::{TSLanguage, TSPoint, TSStateId, TSSymbol};

use super::super::error_costs::{ERROR_COST_PER_MISSING_TREE, ERROR_COST_PER_RECOVERY};
use super::super::language::ts_language_symbol_metadata;
use super::super::length::{length_add, length_zero, Length};
use super::data::{
    ExternalScannerState, SubtreeHeapData, SubtreeHeapDataContent, SubtreeInlineData,
};
use super::storage::{subtree_clone_allocation, subtree_free_internal_node, subtree_pool_free};
use super::{
    SubtreePool, EMPTY_EXTERNAL_SCANNER_STATE, TS_BUILTIN_SYM_END, TS_BUILTIN_SYM_ERROR,
    TS_TREE_STATE_NONE,
};

// Subtree / MutableSubtree
// ---------------------------------------------------------------------------

/// Compact syntax-tree handle matching the C runtime's representation.
///
/// The inline arm and pointer arm intentionally overlap. Call `is_inline`
/// before reading either representation; all union access stays in this impl.
#[repr(C)]
#[derive(Clone, Copy)]
pub union Subtree {
    data: SubtreeInlineData,
    ptr: *const SubtreeHeapData,
}

/// Handle used when subtree mutation may be required.
///
/// The intrusive reference count means this wrapper does not itself prove
/// uniqueness; callers establish uniqueness before invoking mutation methods.
#[repr(C)]
#[derive(Clone, Copy)]
pub union MutableSubtree {
    data: SubtreeInlineData,
    ptr: *mut SubtreeHeapData,
}

const EMPTY_SUBTREE_DATA: SubtreeInlineData = SubtreeInlineData {
    flags: 0,
    symbol: 0,
    parse_state: 0,
    padding_columns: 0,
    rows_and_lookahead: 0,
    padding_bytes: 0,
    size_bytes: 0,
};

impl PartialEq for Subtree {
    fn eq(&self, other: &Self) -> bool {
        match (self.is_inline(), other.is_inline()) {
            (true, true) => unsafe { self.data == other.data },
            (false, false) => unsafe { self.ptr == other.ptr },
            _ => false,
        }
    }
}

impl Eq for Subtree {}

// SAFETY: Heap subtrees are immutable while shared. Their only shared mutation
// is the atomic reference count. Mutable access is used only after callers have
// established unique ownership of the allocation.
unsafe impl Send for Subtree {}
unsafe impl Sync for Subtree {}
unsafe impl Send for MutableSubtree {}

impl Subtree {
    pub(super) const fn from_inline(data: SubtreeInlineData) -> Self {
        debug_assert!(data.is_inline());
        Self { data }
    }

    pub(super) const fn from_heap(ptr: NonNull<SubtreeHeapData>) -> Self {
        Self::from_ptr(ptr.as_ptr())
    }

    const fn from_ptr(ptr: *const SubtreeHeapData) -> Self {
        // Initialize the full eight-byte storage before writing the pointer.
        // This matters on 32-bit targets, where the pointer arm is smaller.
        let mut result = Self {
            data: EMPTY_SUBTREE_DATA,
        };
        result.ptr = ptr;
        result
    }

    const unsafe fn heap_ptr(self) -> NonNull<SubtreeHeapData> {
        debug_assert!(!self.is_inline());
        NonNull::new_unchecked(self.ptr.cast_mut())
    }

    /// Whether this handle is the null subtree sentinel.
    pub const fn is_null(self) -> bool {
        !self.is_inline() && unsafe { self.data.is_zero() }
    }

    /// Whether this leaf is stored directly in the handle.
    pub const fn is_inline(self) -> bool {
        // SAFETY: All bit patterns are valid for the byte-only inline arm.
        unsafe { self.data.is_inline() }
    }

    const fn inline_data(self) -> Option<SubtreeInlineData> {
        if self.is_inline() {
            // SAFETY: The tag identifies the active inline representation.
            Some(unsafe { self.data })
        } else {
            None
        }
    }

    pub(super) fn inline_data_mut(&mut self) -> Option<&mut SubtreeInlineData> {
        if self.is_inline() {
            // SAFETY: The tag identifies the active inline representation.
            Some(unsafe { &mut self.data })
        } else {
            None
        }
    }

    /// Borrow the heap node represented by this non-inline handle.
    pub const unsafe fn heap_data(&self) -> &SubtreeHeapData {
        self.heap_ptr().as_ref()
    }

    #[inline]
    pub unsafe fn symbol(self) -> TSSymbol {
        if let Some(data) = self.inline_data() {
            TSSymbol::from(data.symbol)
        } else {
            self.heap_data().symbol
        }
    }

    #[inline]
    pub const unsafe fn visible(self) -> bool {
        if let Some(data) = self.inline_data() {
            data.visible()
        } else if self.is_null() {
            false
        } else {
            self.heap_data().visible()
        }
    }

    #[inline]
    pub const unsafe fn named(self) -> bool {
        if let Some(data) = self.inline_data() {
            data.named()
        } else if self.is_null() {
            false
        } else {
            self.heap_data().named()
        }
    }

    #[inline]
    pub const unsafe fn extra(self) -> bool {
        if let Some(data) = self.inline_data() {
            data.extra()
        } else if self.is_null() {
            false
        } else {
            self.heap_data().extra()
        }
    }

    #[inline]
    pub const unsafe fn has_changes(self) -> bool {
        if let Some(data) = self.inline_data() {
            data.has_changes()
        } else if self.is_null() {
            false
        } else {
            self.heap_data().has_changes()
        }
    }

    #[inline]
    pub const unsafe fn missing(self) -> bool {
        if let Some(data) = self.inline_data() {
            data.is_missing()
        } else if self.is_null() {
            false
        } else {
            self.heap_data().is_missing()
        }
    }

    #[inline]
    pub const unsafe fn is_keyword(self) -> bool {
        if let Some(data) = self.inline_data() {
            data.is_keyword()
        } else if self.is_null() {
            false
        } else {
            self.heap_data().is_keyword()
        }
    }

    #[inline]
    pub const unsafe fn parse_state(self) -> TSStateId {
        if let Some(data) = self.inline_data() {
            data.parse_state
        } else if self.is_null() {
            TS_TREE_STATE_NONE
        } else {
            self.heap_data().parse_state
        }
    }

    #[inline]
    pub unsafe fn lookahead_bytes(self) -> u32 {
        if let Some(data) = self.inline_data() {
            u32::from(data.lookahead_bytes())
        } else if self.is_null() {
            0
        } else {
            self.heap_data().lookahead_bytes
        }
    }

    #[inline]
    const unsafe fn children_ptr(&self) -> NonNull<Self> {
        debug_assert!(!self.is_inline() && !self.is_null());
        debug_assert!(self.heap_data().child_count > 0);
        NonNull::new_unchecked(
            self.heap_ptr()
                .cast::<Self>()
                .as_ptr()
                .sub(self.heap_data().child_count as usize),
        )
    }

    #[inline]
    pub unsafe fn child(&self, index: u32) -> &Self {
        self.children().get_unchecked(index as usize)
    }

    pub const unsafe fn children(&self) -> &[Self] {
        let count = self.child_count() as usize;
        if count == 0 {
            &[]
        } else {
            core::slice::from_raw_parts(self.children_ptr().as_ptr(), count)
        }
    }

    #[inline]
    pub unsafe fn padding(self) -> Length {
        if let Some(data) = self.inline_data() {
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
            self.heap_data().padding
        }
    }

    #[inline]
    pub unsafe fn size(self) -> Length {
        if let Some(data) = self.inline_data() {
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
            self.heap_data().size
        }
    }

    #[inline]
    pub unsafe fn total_size(self) -> Length {
        length_add(self.padding(), self.size())
    }

    #[inline]
    pub unsafe fn total_bytes(self) -> u32 {
        self.total_size().bytes
    }

    #[inline]
    pub const unsafe fn child_count(self) -> u32 {
        if self.is_inline() || self.is_null() {
            0
        } else {
            self.heap_data().child_count
        }
    }

    #[inline]
    pub unsafe fn repeat_depth(self) -> u32 {
        if self.is_inline() || self.is_null() || self.heap_data().child_count == 0 {
            0
        } else {
            u32::from(self.heap_data().children().repeat_depth)
        }
    }

    #[inline]
    pub const unsafe fn visible_descendant_count(self) -> u32 {
        if self.is_inline() || self.is_null() || self.heap_data().child_count == 0 {
            0
        } else {
            self.heap_data().children().visible_descendant_count
        }
    }

    #[inline]
    pub const unsafe fn visible_child_count(self) -> u32 {
        if self.child_count() > 0 {
            self.heap_data().children().visible_child_count
        } else {
            0
        }
    }

    #[inline]
    pub const unsafe fn error_cost(self) -> u32 {
        if self.missing() {
            ERROR_COST_PER_MISSING_TREE + ERROR_COST_PER_RECOVERY
        } else if self.is_inline() || self.is_null() {
            0
        } else {
            self.heap_data().error_cost
        }
    }

    #[inline]
    pub const unsafe fn dynamic_precedence(self) -> i32 {
        if self.is_inline() || self.is_null() || self.heap_data().child_count == 0 {
            0
        } else {
            self.heap_data().children().dynamic_precedence
        }
    }

    #[inline]
    pub const unsafe fn production_id(self) -> u16 {
        if self.child_count() > 0 {
            self.heap_data().children().production_id
        } else {
            0
        }
    }

    #[inline]
    pub const unsafe fn has_external_tokens(self) -> bool {
        !self.is_inline() && !self.is_null() && self.heap_data().has_external_tokens()
    }

    #[inline]
    pub const unsafe fn has_external_scanner_state_change(self) -> bool {
        !self.is_inline() && !self.is_null() && self.heap_data().has_external_scanner_state_change()
    }

    #[inline]
    pub const unsafe fn depends_on_column(self) -> bool {
        !self.is_inline() && !self.is_null() && self.heap_data().depends_on_column()
    }

    #[inline]
    pub unsafe fn is_error(self) -> bool {
        self.symbol() == TS_BUILTIN_SYM_ERROR
    }

    #[inline]
    pub unsafe fn is_eof(self) -> bool {
        self.symbol() == TS_BUILTIN_SYM_END
    }

    #[inline]
    pub const fn into_mut(self) -> MutableSubtree {
        if self.is_inline() {
            MutableSubtree {
                data: unsafe { self.data },
            }
        } else {
            MutableSubtree::from_ptr(unsafe { self.ptr.cast_mut() })
        }
    }

    pub unsafe fn clone_mut(self) -> MutableSubtree {
        let data = self.heap_data();
        let result = subtree_clone_allocation(self);
        let content = match &data.data {
            SubtreeHeapDataContent::Children(children) => {
                SubtreeHeapDataContent::Children(*children)
            }
            SubtreeHeapDataContent::ExternalScannerState(state) => {
                SubtreeHeapDataContent::ExternalScannerState(state.copy())
            }
            SubtreeHeapDataContent::LookaheadChar(character) => {
                SubtreeHeapDataContent::LookaheadChar(*character)
            }
        };
        result.as_ptr().write(SubtreeHeapData {
            ref_count: AtomicU32::new(1),
            padding: data.padding,
            size: data.size,
            lookahead_bytes: data.lookahead_bytes,
            error_cost: data.error_cost,
            child_count: data.child_count,
            symbol: data.symbol,
            parse_state: data.parse_state,
            flags: data.flags,
            data: content,
        });
        MutableSubtree::from_heap(result)
    }

    pub unsafe fn make_mut(self, pool: &mut SubtreePool) -> MutableSubtree {
        if self.is_inline() || self.is_null() {
            return self.into_mut();
        }
        if self.heap_data().ref_count() == 1 {
            return self.into_mut();
        }
        let result = self.clone_mut();
        self.release(pool);
        result
    }

    pub unsafe fn retain(self) {
        if self.is_inline() || self.is_null() {
            return;
        }
        let ref_count = &self.heap_data().ref_count;
        debug_assert!(ref_count.load(Ordering::Relaxed) > 0);
        let previous = ref_count.fetch_add(1, Ordering::SeqCst);
        debug_assert!(previous.wrapping_add(1) != 0);
    }

    pub unsafe fn release(self, pool: &mut SubtreePool) {
        if self.is_inline() || self.is_null() {
            return;
        }
        pool.tree_stack.clear();

        let ref_count = &self.heap_data().ref_count;
        debug_assert!(ref_count.load(Ordering::Relaxed) > 0);
        if ref_count.fetch_sub(1, Ordering::SeqCst) == 1 {
            pool.tree_stack.push(self.into_mut());
        }

        while !pool.tree_stack.is_empty() {
            let mut tree = pool.tree_stack.pop();
            if tree.heap_data().child_count > 0 {
                let immutable_tree = tree.into_immutable();
                let children = immutable_tree.children();
                for &child in children {
                    if child.is_inline() {
                        continue;
                    }
                    let child_ref = &child.heap_data().ref_count;
                    debug_assert!(child_ref.load(Ordering::Relaxed) > 0);
                    if child_ref.fetch_sub(1, Ordering::SeqCst) == 1 {
                        pool.tree_stack.push(child.into_mut());
                    }
                }
                subtree_free_internal_node(tree.into_immutable());
            } else {
                if tree.heap_data().has_external_tokens() {
                    tree.heap_data_mut().external_scanner_state_mut().delete();
                }
                subtree_pool_free(pool, tree);
            }
        }
    }

    pub unsafe fn last_external_token(self) -> Self {
        let mut tree = self;
        if !tree.has_external_tokens() {
            return NULL_SUBTREE;
        }
        loop {
            let data = tree.heap_data();
            if data.child_count == 0 {
                return tree;
            }
            for &child in tree.children().iter().rev() {
                if child.has_external_tokens() {
                    tree = child;
                    break;
                }
            }
        }
    }

    pub unsafe fn external_scanner_state(&self) -> &ExternalScannerState {
        if self.is_null() || self.is_inline() {
            return &EMPTY_EXTERNAL_SCANNER_STATE;
        }
        let data = self.heap_data();
        if data.has_external_tokens() && data.child_count == 0 {
            data.external_scanner_state()
        } else {
            &EMPTY_EXTERNAL_SCANNER_STATE
        }
    }

    pub unsafe fn has_same_external_scanner_state(self, other: Self) -> bool {
        self.external_scanner_state().as_bytes() == other.external_scanner_state().as_bytes()
    }
}

impl MutableSubtree {
    pub(super) const fn from_heap(ptr: NonNull<SubtreeHeapData>) -> Self {
        Self::from_ptr(ptr.as_ptr())
    }

    const fn from_ptr(ptr: *mut SubtreeHeapData) -> Self {
        let mut result = Self {
            data: EMPTY_SUBTREE_DATA,
        };
        result.ptr = ptr;
        result
    }

    pub(super) const unsafe fn heap_ptr(self) -> NonNull<SubtreeHeapData> {
        debug_assert!(!self.is_inline());
        NonNull::new_unchecked(self.ptr)
    }

    pub const fn is_inline(self) -> bool {
        // SAFETY: All bit patterns are valid for the byte-only inline arm.
        unsafe { self.data.is_inline() }
    }

    pub(super) const fn inline_data(self) -> Option<SubtreeInlineData> {
        if self.is_inline() {
            // SAFETY: The tag identifies the active inline representation.
            Some(unsafe { self.data })
        } else {
            None
        }
    }

    pub(super) fn inline_data_mut(&mut self) -> Option<&mut SubtreeInlineData> {
        if self.is_inline() {
            // SAFETY: The tag identifies the active inline representation.
            Some(unsafe { &mut self.data })
        } else {
            None
        }
    }

    /// Borrow the heap node represented by this mutable handle.
    pub const unsafe fn heap_data(&self) -> &SubtreeHeapData {
        self.heap_ptr().as_ref()
    }

    /// Mutably borrow the heap node represented by this handle.
    pub unsafe fn heap_data_mut(&mut self) -> &mut SubtreeHeapData {
        self.heap_ptr().as_mut()
    }

    /// Mutably borrow this internal node's children.
    pub unsafe fn children_mut(&mut self) -> &mut [Subtree] {
        let count = self.heap_data().child_count as usize;
        if count == 0 {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(self.into_immutable().children_ptr().as_ptr(), count)
        }
    }

    #[inline]
    pub(super) unsafe fn child(self, index: usize) -> Subtree {
        *self.into_immutable().children().get_unchecked(index)
    }

    #[inline]
    pub(super) unsafe fn child_mut(&mut self, index: usize) -> &mut Subtree {
        self.children_mut().get_unchecked_mut(index)
    }

    #[inline]
    pub unsafe fn set_extra(&mut self, is_extra: bool) {
        if let Some(data) = self.inline_data_mut() {
            data.set_extra(is_extra);
        } else {
            self.heap_data_mut().set_extra(is_extra);
        }
    }

    #[inline]
    pub const fn into_immutable(self) -> Subtree {
        if self.is_inline() {
            Subtree {
                data: unsafe { self.data },
            }
        } else {
            Subtree::from_ptr(unsafe { self.ptr.cast_const() })
        }
    }

    pub unsafe fn set_symbol(&mut self, symbol: TSSymbol, language: *const TSLanguage) {
        let metadata = ts_language_symbol_metadata(language, symbol);
        if let Some(data) = self.inline_data_mut() {
            debug_assert!(symbol < TSSymbol::from(u8::MAX));
            data.symbol = symbol as u8;
            data.set_named(metadata.named);
            data.set_visible(metadata.visible);
        } else {
            let data = self.heap_data_mut();
            data.symbol = symbol;
            data.set_named(metadata.named);
            data.set_visible(metadata.visible);
        }
    }

    pub unsafe fn set_external_scanner_state(mut self, bytes: &[u8]) {
        let data = self.heap_data_mut();
        debug_assert_eq!(data.child_count, 0);
        debug_assert!(data.has_external_tokens());
        data.data =
            SubtreeHeapDataContent::ExternalScannerState(ExternalScannerState::from_bytes(bytes));
    }
}

pub const NULL_SUBTREE: Subtree = Subtree::from_ptr(core::ptr::null());

// Keep both union arms pointer-sized and preserve the C-compatible byte layout
// that makes the low-bit inline tag observable through either arm.
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
