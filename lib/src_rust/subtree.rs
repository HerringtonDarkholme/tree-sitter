//! Internal syntax-tree representation and subtree construction.
//!
//! A [`Subtree`] is the value carried by lexer results and parse-stack edges.
//! Leaves represent tokens; internal subtrees represent completed grammar
//! productions. This module builds those values and computes the cached sizes,
//! child counts, error costs, precedence, scanner state, and visibility data
//! needed by parsing and public tree navigation.
//!
//! The implementation is split along ownership boundaries:
//!
//! - `data` defines the inline and heap layouts;
//! - `handle` contains the compact immutable and mutable handles and all union
//!   access;
//! - `storage` allocates, retains, releases, and pools subtree memory;
//! - `edit` updates geometry and change flags after an input edit; and
//! - `debug` renders S-expressions and DOT graphs.
//!
//! Shared heap subtrees are immutable. Mutation first obtains a uniquely owned
//! [`MutableSubtree`], cloning the allocation when necessary.

use core::{
    cell::Cell,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize},
};

use crate::ffi::{TSLanguage, TSPoint, TSStateId, TSSymbol};

use super::error_costs::{
    ERROR_COST_PER_MISSING_TREE, ERROR_COST_PER_RECOVERY, ERROR_COST_PER_SKIPPED_CHAR,
    ERROR_COST_PER_SKIPPED_LINE, ERROR_COST_PER_SKIPPED_TREE,
};
use super::language::{language_alias_sequence_slice, ts_language_symbol_metadata};
use super::length::{length_add, length_zero, Length};
use super::utils::Array;

mod data;
use data::{
    ExternalScannerState, ExternalScannerStateData, SubtreeChildrenData, SubtreeHeapData,
    SubtreeInlineData, SubtreeInternalData, SubtreeLeafData, SubtreeLeafDataContent,
    EXTERNAL_SCANNER_STATE_INLINE_SIZE, INLINE_EXTRA, INLINE_IS_INLINE, INLINE_IS_KEYWORD,
    INLINE_NAMED, INLINE_VISIBLE,
};

mod handle;
pub use handle::{MutableSubtree, Subtree, NULL_SUBTREE};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const TS_TREE_STATE_NONE: TSStateId = u16::MAX;
const TS_MAX_INLINE_TREE_LENGTH: u8 = u8::MAX;
/// Virtual address-space capacity of one pointer-stable subtree slab.
///
/// This branch is an upper-bound experiment: one parse/tree family receives a
/// single contiguous virtual range, records are bump-allocated, and dead
/// records are not reused. Pages are enabled on demand, so unused capacity does
/// not contribute to RSS.
#[cfg(target_pointer_width = "64")]
const TS_SUBTREE_SLAB_CAPACITY: usize = 1usize << 32;
#[cfg(target_pointer_width = "32")]
const TS_SUBTREE_SLAB_CAPACITY: usize = 512 * 1024 * 1024;

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

pub type MutableSubtreeArray = Array<MutableSubtree>;

/// Contiguous child-handle buffer.
///
/// A null `arena` uses the ordinary generic-array allocator. A non-null arena
/// means capacity comes from the subtree slab and is abandoned, rather than
/// individually freed, when the array grows or is deleted.
pub struct SubtreeArray {
    pub contents: *mut Subtree,
    pub size: u32,
    pub capacity: u32,
    arena: *mut SubtreeArena,
    /// Arena rewind epoch in which `contents` was allocated. Persistent parser
    /// scratch arrays lazily discard stale capacity after a completed tree is
    /// deleted and the arena is rewound for the next parse.
    arena_generation: u32,
}

// ---------------------------------------------------------------------------
// SubtreePool
// ---------------------------------------------------------------------------

pub struct SubtreeArena {
    /// Parser/tree-family owners of this allocation.
    ref_count: AtomicU32,
    /// Next unused payload byte. Allocations may occur from copied trees on
    /// different threads, so bumping is atomic even though parsing itself is
    /// single-threaded.
    offset: AtomicUsize,
    /// Payload bytes whose virtual pages have been made readable/writable.
    committed: AtomicUsize,
    /// Incremented whenever the bump cursor is rewound. This invalidates
    /// cached scratch-array pointers without touching published tree records.
    generation: AtomicU32,
    /// False while subtree ownership is parser-private; true after accepted
    /// counts have been rebuilt for publication.
    published: bool,
}

pub struct SubtreePool {
    /// Current slab. The parser retains this after publishing a tree so it can
    /// reuse the allocation when that tree is deleted before the next parse.
    arena: *mut SubtreeArena,
    /// Scratch stack used by iterative release/compress operations.
    pub(super) tree_stack: MutableSubtreeArray,
}

impl SubtreePool {
    /// Storage domain used to resolve arena-relative heap indexes.
    pub(super) const fn arena(&self) -> *mut SubtreeArena {
        self.arena
    }
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
pub(super) use storage::subtree_array_prepare_scratch;
pub(super) use storage::{
    subtree_arena_release, subtree_arena_retain, subtree_pool_from_arena,
    subtree_pool_prepare_for_parse, subtree_pool_retain_arena,
};
pub use storage::{
    subtree_array_clear, subtree_array_copy, subtree_array_delete, subtree_array_new,
    subtree_array_remove_trailing_extras, subtree_array_reverse, subtree_pool_delete,
    subtree_pool_new,
};
use storage::{
    subtree_pool_allocate, subtree_pool_allocate_inline, subtree_reuse_children,
    subtree_take_children,
};

// Compatibility accessors still used by the legacy query implementation.

#[inline]
pub unsafe fn subtree_symbol(self_: Subtree, arena: *mut SubtreeArena) -> TSSymbol {
    self_.symbol(arena)
}

/// Allocation size for a heap subtree with `child_count` preceding children.
#[inline]
pub const fn subtree_alloc_size(child_count: u32) -> usize {
    subtree_child_storage_size(child_count) + core::mem::size_of::<SubtreeInternalData>()
}

/// Bytes occupied by the child prefix, including any padding required to keep
/// the following heap header naturally aligned.
#[inline]
pub const fn subtree_child_storage_size(child_count: u32) -> usize {
    let bytes = child_count as usize * core::mem::size_of::<Subtree>();
    let alignment = core::mem::align_of::<SubtreeInternalData>();
    (bytes + alignment - 1) & !(alignment - 1)
}

#[inline]
pub unsafe fn subtree_is_repetition(self_: Subtree, arena: *mut SubtreeArena) -> u32 {
    if self_.is_inline() || self_.is_null() {
        0
    } else {
        u32::from(
            !self_.heap_data(arena).named()
                && !self_.heap_data(arena).visible()
                && self_.heap_data(arena).child_count != 0,
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

/// Reduction-facing snapshot of one child subtree's hot metadata.
///
/// Heap handles are resolved once while constructing this value. The
/// summarization loop can then update the new parent without repeatedly
/// reconstructing `arena_base + index` or forcing the compiler to reason about
/// aliasing between parent writes and child-header reads.
#[derive(Clone, Copy)]
struct ReductionSubtreeSummary {
    padding: Length,
    size: Length,
    lookahead_bytes: u32,
    error_cost: u32,
    child_count: u32,
    visible_child_count: u32,
    named_child_count: u32,
    visible_descendant_count: u32,
    dynamic_precedence: i32,
    symbol: TSSymbol,
    visible: bool,
    named: bool,
    extra: bool,
    has_external_tokens: bool,
    has_external_scanner_state_change: bool,
}

#[inline]
unsafe fn subtree_resolve_for_reduction(
    tree: Subtree,
    arena: *mut SubtreeArena,
) -> ReductionSubtreeSummary {
    debug_assert!(!tree.is_null());
    if let Some(data) = tree.inline_data(arena) {
        let size_bytes = u32::from(data.size_bytes);
        ReductionSubtreeSummary {
            padding: Length {
                bytes: u32::from(data.padding_bytes),
                extent: TSPoint {
                    row: u32::from(data.padding_rows()),
                    column: u32::from(data.padding_columns),
                },
            },
            size: Length {
                bytes: size_bytes,
                extent: TSPoint {
                    row: 0,
                    column: size_bytes,
                },
            },
            lookahead_bytes: u32::from(data.lookahead_bytes()),
            error_cost: if data.is_missing() {
                ERROR_COST_PER_MISSING_TREE + ERROR_COST_PER_RECOVERY
            } else {
                0
            },
            child_count: 0,
            visible_child_count: 0,
            named_child_count: 0,
            visible_descendant_count: 0,
            dynamic_precedence: 0,
            symbol: TSSymbol::from(data.symbol),
            visible: data.visible(),
            named: data.named(),
            extra: data.extra(),
            has_external_tokens: false,
            has_external_scanner_state_change: false,
        }
    } else {
        let data = tree.heap_data(arena);
        let (visible_child_count, named_child_count, visible_descendant_count, dynamic_precedence) =
            if data.child_count == 0 {
                (0, 0, 0, 0)
            } else {
                let children = data.children();
                (
                    children.visible_child_count,
                    children.named_child_count,
                    children.visible_descendant_count,
                    children.dynamic_precedence,
                )
            };
        ReductionSubtreeSummary {
            padding: data.padding,
            size: data.size,
            lookahead_bytes: data.lookahead_bytes,
            error_cost: if data.is_missing() {
                ERROR_COST_PER_MISSING_TREE + ERROR_COST_PER_RECOVERY
            } else {
                data.error_cost
            },
            child_count: data.child_count,
            visible_child_count,
            named_child_count,
            visible_descendant_count,
            dynamic_precedence,
            symbol: data.symbol,
            visible: data.visible(),
            named: data.named(),
            extra: data.extra(),
            has_external_tokens: data.has_external_tokens(),
            has_external_scanner_state_change: data.has_external_scanner_state_change(),
        }
    }
}

unsafe fn subtree_set_has_changes(self_: &mut MutableSubtree, arena: *mut SubtreeArena) {
    if let Some(data) = self_.inline_data_mut(arena) {
        data.set_has_changes(true);
    } else {
        self_.heap_data_mut(arena).set_has_changes(true);
    }
}

// Subtree construction

#[allow(clippy::too_many_arguments)]
/// Create a leaf subtree.
///
/// Small leaves use an eight-byte compact arena record when the symbol,
/// padding, size, and lookahead byte counts fit its packed limits. Larger leaves
/// or leaves carrying external scanner state use `SubtreeHeapData` from the
/// parser's subtree pool. Both forms are addressed by a four-byte `Subtree`.
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
        let data = subtree_pool_allocate_inline(pool);
        data.as_ptr().write(SubtreeInlineData {
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
        });
        Subtree::from_inline(pool.arena(), data)
    } else {
        let data = subtree_pool_allocate(pool);
        let mut leaf_data = SubtreeLeafData {
            header: SubtreeHeapData {
                parser_shared: Cell::new(false),
                published_shared: AtomicBool::new(false),
                parser_visited: Cell::new(false),
                padding,
                size,
                lookahead_bytes,
                error_cost: 0,
                child_count: 0,
                symbol,
                parse_state,
                flags: 0,
            },
            content: if has_external_tokens {
                SubtreeLeafDataContent::ExternalScannerState(ExternalScannerState {
                    data: ExternalScannerStateData::Inline([0; EXTERNAL_SCANNER_STATE_INLINE_SIZE]),
                    length: 0,
                })
            } else {
                SubtreeLeafDataContent::LookaheadChar(0)
            },
        };
        leaf_data.header.set_visible(metadata.visible);
        leaf_data.header.set_named(metadata.named);
        leaf_data.header.set_extra(extra);
        leaf_data
            .header
            .set_has_external_tokens(has_external_tokens);
        leaf_data.header.set_depends_on_column(depends_on_column);
        leaf_data.header.set_is_keyword(is_keyword);
        data.cast::<SubtreeLeafData>().as_ptr().write(leaf_data);
        Subtree::from_heap(pool.arena(), data)
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
    result
        .into_mut()
        .heap_data_mut(pool.arena())
        .set_leaf_content(SubtreeLeafDataContent::LookaheadChar(lookahead_char));
    result
}

unsafe fn subtree_init_node_data(
    arena: *mut SubtreeArena,
    data: NonNull<SubtreeHeapData>,
    symbol: TSSymbol,
    child_count: u32,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    let metadata = ts_language_symbol_metadata(language, symbol);
    let mut internal_data = SubtreeInternalData {
        header: SubtreeHeapData {
            parser_shared: Cell::new(false),
            published_shared: AtomicBool::new(false),
            parser_visited: Cell::new(false),
            padding: length_zero(),
            size: length_zero(),
            lookahead_bytes: 0,
            error_cost: 0,
            child_count,
            symbol,
            parse_state: 0,
            flags: 0,
        },
        children: SubtreeChildrenData {
            visible_child_count: 0,
            named_child_count: 0,
            visible_descendant_count: 0,
            dynamic_precedence: 0,
            repeat_depth: 0,
            production_id: production_id as u16,
        },
    };
    internal_data.header.set_visible(metadata.visible);
    internal_data.header.set_named(metadata.named);
    internal_data.header.set_internal(true);
    data.cast::<SubtreeInternalData>()
        .as_ptr()
        .write(internal_data);
    MutableSubtree::from_heap(arena, data)
}

/// Create a heap internal node by taking ownership of its child array.
///
/// The child array is resized so the `SubtreeHeapData` header can live directly
/// after the child slice in one compact allocation:
/// `[child_0, child_1, ... child_n][SubtreeHeapData]`.
pub unsafe fn subtree_new_node(
    pool: &mut SubtreePool,
    symbol: TSSymbol,
    children: SubtreeArray,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    let (data, child_count) = subtree_take_children(pool, children);

    let arena = pool.arena();
    let result = subtree_init_node_data(arena, data, symbol, child_count, production_id, language);
    subtree_summarize_children(result, arena, language);
    result
}

/// Build a temporary internal node in parser-owned reusable storage.
///
/// The returned subtree borrows `children`'s allocation and must not be
/// retained or released. It becomes invalid as soon as `children` is changed.
pub(super) unsafe fn subtree_new_scratch_node(
    arena: *mut SubtreeArena,
    symbol: TSSymbol,
    children: &mut SubtreeArray,
    language: *const TSLanguage,
) -> Subtree {
    let data = subtree_reuse_children(children);
    let result = subtree_init_node_data(arena, data, symbol, children.size, 0, language);
    subtree_summarize_children(result, arena, language);
    result.into_immutable()
}

pub unsafe fn subtree_new_error_node(
    pool: &mut SubtreePool,
    children: SubtreeArray,
    extra: bool,
    language: *const TSLanguage,
) -> Subtree {
    let mut result = subtree_new_node(pool, TS_BUILTIN_SYM_ERROR, children, 0, language);
    result.heap_data_mut(pool.arena()).set_extra(extra);
    result.into_immutable()
}

pub unsafe fn subtree_new_missing_leaf(
    pool: &mut SubtreePool,
    symbol: TSSymbol,
    padding: Length,
    lookahead_bytes: u32,
    language: *const TSLanguage,
) -> Subtree {
    let result = subtree_new_leaf(
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
    let mut result = result.into_mut();
    if let Some(data) = result.inline_data_mut(pool.arena()) {
        data.set_is_missing(true);
    } else {
        result.heap_data_mut(pool.arena()).set_is_missing(true);
    }
    result.into_immutable()
}

// Subtree mutation and ownership

// Subtree balancing and summarization

/// Replace conservative parse-time sharing marks with exact accepted-DAG
/// sharing before final balancing.
pub(super) unsafe fn subtree_prepare_for_balancing(
    root: Subtree,
    arena: *mut SubtreeArena,
    stack: &mut MutableSubtreeArray,
) {
    debug_assert!(!storage::subtree_arena_is_published(arena));

    stack.clear();
    stack.push(root.into_mut());
    while !stack.is_empty() {
        let tree = stack.pop().into_immutable();
        if tree.is_null() || tree.is_inline() {
            continue;
        }
        let data = tree.heap_data(arena);
        if data.parser_visited() {
            data.mark_parser_shared();
            continue;
        }
        data.set_parser_visited(true);
        data.parser_shared.set(false);
        for &child in tree.children(arena) {
            stack.push(child.into_mut());
        }
    }
}

pub(super) unsafe fn subtree_publish(arena: *mut SubtreeArena) {
    storage::subtree_arena_publish(arena);
}

pub unsafe fn subtree_compress(
    self_: MutableSubtree,
    arena: *mut SubtreeArena,
    count: u32,
    language: *const TSLanguage,
    stack: &mut MutableSubtreeArray,
) {
    let initial_stack_size = stack.size;

    let mut tree = self_;
    let symbol = tree.heap_data(arena).symbol;
    for _ in 0..count {
        if tree.into_immutable().shared(arena) || tree.heap_data(arena).child_count < 2 {
            break;
        }

        let mut child = tree.child(arena, 0).into_mut();
        if child.is_inline()
            || child.heap_data(arena).child_count < 2
            || child.into_immutable().shared(arena)
            || child.heap_data(arena).symbol != symbol
        {
            break;
        }

        let mut grandchild = child.child(arena, 0).into_mut();
        if grandchild.is_inline()
            || grandchild.heap_data(arena).child_count < 2
            || grandchild.into_immutable().shared(arena)
            || grandchild.heap_data(arena).symbol != symbol
        {
            break;
        }

        // Rotate: tree[0] = grandchild, child[0] = grandchild[last], grandchild[last] = child
        let gc_last = grandchild.heap_data(arena).child_count as usize - 1;
        *tree.child_mut(arena, 0) = grandchild.into_immutable();
        *child.child_mut(arena, 0) = grandchild.child(arena, gc_last);
        *grandchild.child_mut(arena, gc_last) = child.into_immutable();
        stack.push(tree);
        tree = grandchild;
    }

    while stack.size > initial_stack_size {
        tree = stack.pop();
        let child = tree.child(arena, 0).into_mut();
        let grandchild = child
            .child(arena, child.heap_data(arena).child_count as usize - 1)
            .into_mut();
        subtree_summarize_children(grandchild, arena, language);
        subtree_summarize_children(child, arena, language);
        subtree_summarize_children(tree, arena, language);
    }
}

pub unsafe fn subtree_summarize_children(
    mut self_: MutableSubtree,
    arena: *mut SubtreeArena,
    language: *const TSLanguage,
) {
    debug_assert!(!self_.is_inline());

    let immutable_tree = self_.into_immutable();
    let children = immutable_tree.children(arena);
    let data = self_.heap_data_mut(arena);
    data.children_mut().named_child_count = 0;
    data.children_mut().visible_child_count = 0;
    data.error_cost = 0;
    data.children_mut().repeat_depth = 0;
    data.children_mut().visible_descendant_count = 0;
    data.set_has_external_tokens(false);
    data.set_has_external_scanner_state_change(false);
    data.children_mut().dynamic_precedence = 0;

    let mut structural_index: u32 = 0;
    let alias_sequence =
        language_alias_sequence_slice(language, u32::from(data.children().production_id));
    let mut lookahead_end_byte: u32 = 0;

    for (i, child) in children.iter().copied().enumerate() {
        let i = i as u32;
        let child = subtree_resolve_for_reduction(child, arena);

        if child.has_external_scanner_state_change {
            data.set_has_external_scanner_state_change(true);
        }

        if i == 0 {
            data.padding = child.padding;
            data.size = child.size;
        } else {
            data.size = length_add(data.size, length_add(child.padding, child.size));
        }

        let child_lookahead_end_byte = data.padding.bytes + data.size.bytes + child.lookahead_bytes;
        if child_lookahead_end_byte > lookahead_end_byte {
            lookahead_end_byte = child_lookahead_end_byte;
        }

        if child.symbol != TS_BUILTIN_SYM_ERROR_REPEAT {
            data.error_cost += child.error_cost;
        }

        let grandchild_count = child.child_count;
        if (data.symbol == TS_BUILTIN_SYM_ERROR || data.symbol == TS_BUILTIN_SYM_ERROR_REPEAT)
            && !child.extra
            && !(child.symbol == TS_BUILTIN_SYM_ERROR && grandchild_count == 0)
        {
            if child.visible {
                data.error_cost += ERROR_COST_PER_SKIPPED_TREE;
            } else if grandchild_count > 0 {
                data.error_cost += ERROR_COST_PER_SKIPPED_TREE * child.visible_child_count;
            }
        }

        data.children_mut().dynamic_precedence += child.dynamic_precedence;
        data.children_mut().visible_descendant_count += child.visible_descendant_count;

        let alias_symbol = alias_sequence
            .get(structural_index as usize)
            .copied()
            .unwrap_or(0);
        if !child.extra && child.symbol != 0 && alias_symbol != 0 {
            data.children_mut().visible_descendant_count += 1;
            data.children_mut().visible_child_count += 1;
            if ts_language_symbol_metadata(language, alias_symbol).named {
                data.children_mut().named_child_count += 1;
            }
        } else if child.visible {
            data.children_mut().visible_descendant_count += 1;
            data.children_mut().visible_child_count += 1;
            if child.named {
                data.children_mut().named_child_count += 1;
            }
        } else if grandchild_count > 0 {
            data.children_mut().visible_child_count += child.visible_child_count;
            data.children_mut().named_child_count += child.named_child_count;
        }

        if child.has_external_tokens {
            data.set_has_external_tokens(true);
        }

        if child.symbol == TS_BUILTIN_SYM_ERROR {
            data.parse_state = TS_TREE_STATE_NONE;
        }

        if !child.extra {
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
            && first_child.symbol(arena) == data.symbol
        {
            if first_child.repeat_depth(arena) > last_child.repeat_depth(arena) {
                data.children_mut().repeat_depth = (first_child.repeat_depth(arena) + 1) as u16;
            } else {
                data.children_mut().repeat_depth = (last_child.repeat_depth(arena) + 1) as u16;
            }
        }
    }
}

// Subtree comparison and editing

pub unsafe fn subtree_compare(left: Subtree, right: Subtree, pool: &mut SubtreePool) -> i32 {
    let arena = pool.arena();
    pool.tree_stack.push(left.into_mut());
    pool.tree_stack.push(right.into_mut());

    while pool.tree_stack.size > 0 {
        let right = pool.tree_stack.pop().into_immutable();
        let left = pool.tree_stack.pop().into_immutable();

        let left_symbol = left.symbol(arena);
        let right_symbol = right.symbol(arena);
        let left_child_count = left.child_count(arena);
        let right_child_count = right.child_count(arena);

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

        let left_children = left.children(arena);
        let right_children = right.children(arena);
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
    use core::ptr;

    use super::*;
    use crate::ffi::{TSInputEdit, TSPoint};

    unsafe fn inline_leaf(pool: &mut SubtreePool, size: u8) -> Subtree {
        let data = subtree_pool_allocate_inline(pool);
        data.as_ptr().write(SubtreeInlineData {
            flags: INLINE_IS_INLINE | INLINE_VISIBLE | INLINE_NAMED,
            symbol: 1,
            parse_state: 1,
            padding_columns: 0,
            rows_and_lookahead: 0,
            padding_bytes: 0,
            size_bytes: size,
        });
        Subtree::from_inline(pool.arena(), data)
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
            let leaf = inline_leaf(&mut pool, 5);
            let tree = subtree_edit(leaf, &insertion(2, 1), &mut pool);
            let arena = pool.arena();

            assert!(tree.is_inline());
            assert_eq!(tree.size(arena).bytes, 6);
            assert!(tree.has_changes(arena));

            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn edit_promotes_leaf_that_no_longer_fits_inline() {
        unsafe {
            let mut pool = subtree_pool_new(4);
            let leaf = inline_leaf(&mut pool, 250);
            let tree = subtree_edit(leaf, &insertion(2, 10), &mut pool);
            let arena = pool.arena();

            assert!(!tree.is_inline());
            assert_eq!(tree.size(arena).bytes, 260);
            assert!(tree.has_changes(arena));

            tree.release(&mut pool);
            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn edit_marks_the_affected_child() {
        unsafe {
            let mut pool = subtree_pool_new(4);
            let mut children = SubtreeArray::new();
            children.push(inline_leaf(&mut pool, 5));
            children.push(inline_leaf(&mut pool, 5));
            let parent =
                subtree_new_node(&mut pool, TS_BUILTIN_SYM_ERROR, children, 0, ptr::null())
                    .into_immutable();

            let tree = subtree_edit(parent, &insertion(2, 1), &mut pool);
            let arena = pool.arena();
            assert!(tree.has_changes(arena));
            assert!((*tree.child(arena, 0)).has_changes(arena));
            assert!(!(*tree.child(arena, 1)).has_changes(arena));

            tree.release(&mut pool);
            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn scratch_node_borrows_reusable_child_storage() {
        unsafe {
            let mut pool = subtree_pool_new(0);
            let mut children = subtree_array_new(&mut pool);
            let arena = pool.arena();
            children.push(inline_leaf(&mut pool, 2));
            children.push(inline_leaf(&mut pool, 3));

            let first =
                subtree_new_scratch_node(arena, TS_BUILTIN_SYM_ERROR, &mut children, ptr::null());
            assert_eq!(first.child_count(arena), 2);

            children.clear();
            children.push(inline_leaf(&mut pool, 4));
            let second =
                subtree_new_scratch_node(arena, TS_BUILTIN_SYM_ERROR, &mut children, ptr::null());
            assert_eq!(second.child_count(arena), 1);
            assert_eq!(second.child(arena, 0).size(arena).bytes, 4);

            children.delete();
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

            let mut children = SubtreeArray::new();
            children.push(child1);
            children.push(child2);

            let parent = subtree_new_node(
                &mut pool,
                TS_BUILTIN_SYM_ERROR_REPEAT,
                children,
                0,
                ptr::null(),
            );
            let parent_tree = parent.into_immutable();
            let arena = pool.arena();

            assert_eq!(parent_tree.child_count(arena), 2);
            assert_eq!(parent_tree.children(arena).len(), 2);
            assert_eq!(
                parent_tree.children(arena)[0].symbol(arena),
                TS_BUILTIN_SYM_ERROR
            );
            assert_eq!(
                parent_tree.children(arena)[1].symbol(arena),
                TS_BUILTIN_SYM_ERROR
            );

            parent_tree.release(&mut pool);
            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn exact_sharing_does_not_imply_shared_descendants() {
        unsafe {
            let mut pool = subtree_pool_new(0);
            let mut descendant_children = SubtreeArray::new();
            descendant_children.push(inline_leaf(&mut pool, 2));
            let descendant = subtree_new_node(
                &mut pool,
                TS_BUILTIN_SYM_ERROR_REPEAT,
                descendant_children,
                0,
                ptr::null(),
            )
            .into_immutable();

            let mut shared_children = SubtreeArray::new();
            shared_children.push(descendant);
            let shared = subtree_new_node(
                &mut pool,
                TS_BUILTIN_SYM_ERROR_REPEAT,
                shared_children,
                0,
                ptr::null(),
            )
            .into_immutable();
            shared.retain(pool.arena());

            let mut root_children = SubtreeArray::new();
            root_children.push(shared);
            root_children.push(shared);
            let root = subtree_new_node(
                &mut pool,
                TS_BUILTIN_SYM_ERROR_REPEAT,
                root_children,
                0,
                ptr::null(),
            )
            .into_immutable();

            let mut traversal = MutableSubtreeArray::new();
            subtree_prepare_for_balancing(root, pool.arena(), &mut traversal);

            // The shared parent owns one physical child edge even though the
            // parent itself is reachable twice. Balancing must therefore skip
            // the whole shared subgraph instead of treating this child's own
            // exact-sharing flag as permission to mutate it.
            assert!(shared.shared(pool.arena()));
            assert!(!descendant.shared(pool.arena()));

            traversal.delete();
            root.release(&mut pool);
            subtree_pool_delete(&mut pool);
        }
    }

    #[test]
    fn released_heap_nodes_remain_in_the_slab_until_rewind() {
        unsafe {
            let mut pool = subtree_pool_new(0);
            let first = subtree_new_error(
                &mut pool,
                b'a' as i32,
                length_zero(),
                length_zero(),
                0,
                0,
                ptr::null(),
            );
            let arena = pool.arena();
            let first_address = core::ptr::from_ref(first.heap_data(arena)).cast::<u8>();
            assert!(storage::subtree_pool_contains(&pool, first_address));
            let used_after_first = storage::subtree_pool_used_bytes(&pool);

            first.release(&mut pool);
            assert_eq!(storage::subtree_pool_used_bytes(&pool), used_after_first);

            let second = subtree_new_error(
                &mut pool,
                b'b' as i32,
                length_zero(),
                length_zero(),
                0,
                0,
                ptr::null(),
            );
            let second_address = core::ptr::from_ref(second.heap_data(arena)).cast::<u8>();
            assert!(storage::subtree_pool_contains(&pool, second_address));
            assert_ne!(first_address, second_address);
            assert!(storage::subtree_pool_used_bytes(&pool) > used_after_first);

            second.release(&mut pool);
            subtree_pool_prepare_for_parse(&mut pool);
            assert_eq!(storage::subtree_pool_used_bytes(&pool), 0);
            subtree_pool_delete(&mut pool);
        }
    }
}
