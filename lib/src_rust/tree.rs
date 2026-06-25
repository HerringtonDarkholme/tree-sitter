#![allow(dead_code)]
#![allow(non_snake_case)]

use core::ffi::c_void;

use crate::ffi::{TSLanguage, TSNode, TSPoint, TSRange, TSSymbol};

use super::alloc::{ts_calloc, ts_free, ts_malloc};
use super::get_changed_ranges::{
    ts_range_array_get_changed_ranges_ref, ts_range_edit_ref, ts_subtree_get_changed_ranges_ref,
    TSRangeArray,
};
use super::language::{ts_language_copy, ts_language_delete};
use super::length::{length_add, Length};
use super::node::ts_node_new;
use super::subtree::{
    ts_subtree_edit, ts_subtree_padding, ts_subtree_pool_delete, ts_subtree_pool_new,
    ts_subtree_print_dot_graph, ts_subtree_release, ts_subtree_retain, Subtree,
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

/// `ParentCacheEntry` — used for parent lookups (defined in tree.h)
#[repr(C)]
pub struct ParentCacheEntry {
    pub child: *const Subtree,
    pub parent: *const Subtree,
    pub position: Length,
    pub alias_symbol: TSSymbol,
}

/// `TSTree` — the main tree struct (defined in tree.h)
#[repr(C)]
pub struct TSTree {
    pub root: Subtree,
    pub language: *const TSLanguage,
    pub included_ranges: *mut TSRange,
    pub included_range_count: u32,
}

#[inline]
const unsafe fn tree_ref<'a>(tree: *const TSTree) -> &'a TSTree {
    &*tree
}

#[inline]
unsafe fn tree_mut<'a>(tree: *mut TSTree) -> &'a mut TSTree {
    &mut *tree
}

unsafe fn ts_tree_init_ref(
    tree: &mut TSTree,
    root: Subtree,
    language: *const TSLanguage,
    included_ranges: *const TSRange,
    included_range_count: u32,
) {
    tree.root = root;
    tree.language = ts_language_copy(language);
    tree.included_ranges =
        ts_calloc(included_range_count as usize, std::mem::size_of::<TSRange>()).cast::<TSRange>();
    if included_range_count > 0 {
        std::ptr::copy_nonoverlapping(
            included_ranges,
            tree.included_ranges,
            included_range_count as usize,
        );
    }
    tree.included_range_count = included_range_count;
}

unsafe fn ts_tree_copy_ref(tree: &TSTree) -> *mut TSTree {
    ts_subtree_retain(tree.root);
    ts_tree_new(
        tree.root,
        tree.language,
        tree.included_ranges,
        tree.included_range_count,
    )
}

unsafe fn ts_tree_delete_ref(tree: &mut TSTree) {
    let mut pool = ts_subtree_pool_new(0);
    ts_subtree_release(&mut pool, tree.root);
    ts_subtree_pool_delete(&mut pool);
    ts_language_delete(tree.language);
    ts_free(tree.included_ranges.cast::<c_void>());
}

unsafe fn ts_tree_root_node_ref(tree_ptr: *const TSTree, tree: &TSTree) -> TSNode {
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

const fn ts_tree_language_ref(tree: &TSTree) -> *const TSLanguage {
    tree.language
}

unsafe fn ts_tree_included_ranges_ref(tree: &TSTree, length: &mut u32) -> *mut TSRange {
    *length = tree.included_range_count;
    let ranges =
        ts_calloc(tree.included_range_count as usize, std::mem::size_of::<TSRange>()).cast::<TSRange>();
    if tree.included_range_count > 0 {
        std::ptr::copy_nonoverlapping(
            tree.included_ranges,
            ranges,
            tree.included_range_count as usize,
        );
    }
    ranges
}

#[inline]
unsafe fn tree_included_range_mut(tree: &mut TSTree, index: u32) -> &mut TSRange {
    &mut *tree.included_ranges.add(index as usize)
}

fn tree_cursor_empty() -> TreeCursor {
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

unsafe fn ts_tree_edit_ref(tree: &mut TSTree, edit: &TSInputEdit) {
    for i in 0..tree.included_range_count {
        ts_range_edit_ref(tree_included_range_mut(tree, i), edit);
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

#[no_mangle]
pub unsafe extern "C" fn ts_tree_new(
    root: Subtree,
    language: *const TSLanguage,
    included_ranges: *const TSRange,
    included_range_count: u32,
) -> *mut TSTree {
    let result = ts_malloc(std::mem::size_of::<TSTree>()).cast::<TSTree>();
    let tree = tree_mut(result);
    ts_tree_init_ref(tree, root, language, included_ranges, included_range_count);
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_copy(self_: *const TSTree) -> *mut TSTree {
    let tree = tree_ref(self_);
    ts_tree_copy_ref(tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_delete(self_: *mut TSTree) {
    if self_.is_null() {
        return;
    }
    let tree = tree_mut(self_);
    ts_tree_delete_ref(tree);
    ts_free(self_.cast::<c_void>());
}

// ---------------------------------------------------------------------------
// Accessors: ts_tree_root_node, ts_tree_root_node_with_offset,
//            ts_tree_language, ts_tree_included_ranges
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node(self_: *const TSTree) -> TSNode {
    let tree = tree_ref(self_);
    ts_tree_root_node_ref(self_, tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node_with_offset(
    self_: *const TSTree,
    offset_bytes: u32,
    offset_extent: TSPoint,
) -> TSNode {
    let tree = tree_ref(self_);
    ts_tree_root_node_with_offset_ref(self_, tree, offset_bytes, offset_extent)
}

#[no_mangle]
pub const unsafe extern "C" fn ts_tree_language(self_: *const TSTree) -> *const TSLanguage {
    let tree = tree_ref(self_);
    ts_tree_language_ref(tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_included_ranges(
    self_: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    let tree = tree_ref(self_);
    let length = &mut *length;
    ts_tree_included_ranges_ref(tree, length)
}

// ---------------------------------------------------------------------------
// Mutation & diagnostics: ts_tree_edit, ts_tree_get_changed_ranges,
//                         _ts_dup, ts_tree_print_dot_graph
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_edit(self_: *mut TSTree, edit: *const TSInputEdit) {
    let tree = tree_mut(self_);
    let edit = &*edit;
    ts_tree_edit_ref(tree, edit);
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_get_changed_ranges(
    old_tree: *const TSTree,
    new_tree: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    let old_tree_ref = tree_ref(old_tree);
    let new_tree_ref = tree_ref(new_tree);
    let mut cursor1 = tree_cursor_empty();
    let mut cursor2 = tree_cursor_empty();
    ts_tree_cursor_init_ref(&mut cursor1, ts_tree_root_node_ref(old_tree, old_tree_ref));
    ts_tree_cursor_init_ref(&mut cursor2, ts_tree_root_node_ref(new_tree, new_tree_ref));

    let mut included_range_differences = TSRangeArray {
        contents: std::ptr::null_mut(),
        size: 0,
        capacity: 0,
    };
    ts_range_array_get_changed_ranges_ref(
        old_tree_ref.included_ranges,
        old_tree_ref.included_range_count,
        new_tree_ref.included_ranges,
        new_tree_ref.included_range_count,
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
    let tree = tree_ref(self_);
    ts_tree_print_dot_graph_ref(tree, file_descriptor);
}

#[cfg(target_family = "wasm")]
#[no_mangle]
pub unsafe extern "C" fn ts_tree_print_dot_graph(self_: *const TSTree, file_descriptor: i32) {
    let _ = self_;
    let _ = file_descriptor;
}
