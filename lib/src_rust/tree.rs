use core::ffi::c_void;

use crate::ffi::{TSLanguage, TSNode, TSPoint, TSRange};

use super::alloc::{calloc, free, malloc};
use super::get_changed_ranges::{
    range_array_get_changed_ranges_ref, range_edit_ref, range_slice, subtree_get_changed_ranges_ref,
};
use super::length::{length_add, Length};
use super::node::node_new;
use super::subtree::{
    subtree_edit, subtree_padding, subtree_pool_delete, subtree_pool_new, subtree_release,
    subtree_retain, tree_arena_release, tree_arena_retain, Subtree, TreeArena,
};
// Only used by `tree_print_dot_graph_ref`, which is unavailable on wasm.
#[cfg(not(target_family = "wasm"))]
use super::subtree::subtree_print_dot_graph;
use super::tree_cursor::{tree_cursor_init_ref, TreeCursor};
use super::utils::array_new;
use super::utils::{ptr_mut, ptr_ref};

// ---------------------------------------------------------------------------
// Platform C-library functions used for DOT output.
// ---------------------------------------------------------------------------

#[cfg(not(target_family = "wasm"))]
extern "C" {
    // `fdopen` is spelled `_fdopen` on Windows; `fclose` keeps its name on all
    // platforms.
    #[cfg_attr(target_os = "windows", link_name = "_fdopen")]
    fn fdopen(fd: i32, mode: *const i8) -> *mut c_void;
    fn fclose(f: *mut c_void) -> i32;
}

#[cfg(not(any(target_os = "windows", target_family = "wasm")))]
extern "C" {
    fn dup(fd: i32) -> i32;
}

use crate::ffi::TSInputEdit;

// ---------------------------------------------------------------------------
// Types from tree.h
// ---------------------------------------------------------------------------

/// Owned parse tree returned by the parser.
///
/// The tree retains the root subtree, a copied language reference, the included
/// ranges used for parsing, and optionally the arena that owns internal nodes
/// created during the Rust parser's normal parse path. Public APIs expose only
/// opaque pointers to trees, so this structure uses Rust layout.
pub struct TSTree {
    /// Root syntax subtree, retained by the tree.
    pub(super) root: Subtree,
    /// Language used to parse this tree.
    pub(super) language: *const TSLanguage,
    /// Copied included ranges for tree comparison and public APIs.
    pub(super) included_ranges: *mut TSRange,
    /// Number of entries in `included_ranges`.
    pub(super) included_range_count: u32,
    /// Shared arena for arena-owned internal nodes.
    pub(super) arena: *mut TreeArena,
}

unsafe fn tree_init_ref(
    tree: &mut TSTree,
    root: Subtree,
    language: *const TSLanguage,
    included_ranges: &[TSRange],
    arena: *mut TreeArena,
) {
    tree.root = root;
    tree.language = language;
    tree.included_range_count = included_ranges.len() as u32;
    tree.arena = arena;
    tree.included_ranges =
        calloc(included_ranges.len(), core::mem::size_of::<TSRange>()).cast::<TSRange>();
    if !included_ranges.is_empty() {
        core::ptr::copy_nonoverlapping(
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
unsafe fn tree_copy_ref(tree: &TSTree) -> *mut TSTree {
    subtree_retain(tree.root);
    tree_arena_retain(tree.arena);
    tree_new_with_arena(
        tree.root,
        tree.language,
        tree.included_ranges,
        tree.included_range_count,
        tree.arena,
    )
}

/// Release all owned references and buffers for a tree.
unsafe fn tree_delete_ref(tree: &mut TSTree) {
    let mut pool = subtree_pool_new(0);
    subtree_release(&mut pool, tree.root);
    subtree_pool_delete(&mut pool);
    tree_arena_release(tree.arena);
    free(tree.included_ranges.cast::<c_void>());
}

pub unsafe fn tree_root_node_ref(tree_ptr: *const TSTree, tree: &TSTree) -> TSNode {
    node_new(tree_ptr, &tree.root, subtree_padding(tree.root), 0)
}

unsafe fn tree_root_node_with_offset_ref(
    tree_ptr: *const TSTree,
    tree: &TSTree,
    offset_bytes: u32,
    offset_extent: TSPoint,
) -> TSNode {
    let offset = Length {
        bytes: offset_bytes,
        extent: offset_extent,
    };
    node_new(
        tree_ptr,
        &tree.root,
        length_add(offset, subtree_padding(tree.root)),
        0,
    )
}

unsafe fn tree_included_ranges_ref(tree: &TSTree, length: &mut u32) -> *mut TSRange {
    *length = tree.included_range_count;
    let ranges = calloc(
        tree.included_range_count as usize,
        core::mem::size_of::<TSRange>(),
    )
    .cast::<TSRange>();
    if tree.included_range_count > 0 {
        core::ptr::copy_nonoverlapping(
            tree.included_ranges,
            ranges,
            tree.included_range_count as usize,
        );
    }
    ranges
}

const fn tree_cursor_empty() -> TreeCursor {
    TreeCursor {
        tree: core::ptr::null(),
        stack: array_new(),
        root_alias_symbol: 0,
    }
}

/// Apply an edit to the tree's ranges and root subtree.
///
/// The edit rewrites byte/point positions in-place where possible and marks
/// affected subtrees as changed for later tree comparison.
unsafe fn tree_edit_ref(tree: &mut TSTree, edit: &TSInputEdit) {
    let included_ranges = if tree.included_range_count == 0 {
        &mut []
    } else {
        core::slice::from_raw_parts_mut(tree.included_ranges, tree.included_range_count as usize)
    };
    for range in included_ranges {
        range_edit_ref(range, edit);
    }
    let mut pool = subtree_pool_new(0);
    tree.root = subtree_edit(tree.root, edit, &mut pool);
    subtree_pool_delete(&mut pool);
}

#[cfg(not(target_family = "wasm"))]
unsafe fn tree_print_dot_graph_ref(tree: &TSTree, file_descriptor: i32) {
    // On Windows `_ts_dup` takes the OS handle behind the fd (mirroring
    // lib/src/tree.c); elsewhere it duplicates the fd directly.
    #[cfg(target_os = "windows")]
    let dup_fd = _ts_dup(win_dot_graph::_get_osfhandle(file_descriptor) as win_dot_graph::Handle);
    #[cfg(not(target_os = "windows"))]
    let dup_fd = _ts_dup(file_descriptor);
    let file = fdopen(dup_fd, c"a".as_ptr().cast::<i8>());
    subtree_print_dot_graph(tree.root, tree.language, file);
    fclose(file);
}

// ---------------------------------------------------------------------------
// Lifecycle: tree_new, ts_tree_copy, ts_tree_delete
// ---------------------------------------------------------------------------

pub unsafe fn tree_new_with_arena(
    root: Subtree,
    language: *const TSLanguage,
    included_ranges: *const TSRange,
    included_range_count: u32,
    arena: *mut TreeArena,
) -> *mut TSTree {
    let result = malloc(core::mem::size_of::<TSTree>()).cast::<TSTree>();
    let tree = ptr_mut(result);
    let included_ranges = range_slice(included_ranges, included_range_count);
    tree_init_ref(tree, root, language, included_ranges, arena);
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_copy(self_: *const TSTree) -> *mut TSTree {
    let tree = ptr_ref(self_);
    tree_copy_ref(tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_delete(self_: *mut TSTree) {
    if self_.is_null() {
        return;
    }
    let tree = ptr_mut(self_);
    tree_delete_ref(tree);
    free(self_.cast::<c_void>());
}

// ---------------------------------------------------------------------------
// Accessors: ts_tree_root_node, ts_tree_root_node_with_offset,
//            ts_tree_language, ts_tree_included_ranges
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node(self_: *const TSTree) -> TSNode {
    let tree = ptr_ref(self_);
    tree_root_node_ref(self_, tree)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node_with_offset(
    self_: *const TSTree,
    offset_bytes: u32,
    offset_extent: TSPoint,
) -> TSNode {
    let tree = ptr_ref(self_);
    tree_root_node_with_offset_ref(self_, tree, offset_bytes, offset_extent)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_language(self_: *const TSTree) -> *const TSLanguage {
    let tree = ptr_ref(self_);
    tree.language
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_included_ranges(
    self_: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    let tree = ptr_ref(self_);
    let length = ptr_mut(length);
    tree_included_ranges_ref(tree, length)
}

// ---------------------------------------------------------------------------
// Mutation & diagnostics: ts_tree_edit, ts_tree_get_changed_ranges,
//                         _ts_dup, ts_tree_print_dot_graph
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_edit(self_: *mut TSTree, edit: *const TSInputEdit) {
    let tree = ptr_mut(self_);
    let edit = ptr_ref(edit);
    tree_edit_ref(tree, edit);
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_get_changed_ranges(
    old_tree: *const TSTree,
    new_tree: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    let old_tree_ref = ptr_ref(old_tree);
    let new_tree_ref = ptr_ref(new_tree);
    let length = ptr_mut(length);
    let mut cursor1 = tree_cursor_empty();
    let mut cursor2 = tree_cursor_empty();
    tree_cursor_init_ref(&mut cursor1, tree_root_node_ref(old_tree, old_tree_ref));
    tree_cursor_init_ref(&mut cursor2, tree_root_node_ref(new_tree, new_tree_ref));

    let mut included_range_differences = array_new();
    let old_included_ranges = range_slice(
        old_tree_ref.included_ranges,
        old_tree_ref.included_range_count,
    );
    let new_included_ranges = range_slice(
        new_tree_ref.included_ranges,
        new_tree_ref.included_range_count,
    );
    range_array_get_changed_ranges_ref(
        old_included_ranges,
        new_included_ranges,
        &mut included_range_differences,
    );

    let mut result: *mut TSRange = core::ptr::null_mut();
    *length = subtree_get_changed_ranges_ref(
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
        free(included_range_differences.contents.cast::<c_void>());
    }
    // array_delete for cursor stacks
    if !cursor1.stack.contents.is_null() {
        free(cursor1.stack.contents.cast::<c_void>());
    }
    if !cursor2.stack.contents.is_null() {
        free(cursor2.stack.contents.cast::<c_void>());
    }

    result
}

#[cfg(not(any(target_os = "windows", target_family = "wasm")))]
#[no_mangle]
pub unsafe extern "C" fn _ts_dup(file_descriptor: i32) -> i32 {
    dup(file_descriptor)
}

// Windows fd duplication for the dot-graph FILE*, mirroring lib/src/tree.c: the
// fd's OS handle is duplicated and reopened so the temporary FILE* can be closed
// without closing the caller's fd.
#[cfg(all(target_os = "windows", not(target_family = "wasm")))]
mod win_dot_graph {
    use core::ffi::c_void;

    pub type Handle = *mut c_void;
    pub const DUPLICATE_SAME_ACCESS: u32 = 0x0000_0002;

    extern "system" {
        pub fn GetCurrentProcess() -> Handle;
        pub fn DuplicateHandle(
            source_process: Handle,
            source_handle: Handle,
            target_process: Handle,
            target_handle: *mut Handle,
            desired_access: u32,
            inherit_handle: i32,
            options: u32,
        ) -> i32;
    }
    extern "C" {
        pub fn _get_osfhandle(fd: i32) -> isize;
        pub fn _open_osfhandle(osfhandle: isize, flags: i32) -> i32;
    }
}

#[cfg(all(target_os = "windows", not(target_family = "wasm")))]
#[no_mangle]
pub unsafe extern "C" fn _ts_dup(handle: win_dot_graph::Handle) -> i32 {
    let mut dup_handle: win_dot_graph::Handle = core::ptr::null_mut();
    if win_dot_graph::DuplicateHandle(
        win_dot_graph::GetCurrentProcess(),
        handle,
        win_dot_graph::GetCurrentProcess(),
        &mut dup_handle,
        0,
        0,
        win_dot_graph::DUPLICATE_SAME_ACCESS,
    ) == 0
    {
        return -1;
    }
    win_dot_graph::_open_osfhandle(dup_handle as isize, 0)
}

#[cfg(not(target_family = "wasm"))]
#[no_mangle]
pub unsafe extern "C" fn ts_tree_print_dot_graph(self_: *const TSTree, file_descriptor: i32) {
    let tree = ptr_ref(self_);
    tree_print_dot_graph_ref(tree, file_descriptor);
}

#[cfg(target_family = "wasm")]
#[no_mangle]
pub unsafe extern "C" fn ts_tree_print_dot_graph(self_: *const TSTree, file_descriptor: i32) {
    let _ = self_;
    let _ = file_descriptor;
}

#[cfg(test)]
mod tests {
    use core::ptr;

    use super::*;
    use crate::core_impl::length::length_zero;
    use crate::core_impl::subtree::{
        subtree_child_count, subtree_from_mut, subtree_new_error, subtree_new_node_in_arena,
        tree_arena_new, TS_BUILTIN_SYM_ERROR_REPEAT,
    };

    #[test]
    fn arena_tree_copy_delete_uses_tree_arena_lifetime() {
        unsafe {
            let mut pool = subtree_pool_new(0);
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
            let children = [child1, child2];

            let arena = tree_arena_new();
            let root = subtree_from_mut(subtree_new_node_in_arena(
                arena,
                TS_BUILTIN_SYM_ERROR_REPEAT,
                children.as_ptr(),
                children.len() as u32,
                0,
                ptr::null(),
            ));

            assert_eq!(subtree_child_count(root), 2);
            let tree = tree_new_with_arena(root, ptr::null(), ptr::null(), 0, arena);
            let copy = ts_tree_copy(tree);

            assert_eq!((*tree).arena, arena);
            assert_eq!((*copy).arena, arena);
            ts_tree_delete(tree);
            ts_tree_delete(copy);
            subtree_pool_delete(&mut pool);
        }
    }
}
