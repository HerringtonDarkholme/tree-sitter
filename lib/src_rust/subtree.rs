use core::{
    ptr,
    ptr::NonNull,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::ffi::{TSInputEdit, TSLanguage, TSPoint, TSStateId, TSSymbol};

use super::error_costs::{
    ERROR_COST_PER_MISSING_TREE, ERROR_COST_PER_RECOVERY, ERROR_COST_PER_SKIPPED_CHAR,
    ERROR_COST_PER_SKIPPED_LINE, ERROR_COST_PER_SKIPPED_TREE,
};
use super::language::{
    language_alias_sequence_slice, language_field_map_slice, language_full,
    language_write_symbol_as_dot_string, ts_language_symbol_metadata, ts_language_symbol_name,
};
use super::length::{length_add, length_saturating_sub, length_sub, length_zero, Length};
use super::utils::Array;

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
/// The first bit overlaps the low bit of the union's pointer arm. Allocator
/// alignment keeps that bit clear for heap subtrees, so it distinguishes an
/// inline subtree without making the handle larger than a pointer.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SubtreeInlineData {
    /// Packed `is_inline`, `visible`, `named`, `extra`, `has_changes`,
    /// `is_missing`, and `is_keyword` flags.
    pub flags: u8,
    pub symbol: u8,
    pub parse_state: u16,
    pub padding_columns: u8,
    /// Low 4 bits = `padding_rows`, high 4 bits = `lookahead_bytes`
    pub rows_and_lookahead: u8,
    pub padding_bytes: u8,
    pub size_bytes: u8,
}

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
    const fn is_zero(self) -> bool {
        self.flags == 0
            && self.symbol == 0
            && self.parse_state == 0
            && self.padding_columns == 0
            && self.rows_and_lookahead == 0
            && self.padding_bytes == 0
            && self.size_bytes == 0
    }

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
    pub ref_count: AtomicU32,
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
    pub fn ref_count(&self) -> u32 {
        self.ref_count.load(Ordering::Relaxed)
    }

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
    const fn from_inline(data: SubtreeInlineData) -> Self {
        debug_assert!(data.is_inline());
        Self { data }
    }

    const fn from_heap(ptr: NonNull<SubtreeHeapData>) -> Self {
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

    fn inline_data_mut(&mut self) -> Option<&mut SubtreeInlineData> {
        if self.is_inline() {
            // SAFETY: The tag identifies the active inline representation.
            Some(unsafe { &mut self.data })
        } else {
            None
        }
    }

    /// Borrow the heap node represented by this non-inline handle.
    pub const unsafe fn heap_data<'a>(self) -> &'a SubtreeHeapData {
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
    const unsafe fn children_ptr(self) -> NonNull<Self> {
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
    pub unsafe fn child<'a>(self, index: u32) -> &'a Self {
        self.children().get_unchecked(index as usize)
    }

    pub const unsafe fn children<'a>(self) -> &'a [Self] {
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
            let tree = pool.tree_stack.pop();
            if tree.heap_data().child_count > 0 {
                let children = tree.into_immutable().children();
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

    pub unsafe fn external_scanner_state<'a>(self) -> &'a ExternalScannerState {
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
    const fn from_heap(ptr: NonNull<SubtreeHeapData>) -> Self {
        Self::from_ptr(ptr.as_ptr())
    }

    const fn from_ptr(ptr: *mut SubtreeHeapData) -> Self {
        let mut result = Self {
            data: EMPTY_SUBTREE_DATA,
        };
        result.ptr = ptr;
        result
    }

    const unsafe fn heap_ptr(self) -> NonNull<SubtreeHeapData> {
        debug_assert!(!self.is_inline());
        NonNull::new_unchecked(self.ptr)
    }

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

    fn inline_data_mut(&mut self) -> Option<&mut SubtreeInlineData> {
        if self.is_inline() {
            // SAFETY: The tag identifies the active inline representation.
            Some(unsafe { &mut self.data })
        } else {
            None
        }
    }

    /// Borrow the heap node represented by this mutable handle.
    pub const unsafe fn heap_data<'a>(self) -> &'a SubtreeHeapData {
        self.heap_ptr().as_ref()
    }

    /// Mutably borrow the heap node represented by this handle.
    pub unsafe fn heap_data_mut<'a>(self) -> &'a mut SubtreeHeapData {
        self.heap_ptr().as_mut()
    }

    /// Mutably borrow this internal node's children.
    pub unsafe fn children_mut<'a>(self) -> &'a mut [Subtree] {
        let count = self.heap_data().child_count as usize;
        if count == 0 {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(self.into_immutable().children_ptr().as_ptr(), count)
        }
    }

    #[inline]
    unsafe fn child(self, index: usize) -> Subtree {
        *self.children_mut().get_unchecked(index)
    }

    #[inline]
    unsafe fn child_mut<'a>(self, index: usize) -> &'a mut Subtree {
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

    pub unsafe fn set_external_scanner_state(self, bytes: &[u8]) {
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

mod storage;
pub use storage::{
    subtree_array_clear, subtree_array_copy, subtree_array_delete,
    subtree_array_remove_trailing_extras, subtree_array_reverse, subtree_pool_delete,
    subtree_pool_new,
};
use storage::{
    subtree_clone_allocation, subtree_free_internal_node, subtree_pool_allocate, subtree_pool_free,
    subtree_reuse_children, subtree_take_children,
};

// Compatibility accessors still used by the legacy query implementation.

#[inline]
pub unsafe fn subtree_symbol(self_: Subtree) -> TSSymbol {
    self_.symbol()
}

/// Allocation size for a heap subtree with `child_count` preceding children.
#[inline]
pub const fn subtree_alloc_size(child_count: u32) -> usize {
    child_count as usize * core::mem::size_of::<Subtree>() + core::mem::size_of::<SubtreeHeapData>()
}

#[inline]
pub unsafe fn subtree_is_repetition(self_: Subtree) -> u32 {
    if self_.is_inline() || self_.is_null() {
        0
    } else {
        u32::from(
            !self_.heap_data().named()
                && !self_.heap_data().visible()
                && self_.heap_data().child_count != 0,
        )
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
    if let Some(data) = self_.inline_data_mut() {
        data.set_has_changes(true);
    } else {
        self_.heap_data_mut().set_has_changes(true);
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
        Subtree::from_inline(SubtreeInlineData {
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
        })
    } else {
        let data = subtree_pool_allocate(pool);
        data.as_ptr().write(SubtreeHeapData {
            ref_count: AtomicU32::new(1),
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
        });
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
    result.into_mut().heap_data_mut().data = SubtreeHeapDataContent::LookaheadChar(lookahead_char);
    result
}

unsafe fn subtree_init_node_data(
    data: NonNull<SubtreeHeapData>,
    symbol: TSSymbol,
    child_count: u32,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    let metadata = ts_language_symbol_metadata(language, symbol);
    data.as_ptr().write(SubtreeHeapData {
        ref_count: AtomicU32::new(1),
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
    });
    MutableSubtree::from_heap(data)
}

/// Create a heap internal node by taking ownership of its child array.
///
/// The child array is resized so the `SubtreeHeapData` header can live directly
/// after the child slice in one compact allocation:
/// `[child_0, child_1, ... child_n][SubtreeHeapData]`.
pub unsafe fn subtree_new_node(
    symbol: TSSymbol,
    children: SubtreeArray,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    let (data, child_count) = subtree_take_children(children);

    let result = subtree_init_node_data(data, symbol, child_count, production_id, language);
    subtree_summarize_children(result, language);
    result
}

/// Build a temporary internal node in parser-owned reusable storage.
///
/// The returned subtree borrows `children`'s allocation and must not be
/// retained or released. It becomes invalid as soon as `children` is changed.
pub(super) unsafe fn subtree_new_scratch_node(
    symbol: TSSymbol,
    children: &mut SubtreeArray,
    language: *const TSLanguage,
) -> Subtree {
    let data = subtree_reuse_children(children);
    let result = subtree_init_node_data(data, symbol, children.size, 0, language);
    subtree_summarize_children(result, language);
    result.into_immutable()
}

pub unsafe fn subtree_new_error_node(
    children: SubtreeArray,
    extra: bool,
    language: *const TSLanguage,
) -> Subtree {
    let result = subtree_new_node(TS_BUILTIN_SYM_ERROR, children, 0, language);
    result.heap_data_mut().set_extra(extra);
    result.into_immutable()
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
    if let Some(data) = result.inline_data_mut() {
        data.set_is_missing(true);
    } else {
        result.into_mut().heap_data_mut().set_is_missing(true);
    }
    result
}

// Subtree mutation and ownership

// Subtree balancing and summarization

pub unsafe fn subtree_compress(
    self_: MutableSubtree,
    count: u32,
    language: *const TSLanguage,
    stack: &mut MutableSubtreeArray,
) {
    let initial_stack_size = stack.size;

    let mut tree = self_;
    let symbol = tree.heap_data().symbol;
    for _ in 0..count {
        if tree.heap_data().ref_count() > 1 || tree.heap_data().child_count < 2 {
            break;
        }

        let child = tree.child(0).into_mut();
        if child.is_inline()
            || child.heap_data().child_count < 2
            || child.heap_data().ref_count() > 1
            || child.heap_data().symbol != symbol
        {
            break;
        }

        let grandchild = child.child(0).into_mut();
        if grandchild.is_inline()
            || grandchild.heap_data().child_count < 2
            || grandchild.heap_data().ref_count() > 1
            || grandchild.heap_data().symbol != symbol
        {
            break;
        }

        // Rotate: tree[0] = grandchild, child[0] = grandchild[last], grandchild[last] = child
        let gc_last = grandchild.heap_data().child_count as usize - 1;
        *tree.child_mut(0) = grandchild.into_immutable();
        *child.child_mut(0) = grandchild.child(gc_last);
        *grandchild.child_mut(gc_last) = child.into_immutable();
        stack.push(tree);
        tree = grandchild;
    }

    while stack.size > initial_stack_size {
        tree = stack.pop();
        let child = tree.child(0).into_mut();
        let grandchild = child
            .child(child.heap_data().child_count as usize - 1)
            .into_mut();
        subtree_summarize_children(grandchild, language);
        subtree_summarize_children(child, language);
        subtree_summarize_children(tree, language);
    }
}

pub unsafe fn subtree_summarize_children(self_: MutableSubtree, language: *const TSLanguage) {
    debug_assert!(!self_.is_inline());

    let data = self_.heap_data_mut();
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

    let children = self_.into_immutable().children();
    for (i, child) in children.iter().copied().enumerate() {
        let i = i as u32;

        if data.size.extent.row == 0 && child.depends_on_column() {
            data.set_depends_on_column(true);
        }

        if child.has_external_scanner_state_change() {
            data.set_has_external_scanner_state_change(true);
        }

        if i == 0 {
            data.padding = child.padding();
            data.size = child.size();
        } else {
            data.size = length_add(data.size, child.total_size());
        }

        let child_lookahead_end_byte =
            data.padding.bytes + data.size.bytes + child.lookahead_bytes();
        if child_lookahead_end_byte > lookahead_end_byte {
            lookahead_end_byte = child_lookahead_end_byte;
        }

        if child.symbol() != TS_BUILTIN_SYM_ERROR_REPEAT {
            data.error_cost += child.error_cost();
        }

        let grandchild_count = child.child_count();
        if (data.symbol == TS_BUILTIN_SYM_ERROR || data.symbol == TS_BUILTIN_SYM_ERROR_REPEAT)
            && !child.extra()
            && !(child.is_error() && grandchild_count == 0)
        {
            if child.visible() {
                data.error_cost += ERROR_COST_PER_SKIPPED_TREE;
            } else if grandchild_count > 0 {
                data.error_cost +=
                    ERROR_COST_PER_SKIPPED_TREE * child.heap_data().children().visible_child_count;
            }
        }

        data.children_mut().dynamic_precedence += child.dynamic_precedence();
        data.children_mut().visible_descendant_count += child.visible_descendant_count();

        let alias_symbol = alias_sequence
            .get(structural_index as usize)
            .copied()
            .unwrap_or(0);
        if !child.extra() && child.symbol() != 0 && alias_symbol != 0 {
            data.children_mut().visible_descendant_count += 1;
            data.children_mut().visible_child_count += 1;
            if ts_language_symbol_metadata(language, alias_symbol).named {
                data.children_mut().named_child_count += 1;
            }
        } else if child.visible() {
            data.children_mut().visible_descendant_count += 1;
            data.children_mut().visible_child_count += 1;
            if child.named() {
                data.children_mut().named_child_count += 1;
            }
        } else if grandchild_count > 0 {
            data.children_mut().visible_child_count +=
                child.heap_data().children().visible_child_count;
            data.children_mut().named_child_count += child.heap_data().children().named_child_count;
        }

        if child.has_external_tokens() {
            data.set_has_external_tokens(true);
        }

        if child.is_error() {
            data.parse_state = TS_TREE_STATE_NONE;
        }

        if !child.extra() {
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
            && first_child.symbol() == data.symbol
        {
            if first_child.repeat_depth() > last_child.repeat_depth() {
                data.children_mut().repeat_depth = (first_child.repeat_depth() + 1) as u16;
            } else {
                data.children_mut().repeat_depth = (last_child.repeat_depth() + 1) as u16;
            }
        }
    }
}

// Subtree comparison and editing

pub unsafe fn subtree_compare(left: Subtree, right: Subtree, pool: &mut SubtreePool) -> i32 {
    pool.tree_stack.push(left.into_mut());
    pool.tree_stack.push(right.into_mut());

    while pool.tree_stack.size > 0 {
        let right = pool.tree_stack.pop().into_immutable();
        let left = pool.tree_stack.pop().into_immutable();

        let left_symbol = left.symbol();
        let right_symbol = right.symbol();
        let left_child_count = left.child_count();
        let right_child_count = right.child_count();

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

        let left_children = left.children();
        let right_children = right.children();
        let mut i = left_child_count;
        while i > 0 {
            i -= 1;
            let left_child = *left_children.get_unchecked(i as usize);
            let right_child = *right_children.get_unchecked(i as usize);
            pool.tree_stack.push(left_child.into_mut());
            pool.tree_stack.push(right_child.into_mut());
        }
    }

    0
}

mod edit;
pub use edit::subtree_edit;

mod debug;
pub use debug::{subtree_print_dot_graph, subtree_string};

#[cfg(test)]
mod tests {
    use super::*;

    fn inline_leaf(size: u8) -> Subtree {
        Subtree::from_inline(SubtreeInlineData {
            flags: INLINE_IS_INLINE | INLINE_VISIBLE | INLINE_NAMED,
            symbol: 1,
            parse_state: 1,
            padding_columns: 0,
            rows_and_lookahead: 0,
            padding_bytes: 0,
            size_bytes: size,
        })
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

            assert!(tree.is_inline());
            assert_eq!(tree.size().bytes, 6);
            assert!(tree.has_changes());

            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn edit_promotes_leaf_that_no_longer_fits_inline() {
        unsafe {
            let mut pool = subtree_pool_new(4);
            let tree = subtree_edit(inline_leaf(250), &insertion(2, 10), &mut pool);

            assert!(!tree.is_inline());
            assert_eq!(tree.size().bytes, 260);
            assert!(tree.has_changes());

            tree.release(&mut pool);
            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn edit_marks_the_affected_child() {
        unsafe {
            let mut pool = subtree_pool_new(4);
            let mut children = Array::new();
            children.push(inline_leaf(5));
            children.push(inline_leaf(5));
            let parent =
                subtree_new_node(TS_BUILTIN_SYM_ERROR, children, 0, ptr::null()).into_immutable();

            let tree = subtree_edit(parent, &insertion(2, 1), &mut pool);
            assert!(tree.has_changes());
            assert!((*tree.child(0)).has_changes());
            assert!(!(*tree.child(1)).has_changes());

            tree.release(&mut pool);
            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn scratch_node_borrows_reusable_child_storage() {
        unsafe {
            let mut children = Array::new();
            children.push(inline_leaf(2));
            children.push(inline_leaf(3));

            let first = subtree_new_scratch_node(TS_BUILTIN_SYM_ERROR, &mut children, ptr::null());
            assert_eq!(first.child_count(), 2);

            children.clear();
            children.push(inline_leaf(4));
            let second = subtree_new_scratch_node(TS_BUILTIN_SYM_ERROR, &mut children, ptr::null());
            assert_eq!(second.child_count(), 1);
            assert_eq!(second.child(0).size().bytes, 4);

            children.delete();
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

            let mut children = Array::new();
            children.push(child1);
            children.push(child2);

            let parent = subtree_new_node(TS_BUILTIN_SYM_ERROR_REPEAT, children, 0, ptr::null());
            let parent_tree = parent.into_immutable();

            assert_eq!(parent_tree.child_count(), 2);
            assert_eq!(parent_tree.children().len(), 2);
            assert_eq!(parent_tree.children()[0].symbol(), TS_BUILTIN_SYM_ERROR);
            assert_eq!(parent_tree.children()[1].symbol(), TS_BUILTIN_SYM_ERROR);

            parent_tree.release(&mut pool);
            subtree_pool_delete(&mut pool);
        }
    }
}
