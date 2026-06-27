#![allow(dead_code)]

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
