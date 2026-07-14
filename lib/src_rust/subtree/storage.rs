use core::ffi::c_void;
use core::ptr::{self, NonNull};

use super::{
    calloc, free, malloc, subtree_extra, subtree_release, subtree_retain, Array,
    ExternalScannerState, ExternalScannerStateData, MutableSubtree, Subtree, SubtreeArray,
    SubtreeHeapData, SubtreePool, EXTERNAL_SCANNER_STATE_INLINE_SIZE, TS_MAX_TREE_POOL_SIZE,
};

pub unsafe fn external_scanner_state_new(data: *const u8, length: u32) -> ExternalScannerState {
    let state = if length > EXTERNAL_SCANNER_STATE_INLINE_SIZE as u32 {
        let bytes = NonNull::new_unchecked(malloc(length as usize).cast::<u8>());
        ptr::copy_nonoverlapping(data, bytes.as_ptr(), length as usize);
        ExternalScannerStateData::Heap(bytes)
    } else {
        let mut bytes = [0; EXTERNAL_SCANNER_STATE_INLINE_SIZE];
        ptr::copy_nonoverlapping(data, bytes.as_mut_ptr(), length as usize);
        ExternalScannerStateData::Inline(bytes)
    };
    ExternalScannerState {
        data: state,
        length,
    }
}

pub unsafe fn external_scanner_state_copy(state: &ExternalScannerState) -> ExternalScannerState {
    let data = match &state.data {
        ExternalScannerStateData::Inline(bytes) => ExternalScannerStateData::Inline(*bytes),
        ExternalScannerStateData::Heap(bytes) => {
            let copy = NonNull::new_unchecked(malloc(state.length as usize).cast::<u8>());
            ptr::copy_nonoverlapping(bytes.as_ptr(), copy.as_ptr(), state.length as usize);
            ExternalScannerStateData::Heap(copy)
        }
    };
    ExternalScannerState {
        data,
        length: state.length,
    }
}

pub unsafe fn external_scanner_state_delete(state: &mut ExternalScannerState) {
    if let ExternalScannerStateData::Heap(bytes) = state.data {
        free(bytes.as_ptr().cast::<c_void>());
    }
}

pub const fn external_scanner_state_data(state: &ExternalScannerState) -> *const u8 {
    match &state.data {
        ExternalScannerStateData::Inline(bytes) => bytes.as_ptr(),
        ExternalScannerStateData::Heap(bytes) => bytes.as_ptr(),
    }
}

pub unsafe fn external_scanner_state_eq(
    state: &ExternalScannerState,
    buffer: *const u8,
    length: u32,
) -> bool {
    if state.length != length {
        return false;
    }
    if length == 0 {
        return true;
    }
    let length = length as usize;
    core::slice::from_raw_parts(external_scanner_state_data(state), length)
        == core::slice::from_raw_parts(buffer, length)
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
                subtree_retain(tree);
            }
        }
    }
}

pub unsafe fn subtree_array_clear(pool: &mut SubtreePool, trees: &mut SubtreeArray) {
    for &tree in trees.as_slice() {
        subtree_release(pool, tree);
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
        if subtree_extra(last) {
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
            free(tree.heap_ptr().cast::<c_void>());
        }
        pool.free_trees.delete();
    }
    if !pool.tree_stack.contents.is_null() {
        pool.tree_stack.delete();
    }
}

pub(super) unsafe fn subtree_pool_allocate(pool: &mut SubtreePool) -> *mut SubtreeHeapData {
    if pool.free_trees.size > 0 {
        pool.free_trees.pop().heap_ptr()
    } else {
        malloc(core::mem::size_of::<SubtreeHeapData>()).cast::<SubtreeHeapData>()
    }
}

pub(super) unsafe fn subtree_pool_free(pool: &mut SubtreePool, tree: MutableSubtree) {
    if pool.free_trees.capacity > 0 && pool.free_trees.size < TS_MAX_TREE_POOL_SIZE {
        pool.free_trees.push(tree);
    } else {
        free(tree.heap_ptr().cast::<c_void>());
    }
}
