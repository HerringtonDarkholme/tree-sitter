use core::ffi::c_void;
use core::ptr;

use super::alloc::{free, malloc, realloc};

/// Convert a non-null raw pointer from the C API into a shared reference.
///
/// # Safety
/// `ptr` must be non-null, properly aligned, initialized, and valid for reads
/// for the returned lifetime.
#[inline]
pub unsafe fn ptr_ref<'a, T>(ptr: *const T) -> &'a T {
    debug_assert!(!ptr.is_null());
    ptr.as_ref().unwrap_unchecked()
}

/// Convert a non-null raw pointer from the C API into a mutable reference.
///
/// # Safety
/// `ptr` must be non-null, properly aligned, initialized, uniquely borrowed,
/// and valid for writes for the returned lifetime.
#[inline]
pub unsafe fn ptr_mut<'a, T>(ptr: *mut T) -> &'a mut T {
    debug_assert!(!ptr.is_null());
    ptr.as_mut().unwrap_unchecked()
}

// ---------------------------------------------------------------------------
// Generic array helpers, mirrors C `array.h`
// ---------------------------------------------------------------------------

/// Generic dynamic array used by the Rust runtime.
///
/// This type is internal and deliberately uses Rust layout. ABI-facing storage
/// that carries the same three values defines its own fixed-layout adapter.
pub struct Array<T> {
    pub contents: *mut T,
    pub size: u32,
    pub capacity: u32,
}

impl<T> Array<T> {
    pub const fn new() -> Self {
        Self {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
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
    pub const fn as_slice(&self) -> &[T] {
        if self.is_empty() {
            &[]
        } else {
            // SAFETY: Array operations keep the first `size` elements initialized.
            unsafe { core::slice::from_raw_parts(self.contents, self.len()) }
        }
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.is_empty() {
            &mut []
        } else {
            // SAFETY: A mutable Array borrow uniquely borrows its initialized elements.
            unsafe { core::slice::from_raw_parts_mut(self.contents, self.len()) }
        }
    }

    pub unsafe fn delete(&mut self) {
        if !self.contents.is_null() {
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
        if new_capacity <= self.capacity {
            return;
        }

        let byte_count = new_capacity as usize * core::mem::size_of::<T>();
        self.contents = if self.contents.is_null() {
            malloc(byte_count).cast::<T>()
        } else {
            realloc(self.contents.cast::<c_void>(), byte_count).cast::<T>()
        };
        self.capacity = new_capacity;
    }

    #[inline]
    unsafe fn grow(&mut self, count: u32) {
        let new_size = self.size + count;
        if new_size > self.capacity {
            self.reserve((self.capacity * 2).max(8).max(new_size));
        }
    }

    #[inline]
    pub unsafe fn push(&mut self, element: T) {
        self.grow(1);
        ptr::write(self.contents.add(self.size as usize), element);
        self.size += 1;
    }

    /// Extend the array with `count` elements represented by all-zero bytes.
    #[inline]
    pub unsafe fn grow_by(&mut self, count: u32) {
        if count == 0 {
            return;
        }
        self.grow(count);
        ptr::write_bytes(self.contents.add(self.size as usize), 0, count as usize);
        self.size += count;
    }

    #[inline]
    pub unsafe fn pop(&mut self) -> T {
        self.size -= 1;
        ptr::read(self.contents.add(self.size as usize))
    }

    #[inline]
    pub unsafe fn get_unchecked(&self, index: u32) -> &T {
        self.as_slice().get_unchecked(index as usize)
    }

    #[inline]
    pub unsafe fn get_unchecked_mut(&mut self, index: u32) -> &mut T {
        self.as_mut_slice().get_unchecked_mut(index as usize)
    }

    #[inline]
    pub unsafe fn last_unchecked(&self) -> &T {
        self.as_slice().last().unwrap_unchecked()
    }

    #[inline]
    pub unsafe fn last_unchecked_mut(&mut self) -> &mut T {
        self.as_mut_slice().last_mut().unwrap_unchecked()
    }

    pub unsafe fn erase(&mut self, index: u32) {
        debug_assert!(index < self.size);
        let count = self.size as usize - index as usize - 1;
        if count > 0 {
            ptr::copy(
                self.contents.add(index as usize + 1),
                self.contents.add(index as usize),
                count,
            );
        }
        self.size -= 1;
    }

    pub unsafe fn insert(&mut self, index: u32, element: T) {
        self.grow(1);
        let count = self.size as usize - index as usize;
        if count > 0 {
            ptr::copy(
                self.contents.add(index as usize),
                self.contents.add(index as usize + 1),
                count,
            );
        }
        ptr::write(self.contents.add(index as usize), element);
        self.size += 1;
    }

    pub unsafe fn splice(
        &mut self,
        index: u32,
        old_count: u32,
        new_count: u32,
        new_contents: *const T,
    ) {
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
        self.reserve(source.size);
        self.size = source.size;
        if !source.is_empty() {
            ptr::copy(source.contents, self.contents, source.len());
        }
    }
}

pub fn array_init<T>(arr: &mut Array<T>) {
    arr.size = 0;
    arr.capacity = 0;
    arr.contents = ptr::null_mut();
}

pub unsafe fn array_delete<T>(arr: &mut Array<T>) {
    arr.delete();
}

#[inline]
pub fn array_clear<T>(arr: &mut Array<T>) {
    arr.clear();
}

#[inline]
pub unsafe fn array_reserve<T>(arr: &mut Array<T>, new_capacity: u32) {
    arr.reserve(new_capacity);
}

#[inline]
pub unsafe fn array_push<T>(arr: &mut Array<T>, element: T) {
    arr.push(element);
}

/// Grow the array's length by `count`, zero-initializing the new elements.
///
/// Mirrors the C `array_grow_by` macro: reserves capacity, zeroes the new
/// trailing region, then bumps `size`. The new elements must be valid when
/// represented as all-zero bytes (e.g. integers, or structs of such).
#[inline]
pub unsafe fn array_grow_by<T>(arr: &mut Array<T>, count: u32) {
    arr.grow_by(count);
}

#[inline]
pub unsafe fn array_pop<T>(arr: &mut Array<T>) -> T {
    arr.pop()
}

#[inline]
pub unsafe fn array_get_ref<T>(arr: &Array<T>, index: u32) -> &T {
    arr.get_unchecked(index)
}

#[inline]
pub unsafe fn array_get_mut<T>(arr: &mut Array<T>, index: u32) -> &mut T {
    arr.get_unchecked_mut(index)
}

#[inline]
pub unsafe fn array_back_ref<T>(arr: &Array<T>) -> &T {
    arr.last_unchecked()
}

#[inline]
pub unsafe fn array_back_mut<T>(arr: &mut Array<T>) -> &mut T {
    arr.last_unchecked_mut()
}

pub unsafe fn array_erase<T>(arr: &mut Array<T>, index: u32) {
    arr.erase(index);
}

pub unsafe fn array_insert<T>(arr: &mut Array<T>, index: u32, element: T) {
    arr.insert(index, element);
}

pub const fn array_new<T>() -> Array<T> {
    Array::new()
}

pub unsafe fn array_splice<T>(
    arr: &mut Array<T>,
    index: u32,
    old_count: u32,
    new_count: u32,
    new_contents: *const T,
) {
    arr.splice(index, old_count, new_count, new_contents);
}

pub fn array_swap<T>(left: &mut Array<T>, right: &mut Array<T>) {
    core::mem::swap(left, right);
}

pub unsafe fn array_assign<T>(destination: &mut Array<T>, source: &Array<T>) {
    destination.assign(source);
}
