//! Memory layouts stored behind compact subtree handles.
//!
//! Small leaves use the legacy-named [`SubtreeInlineData`] as a compact
//! eight-byte arena record. Larger leaves and all internal nodes use
//! [`SubtreeHeapData`], whose content is either leaf metadata or an owned child
//! allocation. Parser-facing subtree handles are four-byte tagged arena
//! indexes. This module also defines the inline-or-heap storage for serialized
//! external-scanner state.
//!
//! These types describe representation only. Handle discrimination and
//! sharing operations belong to `handle`, while allocation belongs to
//! `storage`.

use core::{
    cell::Cell,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::ffi::{TSStateId, TSSymbol};

use super::super::length::Length;

// ExternalScannerState
// ---------------------------------------------------------------------------

pub(super) const EXTERNAL_SCANNER_STATE_INLINE_SIZE: usize = 24;

pub struct ExternalScannerState {
    /// Owned serialized scanner bytes.
    pub(super) data: ExternalScannerStateData,
    /// Serialized byte count.
    pub length: u32,
}

// SAFETY: Scanner state is immutable after it is stored in a subtree. The heap
// variant owns its allocation and exposes it only as read-only bytes.
unsafe impl Sync for ExternalScannerState {}

pub(super) enum ExternalScannerStateData {
    Inline([u8; EXTERNAL_SCANNER_STATE_INLINE_SIZE]),
    Heap(NonNull<u8>),
}

// ---------------------------------------------------------------------------
// SubtreeInlineData — bitfield-packed inline node
// ---------------------------------------------------------------------------

/// Compact eight-byte leaf record addressed by a tagged arena index.
///
/// `INLINE_IS_INLINE` remains part of the record for compatibility with the
/// former hybrid-handle layout. The four-byte handle now uses its own low-bit
/// tag to distinguish this record from a full heap record.
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

pub(super) const INLINE_IS_INLINE: u8 = 1 << 0;
pub(super) const INLINE_VISIBLE: u8 = 1 << 1;
pub(super) const INLINE_NAMED: u8 = 1 << 2;
pub(super) const INLINE_EXTRA: u8 = 1 << 3;
const INLINE_HAS_CHANGES: u8 = 1 << 4;
const INLINE_IS_MISSING: u8 = 1 << 5;
pub(super) const INLINE_IS_KEYWORD: u8 = 1 << 6;

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
    /// Conservative parser-private copy-on-write oracle.
    pub parser_shared: Cell<bool>,
    /// Concurrent copy-on-write oracle after arena publication.
    pub published_shared: AtomicBool,
    /// Parser-private mark used to rebuild exact accepted-DAG counts.
    pub parser_visited: Cell<bool>,
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

// SAFETY: `parser_shared` and `parser_visited` are mutated only while the arena
// is exclusively parser-owned. They are frozen before the arena is published;
// published ownership changes use only `published_shared`.
unsafe impl Sync for SubtreeHeapData {}

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
    pub fn parser_shared(&self) -> bool {
        self.parser_shared.get()
    }

    #[inline(always)]
    pub fn mark_parser_shared(&self) {
        self.parser_shared.set(true);
    }

    #[inline(always)]
    pub fn parser_visited(&self) -> bool {
        self.parser_visited.get()
    }

    #[inline(always)]
    pub fn set_parser_visited(&self, value: bool) {
        self.parser_visited.set(value);
    }

    #[inline(always)]
    pub fn published_shared(&self) -> bool {
        self.published_shared.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn mark_published_shared(&self) {
        self.published_shared.store(true, Ordering::Relaxed);
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
    #[inline(always)]
    pub fn set_is_keyword(&mut self, value: bool) {
        set_u16_flag(&mut self.flags, HEAP_IS_KEYWORD, value);
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

    pub const fn lookahead_char(&self) -> i32 {
        let SubtreeHeapDataContent::LookaheadChar(character) = &self.data else {
            panic!("error leaf must contain its lookahead character")
        };
        *character
    }
}
