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

use core::{ptr, ptr::NonNull, sync::atomic::AtomicU32};

use crate::ffi::{TSInputEdit, TSLanguage, TSStateId, TSSymbol};

use super::error_costs::{
    ERROR_COST_PER_RECOVERY, ERROR_COST_PER_SKIPPED_CHAR, ERROR_COST_PER_SKIPPED_LINE,
    ERROR_COST_PER_SKIPPED_TREE,
};
use super::language::{
    language_alias_sequence_slice, language_field_map_slice, language_full,
    language_write_symbol_as_dot_string, ts_language_symbol_metadata, ts_language_symbol_name,
};
use super::length::{length_add, length_saturating_sub, length_sub, length_zero, Length};
use super::utils::Array;

mod data;
use data::{
    ExternalScannerState, ExternalScannerStateData, SubtreeChildrenData, SubtreeHeapData,
    SubtreeHeapDataContent, SubtreeInlineData, EXTERNAL_SCANNER_STATE_INLINE_SIZE, INLINE_EXTRA,
    INLINE_IS_INLINE, INLINE_IS_KEYWORD, INLINE_NAMED, INLINE_VISIBLE,
};

mod handle;
pub use handle::{MutableSubtree, Subtree, NULL_SUBTREE};

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
use storage::{subtree_pool_allocate, subtree_reuse_children, subtree_take_children};

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
        let mut heap_data = SubtreeHeapData {
            ref_count: AtomicU32::new(1),
            padding,
            size,
            lookahead_bytes,
            error_cost: 0,
            child_count: 0,
            symbol,
            parse_state,
            flags: 0,
            data: if has_external_tokens {
                SubtreeHeapDataContent::ExternalScannerState(ExternalScannerState {
                    data: ExternalScannerStateData::Inline([0; EXTERNAL_SCANNER_STATE_INLINE_SIZE]),
                    length: 0,
                })
            } else {
                SubtreeHeapDataContent::LookaheadChar(0)
            },
        };
        heap_data.set_visible(metadata.visible);
        heap_data.set_named(metadata.named);
        heap_data.set_extra(extra);
        heap_data.set_has_external_tokens(has_external_tokens);
        heap_data.set_depends_on_column(depends_on_column);
        heap_data.set_is_keyword(is_keyword);
        data.as_ptr().write(heap_data);
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
    let mut heap_data = SubtreeHeapData {
        ref_count: AtomicU32::new(1),
        padding: length_zero(),
        size: length_zero(),
        lookahead_bytes: 0,
        error_cost: 0,
        child_count,
        symbol,
        parse_state: 0,
        flags: 0,
        data: SubtreeHeapDataContent::Children(SubtreeChildrenData {
            visible_child_count: 0,
            named_child_count: 0,
            visible_descendant_count: 0,
            dynamic_precedence: 0,
            repeat_depth: 0,
            production_id: production_id as u16,
        }),
    };
    heap_data.set_visible(metadata.visible);
    heap_data.set_named(metadata.named);
    data.as_ptr().write(heap_data);
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
    let mut result = subtree_new_node(TS_BUILTIN_SYM_ERROR, children, 0, language);
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

        let mut child = tree.child(0).into_mut();
        if child.is_inline()
            || child.heap_data().child_count < 2
            || child.heap_data().ref_count() > 1
            || child.heap_data().symbol != symbol
        {
            break;
        }

        let mut grandchild = child.child(0).into_mut();
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

pub unsafe fn subtree_summarize_children(mut self_: MutableSubtree, language: *const TSLanguage) {
    debug_assert!(!self_.is_inline());

    let immutable_tree = self_.into_immutable();
    let children = immutable_tree.children();
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
    use crate::ffi::TSPoint;

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
