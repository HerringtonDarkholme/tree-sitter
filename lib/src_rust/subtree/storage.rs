use core::ffi::c_void;
use core::ptr::{self, NonNull};

use super::super::alloc::{calloc, free, malloc, realloc};
use super::{
    subtree_alloc_size, Array, ExternalScannerState, ExternalScannerStateData, MutableSubtree,
    Subtree, SubtreeArray, SubtreeHeapData, SubtreePool, EXTERNAL_SCANNER_STATE_INLINE_SIZE,
    TS_MAX_TREE_POOL_SIZE,
};

impl ExternalScannerState {
    pub unsafe fn from_bytes(bytes: &[u8]) -> Self {
        let length = u32::try_from(bytes.len()).unwrap();
        let data = if bytes.len() > EXTERNAL_SCANNER_STATE_INLINE_SIZE {
            let copy = NonNull::new_unchecked(malloc(bytes.len()).cast::<u8>());
            copy.as_ptr()
                .copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
            ExternalScannerStateData::Heap(copy)
        } else {
            let mut copy = [0; EXTERNAL_SCANNER_STATE_INLINE_SIZE];
            copy[..bytes.len()].copy_from_slice(bytes);
            ExternalScannerStateData::Inline(copy)
        };
        Self { data, length }
    }

    pub unsafe fn copy(&self) -> Self {
        Self::from_bytes(self.as_bytes())
    }

    pub unsafe fn delete(&mut self) {
        if let ExternalScannerStateData::Heap(bytes) = self.data {
            free(bytes.as_ptr().cast::<c_void>());
        }
        self.data = ExternalScannerStateData::Inline([0; EXTERNAL_SCANNER_STATE_INLINE_SIZE]);
        self.length = 0;
    }

    pub fn as_bytes(&self) -> &[u8] {
        let length = self.length as usize;
        match &self.data {
            ExternalScannerStateData::Inline(bytes) => &bytes[..length],
            ExternalScannerStateData::Heap(bytes) => unsafe {
                core::slice::from_raw_parts(bytes.as_ptr(), length)
            },
        }
    }
}

pub unsafe fn subtree_array_copy(source: &SubtreeArray, destination: &mut SubtreeArray) {
    destination.size = source.size;
    destination.capacity = source.capacity;
    destination.contents = source.contents;
    if source.capacity > 0 {
        destination.contents =
            calloc(source.capacity as usize, core::mem::size_of::<Subtree>()).cast::<Subtree>();
        if !source.is_empty() {
            destination
                .as_mut_slice()
                .copy_from_slice(source.as_slice());
            for &tree in destination.as_slice() {
                tree.retain();
            }
        }
    }
}

pub unsafe fn subtree_array_clear(pool: &mut SubtreePool, trees: &mut SubtreeArray) {
    for &tree in trees.as_slice() {
        tree.release(pool);
    }
    trees.size = 0;
}

pub unsafe fn subtree_array_delete(pool: &mut SubtreePool, trees: &mut SubtreeArray) {
    subtree_array_clear(pool, trees);
    if !trees.contents.is_null() {
        free(trees.contents.cast::<c_void>());
    }
    trees.contents = ptr::null_mut();
    trees.size = 0;
    trees.capacity = 0;
}

pub unsafe fn subtree_array_remove_trailing_extras(
    trees: &mut SubtreeArray,
    destination: &mut SubtreeArray,
) {
    destination.size = 0;
    while let Some(&last) = trees.as_slice().last() {
        if last.extra() {
            trees.size -= 1;
            destination.push(last);
        } else {
            break;
        }
    }
    subtree_array_reverse(destination);
}

pub fn subtree_array_reverse(trees: &mut SubtreeArray) {
    trees.as_mut_slice().reverse();
}

pub unsafe fn subtree_pool_new(capacity: u32) -> SubtreePool {
    let mut pool = SubtreePool {
        free_trees: Array::new(),
        tree_stack: Array::new(),
    };
    pool.free_trees.reserve(capacity);
    pool
}

pub unsafe fn subtree_pool_delete(pool: &mut SubtreePool) {
    if !pool.free_trees.contents.is_null() {
        for &tree in pool.free_trees.as_slice() {
            free(tree.heap_ptr().as_ptr().cast::<c_void>());
        }
        pool.free_trees.delete();
    }
    if !pool.tree_stack.contents.is_null() {
        pool.tree_stack.delete();
    }
}

pub(super) unsafe fn subtree_pool_allocate(pool: &mut SubtreePool) -> NonNull<SubtreeHeapData> {
    if pool.free_trees.size > 0 {
        pool.free_trees.pop().heap_ptr()
    } else {
        NonNull::new_unchecked(malloc(core::mem::size_of::<SubtreeHeapData>()).cast())
    }
}

/// Allocate a node header after a copy of `tree`'s children.
pub(super) unsafe fn subtree_clone_allocation(tree: Subtree) -> NonNull<SubtreeHeapData> {
    let child_count = tree.child_count();
    let children =
        NonNull::new_unchecked(malloc(subtree_alloc_size(child_count)).cast::<Subtree>());
    let copied_children = core::slice::from_raw_parts_mut(children.as_ptr(), child_count as usize);
    copied_children.copy_from_slice(tree.children());
    for child in copied_children {
        child.retain();
    }
    NonNull::new_unchecked(
        children
            .as_ptr()
            .add(child_count as usize)
            .cast::<SubtreeHeapData>(),
    )
}

/// Place a node header after an array's initialized children.
pub(super) unsafe fn subtree_take_children(
    children: &mut SubtreeArray,
) -> NonNull<SubtreeHeapData> {
    let byte_size = subtree_alloc_size(children.size);
    if (children.capacity as usize) * core::mem::size_of::<Subtree>() < byte_size {
        children.contents =
            realloc(children.contents.cast::<c_void>(), byte_size).cast::<Subtree>();
        children.capacity = (byte_size / core::mem::size_of::<Subtree>()) as u32;
    }
    NonNull::new_unchecked(
        children
            .contents
            .add(children.size as usize)
            .cast::<SubtreeHeapData>(),
    )
}

/// Free the combined child-and-header allocation of an internal node.
pub(super) unsafe fn subtree_free_internal_node(tree: Subtree) {
    debug_assert!(tree.child_count() > 0);
    free(tree.children().as_ptr().cast_mut().cast::<c_void>());
}

pub(super) unsafe fn subtree_pool_free(pool: &mut SubtreePool, tree: MutableSubtree) {
    if pool.free_trees.capacity > 0 && pool.free_trees.size < TS_MAX_TREE_POOL_SIZE {
        pool.free_trees.push(tree);
    } else {
        free(tree.heap_ptr().as_ptr().cast::<c_void>());
    }
}
