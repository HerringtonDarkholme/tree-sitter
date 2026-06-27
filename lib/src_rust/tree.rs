#![allow(dead_code)]
#![allow(non_snake_case)]

use core::ffi::c_void;

use crate::ffi::{TSLanguage, TSNode, TSPoint, TSRange, TSSymbol};

use super::alloc::{ts_calloc, ts_free, ts_malloc};
use super::get_changed_ranges::{
    ts_range_array_get_changed_ranges_ref, ts_range_edit_ref, ts_range_slice,
    ts_subtree_get_changed_ranges_ref, TSRangeArray,
};
use super::language::{ts_language_copy, ts_language_delete};
use super::length::{length_add, Length};
use super::node::ts_node_new;
use super::subtree::{
    ts_subtree_edit, ts_subtree_padding, ts_subtree_pool_delete, ts_subtree_pool_new,
    ts_subtree_print_dot_graph, ts_subtree_release, ts_subtree_retain, ts_tree_arena_release,
    ts_tree_arena_retain, Subtree, TreeArena,
};
use super::tree_cursor::{ts_tree_cursor_init_ref, TreeCursor, TreeCursorEntryArray};

// ---------------------------------------------------------------------------
// Extern C functions (still in C or other Rust modules)
// ---------------------------------------------------------------------------

extern "C" {
    #[cfg(not(target_os = "windows"))]
    fn dup(fd: i32) -> i32;
    #[cfg(not(target_os = "windows"))]
    fn fdopen(fd: i32, mode: *const i8) -> *mut c_void;
    #[cfg(not(target_os = "windows"))]
    fn fclose(f: *mut c_void) -> i32;
}

use crate::ffi::TSInputEdit;

// ---------------------------------------------------------------------------
// Types from tree.h
// ---------------------------------------------------------------------------

/// Cached parent lookup entry used by node APIs.
///
/// Trees do not store parent pointers in every subtree. Parent lookups walk the
/// tree and can populate this small cache with the child pointer, its parent,
/// the child's start position, and the alias visible at that child.
#[repr(C)]
pub struct ParentCacheEntry {
    /// Child subtree pointer that was searched.
    pub child: *const Subtree,
    /// Parent subtree containing `child`.
    pub parent: *const Subtree,
    /// Start position of `child`.
    pub position: Length,
    /// Alias symbol applied to `child`, or zero when none.
    pub alias_symbol: TSSymbol,
}

/// Owned parse tree returned by the parser.
///
/// The tree retains the root subtree, a copied language reference, the included
/// ranges used for parsing, and optionally the arena that owns internal nodes
/// created during the Rust parser's normal parse path.
#[repr(C)]
pub struct TSTree {
    /// Root syntax subtree, retained by the tree.
    pub root: Subtree,
    /// Language used to parse this tree.
    pub language: *const TSLanguage,
    /// Copied included ranges for incremental diffing and public APIs.
    pub included_ranges: *mut TSRange,
    /// Number of entries in `included_ranges`.
    pub included_range_count: u32,
    /// Shared arena for arena-owned internal nodes.
    pub arena: *mut TreeArena,
}

unsafe fn ts_tree_init_ref(
    tree: &mut TSTree,
    root: Subtree,
    language: *const TSLanguage,
    included_ranges: &[TSRange],
    arena: *mut TreeArena,
) {
    tree.root = root;
    tree.language = ts_language_copy(language);
    tree.included_range_count = included_ranges.len() as u32;
    tree.arena = arena;
    tree.included_ranges =
        ts_calloc(included_ranges.len(), std::mem::size_of::<TSRange>()).cast::<TSRange>();
    if !included_ranges.is_empty() {
        std::ptr::copy_nonoverlapping(
            included_ranges.as_ptr(),
            tree.included_ranges,
            included_ranges.len(),
        );
    }
}

/// Copy a tree by retaining shared immutable storage.
///
/// Subtrees and arenas are reference counted, so copying a tree is cheap and
/// does not clone the entire syntax graph.
unsafe fn ts_tree_copy_ref(tree: &TSTree) -> *mut TSTree {
    ts_subtree_retain(tree.root);
    ts_tree_arena_retain(tree.arena);
    ts_tree_new_with_arena(
        tree.root,
        tree.language,
        tree.included_ranges,
        tree.included_range_count,
        tree.arena,
    )
}

/// Release all owned references and buffers for a tree.
unsafe fn ts_tree_delete_ref(tree: &mut TSTree) {
    let mut pool = ts_subtree_pool_new(0);
    ts_subtree_release(&mut pool, tree.root);
    ts_subtree_pool_delete(&mut pool);
    ts_tree_arena_release(tree.arena);
    ts_language_delete(tree.language);
    ts_free(tree.included_ranges.cast::<c_void>());
}

pub unsafe fn ts_tree_root_node_ref(tree_ptr: *const TSTree, tree: &TSTree) -> TSNode {
    ts_node_new(tree_ptr, &tree.root, ts_subtree_padding(tree.root), 0)
}

unsafe fn ts_tree_root_node_with_offset_ref(
    tree_ptr: *const TSTree,
    tree: &TSTree,
    offset_bytes: u32,
    offset_extent: TSPoint,
) -> TSNode {
    let offset = Length {
        bytes: offset_bytes,
        extent: offset_extent,
    };
    ts_node_new(
        tree_ptr,
        &tree.root,
        length_add(offset, ts_subtree_padding(tree.root)),
        0,
    )
}

unsafe fn ts_tree_included_ranges_ref(tree: &TSTree, length: &mut u32) -> *mut TSRange {
    *length = tree.included_range_count;
    let ranges = ts_calloc(
        tree.included_range_count as usize,
        std::mem::size_of::<TSRange>(),
    )
    .cast::<TSRange>();
    if tree.included_range_count > 0 {
        std::ptr::copy_nonoverlapping(
            tree.included_ranges,
            ranges,
            tree.included_range_count as usize,
        );
    }
    ranges
}

const fn tree_cursor_empty() -> TreeCursor {
    TreeCursor {
        tree: std::ptr::null(),
        stack: TreeCursorEntryArray {
            contents: std::ptr::null_mut(),
            size: 0,
            capacity: 0,
        },
        root_alias_symbol: 0,
    }
}

/// Apply an edit to the tree's ranges and root subtree.
///
/// The edit rewrites byte/point positions in-place where possible and marks
/// affected subtrees as changed so an incremental parse can decide what to
/// reuse.
unsafe fn ts_tree_edit_ref(tree: &mut TSTree, edit: &TSInputEdit) {
    let included_ranges = if tree.included_range_count == 0 {
        &mut []
    } else {
        std::slice::from_raw_parts_mut(tree.included_ranges, tree.included_range_count as usize)
    };
    for range in included_ranges {
        ts_range_edit_ref(range, edit);
    }
    let mut pool = ts_subtree_pool_new(0);
    tree.root = ts_subtree_edit(tree.root, edit, &mut pool);
    ts_subtree_pool_delete(&mut pool);
}

#[cfg(not(target_family = "wasm"))]
unsafe fn ts_tree_print_dot_graph_ref(tree: &TSTree, file_descriptor: i32) {
    let file = fdopen(_ts_dup(file_descriptor), c"a".as_ptr().cast::<i8>());
    ts_subtree_print_dot_graph(tree.root, tree.language, file);
    fclose(file);
}

// ---------------------------------------------------------------------------
// Lifecycle: ts_tree_new, ts_tree_copy, ts_tree_delete
// ---------------------------------------------------------------------------

pub unsafe fn ts_tree_new(
    root: Subtree,
    language: *const TSLanguage,
    included_ranges: *const TSRange,
    included_range_count: u32,
) -> *mut TSTree {
    ts_tree_new_with_arena(
        root,
        language,
        included_ranges,
        included_range_count,
        std::ptr::null_mut(),
    )
}

pub unsafe fn ts_tree_new_with_arena(
    root: Subtree,
    language: *const TSLanguage,
    included_ranges: *const TSRange,
    included_range_count: u32,
    arena: *mut TreeArena,
) -> *mut TSTree {
    let result = ts_malloc(std::mem::size_of::<TSTree>()).cast::<TSTree>();
    let tree = result.as_mut().unwrap_unchecked();
    let included_ranges = ts_range_slice(included_ranges, included_range_count);
    ts_tree_init_ref(tree, root, language, included_ranges, arena);
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_copy(self_: *const TSTree) -> *mut TSTree {
    let tree = self_.as_ref().unwrap_unchecked();
    ts_tree_copy_ref(tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_delete(self_: *mut TSTree) {
    if self_.is_null() {
        return;
    }
    let tree = self_.as_mut().unwrap_unchecked();
    ts_tree_delete_ref(tree);
    ts_free(self_.cast::<c_void>());
}

// ---------------------------------------------------------------------------
// Accessors: ts_tree_root_node, ts_tree_root_node_with_offset,
//            ts_tree_language, ts_tree_included_ranges
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node(self_: *const TSTree) -> TSNode {
    let tree = self_.as_ref().unwrap_unchecked();
    ts_tree_root_node_ref(self_, tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node_with_offset(
    self_: *const TSTree,
    offset_bytes: u32,
    offset_extent: TSPoint,
) -> TSNode {
    let tree = self_.as_ref().unwrap_unchecked();
    ts_tree_root_node_with_offset_ref(self_, tree, offset_bytes, offset_extent)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_language(self_: *const TSTree) -> *const TSLanguage {
    let tree = self_.as_ref().unwrap_unchecked();
    tree.language
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_included_ranges(
    self_: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    let tree = self_.as_ref().unwrap_unchecked();
    let length = length.as_mut().unwrap_unchecked();
    ts_tree_included_ranges_ref(tree, length)
}

// ---------------------------------------------------------------------------
// Mutation & diagnostics: ts_tree_edit, ts_tree_get_changed_ranges,
//                         _ts_dup, ts_tree_print_dot_graph
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_edit(self_: *mut TSTree, edit: *const TSInputEdit) {
    let tree = self_.as_mut().unwrap_unchecked();
    let edit = edit.as_ref().unwrap_unchecked();
    ts_tree_edit_ref(tree, edit);
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_get_changed_ranges(
    old_tree: *const TSTree,
    new_tree: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    let old_tree_ref = old_tree.as_ref().unwrap_unchecked();
    let new_tree_ref = new_tree.as_ref().unwrap_unchecked();
    let length = length.as_mut().unwrap_unchecked();
    let mut cursor1 = tree_cursor_empty();
    let mut cursor2 = tree_cursor_empty();
    ts_tree_cursor_init_ref(&mut cursor1, ts_tree_root_node_ref(old_tree, old_tree_ref));
    ts_tree_cursor_init_ref(&mut cursor2, ts_tree_root_node_ref(new_tree, new_tree_ref));

    let mut included_range_differences = TSRangeArray {
        contents: std::ptr::null_mut(),
        size: 0,
        capacity: 0,
    };
    let old_included_ranges = ts_range_slice(
        old_tree_ref.included_ranges,
        old_tree_ref.included_range_count,
    );
    let new_included_ranges = ts_range_slice(
        new_tree_ref.included_ranges,
        new_tree_ref.included_range_count,
    );
    ts_range_array_get_changed_ranges_ref(
        old_included_ranges,
        new_included_ranges,
        &mut included_range_differences,
    );

    let mut result: *mut TSRange = std::ptr::null_mut();
    *length = ts_subtree_get_changed_ranges_ref(
        &old_tree_ref.root,
        &new_tree_ref.root,
        &mut cursor1,
        &mut cursor2,
        old_tree_ref.language,
        &included_range_differences,
        &mut result,
    );

    // array_delete for included_range_differences
    if !included_range_differences.contents.is_null() {
        ts_free(included_range_differences.contents.cast::<c_void>());
    }
    // array_delete for cursor stacks
    if !cursor1.stack.contents.is_null() {
        ts_free(cursor1.stack.contents.cast::<c_void>());
    }
    if !cursor2.stack.contents.is_null() {
        ts_free(cursor2.stack.contents.cast::<c_void>());
    }

    result
}

#[cfg(not(any(target_os = "windows", target_family = "wasm")))]
#[no_mangle]
pub unsafe extern "C" fn _ts_dup(file_descriptor: i32) -> i32 {
    dup(file_descriptor)
}

#[cfg(not(target_family = "wasm"))]
#[no_mangle]
pub unsafe extern "C" fn ts_tree_print_dot_graph(self_: *const TSTree, file_descriptor: i32) {
    let tree = self_.as_ref().unwrap_unchecked();
    ts_tree_print_dot_graph_ref(tree, file_descriptor);
}

#[cfg(target_family = "wasm")]
#[no_mangle]
pub unsafe extern "C" fn ts_tree_print_dot_graph(self_: *const TSTree, file_descriptor: i32) {
    let _ = self_;
    let _ = file_descriptor;
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::*;
    use crate::core_impl::length::length_zero;
    use crate::core_impl::subtree::{
        ts_builtin_sym_error_repeat, ts_subtree_child_count, ts_subtree_from_mut,
        ts_subtree_new_error, ts_subtree_new_node_in_arena, ts_tree_arena_new,
    };

    #[test]
    fn arena_tree_copy_delete_uses_tree_arena_lifetime() {
        unsafe {
            let mut pool = ts_subtree_pool_new(0);
            let child1 = ts_subtree_new_error(
                &mut pool,
                b'a' as i32,
                length_zero(),
                length_zero(),
                0,
                0,
                ptr::null(),
            );
            let child2 = ts_subtree_new_error(
                &mut pool,
                b'b' as i32,
                length_zero(),
                length_zero(),
                0,
                0,
                ptr::null(),
            );
            let children = [child1, child2];

            let arena = ts_tree_arena_new();
            let root = ts_subtree_from_mut(ts_subtree_new_node_in_arena(
                arena,
                ts_builtin_sym_error_repeat,
                children.as_ptr(),
                children.len() as u32,
                0,
                ptr::null(),
            ));

            assert_eq!(ts_subtree_child_count(root), 2);
            let tree = ts_tree_new_with_arena(root, ptr::null(), ptr::null(), 0, arena);
            let copy = ts_tree_copy(tree);

            assert_eq!((*tree).arena, arena);
            assert_eq!((*copy).arena, arena);
            ts_tree_delete(tree);
            ts_tree_delete(copy);
            ts_subtree_pool_delete(&mut pool);
        }
    }
}
