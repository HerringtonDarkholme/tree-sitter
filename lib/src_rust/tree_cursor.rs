//! Stateful navigation through the public visible syntax tree.
//!
//! Unlike a [`TSNode`] search, a [`TreeCursor`] retains the path from its root
//! to its current subtree. Each path entry caches position and child indexes,
//! making repeated parent, sibling, and child moves efficient even though
//! subtrees have no parent pointers.
//!
//! `navigation` mutates this path while flattening hidden nodes. `status`
//! derives the current public node, field, depth, and descendant information.
//! [`TreeCursor`] is stored directly inside the public [`TSTreeCursor`], so the
//! root module owns and asserts that ABI layout; heap path entries remain an
//! internal Rust-layout array.

use core::ptr;

use crate::ffi::{TSNode, TSPoint, TSSymbol, TSTreeCursor};

use super::language::{language_alias_at, ts_language_symbol_metadata};
use super::length::{length_zero, Length};
use super::node::{node_new, ts_node_start_byte, ts_node_start_point};
use super::subtree::Subtree;
use super::tree::TSTree;
use super::utils::Array;
use super::utils::{ptr_mut, ptr_ref};

mod status;
pub use status::{ts_tree_cursor_current_node, ts_tree_cursor_current_status};

mod navigation;
use navigation::{
    tree_cursor_current_depth, tree_cursor_current_descendant_index, tree_cursor_goto_descendant,
    tree_cursor_goto_first_child, tree_cursor_goto_first_child_for_byte_and_point,
    tree_cursor_goto_last_child, tree_cursor_goto_parent,
    tree_cursor_goto_previous_sibling_internal, tree_cursor_parent_node,
};
pub use navigation::{
    tree_cursor_goto_first_child_internal, tree_cursor_goto_next_sibling_internal,
};

use crate::ffi::TSPoint as POINT_ZERO_TYPE;
const POINT_ZERO: POINT_ZERO_TYPE = POINT_ZERO_TYPE { row: 0, column: 0 };

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One stack frame in a tree cursor traversal.
///
/// A cursor stores the path from the root node to the current node. Each frame
/// keeps enough sibling/position bookkeeping to move sideways or back upward
/// without parent pointers in subtrees.
#[derive(Clone, Copy)]
pub struct TreeCursorEntry {
    /// Current subtree pointer for this depth.
    pub subtree: *const Subtree,
    /// Start position of `subtree`.
    pub position: Length,
    /// Raw child index in the parent, including hidden/extra children.
    pub child_index: u32,
    /// Index among non-extra children, used for alias and field tables.
    pub structural_child_index: u32,
    /// Visible descendant index for public cursor APIs.
    pub descendant_index: u32,
}

impl TreeCursorEntry {
    #[inline]
    pub(super) const fn empty() -> Self {
        Self {
            subtree: ptr::null(),
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
        }
    }
}

/// Fixed-layout view of the cursor's entry array inside `TSTreeCursor`.
///
/// The generic runtime `Array<T>` uses Rust layout. Only this adapter needs the
/// historical pointer/size/capacity order because it is embedded directly in
/// public cursor storage.
#[repr(C)]
pub struct TreeCursorStack {
    pub contents: *mut TreeCursorEntry,
    pub size: u32,
    pub capacity: u32,
}

impl TreeCursorStack {
    pub const fn new() -> Self {
        Self {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
        }
    }

    const fn as_array(&self) -> Array<TreeCursorEntry> {
        Array {
            contents: self.contents,
            size: self.size,
            capacity: self.capacity,
        }
    }

    #[inline]
    pub const fn as_slice(&self) -> &[TreeCursorEntry] {
        if self.size == 0 {
            &[]
        } else {
            // SAFETY: Stack operations keep the first `size` entries initialized.
            unsafe { core::slice::from_raw_parts(self.contents, self.size as usize) }
        }
    }

    fn update_from_array(&mut self, array: &Array<TreeCursorEntry>) {
        self.contents = array.contents;
        self.size = array.size;
        self.capacity = array.capacity;
    }

    pub(super) unsafe fn push(&mut self, entry: TreeCursorEntry) {
        let mut array = self.as_array();
        array.push(entry);
        self.update_from_array(&array);
    }

    pub(super) unsafe fn pop(&mut self) -> TreeCursorEntry {
        self.size -= 1;
        ptr::read(self.contents.add(self.size as usize))
    }

    pub(super) fn clear(&mut self) {
        self.size = 0;
    }

    pub(super) unsafe fn delete(&mut self) {
        let mut array = self.as_array();
        array.delete();
        self.update_from_array(&array);
    }

    unsafe fn assign(&mut self, other: &Self) {
        let mut destination = self.as_array();
        let source = other.as_array();
        destination.assign(&source);
        self.update_from_array(&destination);
    }
}

/// Internal cursor representation cast to/from public `TSTreeCursor`.
#[repr(C)]
pub struct TreeCursor {
    /// Tree that owns all subtree pointers in `stack`.
    pub tree: *const TSTree,
    /// Path from root to current cursor node.
    pub stack: TreeCursorStack,
    /// Alias to apply to the root node, or zero.
    pub root_alias_symbol: TSSymbol,
}

// `TreeCursor` is stored directly in the public `TSTreeCursor` value. Keep the
// outer layout fixed while allowing heap-allocated cursor entries to use Rust
// layout.
const _: () = assert!(core::mem::size_of::<TreeCursor>() == core::mem::size_of::<TSTreeCursor>());
const _: () = assert!(core::mem::align_of::<TreeCursor>() == core::mem::align_of::<TSTreeCursor>());
const _: () = assert!(core::mem::offset_of!(TreeCursor, tree) == 0);
const _: () =
    assert!(core::mem::offset_of!(TreeCursor, stack) == core::mem::size_of::<*const ()>());
const _: () = assert!(core::mem::offset_of!(TreeCursorStack, contents) == 0);
const _: () = assert!(
    core::mem::offset_of!(TreeCursorStack, size) == core::mem::size_of::<*const TreeCursorEntry>()
);
const _: () = assert!(
    core::mem::offset_of!(TreeCursorStack, capacity)
        == core::mem::size_of::<*const TreeCursorEntry>() + core::mem::size_of::<u32>()
);
const _: () = assert!(
    core::mem::offset_of!(TreeCursor, root_alias_symbol)
        == 2 * core::mem::size_of::<*const ()>() + 2 * core::mem::size_of::<u32>()
);

#[inline]
unsafe fn cursor_ref<'a>(cursor: *const TSTreeCursor) -> &'a TreeCursor {
    ptr_ref(cursor.cast::<TreeCursor>())
}

#[inline]
unsafe fn cursor_mut<'a>(cursor: *mut TSTreeCursor) -> &'a mut TreeCursor {
    ptr_mut(cursor.cast::<TreeCursor>())
}

#[inline]
unsafe fn out_param_mut<'a, T>(ptr: *mut T) -> &'a mut T {
    ptr_mut(ptr)
}

/// Result of internal navigation.
/// Result returned by the two exported internal cursor navigation functions.
///
/// The exported internal cursor functions return this enum through the C ABI,
/// so its integer representation is part of the frozen symbol contract.
#[repr(C)]
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TreeCursorStep {
    None = 0,
    Hidden = 1,
    Visible = 2,
}

// ---------------------------------------------------------------------------
// Legacy query compatibility
// ---------------------------------------------------------------------------

#[inline]
pub const unsafe fn tree_cursor_entry_slice(arr: &TreeCursorStack) -> &[TreeCursorEntry] {
    arr.as_slice()
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Lifecycle: ts_tree_cursor_new, reset, init, delete
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_new(node: TSNode) -> TSTreeCursor {
    let mut self_ = TSTreeCursor {
        tree: ptr::null(),
        id: ptr::null(),
        context: [0, 0, 0],
    };
    tree_cursor_init_ref(cursor_mut(&mut self_), node);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_reset(self_: *mut TSTreeCursor, node: TSNode) {
    tree_cursor_init_ref(cursor_mut(self_), node);
}

pub unsafe fn tree_cursor_init_ref(cursor: &mut TreeCursor, node: TSNode) {
    cursor.tree = node.tree.cast::<TSTree>();
    cursor.root_alias_symbol = node.context[3] as TSSymbol;
    cursor.stack.clear();
    cursor.stack.push(TreeCursorEntry {
        subtree: node.id.cast::<Subtree>(),
        position: Length {
            bytes: ts_node_start_byte(node),
            extent: ts_node_start_point(node),
        },
        child_index: 0,
        structural_child_index: 0,
        descendant_index: 0,
    });
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_delete(self_: *mut TSTreeCursor) {
    let cursor = cursor_mut(self_);
    cursor.stack.delete();
}

// ---------------------------------------------------------------------------
// Navigation: children
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_internal(
    self_: *mut TSTreeCursor,
) -> TreeCursorStep {
    tree_cursor_goto_first_child_internal(cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child(self_: *mut TSTreeCursor) -> bool {
    tree_cursor_goto_first_child(cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_last_child(self_: *mut TSTreeCursor) -> bool {
    tree_cursor_goto_last_child(cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_byte(
    self_: *mut TSTreeCursor,
    goal_byte: u32,
) -> i64 {
    let cursor = cursor_mut(self_);
    tree_cursor_goto_first_child_for_byte_and_point(cursor, goal_byte, POINT_ZERO)
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_point(
    self_: *mut TSTreeCursor,
    goal_point: TSPoint,
) -> i64 {
    let cursor = cursor_mut(self_);
    tree_cursor_goto_first_child_for_byte_and_point(cursor, 0, goal_point)
}

// ---------------------------------------------------------------------------
// Navigation: siblings, parent, descendant
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling_internal(
    self_: *mut TSTreeCursor,
) -> TreeCursorStep {
    tree_cursor_goto_next_sibling_internal(cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling(self_: *mut TSTreeCursor) -> bool {
    let cursor = cursor_mut(self_);
    match tree_cursor_goto_next_sibling_internal(cursor) {
        TreeCursorStep::Hidden => {
            tree_cursor_goto_first_child(cursor);
            true
        }
        TreeCursorStep::Visible => true,
        _ => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_previous_sibling(self_: *mut TSTreeCursor) -> bool {
    let cursor = cursor_mut(self_);
    match tree_cursor_goto_previous_sibling_internal(cursor) {
        TreeCursorStep::Hidden => {
            tree_cursor_goto_last_child(cursor);
            true
        }
        TreeCursorStep::Visible => true,
        _ => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_parent(self_: *mut TSTreeCursor) -> bool {
    tree_cursor_goto_parent(cursor_mut(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_goto_descendant(
    self_: *mut TSTreeCursor,
    goal_descendant_index: u32,
) {
    tree_cursor_goto_descendant(cursor_mut(self_), goal_descendant_index);
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_descendant_index(
    self_: *const TSTreeCursor,
) -> u32 {
    tree_cursor_current_descendant_index(cursor_ref(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_current_depth(self_: *const TSTreeCursor) -> u32 {
    tree_cursor_current_depth(cursor_ref(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_parent_node(self_: *const TSTreeCursor) -> TSNode {
    tree_cursor_parent_node(cursor_ref(self_))
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_copy(cursor_ptr: *const TSTreeCursor) -> TSTreeCursor {
    let cursor = cursor_ref(cursor_ptr);
    let mut res = TSTreeCursor {
        tree: ptr::null(),
        id: ptr::null(),
        context: [0, 0, 0],
    };
    let copy = cursor_mut(&mut res);
    copy.tree = cursor.tree;
    copy.root_alias_symbol = cursor.root_alias_symbol;
    copy.stack = TreeCursorStack::new();
    copy.stack.assign(&cursor.stack);
    res
}

#[no_mangle]
pub unsafe extern "C" fn ts_tree_cursor_reset_to(dst: *mut TSTreeCursor, src: *const TSTreeCursor) {
    let cursor = cursor_ref(src);
    let copy = cursor_mut(dst);
    copy.tree = cursor.tree;
    copy.root_alias_symbol = cursor.root_alias_symbol;
    copy.stack.clear();
    copy.stack.assign(&cursor.stack);
}
