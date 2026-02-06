#![allow(dead_code)]
#![allow(non_upper_case_globals)]

use core::ffi::c_void;

// Default allocator functions that abort on failure
unsafe fn ts_malloc_default(size: usize) -> *mut c_void {
    let result = unsafe { libc_malloc(size) };
    if size > 0 && result.is_null() {
        alloc_failed("allocate", size);
    }
    result
}

unsafe fn ts_calloc_default(count: usize, size: usize) -> *mut c_void {
    let result = unsafe { libc_calloc(count, size) };
    if count > 0 && result.is_null() {
        alloc_failed("allocate", count * size);
    }
    result
}

unsafe fn ts_realloc_default(buffer: *mut c_void, size: usize) -> *mut c_void {
    let result = unsafe { libc_realloc(buffer, size) };
    if size > 0 && result.is_null() {
        alloc_failed("reallocate", size);
    }
    result
}

fn alloc_failed(action: &str, size: usize) -> ! {
    eprintln!("tree-sitter failed to {action} {size} bytes");
    std::process::abort();
}

// C standard library allocation functions
extern "C" {
    #[link_name = "malloc"]
    fn libc_malloc(size: usize) -> *mut c_void;
    #[link_name = "calloc"]
    fn libc_calloc(count: usize, size: usize) -> *mut c_void;
    #[link_name = "realloc"]
    fn libc_realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    #[link_name = "free"]
    fn libc_free(ptr: *mut c_void);
}

// Global function pointers for allocation — these are the symbols that C code calls
// through the ts_malloc/ts_calloc/ts_realloc/ts_free macros.
#[no_mangle]
pub static mut ts_current_malloc: unsafe extern "C" fn(usize) -> *mut c_void = ts_malloc_default_c;
#[no_mangle]
pub static mut ts_current_calloc: unsafe extern "C" fn(usize, usize) -> *mut c_void =
    ts_calloc_default_c;
#[no_mangle]
pub static mut ts_current_realloc: unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void =
    ts_realloc_default_c;
#[no_mangle]
pub static mut ts_current_free: unsafe extern "C" fn(*mut c_void) = libc_free_c;

// C-ABI wrapper functions for the defaults
unsafe extern "C" fn ts_malloc_default_c(size: usize) -> *mut c_void {
    unsafe { ts_malloc_default(size) }
}

unsafe extern "C" fn ts_calloc_default_c(count: usize, size: usize) -> *mut c_void {
    unsafe { ts_calloc_default(count, size) }
}

unsafe extern "C" fn ts_realloc_default_c(buffer: *mut c_void, size: usize) -> *mut c_void {
    unsafe { ts_realloc_default(buffer, size) }
}

unsafe extern "C" fn libc_free_c(ptr: *mut c_void) {
    unsafe { libc_free(ptr) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_set_allocator(
    new_malloc: Option<unsafe extern "C" fn(usize) -> *mut c_void>,
    new_calloc: Option<unsafe extern "C" fn(usize, usize) -> *mut c_void>,
    new_realloc: Option<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void>,
    new_free: Option<unsafe extern "C" fn(*mut c_void)>,
) {
    unsafe {
        ts_current_malloc = new_malloc.unwrap_or(ts_malloc_default_c);
        ts_current_calloc = new_calloc.unwrap_or(ts_calloc_default_c);
        ts_current_realloc = new_realloc.unwrap_or(ts_realloc_default_c);
        ts_current_free = new_free.unwrap_or(libc_free_c);
    }
}

// Convenience wrappers for internal Rust code
#[inline]
pub unsafe fn ts_malloc(size: usize) -> *mut c_void {
    unsafe { (ts_current_malloc)(size) }
}

#[inline]
pub unsafe fn ts_calloc(count: usize, size: usize) -> *mut c_void {
    unsafe { (ts_current_calloc)(count, size) }
}

#[inline]
pub unsafe fn ts_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    unsafe { (ts_current_realloc)(ptr, size) }
}

#[inline]
pub unsafe fn ts_free(ptr: *mut c_void) {
    unsafe { (ts_current_free)(ptr) }
}
