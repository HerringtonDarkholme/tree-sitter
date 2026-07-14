use core::ffi::c_void;

use crate::ffi::{TSLanguage, TSNode, TSPoint, TSRange};

use super::alloc::{calloc, free, malloc};
use super::get_changed_ranges::{
    range_array_get_changed_ranges_ref, range_edit_ref, subtree_get_changed_ranges_ref,
};
use super::length::{length_add, Length};
use super::node::node_new;
use super::subtree::{
    subtree_edit, subtree_padding, subtree_pool_delete, subtree_pool_new, subtree_release,
    subtree_retain, Subtree,
};
// Only used by `TSTree::print_dot_graph`, which is unavailable on wasm.
#[cfg(not(target_family = "wasm"))]
use super::subtree::subtree_print_dot_graph;
use super::tree_cursor::{tree_cursor_init_ref, TreeCursor};
use super::utils::Array;
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
/// The tree retains the root subtree, a copied language reference, and the
/// included ranges used for parsing. Public APIs expose only opaque pointers to
/// trees, so this structure uses Rust layout.
pub struct TSTree {
    /// Root syntax subtree, retained by the tree.
    pub(super) root: Subtree,
    /// Language used to parse this tree.
    pub(super) language: *const TSLanguage,
    /// Copied included ranges for tree comparison and public APIs.
    pub(super) included_ranges: Array<TSRange>,
}

unsafe fn copy_ranges(included_ranges: &[TSRange]) -> Array<TSRange> {
    let mut result = Array::new();
    let count = u32::try_from(included_ranges.len()).unwrap();
    result.reserve(count);
    if !included_ranges.is_empty() {
        core::ptr::copy_nonoverlapping(
            included_ranges.as_ptr(),
            result.contents,
            included_ranges.len(),
        );
    }
    result.size = count;
    result
}

const fn tree_cursor_empty() -> TreeCursor {
    TreeCursor {
        tree: core::ptr::null(),
        stack: super::tree_cursor::TreeCursorStack::new(),
        root_alias_symbol: 0,
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

impl TSTree {
    pub unsafe fn new(
        root: Subtree,
        language: *const TSLanguage,
        included_ranges: &[TSRange],
    ) -> *mut Self {
        let result = malloc(core::mem::size_of::<Self>()).cast::<Self>();
        core::ptr::write(
            result,
            Self {
                root,
                language,
                included_ranges: copy_ranges(included_ranges),
            },
        );
        result
    }

    pub const unsafe fn included_ranges(&self) -> &[TSRange] {
        self.included_ranges.as_slice()
    }

    unsafe fn included_ranges_mut(&mut self) -> &mut [TSRange] {
        self.included_ranges.as_mut_slice()
    }

    /// Copy a tree by retaining its shared immutable subtree storage.
    unsafe fn copy(&self) -> *mut Self {
        subtree_retain(self.root);
        Self::new(self.root, self.language, self.included_ranges())
    }

    /// Release all references and buffers owned by this tree.
    unsafe fn delete(&mut self) {
        let mut pool = subtree_pool_new(0);
        subtree_release(&mut pool, self.root);
        subtree_pool_delete(&mut pool);
        self.included_ranges.delete();
    }

    pub unsafe fn root_node(&self, tree_ptr: *const Self) -> TSNode {
        node_new(tree_ptr, &self.root, subtree_padding(self.root), 0)
    }

    unsafe fn root_node_with_offset(
        &self,
        tree_ptr: *const Self,
        offset_bytes: u32,
        offset_extent: TSPoint,
    ) -> TSNode {
        let offset = Length {
            bytes: offset_bytes,
            extent: offset_extent,
        };
        node_new(
            tree_ptr,
            &self.root,
            length_add(offset, subtree_padding(self.root)),
            0,
        )
    }

    unsafe fn copy_included_ranges(&self, length: &mut u32) -> *mut TSRange {
        let included_ranges = self.included_ranges();
        *length = u32::try_from(included_ranges.len()).unwrap();
        let ranges =
            calloc(included_ranges.len(), core::mem::size_of::<TSRange>()).cast::<TSRange>();
        if !included_ranges.is_empty() {
            core::ptr::copy_nonoverlapping(included_ranges.as_ptr(), ranges, included_ranges.len());
        }
        ranges
    }

    /// Apply an edit to this tree's ranges and root subtree.
    unsafe fn edit(&mut self, edit: &TSInputEdit) {
        for range in self.included_ranges_mut() {
            range_edit_ref(range, edit);
        }
        let mut pool = subtree_pool_new(0);
        self.root = subtree_edit(self.root, edit, &mut pool);
        subtree_pool_delete(&mut pool);
    }

    #[cfg(not(target_family = "wasm"))]
    unsafe fn print_dot_graph(&self, file_descriptor: i32) {
        // On Windows `_ts_dup` takes the OS handle behind the fd (mirroring
        // lib/src/tree.c); elsewhere it duplicates the fd directly.
        #[cfg(target_os = "windows")]
        let dup_fd =
            _ts_dup(win_dot_graph::_get_osfhandle(file_descriptor) as win_dot_graph::Handle);
        #[cfg(not(target_os = "windows"))]
        let dup_fd = _ts_dup(file_descriptor);
        let file = fdopen(dup_fd, c"a".as_ptr().cast::<i8>());
        subtree_print_dot_graph(self.root, self.language, file);
        fclose(file);
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_copy(self_: *const TSTree) -> *mut TSTree {
    let tree = ptr_ref(self_);
    tree.copy()
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_delete(self_: *mut TSTree) {
    if self_.is_null() {
        return;
    }
    let tree = ptr_mut(self_);
    tree.delete();
    free(self_.cast::<c_void>());
}

// ---------------------------------------------------------------------------
// Accessors: ts_tree_root_node, ts_tree_root_node_with_offset,
//            ts_tree_language, ts_tree_included_ranges
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node(self_: *const TSTree) -> TSNode {
    let tree = ptr_ref(self_);
    tree.root_node(self_)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_root_node_with_offset(
    self_: *const TSTree,
    offset_bytes: u32,
    offset_extent: TSPoint,
) -> TSNode {
    let tree = ptr_ref(self_);
    tree.root_node_with_offset(self_, offset_bytes, offset_extent)
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
    tree.copy_included_ranges(length)
}

// ---------------------------------------------------------------------------
// Mutation & diagnostics: ts_tree_edit, ts_tree_get_changed_ranges,
//                         _ts_dup, ts_tree_print_dot_graph
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_edit(self_: *mut TSTree, edit: *const TSInputEdit) {
    let tree = ptr_mut(self_);
    let edit = ptr_ref(edit);
    tree.edit(edit);
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
    tree_cursor_init_ref(&mut cursor1, old_tree_ref.root_node(old_tree));
    tree_cursor_init_ref(&mut cursor2, new_tree_ref.root_node(new_tree));

    let mut included_range_differences = Array::new();
    let old_included_ranges = old_tree_ref.included_ranges();
    let new_included_ranges = new_tree_ref.included_ranges();
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

    included_range_differences.delete();
    cursor1.stack.delete();
    cursor2.stack.delete();

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
    tree.print_dot_graph(file_descriptor);
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
        subtree_child_count, subtree_from_mut, subtree_new_error, subtree_new_node,
        TS_BUILTIN_SYM_ERROR_REPEAT,
    };

    #[test]
    fn copied_tree_retains_its_root() {
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
            let mut children = Array::new();
            children.push(child1);
            children.push(child2);
            let root = subtree_from_mut(subtree_new_node(
                TS_BUILTIN_SYM_ERROR_REPEAT,
                &mut children,
                0,
                ptr::null(),
            ));

            assert_eq!(subtree_child_count(root), 2);
            let tree = TSTree::new(root, ptr::null(), &[]);
            let copy = ts_tree_copy(tree);

            ts_tree_delete(tree);
            assert_eq!(subtree_child_count((*copy).root), 2);
            ts_tree_delete(copy);
            subtree_pool_delete(&mut pool);
        }
    }
}
