#![allow(dead_code)]

use std::ffi::c_void;
use std::ptr;

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

/// Generic dynamic array, mirrors C `Array(T)`.
#[repr(C)]
pub struct Array<T> {
    pub contents: *mut T,
    pub size: u32,
    pub capacity: u32,
}

pub fn array_init<T>(arr: &mut Array<T>) {
    arr.size = 0;
    arr.capacity = 0;
    arr.contents = ptr::null_mut();
}

pub unsafe fn array_delete<T>(arr: &mut Array<T>) {
    if !arr.contents.is_null() {
        free(arr.contents.cast::<c_void>());
    }
    arr.contents = ptr::null_mut();
    arr.size = 0;
    arr.capacity = 0;
}

#[inline]
pub fn array_clear<T>(arr: &mut Array<T>) {
    arr.size = 0;
}

#[inline]
pub unsafe fn array_reserve<T>(arr: &mut Array<T>, new_capacity: u32) {
    if new_capacity > arr.capacity {
        let elem_size = std::mem::size_of::<T>();
        if arr.contents.is_null() {
            arr.contents = malloc(new_capacity as usize * elem_size).cast::<T>();
        } else {
            arr.contents = realloc(
                arr.contents.cast::<c_void>(),
                new_capacity as usize * elem_size,
            )
            .cast::<T>();
        }
        arr.capacity = new_capacity;
    }
}

#[inline]
pub unsafe fn array_grow<T>(arr: &mut Array<T>, count: u32) {
    let new_size = arr.size + count;
    if new_size > arr.capacity {
        let mut new_capacity = arr.capacity * 2;
        if new_capacity < 8 {
            new_capacity = 8;
        }
        if new_capacity < new_size {
            new_capacity = new_size;
        }
        array_reserve(arr, new_capacity);
    }
}

#[inline]
pub unsafe fn array_push<T>(arr: &mut Array<T>, element: T) {
    array_grow(arr, 1);
    ptr::write(arr.contents.add(arr.size as usize), element);
    arr.size += 1;
}

/// Grow the array's length by `count`, zero-initializing the new elements.
///
/// Mirrors the C `array_grow_by` macro: reserves capacity, zeroes the new
/// trailing region, then bumps `size`. The new elements must be valid when
/// represented as all-zero bytes (e.g. integers, or structs of such).
#[inline]
pub unsafe fn array_grow_by<T>(arr: &mut Array<T>, count: u32) {
    if count == 0 {
        return;
    }
    array_grow(arr, count);
    ptr::write_bytes(arr.contents.add(arr.size as usize), 0, count as usize);
    arr.size += count;
}

#[inline]
pub unsafe fn array_pop<T>(arr: &mut Array<T>) -> T {
    arr.size -= 1;
    ptr::read(arr.contents.add(arr.size as usize))
}

#[inline]
pub unsafe fn array_get_ref<T>(arr: &Array<T>, index: u32) -> &T {
    debug_assert!(index < arr.size);
    ptr_ref(arr.contents.add(index as usize))
}

#[inline]
pub unsafe fn array_get_mut<T>(arr: &mut Array<T>, index: u32) -> &mut T {
    debug_assert!(index < arr.size);
    ptr_mut(arr.contents.add(index as usize))
}

#[inline]
pub unsafe fn array_back_ref<T>(arr: &Array<T>) -> &T {
    debug_assert!(arr.size > 0);
    ptr_ref(arr.contents.add(arr.size as usize - 1))
}

#[inline]
pub unsafe fn array_back_mut<T>(arr: &mut Array<T>) -> &mut T {
    debug_assert!(arr.size > 0);
    ptr_mut(arr.contents.add(arr.size as usize - 1))
}

pub unsafe fn array_erase<T>(arr: &mut Array<T>, index: u32) {
    debug_assert!(index < arr.size);
    let count = arr.size as usize - index as usize - 1;
    if count > 0 {
        ptr::copy(
            arr.contents.add(index as usize + 1),
            arr.contents.add(index as usize),
            count,
        );
    }
    arr.size -= 1;
}

pub unsafe fn array_insert<T>(arr: &mut Array<T>, index: u32, element: T) {
    array_grow(arr, 1);
    let count = arr.size as usize - index as usize;
    if count > 0 {
        ptr::copy(
            arr.contents.add(index as usize),
            arr.contents.add(index as usize + 1),
            count,
        );
    }
    ptr::write(arr.contents.add(index as usize), element);
    arr.size += 1;
}

pub const fn array_new<T>() -> Array<T> {
    Array {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    }
}

pub unsafe fn array_splice<T>(
    arr: &mut Array<T>,
    index: u32,
    old_count: u32,
    new_count: u32,
    new_contents: *const T,
) {
    let new_size = arr.size + new_count - old_count;
    let old_end = index + old_count;
    let new_end = index + new_count;
    debug_assert!(old_end <= arr.size);

    array_reserve(arr, new_size);

    let contents = arr.contents;
    let count = (arr.size - old_end) as usize;
    if count > 0 {
        ptr::copy(
            contents.add(old_end as usize),
            contents.add(new_end as usize),
            count,
        );
    }
    if new_count > 0 && !new_contents.is_null() {
        ptr::copy(
            new_contents,
            contents.add(index as usize),
            new_count as usize,
        );
    }
    arr.size = new_size;
}

pub fn array_swap<T>(self_: &mut Array<T>, other: &mut Array<T>) {
    std::mem::swap(self_, other);
}

pub unsafe fn array_assign<T>(self_: &mut Array<T>, other: &Array<T>) {
    array_reserve(self_, other.size);
    self_.size = other.size;
    if other.size > 0 {
        ptr::copy(other.contents, self_.contents, other.size as usize);
    }
}
