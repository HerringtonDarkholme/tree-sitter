//! Allocation, pooling, and owned-array operations for subtrees.
//!
//! Heap subtree allocations use a pointer-stable, demand-backed contiguous
//! arena. This module centralizes arena ownership, bump allocation, cloning,
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
    EXTERNAL_SCANNER_STATE_INLINE_SIZE,
};
use super::handle::Subtree;
use super::{
    subtree_alloc_size, subtree_child_storage_size, SubtreeArena, SubtreeArray, SubtreePool,
    TS_SUBTREE_SLAB_CAPACITY,
};

/// Commit in chunks that are multiples of both common 4 KiB pages and macOS's
/// 16 KiB pages. The first chunk contains only the arena header, so payload
/// commits can always begin at a page boundary.
const ARENA_COMMIT_GRANULARITY: usize = 64 * 1024;
const ARENA_DATA_OFFSET: usize = ARENA_COMMIT_GRANULARITY;

#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "solaris",
    target_os = "illumos",
))]
mod virtual_memory {
    use core::ffi::{c_int, c_void};
    use core::ptr;

    const PROT_NONE: c_int = 0;
    const PROT_READ: c_int = 1;
    const PROT_WRITE: c_int = 2;
    const MAP_PRIVATE: c_int = 2;
    #[cfg(any(target_os = "android", target_os = "linux"))]
    const MAP_ANONYMOUS: c_int = 0x20;
    #[cfg(any(
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "ios",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd",
    ))]
    const MAP_ANONYMOUS: c_int = 0x1000;
    #[cfg(any(target_os = "solaris", target_os = "illumos"))]
    const MAP_ANONYMOUS: c_int = 0x100;

    extern "C" {
        fn mmap(
            address: *mut c_void,
            length: usize,
            protection: c_int,
            flags: c_int,
            file_descriptor: c_int,
            offset: i64,
        ) -> *mut c_void;
        fn mprotect(address: *mut c_void, length: usize, protection: c_int) -> c_int;
        fn munmap(address: *mut c_void, length: usize) -> c_int;
    }

    pub unsafe fn reserve(byte_count: usize) -> *mut u8 {
        let result = mmap(
            ptr::null_mut(),
            byte_count,
            PROT_NONE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        );
        if result as isize == -1 {
            ptr::null_mut()
        } else {
            result.cast::<u8>()
        }
    }

    pub unsafe fn commit(address: *mut u8, byte_count: usize) -> bool {
        mprotect(address.cast::<c_void>(), byte_count, PROT_READ | PROT_WRITE) == 0
    }

    pub unsafe fn release(address: *mut u8, byte_count: usize) {
        let result = munmap(address.cast::<c_void>(), byte_count);
        debug_assert_eq!(result, 0);
    }
}

#[cfg(target_os = "windows")]
mod virtual_memory {
    use core::ffi::c_void;
    use core::ptr;

    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_NOACCESS: u32 = 0x01;
    const PAGE_READWRITE: u32 = 0x04;

    extern "system" {
        fn VirtualAlloc(
            address: *mut c_void,
            size: usize,
            allocation_type: u32,
            protection: u32,
        ) -> *mut c_void;
        fn VirtualFree(address: *mut c_void, size: usize, free_type: u32) -> i32;
    }

    pub unsafe fn reserve(byte_count: usize) -> *mut u8 {
        VirtualAlloc(ptr::null_mut(), byte_count, MEM_RESERVE, PAGE_NOACCESS).cast::<u8>()
    }

    pub unsafe fn commit(address: *mut u8, byte_count: usize) -> bool {
        !VirtualAlloc(
            address.cast::<c_void>(),
            byte_count,
            MEM_COMMIT,
            PAGE_READWRITE,
        )
        .is_null()
    }

    pub unsafe fn release(address: *mut u8, _byte_count: usize) {
        let result = VirtualFree(address.cast::<c_void>(), 0, MEM_RELEASE);
        debug_assert_ne!(result, 0);
    }
}

/// Platforms without page-reservation APIs retain the same stable-pointer
/// semantics, but their allocator decides how eagerly the address range is
/// backed. Native Unix and Windows builds use the demand-paged paths above.
#[cfg(not(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "solaris",
    target_os = "illumos",
    target_os = "windows",
)))]
mod virtual_memory {
    use super::{free, malloc};
    use core::ffi::c_void;

    pub unsafe fn reserve(byte_count: usize) -> *mut u8 {
        malloc(byte_count).cast::<u8>()
    }

    pub const unsafe fn commit(_address: *mut u8, _byte_count: usize) -> bool {
        true
    }

    pub unsafe fn release(address: *mut u8, _byte_count: usize) {
        free(address.cast::<c_void>());
    }
}

unsafe fn subtree_arena_new() -> *mut SubtreeArena {
    let allocation_size = ARENA_DATA_OFFSET + TS_SUBTREE_SLAB_CAPACITY;
    let Some(arena) =
        NonNull::new(virtual_memory::reserve(allocation_size)).map(NonNull::cast::<SubtreeArena>)
    else {
        alloc_failed("reserve subtree slab address space", allocation_size);
    };
    let arena = arena.as_ptr();
    if !virtual_memory::commit(arena.cast::<u8>(), ARENA_DATA_OFFSET) {
        virtual_memory::release(arena.cast::<u8>(), allocation_size);
        alloc_failed("commit subtree slab header", ARENA_DATA_OFFSET);
    }
    arena.write(SubtreeArena {
        ref_count: AtomicU32::new(1),
        // Index zero is the null-subtree sentinel. Keep the first aligned word
        // unused so every heap record has a nonzero arena-relative index and
        // its low inline-tag bit remains clear.
        offset: AtomicUsize::new(core::mem::align_of::<SubtreeHeapData>()),
        committed: AtomicUsize::new(0),
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

unsafe fn subtree_arena_commit_through(arena: *mut SubtreeArena, end: usize) {
    let arena_ref = &*arena;
    let mut committed = arena_ref.committed.load(Ordering::Acquire);
    while committed < end {
        let desired = end
            .next_multiple_of(ARENA_COMMIT_GRANULARITY)
            .min(TS_SUBTREE_SLAB_CAPACITY);
        let address = subtree_arena_data(arena).add(committed);
        if !virtual_memory::commit(address, desired - committed) {
            alloc_failed("commit subtree slab pages", desired - committed);
        }

        match arena_ref.committed.compare_exchange(
            committed,
            desired,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => return,
            Err(actual) if actual >= end => return,
            Err(actual) => committed = actual,
        }
    }
}

unsafe fn subtree_arena_allocate(
    arena: *mut SubtreeArena,
    byte_count: usize,
    alignment: usize,
) -> NonNull<u8> {
    debug_assert!(!arena.is_null());
    debug_assert!(alignment.is_power_of_two());
    let arena_ref = &*arena;
    let mut current = arena_ref.offset.load(Ordering::Relaxed);
    loop {
        let start = current.next_multiple_of(alignment);
        let Some(end) = start.checked_add(byte_count) else {
            alloc_failed("allocate subtree slab block", byte_count);
        };
        if end > TS_SUBTREE_SLAB_CAPACITY {
            alloc_failed("allocate subtree slab block", byte_count);
        }
        match arena_ref.offset.compare_exchange_weak(
            current,
            end,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                subtree_arena_commit_through(arena, end);
                return NonNull::new_unchecked(subtree_arena_data(arena).add(start));
            }
            Err(actual) => current = actual,
        }
    }
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
        virtual_memory::release(
            arena.cast::<u8>(),
            ARENA_DATA_OFFSET + TS_SUBTREE_SLAB_CAPACITY,
        );
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

impl SubtreeArray {
    pub const fn new() -> Self {
        Self {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
            arena: ptr::null_mut(),
            arena_generation: 0,
        }
    }

    unsafe fn new_in(pool: &mut SubtreePool) -> Self {
        let arena = subtree_pool_ensure_arena(pool);
        Self {
            arena,
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
                self.arena,
                byte_count,
                core::mem::align_of::<SubtreeHeapData>(),
            )
            .cast::<Subtree>()
            .as_ptr();
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
    if array.arena != arena {
        array.delete();
        *array = SubtreeArray::new_in(pool);
    }
}

impl ExternalScannerState {
    pub unsafe fn from_bytes(arena: *mut SubtreeArena, bytes: &[u8]) -> Self {
        let length = u32::try_from(bytes.len()).unwrap();
        let data = if bytes.len() > EXTERNAL_SCANNER_STATE_INLINE_SIZE {
            let copy = subtree_arena_allocate(arena, bytes.len(), 1);
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

    pub unsafe fn copy(&self, arena: *mut SubtreeArena) -> Self {
        Self::from_bytes(arena, self.as_bytes())
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
    *destination = SubtreeArray {
        arena: source.arena,
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
            tree.retain(source.arena);
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
    let arena = subtree_pool_ensure_arena(pool);
    subtree_arena_allocate(
        arena,
        core::mem::size_of::<SubtreeHeapData>(),
        core::mem::align_of::<SubtreeHeapData>(),
    )
    .cast()
}

pub(super) unsafe fn subtree_pool_allocate_inline(
    pool: &mut SubtreePool,
) -> NonNull<SubtreeInlineData> {
    let arena = subtree_pool_ensure_arena(pool);
    subtree_arena_allocate(
        arena,
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
    let children = subtree_arena_allocate(
        arena,
        subtree_alloc_size(child_count),
        core::mem::align_of::<SubtreeHeapData>(),
    )
    .cast::<Subtree>();
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
        let allocation =
            subtree_arena_allocate(arena, byte_size, core::mem::align_of::<SubtreeHeapData>())
                .cast::<Subtree>();
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
