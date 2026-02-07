#![allow(dead_code, non_upper_case_globals, non_snake_case)]

//! Rust replacement for stack.c/h — GLR parse stack with version management.
//!
//! This module implements the branching parse stack used by the GLR parser.
//! Multiple "versions" of the stack can exist simultaneously, representing
//! different parse paths. Versions can be merged when they reach the same
//! state, enabling efficient ambiguity handling.

use std::ffi::c_void;
use std::ptr;

use crate::ffi::{TSLanguage, TSStateId};

use super::alloc::{ts_calloc, ts_free, ts_malloc, ts_realloc};
use super::error_costs::{ERROR_COST_PER_RECOVERY, ERROR_STATE};
use super::length::{length_add, length_zero, Length};
use super::subtree::{
    ts_builtin_sym_error_repeat, ts_external_scanner_state_data, ts_subtree_alloc_size,
    ts_subtree_child_count, ts_subtree_dynamic_precedence, ts_subtree_error_cost,
    ts_subtree_external_scanner_state_eq, ts_subtree_extra, ts_subtree_is_error,
    ts_subtree_named, ts_subtree_padding, ts_subtree_release, ts_subtree_retain,
    ts_subtree_size, ts_subtree_symbol, ts_subtree_total_bytes, ts_subtree_total_size,
    ts_subtree_visible, ts_subtree_visible_descendant_count, ExternalScannerState,
    Subtree, SubtreeArray, SubtreePool, NULL_SUBTREE,
};
use super::subtree::{ts_subtree_array_copy, ts_subtree_array_delete, ts_subtree_array_reverse};
use super::language::ts_language_write_symbol_as_dot_string;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_LINK_COUNT: usize = 8;
const MAX_NODE_POOL_SIZE: usize = 50;
const MAX_ITERATOR_COUNT: usize = 64;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

pub type StackVersion = u32;
pub const STACK_VERSION_NONE: StackVersion = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StackLink {
    pub node: *mut StackNode,
    pub subtree: Subtree,
    pub is_pending: bool,
}

#[repr(C)]
pub struct StackNode {
    pub state: TSStateId,
    pub position: Length,
    pub links: [StackLink; MAX_LINK_COUNT],
    pub link_count: u16,
    pub ref_count: u32,
    pub error_cost: u32,
    pub node_count: u32,
    pub dynamic_precedence: i32,
}

#[repr(C)]
pub struct StackIterator {
    pub node: *mut StackNode,
    pub subtrees: SubtreeArray,
    pub subtree_count: u32,
    pub is_pending: bool,
}

/// Generic dynamic array, mirrors C `Array(T)`.
#[repr(C)]
pub struct Array<T> {
    pub contents: *mut T,
    pub size: u32,
    pub capacity: u32,
}

pub type StackNodeArray = Array<*mut StackNode>;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StackStatus {
    Active = 0,
    Paused = 1,
    Halted = 2,
}

#[repr(C)]
pub struct StackSlice {
    pub subtrees: SubtreeArray,
    pub version: StackVersion,
}

pub type StackSliceArray = Array<StackSlice>;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StackSummaryEntry {
    pub position: Length,
    pub depth: u32,
    pub state: TSStateId,
}

pub type StackSummary = Array<StackSummaryEntry>;

#[repr(C)]
pub struct StackHead {
    pub node: *mut StackNode,
    pub summary: *mut StackSummary,
    pub node_count_at_last_error: u32,
    pub last_external_token: Subtree,
    pub lookahead_when_paused: Subtree,
    pub status: StackStatus,
}

#[repr(C)]
pub struct Stack {
    pub heads: Array<StackHead>,
    pub slices: StackSliceArray,
    pub iterators: Array<StackIterator>,
    pub node_pool: StackNodeArray,
    pub base_node: *mut StackNode,
    pub subtree_pool: *mut SubtreePool,
}

// ---------------------------------------------------------------------------
// Compile-time layout assertions (sizes from C on 64-bit)
// ---------------------------------------------------------------------------

const _: () = assert!(std::mem::size_of::<StackLink>() == 24);
const _: () = assert!(std::mem::size_of::<StackNode>() == 232);
const _: () = assert!(std::mem::size_of::<StackIterator>() == 32);
const _: () = assert!(std::mem::size_of::<StackStatus>() == 4);
const _: () = assert!(std::mem::size_of::<StackSlice>() == 24);
const _: () = assert!(std::mem::size_of::<StackSummaryEntry>() == 20);
const _: () = assert!(std::mem::size_of::<StackHead>() == 48);
const _: () = assert!(std::mem::size_of::<Stack>() == 80);

pub type StackAction = u32;
pub const StackActionNone: StackAction = 0;
pub const StackActionStop: StackAction = 1;
pub const StackActionPop: StackAction = 2;

pub type StackCallback =
    unsafe extern "C" fn(payload: *mut c_void, iterator: *const StackIterator) -> StackAction;

/// Session state for the summarize callback.
#[repr(C)]
struct SummarizeStackSession {
    summary: *mut StackSummary,
    max_depth: u32,
}

// ---------------------------------------------------------------------------
// Extern C declarations
// ---------------------------------------------------------------------------

extern "C" {
    fn fprintf(f: *mut c_void, format: *const i8, ...) -> i32;
    fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
    fn memmove(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
    fn memset(dest: *mut c_void, c: i32, n: usize) -> *mut c_void;

    #[cfg(target_os = "macos")]
    #[link_name = "__stderrp"]
    static stderr: *mut c_void;

    #[cfg(not(target_os = "macos"))]
    static stderr: *mut c_void;
}

// ---------------------------------------------------------------------------
// Array helper functions (generic, mirrors array.h)
// ---------------------------------------------------------------------------

pub unsafe fn array_init<T>(arr: *mut Array<T>) {
    (*arr).size = 0;
    (*arr).capacity = 0;
    (*arr).contents = ptr::null_mut();
}

pub unsafe fn array_delete<T>(arr: *mut Array<T>) {
    if !(*arr).contents.is_null() {
        ts_free((*arr).contents as *mut c_void);
    }
    (*arr).contents = ptr::null_mut();
    (*arr).size = 0;
    (*arr).capacity = 0;
}

pub unsafe fn array_clear<T>(arr: *mut Array<T>) {
    (*arr).size = 0;
}

pub unsafe fn array_reserve<T>(arr: *mut Array<T>, new_capacity: u32) {
    if new_capacity > (*arr).capacity {
        let elem_size = std::mem::size_of::<T>();
        if (*arr).contents.is_null() {
            (*arr).contents = ts_malloc(new_capacity as usize * elem_size) as *mut T;
        } else {
            (*arr).contents =
                ts_realloc((*arr).contents as *mut c_void, new_capacity as usize * elem_size)
                    as *mut T;
        }
        (*arr).capacity = new_capacity;
    }
}

pub unsafe fn array_grow<T>(arr: *mut Array<T>, count: u32) {
    let new_size = (*arr).size + count;
    if new_size > (*arr).capacity {
        let mut new_capacity = (*arr).capacity * 2;
        if new_capacity < 8 {
            new_capacity = 8;
        }
        if new_capacity < new_size {
            new_capacity = new_size;
        }
        array_reserve(arr, new_capacity);
    }
}

pub unsafe fn array_push<T>(arr: *mut Array<T>, element: T) {
    array_grow(arr, 1);
    ptr::write((*arr).contents.add((*arr).size as usize), element);
    (*arr).size += 1;
}

pub unsafe fn array_pop<T>(arr: *mut Array<T>) -> T {
    (*arr).size -= 1;
    ptr::read((*arr).contents.add((*arr).size as usize))
}

pub unsafe fn array_get<T>(arr: *const Array<T>, index: u32) -> *mut T {
    debug_assert!(index < (*arr).size);
    (*arr).contents.add(index as usize)
}

pub unsafe fn array_back<T>(arr: *const Array<T>) -> *mut T {
    debug_assert!((*arr).size > 0);
    (*arr).contents.add((*arr).size as usize - 1)
}

pub unsafe fn array_erase<T>(arr: *mut Array<T>, index: u32) {
    debug_assert!(index < (*arr).size);
    let elem_size = std::mem::size_of::<T>();
    let contents = (*arr).contents as *mut u8;
    memmove(
        contents.add(index as usize * elem_size) as *mut c_void,
        contents.add((index as usize + 1) * elem_size) as *const c_void,
        ((*arr).size as usize - index as usize - 1) * elem_size,
    );
    (*arr).size -= 1;
}

pub unsafe fn array_insert<T>(arr: *mut Array<T>, index: u32, element: T) {
    let elem_size = std::mem::size_of::<T>();
    array_grow(arr, 1);
    let contents = (*arr).contents as *mut u8;
    if ((*arr).size as usize) > index as usize {
        memmove(
            contents.add((index as usize + 1) * elem_size) as *mut c_void,
            contents.add(index as usize * elem_size) as *const c_void,
            ((*arr).size as usize - index as usize) * elem_size,
        );
    }
    *(*arr).contents.add(index as usize) = element;
    (*arr).size += 1;
}

pub unsafe fn array_front<T>(arr: *const Array<T>) -> *mut T {
    array_get(arr, 0)
}

pub unsafe fn array_new<T>() -> Array<T> {
    Array {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    }
}

pub unsafe fn array_splice<T>(
    arr: *mut Array<T>,
    index: u32,
    old_count: u32,
    new_count: u32,
    new_contents: *const T,
) {
    let elem_size = std::mem::size_of::<T>();
    let new_size = (*arr).size + new_count - old_count;
    let old_end = index + old_count;
    let new_end = index + new_count;
    debug_assert!(old_end <= (*arr).size);

    array_reserve(arr, new_size);

    let contents = (*arr).contents as *mut u8;
    if (*arr).size > old_end {
        memmove(
            contents.add(new_end as usize * elem_size) as *mut c_void,
            contents.add(old_end as usize * elem_size) as *const c_void,
            ((*arr).size - old_end) as usize * elem_size,
        );
    }
    if new_count > 0 {
        if !new_contents.is_null() {
            memcpy(
                contents.add(index as usize * elem_size) as *mut c_void,
                new_contents as *const c_void,
                new_count as usize * elem_size,
            );
        }
    }
    (*arr).size = new_size;
}

pub unsafe fn array_swap<T>(self_: *mut Array<T>, other: *mut Array<T>) {
    let tmp_contents = (*self_).contents;
    let tmp_size = (*self_).size;
    let tmp_capacity = (*self_).capacity;
    (*self_).contents = (*other).contents;
    (*self_).size = (*other).size;
    (*self_).capacity = (*other).capacity;
    (*other).contents = tmp_contents;
    (*other).size = tmp_size;
    (*other).capacity = tmp_capacity;
}

pub unsafe fn array_assign<T>(self_: *mut Array<T>, other: *const Array<T>) {
    let elem_size = std::mem::size_of::<T>();
    array_reserve(self_, (*other).size);
    (*self_).size = (*other).size;
    if (*other).size > 0 {
        memcpy(
            (*self_).contents as *mut c_void,
            (*other).contents as *const c_void,
            (*other).size as usize * elem_size,
        );
    }
}

// ---------------------------------------------------------------------------
// Internal (static) functions
// ---------------------------------------------------------------------------

/// Retain (increment ref count) a stack node.
unsafe fn stack_node_retain(self_: *mut StackNode) {
    if self_.is_null() {
        return;
    }
    debug_assert!((*self_).ref_count > 0);
    (*self_).ref_count += 1;
    debug_assert!((*self_).ref_count != 0);
}

/// Release (decrement ref count) a stack node, freeing if zero.
unsafe fn stack_node_release(
    mut self_: *mut StackNode,
    pool: *mut StackNodeArray,
    subtree_pool: *mut SubtreePool,
) {
    loop {
        debug_assert!((*self_).ref_count != 0);
        (*self_).ref_count -= 1;
        if (*self_).ref_count > 0 {
            return;
        }

        let mut first_predecessor: *mut StackNode = ptr::null_mut();
        if (*self_).link_count > 0 {
            let mut i = (*self_).link_count as i32 - 1;
            while i > 0 {
                let link = (*self_).links[i as usize];
                if !link.subtree.ptr.is_null() {
                    ts_subtree_release(subtree_pool, link.subtree);
                }
                stack_node_release(link.node, pool, subtree_pool);
                i -= 1;
            }
            let link = (*self_).links[0];
            if !link.subtree.ptr.is_null() {
                ts_subtree_release(subtree_pool, link.subtree);
            }
            first_predecessor = (*self_).links[0].node;
        }

        if (*pool).size < MAX_NODE_POOL_SIZE as u32 {
            array_push(pool, self_);
        } else {
            ts_free(self_ as *mut c_void);
        }

        if !first_predecessor.is_null() {
            self_ = first_predecessor;
            continue; // goto recur
        }
        return;
    }
}

/// Count visible nodes in a subtree for progress tracking.
unsafe fn stack__subtree_node_count(subtree: Subtree) -> u32 {
    let mut count = ts_subtree_visible_descendant_count(subtree);
    if ts_subtree_visible(subtree) {
        count += 1;
    }
    if ts_subtree_symbol(subtree) == ts_builtin_sym_error_repeat {
        count += 1;
    }
    count
}

/// Allocate a new stack node, reusing from pool if available.
unsafe fn stack_node_new(
    previous_node: *mut StackNode,
    subtree: Subtree,
    is_pending: bool,
    state: TSStateId,
    pool: *mut StackNodeArray,
) -> *mut StackNode {
    let node: *mut StackNode = if (*pool).size > 0 {
        array_pop(pool)
    } else {
        ts_malloc(std::mem::size_of::<StackNode>()) as *mut StackNode
    };

    (*node).ref_count = 1;
    (*node).link_count = 0;
    (*node).state = state;

    if !previous_node.is_null() {
        (*node).link_count = 1;
        (*node).links[0] = StackLink {
            node: previous_node,
            subtree,
            is_pending,
        };

        (*node).position = (*previous_node).position;
        (*node).error_cost = (*previous_node).error_cost;
        (*node).dynamic_precedence = (*previous_node).dynamic_precedence;
        (*node).node_count = (*previous_node).node_count;

        if !subtree.ptr.is_null() {
            (*node).error_cost += ts_subtree_error_cost(subtree);
            (*node).position = length_add((*node).position, ts_subtree_total_size(subtree));
            (*node).node_count += stack__subtree_node_count(subtree);
            (*node).dynamic_precedence += ts_subtree_dynamic_precedence(subtree);
        }
    } else {
        (*node).position = length_zero();
        (*node).error_cost = 0;
    }

    node
}

/// Check if two subtrees are equivalent for merging purposes.
unsafe fn stack__subtree_is_equivalent(left: Subtree, right: Subtree) -> bool {
    if left.ptr == right.ptr {
        return true;
    }
    if left.ptr.is_null() || right.ptr.is_null() {
        return false;
    }

    if ts_subtree_symbol(left) != ts_subtree_symbol(right) {
        return false;
    }

    if ts_subtree_error_cost(left) > 0 && ts_subtree_error_cost(right) > 0 {
        return true;
    }

    ts_subtree_padding(left).bytes == ts_subtree_padding(right).bytes
        && ts_subtree_size(left).bytes == ts_subtree_size(right).bytes
        && ts_subtree_child_count(left) == ts_subtree_child_count(right)
        && ts_subtree_extra(left) == ts_subtree_extra(right)
        && ts_subtree_external_scanner_state_eq(left, right)
}

/// Add a link to a stack node, merging if possible.
unsafe fn stack_node_add_link(
    self_: *mut StackNode,
    link: StackLink,
    subtree_pool: *mut SubtreePool,
) {
    if link.node == self_ {
        return;
    }

    for i in 0..(*self_).link_count as usize {
        let existing_link = &mut (*self_).links[i];
        if stack__subtree_is_equivalent(existing_link.subtree, link.subtree) {
            if existing_link.node == link.node {
                if ts_subtree_dynamic_precedence(link.subtree)
                    > ts_subtree_dynamic_precedence(existing_link.subtree)
                {
                    ts_subtree_retain(link.subtree);
                    ts_subtree_release(subtree_pool, existing_link.subtree);
                    existing_link.subtree = link.subtree;
                    (*self_).dynamic_precedence = (*link.node).dynamic_precedence
                        + ts_subtree_dynamic_precedence(link.subtree);
                }
                return;
            }

            if (*existing_link.node).state == (*link.node).state
                && (*existing_link.node).position.bytes == (*link.node).position.bytes
                && (*existing_link.node).error_cost == (*link.node).error_cost
            {
                for j in 0..(*link.node).link_count as usize {
                    stack_node_add_link(existing_link.node, (*link.node).links[j], subtree_pool);
                }
                let mut dynamic_precedence = (*link.node).dynamic_precedence;
                if !link.subtree.ptr.is_null() {
                    dynamic_precedence += ts_subtree_dynamic_precedence(link.subtree);
                }
                if dynamic_precedence > (*self_).dynamic_precedence {
                    (*self_).dynamic_precedence = dynamic_precedence;
                }
                return;
            }
        }
    }

    if (*self_).link_count as usize == MAX_LINK_COUNT {
        return;
    }

    stack_node_retain(link.node);
    let mut node_count = (*link.node).node_count;
    let mut dynamic_precedence = (*link.node).dynamic_precedence;
    (*self_).links[(*self_).link_count as usize] = link;
    (*self_).link_count += 1;

    if !link.subtree.ptr.is_null() {
        ts_subtree_retain(link.subtree);
        node_count += stack__subtree_node_count(link.subtree);
        dynamic_precedence += ts_subtree_dynamic_precedence(link.subtree);
    }

    if node_count > (*self_).node_count {
        (*self_).node_count = node_count;
    }
    if dynamic_precedence > (*self_).dynamic_precedence {
        (*self_).dynamic_precedence = dynamic_precedence;
    }
}

/// Delete a stack head, releasing its node and subtrees.
unsafe fn stack_head_delete(
    self_: *mut StackHead,
    pool: *mut StackNodeArray,
    subtree_pool: *mut SubtreePool,
) {
    if !(*self_).node.is_null() {
        if !(*self_).last_external_token.ptr.is_null() {
            ts_subtree_release(subtree_pool, (*self_).last_external_token);
        }
        if !(*self_).lookahead_when_paused.ptr.is_null() {
            ts_subtree_release(subtree_pool, (*self_).lookahead_when_paused);
        }
        if !(*self_).summary.is_null() {
            array_delete((*self_).summary);
            ts_free((*self_).summary as *mut c_void);
        }
        stack_node_release((*self_).node, pool, subtree_pool);
    }
}

/// Add a new version to the stack, cloning metadata from an existing version.
unsafe fn ts_stack__add_version(
    self_: *mut Stack,
    original_version: StackVersion,
    node: *mut StackNode,
) -> StackVersion {
    let original_head = &*array_get(&(*self_).heads, original_version);
    let head = StackHead {
        node,
        node_count_at_last_error: original_head.node_count_at_last_error,
        last_external_token: original_head.last_external_token,
        status: StackStatus::Active,
        lookahead_when_paused: NULL_SUBTREE,
        summary: ptr::null_mut(),
    };
    array_push(&mut (*self_).heads, head);
    stack_node_retain(node);
    if !(*array_back(&(*self_).heads)).last_external_token.ptr.is_null() {
        ts_subtree_retain((*array_back(&(*self_).heads)).last_external_token);
    }
    (*self_).heads.size - 1
}

/// Add a slice to the stack's slice array, finding or creating a version.
unsafe fn ts_stack__add_slice(
    self_: *mut Stack,
    original_version: StackVersion,
    node: *mut StackNode,
    subtrees: *mut SubtreeArray,
) {
    let mut i = (*self_).slices.size as i32 - 1;
    while i + 1 > 0 {
        let version = (*array_get(&(*self_).slices, i as u32)).version;
        if (*array_get(&(*self_).heads, version)).node == node {
            let slice = StackSlice {
                subtrees: ptr::read(subtrees),
                version,
            };
            array_insert(&mut (*self_).slices, (i + 1) as u32, slice);
            return;
        }
        i -= 1;
    }

    let version = ts_stack__add_version(self_, original_version, node);
    let slice = StackSlice {
        subtrees: ptr::read(subtrees),
        version,
    };
    array_push(&mut (*self_).slices, slice);
}

/// Core iteration function for walking the stack graph.
unsafe fn stack__iter(
    self_: *mut Stack,
    version: StackVersion,
    callback: StackCallback,
    payload: *mut c_void,
    goal_subtree_count: i32,
) -> StackSliceArray {
    array_clear(&mut (*self_).slices);
    array_clear(&mut (*self_).iterators);

    let head = &*array_get(&(*self_).heads, version);
    let mut new_iterator = StackIterator {
        node: head.node,
        subtrees: SubtreeArray {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
        },
        subtree_count: 0,
        is_pending: true,
    };

    let mut include_subtrees = false;
    if goal_subtree_count >= 0 {
        include_subtrees = true;
        let reserve_count =
            ts_subtree_alloc_size(goal_subtree_count as u32) / std::mem::size_of::<Subtree>();
        array_reserve(
            &mut new_iterator.subtrees as *mut SubtreeArray as *mut Array<Subtree>,
            reserve_count as u32,
        );
    }

    array_push(&mut (*self_).iterators, new_iterator);

    while (*self_).iterators.size > 0 {
        let mut i: u32 = 0;
        let mut size = (*self_).iterators.size;
        while i < size {
            let iterator = &*array_get(&(*self_).iterators, i);
            let node = iterator.node;

            let action = callback(payload, iterator as *const StackIterator);
            let should_pop = (action & StackActionPop) != 0;
            let should_stop = (action & StackActionStop) != 0 || (*node).link_count == 0;

            if should_pop {
                let mut subtrees = ptr::read(&(*array_get(&(*self_).iterators, i)).subtrees);
                if !should_stop {
                    ts_subtree_array_copy(ptr::read(&subtrees), &mut subtrees);
                }
                ts_subtree_array_reverse(&mut subtrees);
                ts_stack__add_slice(self_, version, node, &mut subtrees);
            }

            if should_stop {
                if !should_pop {
                    let iter = &mut *array_get(&mut (*self_).iterators, i);
                    ts_subtree_array_delete((*self_).subtree_pool, &mut iter.subtrees);
                }
                array_erase(&mut (*self_).iterators, i);
                i = i.wrapping_sub(1);
                size -= 1;
                i = i.wrapping_add(1);
                continue;
            }

            let mut j: u32 = 1;
            while j <= (*node).link_count as u32 {
                let next_iterator: *mut StackIterator;
                let link: StackLink;
                if j == (*node).link_count as u32 {
                    link = (*node).links[0];
                    next_iterator = array_get(&mut (*self_).iterators, i);
                } else {
                    if (*self_).iterators.size >= MAX_ITERATOR_COUNT as u32 {
                        j += 1;
                        continue;
                    }
                    link = (*node).links[j as usize];
                    let current_iterator = ptr::read(array_get(&(*self_).iterators, i));
                    array_push(&mut (*self_).iterators, current_iterator);
                    next_iterator = array_back(&(*self_).iterators);
                    ts_subtree_array_copy(
                        ptr::read(&(*next_iterator).subtrees),
                        &mut (*next_iterator).subtrees,
                    );
                }

                (*next_iterator).node = link.node;
                if !link.subtree.ptr.is_null() {
                    if include_subtrees {
                        array_push(
                            &mut (*next_iterator).subtrees as *mut SubtreeArray
                                as *mut Array<Subtree>,
                            link.subtree,
                        );
                        ts_subtree_retain(link.subtree);
                    }

                    if !ts_subtree_extra(link.subtree) {
                        (*next_iterator).subtree_count += 1;
                        if !link.is_pending {
                            (*next_iterator).is_pending = false;
                        }
                    }
                } else {
                    (*next_iterator).subtree_count += 1;
                    (*next_iterator).is_pending = false;
                }
                j += 1;
            }
            i = i.wrapping_add(1);
        }
    }

    ptr::read(&(*self_).slices)
}

// Callbacks for stack__iter
unsafe extern "C" fn pop_count_callback(
    payload: *mut c_void,
    iterator: *const StackIterator,
) -> StackAction {
    let goal_subtree_count = *(payload as *const u32);
    if (*iterator).subtree_count == goal_subtree_count {
        StackActionPop | StackActionStop
    } else {
        StackActionNone
    }
}

unsafe extern "C" fn pop_pending_callback(
    _payload: *mut c_void,
    iterator: *const StackIterator,
) -> StackAction {
    if (*iterator).subtree_count >= 1 {
        if (*iterator).is_pending {
            StackActionPop | StackActionStop
        } else {
            StackActionStop
        }
    } else {
        StackActionNone
    }
}

unsafe extern "C" fn pop_error_callback(
    payload: *mut c_void,
    iterator: *const StackIterator,
) -> StackAction {
    if (*iterator).subtrees.size > 0 {
        let found_error = &mut *(payload as *mut bool);
        if !*found_error
            && ts_subtree_is_error(*(*iterator).subtrees.contents.add(0))
        {
            *found_error = true;
            StackActionPop | StackActionStop
        } else {
            StackActionStop
        }
    } else {
        StackActionNone
    }
}

unsafe extern "C" fn pop_all_callback(
    _payload: *mut c_void,
    iterator: *const StackIterator,
) -> StackAction {
    if (*(*iterator).node).link_count == 0 {
        StackActionPop
    } else {
        StackActionNone
    }
}

unsafe extern "C" fn summarize_stack_callback(
    payload: *mut c_void,
    iterator: *const StackIterator,
) -> StackAction {
    let session = &mut *(payload as *mut SummarizeStackSession);
    let state = (*(*iterator).node).state;
    let depth = (*iterator).subtree_count;
    if depth > session.max_depth {
        return StackActionStop;
    }
    let mut i = (*session.summary).size as i32 - 1;
    while i + 1 > 0 {
        let entry = &*array_get(session.summary, i as u32);
        if entry.depth < depth {
            break;
        }
        if entry.depth == depth && entry.state == state {
            return StackActionNone;
        }
        i -= 1;
    }
    array_push(
        session.summary,
        StackSummaryEntry {
            position: (*(*iterator).node).position,
            depth,
            state,
        },
    );
    StackActionNone
}

// ===========================================================================
// Exported functions from stack.h (called by parser.c)
// ===========================================================================

/// Create a new parse stack.
pub unsafe fn ts_stack_new(subtree_pool: *mut SubtreePool) -> *mut Stack {
    let self_ = ts_calloc(1, std::mem::size_of::<Stack>()) as *mut Stack;

    array_init(&mut (*self_).heads);
    array_init(&mut (*self_).slices);
    array_init(&mut (*self_).iterators);
    array_init(&mut (*self_).node_pool);
    array_reserve(&mut (*self_).heads, 4);
    array_reserve(&mut (*self_).slices, 4);
    array_reserve(&mut (*self_).iterators, 4);
    array_reserve(&mut (*self_).node_pool, MAX_NODE_POOL_SIZE as u32);

    (*self_).subtree_pool = subtree_pool;
    (*self_).base_node =
        stack_node_new(ptr::null_mut(), NULL_SUBTREE, false, 1, &mut (*self_).node_pool);
    ts_stack_clear(self_);

    self_
}

/// Free the parse stack.
pub unsafe fn ts_stack_delete(self_: *mut Stack) {
    if !(*self_).slices.contents.is_null() {
        array_delete(&mut (*self_).slices);
    }
    if !(*self_).iterators.contents.is_null() {
        array_delete(&mut (*self_).iterators);
    }
    stack_node_release((*self_).base_node, &mut (*self_).node_pool, (*self_).subtree_pool);
    for i in 0..(*self_).heads.size {
        stack_head_delete(
            array_get(&mut (*self_).heads, i),
            &mut (*self_).node_pool,
            (*self_).subtree_pool,
        );
    }
    array_clear(&mut (*self_).heads);
    if !(*self_).node_pool.contents.is_null() {
        for i in 0..(*self_).node_pool.size {
            ts_free(*array_get(&(*self_).node_pool, i) as *mut c_void);
        }
        array_delete(&mut (*self_).node_pool);
    }
    array_delete(&mut (*self_).heads);
    ts_free(self_ as *mut c_void);
}

/// Get the number of versions in the stack.
pub unsafe fn ts_stack_version_count(self_: *const Stack) -> u32 {
    (*self_).heads.size
}

/// Get the number of halted versions.
pub unsafe fn ts_stack_halted_version_count(self_: *mut Stack) -> u32 {
    let mut count = 0u32;
    for i in 0..(*self_).heads.size {
        let head = &*array_get(&(*self_).heads, i);
        if head.status == StackStatus::Halted {
            count += 1;
        }
    }
    count
}

/// Get the state at the top of a version.
pub unsafe fn ts_stack_state(self_: *const Stack, version: StackVersion) -> TSStateId {
    (*(*array_get(&(*self_).heads, version)).node).state
}

/// Get the position of a version.
pub unsafe fn ts_stack_position(self_: *const Stack, version: StackVersion) -> Length {
    (*(*array_get(&(*self_).heads, version)).node).position
}

/// Get the last external token for a version.
pub unsafe fn ts_stack_last_external_token(
    self_: *const Stack,
    version: StackVersion,
) -> Subtree {
    (*array_get(&(*self_).heads, version)).last_external_token
}

/// Set the last external token for a version.
pub unsafe fn ts_stack_set_last_external_token(
    self_: *mut Stack,
    version: StackVersion,
    token: Subtree,
) {
    let head = &mut *array_get(&mut (*self_).heads, version);
    if !token.ptr.is_null() {
        ts_subtree_retain(token);
    }
    if !head.last_external_token.ptr.is_null() {
        ts_subtree_release((*self_).subtree_pool, head.last_external_token);
    }
    head.last_external_token = token;
}

/// Get the error cost for a version.
pub unsafe fn ts_stack_error_cost(self_: *const Stack, version: StackVersion) -> u32 {
    let head = &*array_get(&(*self_).heads, version);
    let mut result = (*head.node).error_cost;
    if head.status == StackStatus::Paused
        || ((*head.node).state == ERROR_STATE
            && (*head.node).links[0].subtree.ptr.is_null())
    {
        result += ERROR_COST_PER_RECOVERY;
    }
    result
}

/// Get the node count since last error for a version.
pub unsafe fn ts_stack_node_count_since_error(
    self_: *const Stack,
    version: StackVersion,
) -> u32 {
    let head = &mut *array_get(&(*self_).heads, version);
    if (*head.node).node_count < head.node_count_at_last_error {
        head.node_count_at_last_error = (*head.node).node_count;
    }
    (*head.node).node_count - head.node_count_at_last_error
}

/// Push a subtree onto a version.
pub unsafe fn ts_stack_push(
    self_: *mut Stack,
    version: StackVersion,
    subtree: Subtree,
    pending: bool,
    state: TSStateId,
) {
    let head = &mut *array_get(&mut (*self_).heads, version);
    let new_node = stack_node_new(head.node, subtree, pending, state, &mut (*self_).node_pool);
    if subtree.ptr.is_null() {
        head.node_count_at_last_error = (*new_node).node_count;
    }
    head.node = new_node;
}

/// Pop a given number of entries from a version.
pub unsafe fn ts_stack_pop_count(
    self_: *mut Stack,
    version: StackVersion,
    count: u32,
) -> StackSliceArray {
    stack__iter(
        self_,
        version,
        pop_count_callback,
        &count as *const u32 as *mut c_void,
        count as i32,
    )
}

/// Pop an error from the top of a version.
pub unsafe fn ts_stack_pop_error(
    self_: *mut Stack,
    version: StackVersion,
) -> SubtreeArray {
    let node = (*array_get(&(*self_).heads, version)).node;
    for i in 0..(*node).link_count as usize {
        if !(*node).links[i].subtree.ptr.is_null()
            && ts_subtree_is_error((*node).links[i].subtree)
        {
            let mut found_error = false;
            let pop = stack__iter(
                self_,
                version,
                pop_error_callback,
                &mut found_error as *mut bool as *mut c_void,
                1,
            );
            if pop.size > 0 {
                debug_assert!(pop.size == 1);
                ts_stack_renumber_version(self_, (*array_get(&pop, 0)).version, version);
                return ptr::read(&(*array_get(&pop, 0)).subtrees);
            }
            break;
        }
    }
    SubtreeArray {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    }
}

/// Pop pending entries from a version.
pub unsafe fn ts_stack_pop_pending(
    self_: *mut Stack,
    version: StackVersion,
) -> StackSliceArray {
    let pop = stack__iter(
        self_,
        version,
        pop_pending_callback,
        ptr::null_mut(),
        0,
    );
    if pop.size > 0 {
        ts_stack_renumber_version(self_, (*array_get(&pop, 0)).version, version);
        (*array_get(&pop, 0)).version = version;
    }
    pop
}

/// Pop all entries from a version.
pub unsafe fn ts_stack_pop_all(
    self_: *mut Stack,
    version: StackVersion,
) -> StackSliceArray {
    stack__iter(self_, version, pop_all_callback, ptr::null_mut(), 0)
}

/// Record a summary of parse states near the top of a version.
pub unsafe fn ts_stack_record_summary(
    self_: *mut Stack,
    version: StackVersion,
    max_depth: u32,
) {
    let mut session = SummarizeStackSession {
        summary: ts_malloc(std::mem::size_of::<StackSummary>()) as *mut StackSummary,
        max_depth,
    };
    array_init(session.summary);
    stack__iter(
        self_,
        version,
        summarize_stack_callback,
        &mut session as *mut SummarizeStackSession as *mut c_void,
        -1,
    );
    let head = &mut *array_get(&mut (*self_).heads, version);
    if !head.summary.is_null() {
        array_delete(head.summary);
        ts_free(head.summary as *mut c_void);
    }
    head.summary = session.summary;
}

/// Get the recorded summary for a version.
pub unsafe fn ts_stack_get_summary(
    self_: *mut Stack,
    version: StackVersion,
) -> *mut StackSummary {
    (*array_get(&(*self_).heads, version)).summary
}

/// Get the dynamic precedence of a version.
pub unsafe fn ts_stack_dynamic_precedence(
    self_: *mut Stack,
    version: StackVersion,
) -> i32 {
    (*(*array_get(&(*self_).heads, version)).node).dynamic_precedence
}

/// Check if a version has advanced since the last error.
pub unsafe fn ts_stack_has_advanced_since_error(
    self_: *const Stack,
    version: StackVersion,
) -> bool {
    let head = &*array_get(&(*self_).heads, version);
    let mut node = head.node;
    if (*node).error_cost == 0 {
        return true;
    }
    loop {
        if (*node).link_count > 0 {
            let subtree = (*node).links[0].subtree;
            if !subtree.ptr.is_null() {
                if ts_subtree_total_bytes(subtree) > 0 {
                    return true;
                } else if (*node).node_count > head.node_count_at_last_error
                    && ts_subtree_error_cost(subtree) == 0
                {
                    node = (*node).links[0].node;
                    continue;
                }
            }
        }
        break;
    }
    false
}

/// Remove a version from the stack.
pub unsafe fn ts_stack_remove_version(self_: *mut Stack, version: StackVersion) {
    stack_head_delete(
        array_get(&mut (*self_).heads, version),
        &mut (*self_).node_pool,
        (*self_).subtree_pool,
    );
    array_erase(&mut (*self_).heads, version);
}

/// Renumber version v1 to v2 (move v1 into v2's slot, removing v2).
pub unsafe fn ts_stack_renumber_version(
    self_: *mut Stack,
    v1: StackVersion,
    v2: StackVersion,
) {
    if v1 == v2 {
        return;
    }
    debug_assert!(v2 < v1);
    debug_assert!(v1 < (*self_).heads.size);
    let source_head = &mut *array_get(&mut (*self_).heads, v1);
    let target_head = &mut *array_get(&mut (*self_).heads, v2);
    if !target_head.summary.is_null() && source_head.summary.is_null() {
        source_head.summary = target_head.summary;
        target_head.summary = ptr::null_mut();
    }
    stack_head_delete(
        target_head as *mut StackHead,
        &mut (*self_).node_pool,
        (*self_).subtree_pool,
    );
    *target_head = ptr::read(source_head);
    array_erase(&mut (*self_).heads, v1);
}

/// Swap two versions.
pub unsafe fn ts_stack_swap_versions(
    self_: *mut Stack,
    v1: StackVersion,
    v2: StackVersion,
) {
    let temp = ptr::read(array_get(&(*self_).heads, v1));
    ptr::write(
        array_get(&mut (*self_).heads, v1),
        ptr::read(array_get(&(*self_).heads, v2)),
    );
    ptr::write(array_get(&mut (*self_).heads, v2), temp);
}

/// Copy a version, creating a new one.
pub unsafe fn ts_stack_copy_version(
    self_: *mut Stack,
    version: StackVersion,
) -> StackVersion {
    debug_assert!(version < (*self_).heads.size);
    let version_head = ptr::read(array_get(&(*self_).heads, version));
    array_push(&mut (*self_).heads, version_head);
    let head = &mut *array_back(&(*self_).heads);
    stack_node_retain(head.node);
    if !head.last_external_token.ptr.is_null() {
        ts_subtree_retain(head.last_external_token);
    }
    head.summary = ptr::null_mut();
    (*self_).heads.size - 1
}

/// Merge two versions if possible.
pub unsafe fn ts_stack_merge(
    self_: *mut Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    if !ts_stack_can_merge(self_, version1, version2) {
        return false;
    }
    let head1 = &mut *array_get(&mut (*self_).heads, version1);
    let head2 = &*array_get(&(*self_).heads, version2);
    for i in 0..(*head2.node).link_count as usize {
        stack_node_add_link(head1.node, (*head2.node).links[i], (*self_).subtree_pool);
    }
    if (*head1.node).state == ERROR_STATE {
        head1.node_count_at_last_error = (*head1.node).node_count;
    }
    ts_stack_remove_version(self_, version2);
    true
}

/// Check if two versions can be merged.
pub unsafe fn ts_stack_can_merge(
    self_: *mut Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    let head1 = &*array_get(&(*self_).heads, version1);
    let head2 = &*array_get(&(*self_).heads, version2);
    head1.status == StackStatus::Active
        && head2.status == StackStatus::Active
        && (*head1.node).state == (*head2.node).state
        && (*head1.node).position.bytes == (*head2.node).position.bytes
        && (*head1.node).error_cost == (*head2.node).error_cost
        && ts_subtree_external_scanner_state_eq(
            head1.last_external_token,
            head2.last_external_token,
        )
}

/// Halt a version.
pub unsafe fn ts_stack_halt(self_: *mut Stack, version: StackVersion) {
    (*array_get(&mut (*self_).heads, version)).status = StackStatus::Halted;
}

/// Pause a version with a lookahead token.
pub unsafe fn ts_stack_pause(
    self_: *mut Stack,
    version: StackVersion,
    lookahead: Subtree,
) {
    let head = &mut *array_get(&mut (*self_).heads, version);
    head.status = StackStatus::Paused;
    head.lookahead_when_paused = lookahead;
    head.node_count_at_last_error = (*head.node).node_count;
}

/// Check if a version is active.
pub unsafe fn ts_stack_is_active(self_: *const Stack, version: StackVersion) -> bool {
    (*array_get(&(*self_).heads, version)).status == StackStatus::Active
}

/// Check if a version is halted.
pub unsafe fn ts_stack_is_halted(self_: *const Stack, version: StackVersion) -> bool {
    (*array_get(&(*self_).heads, version)).status == StackStatus::Halted
}

/// Check if a version is paused.
pub unsafe fn ts_stack_is_paused(self_: *const Stack, version: StackVersion) -> bool {
    (*array_get(&(*self_).heads, version)).status == StackStatus::Paused
}

/// Resume a paused version, returning its stored lookahead.
pub unsafe fn ts_stack_resume(
    self_: *mut Stack,
    version: StackVersion,
) -> Subtree {
    let head = &mut *array_get(&mut (*self_).heads, version);
    debug_assert!(head.status == StackStatus::Paused);
    let result = head.lookahead_when_paused;
    head.status = StackStatus::Active;
    head.lookahead_when_paused = NULL_SUBTREE;
    result
}

/// Clear all versions, resetting to initial state.
pub unsafe fn ts_stack_clear(self_: *mut Stack) {
    stack_node_retain((*self_).base_node);
    for i in 0..(*self_).heads.size {
        stack_head_delete(
            array_get(&mut (*self_).heads, i),
            &mut (*self_).node_pool,
            (*self_).subtree_pool,
        );
    }
    array_clear(&mut (*self_).heads);
    array_push(
        &mut (*self_).heads,
        StackHead {
            node: (*self_).base_node,
            status: StackStatus::Active,
            last_external_token: NULL_SUBTREE,
            lookahead_when_paused: NULL_SUBTREE,
            summary: ptr::null_mut(),
            node_count_at_last_error: 0,
        },
    );
}

/// Print the stack as a DOT graph for debugging.
pub unsafe fn ts_stack_print_dot_graph(
    self_: *mut Stack,
    language: *const TSLanguage,
    mut f: *mut c_void,
) -> bool {
    array_reserve(&mut (*self_).iterators, 32);
    if f.is_null() {
        f = stderr;
    }

    fprintf(f, b"digraph stack {\n\0".as_ptr() as *const i8);
    fprintf(f, b"rankdir=\"RL\";\n\0".as_ptr() as *const i8);
    fprintf(
        f,
        b"edge [arrowhead=none]\n\0".as_ptr() as *const i8,
    );

    let mut visited_nodes: Array<*mut StackNode> = Array {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    };

    array_clear(&mut (*self_).iterators);
    for i in 0..(*self_).heads.size {
        let head = &*array_get(&(*self_).heads, i);
        if head.status == StackStatus::Halted {
            continue;
        }

        fprintf(
            f,
            b"node_head_%u [shape=none, label=\"\"]\n\0".as_ptr() as *const i8,
            i,
        );
        fprintf(
            f,
            b"node_head_%u -> node_%p [\0".as_ptr() as *const i8,
            i,
            head.node as *const c_void,
        );

        if head.status == StackStatus::Paused {
            fprintf(f, b"color=red \0".as_ptr() as *const i8);
        }
        fprintf(
            f,
            b"label=%u, fontcolor=blue, weight=10000, labeltooltip=\"node_count: %u\nerror_cost: %u\0".as_ptr() as *const i8,
            i,
            ts_stack_node_count_since_error(self_, i),
            ts_stack_error_cost(self_, i),
        );

        if !head.summary.is_null() {
            fprintf(f, b"\nsummary:\0".as_ptr() as *const i8);
            for j in 0..(*head.summary).size {
                fprintf(
                    f,
                    b" %u\0".as_ptr() as *const i8,
                    (*array_get(head.summary, j)).state as u32,
                );
            }
        }

        if !head.last_external_token.ptr.is_null() {
            let state = &*(*head.last_external_token.ptr).data.external_scanner_state as *const ExternalScannerState;
            let data = ts_external_scanner_state_data(state);
            fprintf(
                f,
                b"\nexternal_scanner_state:\0".as_ptr() as *const i8,
            );
            for j in 0..(*state).length {
                fprintf(
                    f,
                    b" %2X\0".as_ptr() as *const i8,
                    *data.add(j as usize) as u32,
                );
            }
        }

        fprintf(f, b"\"]\n\0".as_ptr() as *const i8);

        let iter = StackIterator {
            node: head.node,
            subtrees: SubtreeArray {
                contents: ptr::null_mut(),
                size: 0,
                capacity: 0,
            },
            subtree_count: 0,
            is_pending: false,
        };
        array_push(&mut (*self_).iterators, iter);
    }

    let mut all_iterators_done = false;
    while !all_iterators_done {
        all_iterators_done = true;

        for i in 0..(*self_).iterators.size {
            let iterator = ptr::read(array_get(&(*self_).iterators, i));
            let mut node = iterator.node;

            for j in 0..visited_nodes.size {
                if *array_get(&visited_nodes, j) == node {
                    node = ptr::null_mut();
                    break;
                }
            }

            if node.is_null() {
                continue;
            }
            all_iterators_done = false;

            fprintf(f, b"node_%p [\0".as_ptr() as *const i8, node as *const c_void);
            if (*node).state == ERROR_STATE {
                fprintf(f, b"label=\"?\"\0".as_ptr() as *const i8);
            } else if (*node).link_count == 1
                && !(*node).links[0].subtree.ptr.is_null()
                && ts_subtree_extra((*node).links[0].subtree)
            {
                fprintf(
                    f,
                    b"shape=point margin=0 label=\"\"\0".as_ptr() as *const i8,
                );
            } else {
                fprintf(
                    f,
                    b"label=\"%d\"\0".as_ptr() as *const i8,
                    (*node).state as i32,
                );
            }

            fprintf(
                f,
                b" tooltip=\"position: %u,%u\nnode_count:%u\nerror_cost: %u\ndynamic_precedence: %d\"];\n\0".as_ptr() as *const i8,
                (*node).position.extent.row + 1,
                (*node).position.extent.column,
                (*node).node_count,
                (*node).error_cost,
                (*node).dynamic_precedence,
            );

            for j in 0..(*node).link_count as usize {
                let link = (*node).links[j];
                fprintf(
                    f,
                    b"node_%p -> node_%p [\0".as_ptr() as *const i8,
                    node as *const c_void,
                    link.node as *const c_void,
                );
                if link.is_pending {
                    fprintf(f, b"style=dashed \0".as_ptr() as *const i8);
                }
                if !link.subtree.ptr.is_null() && ts_subtree_extra(link.subtree) {
                    fprintf(f, b"fontcolor=gray \0".as_ptr() as *const i8);
                }

                if link.subtree.ptr.is_null() {
                    fprintf(f, b"color=red\0".as_ptr() as *const i8);
                } else {
                    fprintf(f, b"label=\"\0".as_ptr() as *const i8);
                    let quoted =
                        ts_subtree_visible(link.subtree) && !ts_subtree_named(link.subtree);
                    if quoted {
                        fprintf(f, b"'\0".as_ptr() as *const i8);
                    }
                    ts_language_write_symbol_as_dot_string(
                        language,
                        f,
                        ts_subtree_symbol(link.subtree),
                    );
                    if quoted {
                        fprintf(f, b"'\0".as_ptr() as *const i8);
                    }
                    fprintf(f, b"\"\0".as_ptr() as *const i8);
                    fprintf(
                        f,
                        b"labeltooltip=\"error_cost: %u\ndynamic_precedence: %d\"\0".as_ptr()
                            as *const i8,
                        ts_subtree_error_cost(link.subtree),
                        ts_subtree_dynamic_precedence(link.subtree),
                    );
                }

                fprintf(f, b"];\n\0".as_ptr() as *const i8);

                let next_iterator: *mut StackIterator;
                if j == 0 {
                    next_iterator = array_get(&mut (*self_).iterators, i);
                } else {
                    array_push(&mut (*self_).iterators, ptr::read(&iterator));
                    next_iterator = array_back(&(*self_).iterators);
                }
                (*next_iterator).node = link.node;
            }

            array_push(&mut visited_nodes, node);
        }
    }

    fprintf(f, b"}\n\0".as_ptr() as *const i8);

    array_delete(&mut visited_nodes);
    true
}
