//! Allocation, pooling, and owned-array operations for subtrees.
//!
//! Heap subtree allocations use an on-demand malloc-backed contiguous arena.
//! This module centralizes arena ownership, bump allocation, cloning,
//! and child-buffer transfer. Subtree records and long external-scanner byte
//! buffers remain in the arena until its whole generation is rewound or
//! released. It also provides the explicit
//! copy/clear/delete operations for [`SubtreeArray`].
//!
//! Parser scratch nodes may borrow a reusable child buffer; ordinary internal
//! nodes instead take ownership of their child allocation. The separate helper
//! functions here keep that lifetime distinction visible at construction
//! sites.

use core::ffi::c_void;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::super::alloc::{alloc_failed, free, malloc, realloc};
use super::super::utils::Array;
use super::data::{
    ExternalScannerState, ExternalScannerStateData, SubtreeHeapData, SubtreeInlineData,
    SubtreeInternalData, SubtreeLeafData, EXTERNAL_SCANNER_STATE_INLINE_SIZE,
};
use super::handle::Subtree;
use super::{
    subtree_alloc_size, subtree_child_storage_size, SubtreeArena, SubtreeArray, SubtreePool,
    TS_SUBTREE_SLAB_CAPACITY,
};

pub(super) const ARENA_INITIAL_CAPACITY: usize = 256 * 1024;
const ARENA_DATA_OFFSET: usize = {
    let size = core::mem::size_of::<SubtreeArena>();
    let alignment = core::mem::align_of::<SubtreeInternalData>();
    (size + alignment - 1) & !(alignment - 1)
};

unsafe fn subtree_arena_new() -> *mut SubtreeArena {
    let arena = malloc(core::mem::size_of::<SubtreeArena>()).cast::<SubtreeArena>();
    arena.write(SubtreeArena {
        ref_count: AtomicU32::new(1),
        // Index zero is the null-subtree sentinel. Keep the first aligned word
        // unused so every heap record has a nonzero arena-relative index and
        // its low inline-tag bit remains clear.
        offset: AtomicUsize::new(core::mem::align_of::<SubtreeHeapData>()),
        capacity: 0,
        retired: ptr::null_mut(),
        generation: AtomicU32::new(1),
        published: false,
    });
    arena
}

#[inline]
pub(super) const unsafe fn subtree_arena_data(arena: *mut SubtreeArena) -> *mut u8 {
    arena.cast::<u8>().add(ARENA_DATA_OFFSET)
}

#[inline(always)]
pub(super) const unsafe fn subtree_arena_is_published(arena: *mut SubtreeArena) -> bool {
    (*arena).published
}

pub(super) unsafe fn subtree_arena_publish(arena: *mut SubtreeArena) {
    debug_assert!(!arena.is_null());
    (*arena).published = true;
}

unsafe fn subtree_arena_grow(pool: &mut SubtreePool, end: usize) {
    let arena = pool.arena;
    debug_assert!(!(*arena).published);
    let old_capacity = (*arena).capacity;
    if old_capacity < end {
        let new_capacity = end
            .checked_next_power_of_two()
            .unwrap_or(TS_SUBTREE_SLAB_CAPACITY)
            .clamp(ARENA_INITIAL_CAPACITY, TS_SUBTREE_SLAB_CAPACITY);
        let allocation_size = ARENA_DATA_OFFSET
            .checked_add(new_capacity)
            .unwrap_or_else(|| alloc_failed("grow subtree arena", new_capacity));
        let new_arena = malloc(allocation_size).cast::<SubtreeArena>();
        let used = (*arena).offset.load(Ordering::Relaxed);
        new_arena.write(SubtreeArena {
            ref_count: AtomicU32::new(1),
            offset: AtomicUsize::new(used),
            capacity: new_capacity,
            retired: arena,
            generation: AtomicU32::new((*arena).generation.load(Ordering::Relaxed)),
            published: false,
        });
        subtree_arena_data(new_arena).copy_from_nonoverlapping(subtree_arena_data(arena), used);
        pool.arena = new_arena;
    }
}

unsafe fn subtree_arena_allocate(
    pool: &mut SubtreePool,
    byte_count: usize,
    alignment: usize,
) -> NonNull<u8> {
    let mut arena = subtree_pool_ensure_arena(pool);
    debug_assert!(!arena.is_null());
    debug_assert!(alignment.is_power_of_two());
    let start = (*arena)
        .offset
        .load(Ordering::Relaxed)
        .next_multiple_of(alignment);
    let Some(end) = start.checked_add(byte_count) else {
        alloc_failed("allocate subtree arena block", byte_count);
    };
    if end > TS_SUBTREE_SLAB_CAPACITY {
        alloc_failed("allocate subtree arena block", byte_count);
    }
    if (*arena).capacity < end {
        subtree_arena_grow(pool, end);
        arena = pool.arena;
    }
    (*arena).offset.store(end, Ordering::Relaxed);
    NonNull::new_unchecked(subtree_arena_data(arena).add(start))
}

pub unsafe fn subtree_arena_retain(arena: *mut SubtreeArena) {
    if arena.is_null() {
        return;
    }
    let previous = (*arena).ref_count.fetch_add(1, Ordering::SeqCst);
    debug_assert!(previous > 0);
    debug_assert_ne!(previous, u32::MAX);
}

pub unsafe fn subtree_arena_release(arena: *mut SubtreeArena) {
    if arena.is_null() {
        return;
    }
    let previous = (*arena).ref_count.fetch_sub(1, Ordering::SeqCst);
    debug_assert!(previous > 0);
    if previous == 1 {
        let mut current = arena;
        while !current.is_null() {
            let retired = (*current).retired;
            free(current.cast::<c_void>());
            current = retired;
        }
    }
}

unsafe fn subtree_pool_ensure_arena(pool: &mut SubtreePool) -> *mut SubtreeArena {
    if pool.arena.is_null() {
        pool.arena = subtree_arena_new();
    }
    pool.arena
}

/// Prepare the parser's retained slab for a fresh parse.
///
/// If the previously returned tree has already been deleted, the parser is the
/// sole arena owner and can rewind the bump cursor. Otherwise the old tree
/// keeps its slab and the parser starts with a new one lazily.
pub unsafe fn subtree_pool_prepare_for_parse(pool: &mut SubtreePool) {
    if pool.arena.is_null() {
        return;
    }
    if (*pool.arena).ref_count.load(Ordering::SeqCst) == 1 {
        (*pool.arena)
            .offset
            .store(core::mem::align_of::<SubtreeHeapData>(), Ordering::SeqCst);
        (*pool.arena).generation.fetch_add(1, Ordering::SeqCst);
        (*pool.arena).published = false;
    } else {
        subtree_arena_release(pool.arena);
        pool.arena = ptr::null_mut();
    }
}

/// Return another owning arena reference for a published tree.
pub unsafe fn subtree_pool_retain_arena(pool: &mut SubtreePool) -> *mut SubtreeArena {
    let arena = subtree_pool_ensure_arena(pool);
    subtree_arena_retain(arena);
    arena
}

/// Create temporary allocation/release state backed by an existing tree arena.
pub unsafe fn subtree_pool_from_arena(arena: *mut SubtreeArena) -> SubtreePool {
    subtree_arena_retain(arena);
    SubtreePool {
        arena,
        tree_stack: Array::new(),
    }
}

/// Create a parser-private byte-for-byte arena copy for cold tree mutation.
/// Published arenas never move; detaching first preserves nodes and cursors in
/// other tree copies while allowing the edit's private arena to grow.
pub unsafe fn subtree_pool_clone_arena(arena: *mut SubtreeArena) -> SubtreePool {
    debug_assert!(!arena.is_null());
    let capacity = (*arena).capacity;
    let allocation_size = ARENA_DATA_OFFSET
        .checked_add(capacity)
        .unwrap_or_else(|| alloc_failed("clone subtree arena", capacity));
    let copy = malloc(allocation_size).cast::<SubtreeArena>();
    let used = (*arena).offset.load(Ordering::Acquire);
    copy.write(SubtreeArena {
        ref_count: AtomicU32::new(1),
        offset: AtomicUsize::new(used),
        capacity,
        retired: ptr::null_mut(),
        generation: AtomicU32::new((*arena).generation.load(Ordering::Acquire)),
        published: false,
    });
    subtree_arena_data(copy).copy_from_nonoverlapping(subtree_arena_data(arena), used);
    SubtreePool {
        arena: copy,
        tree_stack: Array::new(),
    }
}

impl SubtreeArray {
    pub const fn new() -> Self {
        Self {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
            arena: ptr::null_mut(),
            pool: ptr::null_mut(),
            arena_generation: 0,
        }
    }

    unsafe fn new_in(pool: &mut SubtreePool) -> Self {
        let arena = subtree_pool_ensure_arena(pool);
        Self {
            arena,
            pool,
            arena_generation: (*arena).generation.load(Ordering::Acquire),
            ..Self::new()
        }
    }

    /// Drop bump-allocated capacity left over from a previous arena epoch.
    /// The parser only rewinds after releasing its parse state, so stale arrays
    /// must be logically empty even though their cached address is obsolete.
    unsafe fn refresh_arena_generation(&mut self) {
        if self.arena.is_null() {
            return;
        }

        let generation = (*self.arena).generation.load(Ordering::Acquire);
        if self.arena_generation != generation {
            debug_assert_eq!(self.size, 0);
            self.contents = ptr::null_mut();
            self.size = 0;
            self.capacity = 0;
            self.arena_generation = generation;
        }
    }

    #[inline]
    pub const fn len(&self) -> usize {
        self.size as usize
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }

    #[inline]
    unsafe fn current_arena(&self) -> *mut SubtreeArena {
        if self.pool.is_null() {
            self.arena
        } else {
            (*self.pool).arena
        }
    }

    #[inline]
    pub const fn as_slice(&self) -> &[Subtree] {
        if self.is_empty() {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(self.contents, self.len()) }
        }
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [Subtree] {
        if self.is_empty() {
            &mut []
        } else {
            unsafe { core::slice::from_raw_parts_mut(self.contents, self.len()) }
        }
    }

    pub unsafe fn delete(&mut self) {
        if self.arena.is_null() && !self.contents.is_null() {
            free(self.contents.cast::<c_void>());
        }
        *self = Self::new();
    }

    #[inline]
    pub fn clear(&mut self) {
        self.size = 0;
    }

    #[inline]
    pub unsafe fn reserve(&mut self, new_capacity: u32) {
        self.refresh_arena_generation();
        if new_capacity <= self.capacity {
            return;
        }
        let byte_count = new_capacity as usize * core::mem::size_of::<Subtree>();
        let new_contents = if self.arena.is_null() {
            if self.contents.is_null() {
                malloc(byte_count).cast::<Subtree>()
            } else {
                realloc(self.contents.cast::<c_void>(), byte_count).cast::<Subtree>()
            }
        } else {
            let allocation = subtree_arena_allocate(
                &mut *self.pool,
                byte_count,
                core::mem::align_of::<SubtreeHeapData>(),
            )
            .cast::<Subtree>()
            .as_ptr();
            self.arena = (*self.pool).arena;
            self.arena_generation = (*self.arena).generation.load(Ordering::Acquire);
            if !self.is_empty() {
                allocation.copy_from_nonoverlapping(self.contents, self.len());
            }
            allocation
        };
        self.contents = new_contents;
        self.capacity = new_capacity;
    }

    #[inline]
    unsafe fn grow(&mut self, count: u32) {
        self.refresh_arena_generation();
        let new_size = self.size + count;
        if new_size > self.capacity {
            self.reserve((self.capacity * 2).max(8).max(new_size));
        }
    }

    #[inline]
    pub unsafe fn push(&mut self, element: Subtree) {
        self.grow(1);
        self.contents.add(self.size as usize).write(element);
        self.size += 1;
    }

    pub unsafe fn splice(
        &mut self,
        index: u32,
        old_count: u32,
        new_count: u32,
        new_contents: *const Subtree,
    ) {
        self.refresh_arena_generation();
        let new_size = self.size + new_count - old_count;
        let old_end = index + old_count;
        let new_end = index + new_count;
        debug_assert!(old_end <= self.size);

        self.reserve(new_size);
        let trailing_count = (self.size - old_end) as usize;
        if trailing_count > 0 {
            ptr::copy(
                self.contents.add(old_end as usize),
                self.contents.add(new_end as usize),
                trailing_count,
            );
        }
        if new_count > 0 && !new_contents.is_null() {
            ptr::copy(
                new_contents,
                self.contents.add(index as usize),
                new_count as usize,
            );
        }
        self.size = new_size;
    }

    pub unsafe fn assign(&mut self, source: &Self) {
        self.refresh_arena_generation();
        self.reserve(source.size);
        self.size = source.size;
        if !source.is_empty() {
            self.contents
                .copy_from_nonoverlapping(source.contents, source.len());
        }
    }
}

pub unsafe fn subtree_array_new(pool: &mut SubtreePool) -> SubtreeArray {
    SubtreeArray::new_in(pool)
}

/// Ensure a non-owning scratch child buffer belongs to the pool's current
/// storage domain. Its copied handles are borrowed, so rebinding abandons the
/// old capacity without releasing elements.
pub unsafe fn subtree_array_prepare_scratch(pool: &mut SubtreePool, array: &mut SubtreeArray) {
    let arena = subtree_pool_ensure_arena(pool);
    let generation = (*arena).generation.load(Ordering::Acquire);
    if array.arena != arena || array.arena_generation != generation {
        debug_assert!(array.is_empty());
        array.delete();
        *array = SubtreeArray::new_in(pool);
    }
}

impl ExternalScannerState {
    pub unsafe fn from_bytes(pool: &mut SubtreePool, bytes: &[u8]) -> Self {
        let length = u32::try_from(bytes.len()).unwrap();
        let data = if bytes.len() > EXTERNAL_SCANNER_STATE_INLINE_SIZE {
            let copy = subtree_arena_allocate(pool, bytes.len(), 1);
            let arena = pool.arena();
            copy.as_ptr()
                .copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
            ExternalScannerStateData::Heap(
                u32::try_from(copy.as_ptr() as usize - subtree_arena_data(arena) as usize)
                    .expect("external scanner state arena offset fits in u32"),
            )
        } else {
            let mut copy = [0; EXTERNAL_SCANNER_STATE_INLINE_SIZE];
            copy[..bytes.len()].copy_from_slice(bytes);
            ExternalScannerStateData::Inline(copy)
        };
        Self { data, length }
    }

    pub unsafe fn copy(&self, pool: &mut SubtreePool) -> Self {
        let source = *self;
        let length = source.length as usize;
        match source.data {
            ExternalScannerStateData::Inline(bytes) => Self {
                data: ExternalScannerStateData::Inline(bytes),
                length: source.length,
            },
            ExternalScannerStateData::Heap(offset) => {
                let copy = subtree_arena_allocate(pool, length, 1);
                let arena = pool.arena();
                copy.as_ptr().copy_from_nonoverlapping(
                    subtree_arena_data(arena).add(offset as usize),
                    length,
                );
                Self {
                    data: ExternalScannerStateData::Heap(
                        u32::try_from(copy.as_ptr() as usize - subtree_arena_data(arena) as usize)
                            .expect("external scanner state arena offset fits in u32"),
                    ),
                    length: source.length,
                }
            }
        }
    }

    pub unsafe fn as_bytes(&self, arena: *mut SubtreeArena) -> &[u8] {
        let length = self.length as usize;
        match &self.data {
            ExternalScannerStateData::Inline(bytes) => &bytes[..length],
            ExternalScannerStateData::Heap(offset) => {
                core::slice::from_raw_parts(subtree_arena_data(arena).add(*offset as usize), length)
            }
        }
    }
}

pub unsafe fn subtree_array_copy(source: &SubtreeArray, destination: &mut SubtreeArray) {
    *destination = SubtreeArray {
        arena: source.arena,
        pool: source.pool,
        arena_generation: source.arena_generation,
        ..SubtreeArray::new()
    };
    destination.reserve(source.capacity);
    destination.size = source.size;
    if !source.is_empty() {
        destination
            .as_mut_slice()
            .copy_from_slice(source.as_slice());
        for &tree in destination.as_slice() {
            tree.retain(source.current_arena());
        }
    }
}

pub unsafe fn subtree_array_clear(pool: &mut SubtreePool, trees: &mut SubtreeArray) {
    for &tree in trees.as_slice() {
        tree.release(pool);
    }
    trees.clear();
}

pub unsafe fn subtree_array_delete(pool: &mut SubtreePool, trees: &mut SubtreeArray) {
    subtree_array_clear(pool, trees);
    trees.delete();
}

pub unsafe fn subtree_array_remove_trailing_extras(
    trees: &mut SubtreeArray,
    destination: &mut SubtreeArray,
    arena: *mut SubtreeArena,
) {
    destination.size = 0;
    while let Some(&last) = trees.as_slice().last() {
        if last.extra(arena) {
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

pub const unsafe fn subtree_pool_new(capacity: u32) -> SubtreePool {
    let _ = capacity;
    SubtreePool {
        arena: ptr::null_mut(),
        tree_stack: Array::new(),
    }
}

pub unsafe fn subtree_pool_delete(pool: &mut SubtreePool) {
    if !pool.tree_stack.contents.is_null() {
        pool.tree_stack.delete();
    }
    subtree_arena_release(pool.arena);
    pool.arena = ptr::null_mut();
}

pub(super) unsafe fn subtree_pool_allocate(pool: &mut SubtreePool) -> NonNull<SubtreeHeapData> {
    subtree_arena_allocate(
        pool,
        core::mem::size_of::<SubtreeLeafData>(),
        core::mem::align_of::<SubtreeLeafData>(),
    )
    .cast()
}

pub(super) unsafe fn subtree_pool_allocate_inline(
    pool: &mut SubtreePool,
) -> NonNull<SubtreeInlineData> {
    subtree_arena_allocate(
        pool,
        core::mem::size_of::<SubtreeInlineData>(),
        core::mem::align_of::<SubtreeInlineData>(),
    )
    .cast()
}

#[cfg(test)]
pub(super) unsafe fn subtree_pool_used_bytes(pool: &SubtreePool) -> usize {
    if pool.arena.is_null() {
        0
    } else {
        (*pool.arena)
            .offset
            .load(Ordering::Relaxed)
            .saturating_sub(core::mem::align_of::<SubtreeHeapData>())
    }
}

#[cfg(test)]
pub(super) unsafe fn subtree_pool_contains(pool: &SubtreePool, address: *const u8) -> bool {
    if pool.arena.is_null() {
        return false;
    }
    let start = subtree_arena_data(pool.arena) as usize;
    let end = start + TS_SUBTREE_SLAB_CAPACITY;
    (start..end).contains(&(address as usize))
}

/// Allocate a node header after a copy of `tree`'s children.
#[allow(clippy::cast_ptr_alignment)]
pub(super) unsafe fn subtree_clone_allocation(
    pool: &mut SubtreePool,
    tree: Subtree,
) -> NonNull<SubtreeHeapData> {
    let arena = subtree_pool_ensure_arena(pool);
    let child_count = tree.child_count(arena);
    if !tree.heap_data(arena).is_internal() {
        return subtree_pool_allocate(pool);
    }
    let children = subtree_arena_allocate(
        pool,
        subtree_alloc_size(child_count),
        core::mem::align_of::<SubtreeInternalData>(),
    )
    .cast::<Subtree>();
    let arena = pool.arena();
    let copied_children = core::slice::from_raw_parts_mut(children.as_ptr(), child_count as usize);
    copied_children.copy_from_slice(tree.children(arena));
    for child in copied_children {
        child.retain(arena);
    }
    NonNull::new_unchecked(
        children
            .as_ptr()
            .cast::<u8>()
            .add(subtree_child_storage_size(child_count))
            .cast::<SubtreeHeapData>(),
    )
}

/// Ensure an array has room for a node header after its initialized children.
#[allow(clippy::cast_ptr_alignment)]
unsafe fn subtree_reserve_header(children: &mut SubtreeArray) -> NonNull<SubtreeHeapData> {
    children.refresh_arena_generation();
    let byte_size = subtree_alloc_size(children.size);
    if (children.capacity as usize) * core::mem::size_of::<Subtree>() < byte_size {
        children.reserve((byte_size / core::mem::size_of::<Subtree>()) as u32);
    }
    NonNull::new_unchecked(
        children
            .contents
            .cast::<u8>()
            .add(subtree_child_storage_size(children.size))
            .cast::<SubtreeHeapData>(),
    )
}

/// Transfer a child array's allocation into a new internal node.
#[allow(clippy::cast_ptr_alignment)]
pub(super) unsafe fn subtree_take_children(
    pool: &mut SubtreePool,
    mut children: SubtreeArray,
) -> (NonNull<SubtreeHeapData>, u32) {
    children.refresh_arena_generation();
    let child_count = children.size;
    let byte_size = subtree_alloc_size(child_count);
    let required_capacity = byte_size / core::mem::size_of::<Subtree>();
    let arena = subtree_pool_ensure_arena(pool);
    if children.arena != arena || (children.capacity as usize) < required_capacity {
        let allocation = subtree_arena_allocate(
            pool,
            byte_size,
            core::mem::align_of::<SubtreeInternalData>(),
        )
        .cast::<Subtree>();
        children.refresh_arena_generation();
        if child_count > 0 {
            allocation
                .as_ptr()
                .copy_from_nonoverlapping(children.contents, child_count as usize);
        }
        children.delete();
        let data = NonNull::new_unchecked(
            allocation
                .as_ptr()
                .cast::<u8>()
                .add(subtree_child_storage_size(child_count))
                .cast::<SubtreeHeapData>(),
        );
        (data, child_count)
    } else {
        (subtree_reserve_header(&mut children), child_count)
    }
}

/// Place a temporary node header in reusable parser scratch storage.
///
/// The returned header remains valid only until `children` is changed or
/// deleted. Unlike `subtree_take_children`, this does not transfer ownership.
pub(super) unsafe fn subtree_reuse_children(
    children: &mut SubtreeArray,
) -> NonNull<SubtreeHeapData> {
    subtree_reserve_header(children)
}
