#![allow(dead_code)]
#![allow(non_snake_case)]

use core::ffi::c_void;

use crate::ffi::{TSLanguage, TSNode, TSPoint, TSRange, TSSymbol};

use super::alloc::{ts_calloc, ts_free, ts_malloc};
use super::length::{length_add, Length};
use super::subtree::{
    ts_subtree_edit, ts_subtree_padding, ts_subtree_pool_delete, ts_subtree_pool_new,
    ts_subtree_print_dot_graph, ts_subtree_release, ts_subtree_retain, Subtree,
};

// ---------------------------------------------------------------------------
// Extern C functions (still in C or other Rust modules)
// ---------------------------------------------------------------------------

extern "C" {
    // language.rs
    fn ts_language_copy(self_: *const TSLanguage) -> *const TSLanguage;
    fn ts_language_delete(self_: *const TSLanguage);

    // get_changed_ranges.c (still in C)
    fn ts_range_edit(range: *mut TSRange, edit: *const TSInputEdit);
    fn ts_range_array_get_changed_ranges(
        old_ranges: *const TSRange,
        old_range_count: u32,
        new_ranges: *const TSRange,
        new_range_count: u32,
        differences: *mut TSRangeArray,
    );
    fn ts_subtree_get_changed_ranges(
        old_tree: *const Subtree,
        new_tree: *const Subtree,
        cursor1: *mut TreeCursor,
        cursor2: *mut TreeCursor,
        language: *const TSLanguage,
        included_range_differences: *const TSRangeArray,
        ranges: *mut *mut TSRange,
    ) -> u32;

    // tree_cursor.c (still in C)
    fn ts_tree_cursor_init(self_: *mut TreeCursor, node: TSNode);

    // node.c (still in C)
    fn ts_node_new(
        tree: *const TSTree,
        subtree: *const Subtree,
        position: Length,
        alias: TSSymbol,
    ) -> TSNode;

    fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;

    #[cfg(not(target_os = "windows"))]
    fn dup(fd: i32) -> i32;
    #[cfg(not(target_os = "windows"))]
    fn fdopen(fd: i32, mode: *const i8) -> *mut c_void;
    #[cfg(not(target_os = "windows"))]
    fn fclose(f: *mut c_void) -> i32;
}

use crate::ffi::TSInputEdit;

// ---------------------------------------------------------------------------
// Forward-declared types from other modules (not yet rewritten)
// ---------------------------------------------------------------------------

/// TreeCursorEntry — mirrors tree_cursor.h
#[repr(C)]
pub struct TreeCursorEntry {
    pub subtree: *const Subtree,
    pub position: Length,
    pub child_index: u32,
    pub structural_child_index: u32,
    pub descendant_index: u32,
}

/// Array(TreeCursorEntry) — mirrors array.h generic
#[repr(C)]
pub struct TreeCursorEntryArray {
    pub contents: *mut TreeCursorEntry,
    pub size: u32,
    pub capacity: u32,
}

/// TreeCursor — mirrors tree_cursor.h
#[repr(C)]
pub struct TreeCursor {
    pub tree: *const TSTree,
    pub stack: TreeCursorEntryArray,
    pub root_alias_symbol: TSSymbol,
}

/// TSRangeArray — Array(TSRange), mirrors get_changed_ranges.h
#[repr(C)]
pub struct TSRangeArray {
    pub contents: *mut TSRange,
    pub size: u32,
    pub capacity: u32,
}

// ---------------------------------------------------------------------------
// Types from tree.h
// ---------------------------------------------------------------------------

/// ParentCacheEntry — used for parent lookups (defined in tree.h)
#[repr(C)]
pub struct ParentCacheEntry {
    pub child: *const Subtree,
    pub parent: *const Subtree,
    pub position: Length,
    pub alias_symbol: TSSymbol,
}

/// TSTree — the main tree struct (defined in tree.h)
#[repr(C)]
pub struct TSTree {
    pub root: Subtree,
    pub language: *const TSLanguage,
    pub included_ranges: *mut TSRange,
    pub included_range_count: u32,
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
    let range_size = included_range_count as usize * std::mem::size_of::<TSRange>();
    tree.included_ranges =
        ts_calloc(included_range_count as usize, std::mem::size_of::<TSRange>()) as *mut TSRange;
    memcpy(
        tree.included_ranges as *mut c_void,
        included_ranges as *const c_void,
        range_size,
    );
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
    ts_free(tree.included_ranges as *mut c_void);
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

fn ts_tree_language_ref(tree: &TSTree) -> *const TSLanguage {
    tree.language
}

unsafe fn ts_tree_included_ranges_ref(tree: &TSTree, length: &mut u32) -> *mut TSRange {
    *length = tree.included_range_count;
    let range_size = tree.included_range_count as usize * std::mem::size_of::<TSRange>();
    let ranges =
        ts_calloc(tree.included_range_count as usize, std::mem::size_of::<TSRange>())
            as *mut TSRange;
    memcpy(
        ranges as *mut c_void,
        tree.included_ranges as *const c_void,
        range_size,
    );
    ranges
}

unsafe fn ts_tree_edit_ref(tree: &mut TSTree, edit: &TSInputEdit) {
    for i in 0..tree.included_range_count {
        ts_range_edit(tree.included_ranges.add(i as usize), edit);
    }
    let mut pool = ts_subtree_pool_new(0);
    tree.root = ts_subtree_edit(tree.root, edit, &mut pool);
    ts_subtree_pool_delete(&mut pool);
}

#[cfg(not(target_family = "wasm"))]
unsafe fn ts_tree_print_dot_graph_ref(tree: &TSTree, file_descriptor: i32) {
    let file = fdopen(_ts_dup(file_descriptor), b"a\0".as_ptr() as *const i8);
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
    let result = ts_malloc(std::mem::size_of::<TSTree>()) as *mut TSTree;
    let tree = &mut *result;
    ts_tree_init_ref(tree, root, language, included_ranges, included_range_count);
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_copy(self_: *const TSTree) -> *mut TSTree {
    let tree = &*self_;
    ts_tree_copy_ref(tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_delete(self_: *mut TSTree) {
    if self_.is_null() {
        return;
    }
    let tree = &mut *self_;
    ts_tree_delete_ref(tree);
    ts_free(self_ as *mut c_void);
}

// ---------------------------------------------------------------------------
// Accessors: ts_tree_root_node, ts_tree_root_node_with_offset,
//            ts_tree_language, ts_tree_included_ranges
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node(self_: *const TSTree) -> TSNode {
    let tree = &*self_;
    ts_tree_root_node_ref(self_, tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node_with_offset(
    self_: *const TSTree,
    offset_bytes: u32,
    offset_extent: TSPoint,
) -> TSNode {
    let tree = &*self_;
    ts_tree_root_node_with_offset_ref(self_, tree, offset_bytes, offset_extent)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_language(self_: *const TSTree) -> *const TSLanguage {
    let tree = &*self_;
    ts_tree_language_ref(tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_included_ranges(
    self_: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    let tree = &*self_;
    let length = &mut *length;
    ts_tree_included_ranges_ref(tree, length)
}

// ---------------------------------------------------------------------------
// Mutation & diagnostics: ts_tree_edit, ts_tree_get_changed_ranges,
//                         _ts_dup, ts_tree_print_dot_graph
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_edit(self_: *mut TSTree, edit: *const TSInputEdit) {
    let tree = &mut *self_;
    let edit = &*edit;
    ts_tree_edit_ref(tree, edit);
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_get_changed_ranges(
    old_tree: *const TSTree,
    new_tree: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    let old_tree_ref = &*old_tree;
    let new_tree_ref = &*new_tree;
    let mut cursor1 = TreeCursor {
        tree: std::ptr::null(),
        stack: TreeCursorEntryArray {
            contents: std::ptr::null_mut(),
            size: 0,
            capacity: 0,
        },
        root_alias_symbol: 0,
    };
    let mut cursor2 = TreeCursor {
        tree: std::ptr::null(),
        stack: TreeCursorEntryArray {
            contents: std::ptr::null_mut(),
            size: 0,
            capacity: 0,
        },
        root_alias_symbol: 0,
    };
    ts_tree_cursor_init(&mut cursor1, ts_tree_root_node(old_tree));
    ts_tree_cursor_init(&mut cursor2, ts_tree_root_node(new_tree));

    let mut included_range_differences = TSRangeArray {
        contents: std::ptr::null_mut(),
        size: 0,
        capacity: 0,
    };
    ts_range_array_get_changed_ranges(
        old_tree_ref.included_ranges,
        old_tree_ref.included_range_count,
        new_tree_ref.included_ranges,
        new_tree_ref.included_range_count,
        &mut included_range_differences,
    );

    let mut result: *mut TSRange = std::ptr::null_mut();
    *length = ts_subtree_get_changed_ranges(
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
        ts_free(included_range_differences.contents as *mut c_void);
    }
    // array_delete for cursor stacks
    if !cursor1.stack.contents.is_null() {
        ts_free(cursor1.stack.contents as *mut c_void);
    }
    if !cursor2.stack.contents.is_null() {
        ts_free(cursor2.stack.contents as *mut c_void);
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
    let tree = &*self_;
    ts_tree_print_dot_graph_ref(tree, file_descriptor);
}

#[cfg(target_family = "wasm")]
#[no_mangle]
pub unsafe extern "C" fn ts_tree_print_dot_graph(self_: *const TSTree, file_descriptor: i32) {
    let _ = self_;
    let _ = file_descriptor;
}
