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
use super::language::ts_language_write_symbol_as_dot_string;
use super::length::{Length, length_add, length_zero};
use super::subtree::{
    NULL_SUBTREE, Subtree, SubtreeArray, SubtreePool, ts_builtin_sym_error_repeat,
    ts_external_scanner_state_data, ts_subtree_alloc_size, ts_subtree_child_count,
    ts_subtree_dynamic_precedence, ts_subtree_error_cost, ts_subtree_external_scanner_state,
    ts_subtree_external_scanner_state_eq, ts_subtree_extra, ts_subtree_is_error, ts_subtree_named,
    ts_subtree_padding, ts_subtree_release, ts_subtree_retain, ts_subtree_size, ts_subtree_symbol,
    ts_subtree_total_bytes, ts_subtree_total_size, ts_subtree_visible,
    ts_subtree_visible_descendant_count,
};
use super::subtree::{ts_subtree_array_copy, ts_subtree_array_delete, ts_subtree_array_reverse};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_LINK_COUNT: usize = 8;
const MAX_NODE_POOL_SIZE: u32 = 50;
const MAX_ITERATOR_COUNT: u32 = 64;

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

pub type StackCallback = unsafe fn(payload: *mut c_void, iterator: &StackIterator) -> StackAction;

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

    #[cfg(target_os = "macos")]
    #[link_name = "__stderrp"]
    static stderr: *mut c_void;

    #[cfg(not(target_os = "macos"))]
    static stderr: *mut c_void;
}

// ---------------------------------------------------------------------------
// Array helper functions (generic, mirrors array.h)
// ---------------------------------------------------------------------------

pub unsafe fn array_init<T>(arr: &mut Array<T>) {
    arr.size = 0;
    arr.capacity = 0;
    arr.contents = ptr::null_mut();
}

pub unsafe fn array_delete<T>(arr: &mut Array<T>) {
    if !arr.contents.is_null() {
        ts_free(arr.contents.cast::<c_void>());
    }
    arr.contents = ptr::null_mut();
    arr.size = 0;
    arr.capacity = 0;
}

pub unsafe fn array_clear<T>(arr: &mut Array<T>) {
    arr.size = 0;
}

pub unsafe fn array_reserve<T>(arr: &mut Array<T>, new_capacity: u32) {
    if new_capacity > arr.capacity {
        let elem_size = std::mem::size_of::<T>();
        if arr.contents.is_null() {
            arr.contents = ts_malloc(new_capacity as usize * elem_size).cast::<T>();
        } else {
            arr.contents = ts_realloc(
                arr.contents.cast::<c_void>(),
                new_capacity as usize * elem_size,
            )
            .cast::<T>();
        }
        arr.capacity = new_capacity;
    }
}

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

pub unsafe fn array_push<T>(arr: &mut Array<T>, element: T) {
    array_grow(arr, 1);
    ptr::write(arr.contents.add(arr.size as usize), element);
    arr.size += 1;
}

pub unsafe fn array_pop<T>(arr: &mut Array<T>) -> T {
    arr.size -= 1;
    ptr::read(arr.contents.add(arr.size as usize))
}

#[inline]
pub unsafe fn array_get_ref<T>(arr: &Array<T>, index: u32) -> &T {
    debug_assert!(index < arr.size);
    arr.contents.add(index as usize).as_ref().unwrap_unchecked()
}

#[inline]
pub unsafe fn array_get_mut<T>(arr: &mut Array<T>, index: u32) -> &mut T {
    debug_assert!(index < arr.size);
    arr.contents.add(index as usize).as_mut().unwrap_unchecked()
}

#[inline]
pub unsafe fn array_back_ref<T>(arr: &Array<T>) -> &T {
    debug_assert!(arr.size > 0);
    arr.contents
        .add(arr.size as usize - 1)
        .as_ref()
        .unwrap_unchecked()
}

#[inline]
pub unsafe fn array_back_mut<T>(arr: &mut Array<T>) -> &mut T {
    debug_assert!(arr.size > 0);
    arr.contents
        .add(arr.size as usize - 1)
        .as_mut()
        .unwrap_unchecked()
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

pub const unsafe fn array_new<T>() -> Array<T> {
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

pub unsafe fn array_swap<T>(self_: &mut Array<T>, other: &mut Array<T>) {
    std::mem::swap(self_, other);
}

pub unsafe fn array_assign<T>(self_: &mut Array<T>, other: &Array<T>) {
    array_reserve(self_, other.size);
    self_.size = other.size;
    if other.size > 0 {
        ptr::copy(other.contents, self_.contents, other.size as usize);
    }
}

#[inline]
unsafe fn stack_head(self_: &Stack, version: StackVersion) -> &StackHead {
    array_get_ref(&self_.heads, version)
}

#[inline]
unsafe fn stack_head_mut(self_: &mut Stack, version: StackVersion) -> &mut StackHead {
    array_get_mut(&mut self_.heads, version)
}

#[inline]
unsafe fn stack_head_array_pair_mut(
    self_: &mut Array<StackHead>,
    first: StackVersion,
    second: StackVersion,
) -> (&mut StackHead, &mut StackHead) {
    debug_assert_ne!(first, second);
    debug_assert!(first < self_.size);
    debug_assert!(second < self_.size);

    let heads = std::slice::from_raw_parts_mut(self_.contents, self_.size as usize);
    let (lower, upper) = if first < second {
        (first as usize, second as usize)
    } else {
        (second as usize, first as usize)
    };
    let (left, right) = heads.split_at_mut(upper);
    let lower_head = left.get_unchecked_mut(lower);
    let upper_head = right.get_unchecked_mut(0);
    if first < second {
        (lower_head, upper_head)
    } else {
        (upper_head, lower_head)
    }
}

#[inline]
unsafe fn subtree_array_as_array(self_: &SubtreeArray) -> &Array<Subtree> {
    ptr::from_ref(self_)
        .cast::<Array<Subtree>>()
        .as_ref()
        .unwrap_unchecked()
}

#[inline]
unsafe fn subtree_array_as_array_mut(self_: &mut SubtreeArray) -> &mut Array<Subtree> {
    ptr::from_mut(self_)
        .cast::<Array<Subtree>>()
        .as_mut()
        .unwrap_unchecked()
}

// ---------------------------------------------------------------------------
// Internal (static) functions
// ---------------------------------------------------------------------------

/// Retain (increment ref count) a stack node.
unsafe fn stack_node_retain(self_: &mut StackNode) {
    debug_assert!(self_.ref_count > 0);
    self_.ref_count += 1;
    debug_assert!(self_.ref_count != 0);
}

#[inline]
unsafe fn stack_node_mut<'a>(node: *mut StackNode) -> &'a mut StackNode {
    node.as_mut().unwrap_unchecked()
}

#[inline]
unsafe fn stack_node_ref<'a>(node: *const StackNode) -> &'a StackNode {
    node.as_ref().unwrap_unchecked()
}

/// Release (decrement ref count) a stack node, freeing if zero.
unsafe fn stack_node_release(
    self_: &mut StackNode,
    pool: &mut StackNodeArray,
    subtree_pool: &mut SubtreePool,
) {
    let mut self_ = ptr::from_mut(self_);
    loop {
        let node = stack_node_mut(self_);
        debug_assert!(node.ref_count != 0);
        node.ref_count -= 1;
        if node.ref_count > 0 {
            return;
        }

        let first_predecessor = if node.link_count > 0 {
            for i in (1..usize::from(node.link_count)).rev() {
                let link = node.links[i];
                if !link.subtree.ptr.is_null() {
                    ts_subtree_release(subtree_pool, link.subtree);
                }
                stack_node_release(stack_node_mut(link.node), pool, subtree_pool);
            }
            let link = node.links[0];
            if !link.subtree.ptr.is_null() {
                ts_subtree_release(subtree_pool, link.subtree);
            }
            link.node
        } else {
            ptr::null_mut()
        };

        if pool.size < MAX_NODE_POOL_SIZE {
            array_push(pool, self_);
        } else {
            ts_free(self_.cast::<c_void>());
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
    pool: &mut StackNodeArray,
) -> *mut StackNode {
    let node: *mut StackNode = if pool.size > 0 {
        array_pop(pool)
    } else {
        ts_malloc(std::mem::size_of::<StackNode>()).cast::<StackNode>()
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
        && ts_subtree_external_scanner_state_eq(&left, &right)
}

/// Add a link to a stack node, merging if possible.
unsafe fn stack_node_add_link(
    self_: &mut StackNode,
    link: StackLink,
    subtree_pool: &mut SubtreePool,
) {
    let self_ptr = ptr::from_mut(self_);
    if link.node == self_ptr {
        return;
    }

    for i in 0..self_.link_count as usize {
        let existing_link = &mut self_.links[i];
        if stack__subtree_is_equivalent(existing_link.subtree, link.subtree) {
            if existing_link.node == link.node {
                if ts_subtree_dynamic_precedence(link.subtree)
                    > ts_subtree_dynamic_precedence(existing_link.subtree)
                {
                    ts_subtree_retain(link.subtree);
                    ts_subtree_release(subtree_pool, existing_link.subtree);
                    existing_link.subtree = link.subtree;
                    self_.dynamic_precedence = stack_node_ref(link.node).dynamic_precedence
                        + ts_subtree_dynamic_precedence(link.subtree);
                }
                return;
            }

            let existing_node = stack_node_ref(existing_link.node);
            let link_node = stack_node_ref(link.node);
            if existing_node.state == link_node.state
                && existing_node.position.bytes == link_node.position.bytes
                && existing_node.error_cost == link_node.error_cost
            {
                for j in 0..link_node.link_count as usize {
                    stack_node_add_link(
                        stack_node_mut(existing_link.node),
                        link_node.links[j],
                        subtree_pool,
                    );
                }
                let mut dynamic_precedence = link_node.dynamic_precedence;
                if !link.subtree.ptr.is_null() {
                    dynamic_precedence += ts_subtree_dynamic_precedence(link.subtree);
                }
                if dynamic_precedence > self_.dynamic_precedence {
                    self_.dynamic_precedence = dynamic_precedence;
                }
                return;
            }
        }
    }

    if self_.link_count as usize == MAX_LINK_COUNT {
        return;
    }

    stack_node_retain(stack_node_mut(link.node));
    let link_node = stack_node_ref(link.node);
    let mut node_count = link_node.node_count;
    let mut dynamic_precedence = link_node.dynamic_precedence;
    self_.links[self_.link_count as usize] = link;
    self_.link_count += 1;

    if !link.subtree.ptr.is_null() {
        ts_subtree_retain(link.subtree);
        node_count += stack__subtree_node_count(link.subtree);
        dynamic_precedence += ts_subtree_dynamic_precedence(link.subtree);
    }

    if node_count > self_.node_count {
        self_.node_count = node_count;
    }
    if dynamic_precedence > self_.dynamic_precedence {
        self_.dynamic_precedence = dynamic_precedence;
    }
}

/// Delete a stack head, releasing its node and subtrees.
unsafe fn stack_head_delete(
    self_: &mut StackHead,
    pool: &mut StackNodeArray,
    subtree_pool: &mut SubtreePool,
) {
    if !self_.node.is_null() {
        if !self_.last_external_token.ptr.is_null() {
            ts_subtree_release(subtree_pool, self_.last_external_token);
        }
        if !self_.lookahead_when_paused.ptr.is_null() {
            ts_subtree_release(subtree_pool, self_.lookahead_when_paused);
        }
        if !self_.summary.is_null() {
            array_delete(self_.summary.as_mut().unwrap_unchecked());
            ts_free(self_.summary.cast::<c_void>());
        }
        stack_node_release(stack_node_mut(self_.node), pool, subtree_pool);
    }
}

/// Add a new version to the stack, cloning metadata from an existing version.
unsafe fn ts_stack__add_version(
    self_: &mut Stack,
    original_version: StackVersion,
    node: &mut StackNode,
) -> StackVersion {
    let node_ptr = ptr::from_mut(node);
    let original_head = stack_head(self_, original_version);
    let head = StackHead {
        node: node_ptr,
        node_count_at_last_error: original_head.node_count_at_last_error,
        last_external_token: original_head.last_external_token,
        status: StackStatus::Active,
        lookahead_when_paused: NULL_SUBTREE,
        summary: ptr::null_mut(),
    };
    array_push(&mut self_.heads, head);
    stack_node_retain(node);
    let head = array_back_ref(&self_.heads);
    if !head.last_external_token.ptr.is_null() {
        ts_subtree_retain(head.last_external_token);
    }
    self_.heads.size - 1
}

/// Add a slice to the stack's slice array, finding or creating a version.
unsafe fn ts_stack__add_slice(
    self_: &mut Stack,
    original_version: StackVersion,
    node: &mut StackNode,
    subtrees: &SubtreeArray,
) {
    let node_ptr = ptr::from_mut(node);
    for i in (0..self_.slices.size).rev() {
        let version = array_get_ref(&self_.slices, i).version;
        if stack_head(self_, version).node == node_ptr {
            let slice = StackSlice {
                subtrees: ptr::read(subtrees),
                version,
            };
            array_insert(&mut self_.slices, i + 1, slice);
            return;
        }
    }

    let version = ts_stack__add_version(self_, original_version, node);
    let slice = StackSlice {
        subtrees: ptr::read(subtrees),
        version,
    };
    array_push(&mut self_.slices, slice);
}

/// Core iteration function for walking the stack graph.
unsafe fn stack__iter(
    stack: &mut Stack,
    version: StackVersion,
    callback: StackCallback,
    payload: *mut c_void,
    goal_subtree_count: Option<u32>,
) -> StackSliceArray {
    array_clear(&mut stack.slices);
    array_clear(&mut stack.iterators);

    let head = stack_head(stack, version);
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

    if let Some(goal_subtree_count) = goal_subtree_count {
        let reserve_count =
            ts_subtree_alloc_size(goal_subtree_count) / std::mem::size_of::<Subtree>();
        let subtrees = subtree_array_as_array_mut(&mut new_iterator.subtrees);
        array_reserve(subtrees, u32::try_from(reserve_count).unwrap());
    }
    let include_subtrees = goal_subtree_count.is_some();

    array_push(&mut stack.iterators, new_iterator);

    while stack.iterators.size > 0 {
        let mut i: u32 = 0;
        let mut size = stack.iterators.size;
        while i < size {
            let iterator = array_get_ref(&stack.iterators, i);
            let node = iterator.node;

            let action = callback(payload, iterator);
            let should_pop = (action & StackActionPop) != 0;
            let should_stop = (action & StackActionStop) != 0 || (*node).link_count == 0;

            if should_pop {
                let mut subtrees = ptr::read(&array_get_ref(&stack.iterators, i).subtrees);
                if !should_stop {
                    let source_subtrees = ptr::read(&subtrees);
                    ts_subtree_array_copy(&source_subtrees, &mut subtrees);
                }
                ts_subtree_array_reverse(&mut subtrees);
                ts_stack__add_slice(stack, version, stack_node_mut(node), &subtrees);
            }

            if should_stop {
                if !should_pop {
                    let iter = array_get_mut(&mut stack.iterators, i);
                    ts_subtree_array_delete(
                        stack.subtree_pool.as_mut().unwrap_unchecked(),
                        &mut iter.subtrees,
                    );
                }
                array_erase(&mut stack.iterators, i);
                i = i.wrapping_sub(1);
                size -= 1;
                i = i.wrapping_add(1);
                continue;
            }

            let mut j: u32 = 1;
            while j <= u32::from((*node).link_count) {
                let next_iterator: &mut StackIterator;
                let link: StackLink;
                if j == u32::from((*node).link_count) {
                    link = (*node).links[0];
                    next_iterator = array_get_mut(&mut stack.iterators, i);
                } else {
                    if stack.iterators.size >= MAX_ITERATOR_COUNT {
                        j += 1;
                        continue;
                    }
                    link = (*node).links[j as usize];
                    let current_iterator = ptr::read(array_get_ref(&stack.iterators, i));
                    array_push(&mut stack.iterators, current_iterator);
                    next_iterator = array_back_mut(&mut stack.iterators);
                    let source_subtrees = ptr::read(&next_iterator.subtrees);
                    ts_subtree_array_copy(&source_subtrees, &mut next_iterator.subtrees);
                }

                next_iterator.node = link.node;
                if link.subtree.ptr.is_null() {
                    next_iterator.subtree_count += 1;
                    next_iterator.is_pending = false;
                } else {
                    if include_subtrees {
                        let subtrees = subtree_array_as_array_mut(&mut next_iterator.subtrees);
                        array_push(subtrees, link.subtree);
                        ts_subtree_retain(link.subtree);
                    }

                    if !ts_subtree_extra(link.subtree) {
                        next_iterator.subtree_count += 1;
                        if !link.is_pending {
                            next_iterator.is_pending = false;
                        }
                    }
                }
                j += 1;
            }
            i = i.wrapping_add(1);
        }
    }

    ptr::read(&stack.slices)
}

// Callbacks for stack__iter
unsafe fn pop_count_callback(payload: *mut c_void, iterator: &StackIterator) -> StackAction {
    let goal_subtree_count = *payload.cast::<u32>().as_ref().unwrap_unchecked();
    if iterator.subtree_count == goal_subtree_count {
        StackActionPop | StackActionStop
    } else {
        StackActionNone
    }
}

const unsafe fn pop_pending_callback(
    _payload: *mut c_void,
    iterator: &StackIterator,
) -> StackAction {
    if iterator.subtree_count >= 1 {
        if iterator.is_pending {
            StackActionPop | StackActionStop
        } else {
            StackActionStop
        }
    } else {
        StackActionNone
    }
}

unsafe fn pop_error_callback(payload: *mut c_void, iterator: &StackIterator) -> StackAction {
    if iterator.subtrees.size > 0 {
        let found_error = payload.cast::<bool>().as_mut().unwrap_unchecked();
        if !*found_error
            && ts_subtree_is_error(*array_get_ref(
                subtree_array_as_array(&iterator.subtrees),
                0,
            ))
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

unsafe fn pop_all_callback(_payload: *mut c_void, iterator: &StackIterator) -> StackAction {
    let node = stack_node_ref(iterator.node);
    if node.link_count == 0 {
        StackActionPop
    } else {
        StackActionNone
    }
}

unsafe fn summarize_stack_callback(payload: *mut c_void, iterator: &StackIterator) -> StackAction {
    let node = stack_node_ref(iterator.node);
    let session = payload
        .cast::<SummarizeStackSession>()
        .as_mut()
        .unwrap_unchecked();
    let state = node.state;
    let depth = iterator.subtree_count;
    if depth > session.max_depth {
        return StackActionStop;
    }
    let summary = session.summary.as_ref().unwrap_unchecked();
    for i in (0..summary.size).rev() {
        let entry = array_get_ref(summary, i);
        if entry.depth < depth {
            break;
        }
        if entry.depth == depth && entry.state == state {
            return StackActionNone;
        }
    }
    array_push(
        session.summary.as_mut().unwrap_unchecked(),
        StackSummaryEntry {
            position: node.position,
            depth,
            state,
        },
    );
    StackActionNone
}

// ===========================================================================
// Internal stack helpers used by the Rust parser.
// ===========================================================================

/// Create a new parse stack.
pub unsafe fn ts_stack_new(subtree_pool: &mut SubtreePool) -> *mut Stack {
    let self_ = ts_calloc(1, std::mem::size_of::<Stack>()).cast::<Stack>();
    let stack = self_.as_mut().unwrap_unchecked();

    array_init(&mut stack.heads);
    array_init(&mut stack.slices);
    array_init(&mut stack.iterators);
    array_init(&mut stack.node_pool);
    array_reserve(&mut stack.heads, 4);
    array_reserve(&mut stack.slices, 4);
    array_reserve(&mut stack.iterators, 4);
    array_reserve(&mut stack.node_pool, MAX_NODE_POOL_SIZE);

    stack.subtree_pool = subtree_pool;
    stack.base_node = stack_node_new(
        ptr::null_mut(),
        NULL_SUBTREE,
        false,
        1,
        &mut stack.node_pool,
    );
    ts_stack_clear(stack);

    self_
}

/// Free the parse stack.
pub unsafe fn ts_stack_delete(self_: &mut Stack) {
    if !self_.slices.contents.is_null() {
        array_delete(&mut self_.slices);
    }
    if !self_.iterators.contents.is_null() {
        array_delete(&mut self_.iterators);
    }
    let subtree_pool = self_.subtree_pool.as_mut().unwrap_unchecked();
    stack_node_release(
        stack_node_mut(self_.base_node),
        &mut self_.node_pool,
        subtree_pool,
    );
    let heads = &mut self_.heads;
    let node_pool = &mut self_.node_pool;
    for i in 0..heads.size {
        stack_head_delete(array_get_mut(heads, i), node_pool, subtree_pool);
    }
    array_clear(heads);
    if !node_pool.contents.is_null() {
        for i in 0..node_pool.size {
            ts_free((*array_get_ref(node_pool, i)).cast::<c_void>());
        }
        array_delete(node_pool);
    }
    array_delete(heads);
    ts_free(ptr::from_mut(self_).cast::<c_void>());
}

/// Get the number of versions in the stack.
pub const unsafe fn ts_stack_version_count(self_: &Stack) -> u32 {
    self_.heads.size
}

/// Get the number of halted versions.
pub unsafe fn ts_stack_halted_version_count(self_: &Stack) -> u32 {
    let mut count = 0u32;
    for i in 0..self_.heads.size {
        if stack_head(self_, i).status == StackStatus::Halted {
            count += 1;
        }
    }
    count
}

/// Get the state at the top of a version.
pub unsafe fn ts_stack_state(self_: &Stack, version: StackVersion) -> TSStateId {
    stack_node_ref(stack_head(self_, version).node).state
}

/// Get the position of a version.
pub unsafe fn ts_stack_position(self_: &Stack, version: StackVersion) -> Length {
    stack_node_ref(stack_head(self_, version).node).position
}

/// Get the last external token for a version.
pub unsafe fn ts_stack_last_external_token(self_: &Stack, version: StackVersion) -> Subtree {
    stack_head(self_, version).last_external_token
}

/// Set the last external token for a version.
pub unsafe fn ts_stack_set_last_external_token(
    self_: &mut Stack,
    version: StackVersion,
    token: Subtree,
) {
    let subtree_pool = self_.subtree_pool.as_mut().unwrap_unchecked();
    let head = array_get_mut(&mut self_.heads, version);
    if !token.ptr.is_null() {
        ts_subtree_retain(token);
    }
    if !head.last_external_token.ptr.is_null() {
        ts_subtree_release(subtree_pool, head.last_external_token);
    }
    head.last_external_token = token;
}

/// Get the error cost for a version.
pub unsafe fn ts_stack_error_cost(self_: &Stack, version: StackVersion) -> u32 {
    let head = stack_head(self_, version);
    let node = stack_node_ref(head.node);
    let mut result = node.error_cost;
    if head.status == StackStatus::Paused
        || (node.state == ERROR_STATE && node.links[0].subtree.ptr.is_null())
    {
        result += ERROR_COST_PER_RECOVERY;
    }
    result
}

/// Get the node count since last error for a version.
pub unsafe fn ts_stack_node_count_since_error(self_: &mut Stack, version: StackVersion) -> u32 {
    let head = stack_head_mut(self_, version);
    let node = stack_node_ref(head.node);
    if node.node_count < head.node_count_at_last_error {
        head.node_count_at_last_error = node.node_count;
    }
    node.node_count - head.node_count_at_last_error
}

/// Push a subtree onto a version.
pub unsafe fn ts_stack_push(
    stack: &mut Stack,
    version: StackVersion,
    subtree: Subtree,
    pending: bool,
    state: TSStateId,
) {
    let heads = &mut stack.heads;
    let node_pool = &mut stack.node_pool;
    let head = array_get_mut(heads, version);
    let new_node = stack_node_new(head.node, subtree, pending, state, node_pool);
    if subtree.ptr.is_null() {
        head.node_count_at_last_error = (*new_node).node_count;
    }
    head.node = new_node;
}

/// Pop a given number of entries from a version.
pub unsafe fn ts_stack_pop_count(
    self_: &mut Stack,
    version: StackVersion,
    count: u32,
) -> StackSliceArray {
    stack__iter(
        self_,
        version,
        pop_count_callback,
        ptr::addr_of!(count).cast_mut().cast::<c_void>(),
        Some(count),
    )
}

/// Pop an error from the top of a version.
pub unsafe fn ts_stack_pop_error(self_: &mut Stack, version: StackVersion) -> SubtreeArray {
    let node = stack_head(self_, version).node;
    for i in 0..(*node).link_count as usize {
        if !(*node).links[i].subtree.ptr.is_null() && ts_subtree_is_error((*node).links[i].subtree)
        {
            let mut found_error = false;
            let pop = stack__iter(
                self_,
                version,
                pop_error_callback,
                ptr::from_mut(&mut found_error).cast::<c_void>(),
                Some(1),
            );
            if pop.size > 0 {
                debug_assert!(pop.size == 1);
                let first_pop = array_get_ref(&pop, 0);
                ts_stack_renumber_version(self_, first_pop.version, version);
                return ptr::read(&first_pop.subtrees);
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
pub unsafe fn ts_stack_pop_pending(self_: &mut Stack, version: StackVersion) -> StackSliceArray {
    let mut pop = stack__iter(
        self_,
        version,
        pop_pending_callback,
        ptr::null_mut(),
        Some(0),
    );
    if pop.size > 0 {
        let first_pop = array_get_mut(&mut pop, 0);
        ts_stack_renumber_version(self_, first_pop.version, version);
        first_pop.version = version;
    }
    pop
}

/// Pop all entries from a version.
pub unsafe fn ts_stack_pop_all(self_: &mut Stack, version: StackVersion) -> StackSliceArray {
    stack__iter(self_, version, pop_all_callback, ptr::null_mut(), Some(0))
}

/// Record a summary of parse states near the top of a version.
pub unsafe fn ts_stack_record_summary(self_: &mut Stack, version: StackVersion, max_depth: u32) {
    let mut session = SummarizeStackSession {
        summary: ts_malloc(std::mem::size_of::<StackSummary>()).cast::<StackSummary>(),
        max_depth,
    };
    array_init(session.summary.as_mut().unwrap_unchecked());
    stack__iter(
        self_,
        version,
        summarize_stack_callback,
        ptr::from_mut(&mut session).cast::<c_void>(),
        None,
    );
    let head = stack_head_mut(self_, version);
    if !head.summary.is_null() {
        array_delete(head.summary.as_mut().unwrap_unchecked());
        ts_free(head.summary.cast::<c_void>());
    }
    head.summary = session.summary;
}

/// Get the recorded summary for a version.
pub unsafe fn ts_stack_get_summary(stack: &Stack, version: StackVersion) -> *mut StackSummary {
    stack_head(stack, version).summary
}

/// Get the dynamic precedence of a version.
pub unsafe fn ts_stack_dynamic_precedence(self_: &Stack, version: StackVersion) -> i32 {
    stack_node_ref(stack_head(self_, version).node).dynamic_precedence
}

/// Check if a version has advanced since the last error.
pub unsafe fn ts_stack_has_advanced_since_error(self_: &Stack, version: StackVersion) -> bool {
    let head = stack_head(self_, version);
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
pub unsafe fn ts_stack_remove_version(self_: &mut Stack, version: StackVersion) {
    let heads = &mut self_.heads;
    let node_pool = &mut self_.node_pool;
    let subtree_pool = self_.subtree_pool.as_mut().unwrap_unchecked();
    stack_head_delete(array_get_mut(heads, version), node_pool, subtree_pool);
    array_erase(heads, version);
}

/// Renumber version v1 to v2 (move v1 into v2's slot, removing v2).
pub unsafe fn ts_stack_renumber_version(stack: &mut Stack, v1: StackVersion, v2: StackVersion) {
    if v1 == v2 {
        return;
    }
    debug_assert!(v2 < v1);
    debug_assert!(v1 < stack.heads.size);

    let heads = &mut stack.heads;
    let node_pool = &mut stack.node_pool;
    let subtree_pool = stack.subtree_pool.as_mut().unwrap_unchecked();
    let (source_head, target_head) = stack_head_array_pair_mut(heads, v1, v2);
    if !target_head.summary.is_null() && source_head.summary.is_null() {
        source_head.summary = target_head.summary;
        target_head.summary = ptr::null_mut();
    }
    stack_head_delete(target_head, node_pool, subtree_pool);
    *target_head = ptr::read(source_head);
    array_erase(heads, v1);
}

/// Swap two versions.
pub unsafe fn ts_stack_swap_versions(stack: &mut Stack, v1: StackVersion, v2: StackVersion) {
    let temp = ptr::read(array_get_ref(&stack.heads, v1));
    let other = ptr::read(array_get_ref(&stack.heads, v2));
    ptr::write(array_get_mut(&mut stack.heads, v1), other);
    ptr::write(array_get_mut(&mut stack.heads, v2), temp);
}

/// Copy a version, creating a new one.
pub unsafe fn ts_stack_copy_version(stack: &mut Stack, version: StackVersion) -> StackVersion {
    debug_assert!(version < stack.heads.size);
    let version_head = ptr::read(array_get_ref(&stack.heads, version));
    array_push(&mut stack.heads, version_head);
    let head = array_back_mut(&mut stack.heads);
    stack_node_retain(stack_node_mut(head.node));
    if !head.last_external_token.ptr.is_null() {
        ts_subtree_retain(head.last_external_token);
    }
    head.summary = ptr::null_mut();
    stack.heads.size - 1
}

/// Merge two versions if possible.
pub unsafe fn ts_stack_merge(
    stack: &mut Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    if !ts_stack_can_merge(stack, version1, version2) {
        return false;
    }
    {
        let stack_heads = &mut stack.heads;
        let subtree_pool = stack.subtree_pool.as_mut().unwrap_unchecked();
        let (head1, head2) = stack_head_array_pair_mut(stack_heads, version1, version2);
        let head2_node = stack_node_ref(head2.node);
        for i in 0..head2_node.link_count as usize {
            stack_node_add_link(
                stack_node_mut(head1.node),
                head2_node.links[i],
                subtree_pool,
            );
        }
        let head1_node = stack_node_ref(head1.node);
        if head1_node.state == ERROR_STATE {
            head1.node_count_at_last_error = head1_node.node_count;
        }
    }
    ts_stack_remove_version(stack, version2);
    true
}

/// Check if two versions can be merged.
pub unsafe fn ts_stack_can_merge(
    stack: &Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    let head1 = stack_head(stack, version1);
    let head2 = stack_head(stack, version2);
    let node1 = stack_node_ref(head1.node);
    let node2 = stack_node_ref(head2.node);
    head1.status == StackStatus::Active
        && head2.status == StackStatus::Active
        && node1.state == node2.state
        && node1.position.bytes == node2.position.bytes
        && node1.error_cost == node2.error_cost
        && ts_subtree_external_scanner_state_eq(
            &head1.last_external_token,
            &head2.last_external_token,
        )
}

/// Halt a version.
pub unsafe fn ts_stack_halt(self_: &mut Stack, version: StackVersion) {
    stack_head_mut(self_, version).status = StackStatus::Halted;
}

/// Pause a version with a lookahead token.
pub unsafe fn ts_stack_pause(stack: &mut Stack, version: StackVersion, lookahead: Subtree) {
    let head = stack_head_mut(stack, version);
    head.status = StackStatus::Paused;
    head.lookahead_when_paused = lookahead;
    head.node_count_at_last_error = stack_node_ref(head.node).node_count;
}

/// Check if a version is active.
pub unsafe fn ts_stack_is_active(self_: &Stack, version: StackVersion) -> bool {
    stack_head(self_, version).status == StackStatus::Active
}

/// Check if a version is halted.
pub unsafe fn ts_stack_is_halted(self_: &Stack, version: StackVersion) -> bool {
    stack_head(self_, version).status == StackStatus::Halted
}

/// Check if a version is paused.
pub unsafe fn ts_stack_is_paused(self_: &Stack, version: StackVersion) -> bool {
    stack_head(self_, version).status == StackStatus::Paused
}

/// Resume a paused version, returning its stored lookahead.
pub unsafe fn ts_stack_resume(stack: &mut Stack, version: StackVersion) -> Subtree {
    let head = stack_head_mut(stack, version);
    debug_assert!(head.status == StackStatus::Paused);
    let result = head.lookahead_when_paused;
    head.status = StackStatus::Active;
    head.lookahead_when_paused = NULL_SUBTREE;
    result
}

/// Clear all versions, resetting to initial state.
pub unsafe fn ts_stack_clear(self_: &mut Stack) {
    stack_node_retain(stack_node_mut(self_.base_node));
    let heads = &mut self_.heads;
    let node_pool = &mut self_.node_pool;
    let subtree_pool = self_.subtree_pool.as_mut().unwrap_unchecked();
    for i in 0..heads.size {
        stack_head_delete(array_get_mut(heads, i), node_pool, subtree_pool);
    }
    array_clear(heads);
    array_push(
        heads,
        StackHead {
            node: self_.base_node,
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
    stack: &mut Stack,
    language: *const TSLanguage,
    mut f: *mut c_void,
) -> bool {
    array_reserve(&mut stack.iterators, 32);
    if f.is_null() {
        f = stderr;
    }

    fprintf(f, c"digraph stack {\n".as_ptr().cast::<i8>());
    fprintf(f, c"rankdir=\"RL\";\n".as_ptr().cast::<i8>());
    fprintf(f, c"edge [arrowhead=none]\n".as_ptr().cast::<i8>());

    let mut visited_nodes: Array<*mut StackNode> = Array {
        contents: ptr::null_mut(),
        size: 0,
        capacity: 0,
    };

    array_clear(&mut stack.iterators);
    for i in 0..stack.heads.size {
        if stack_head(stack, i).status == StackStatus::Halted {
            continue;
        }
        let node_count_since_error = ts_stack_node_count_since_error(stack, i);
        let error_cost = ts_stack_error_cost(stack, i);
        let head = stack_head(stack, i);

        fprintf(
            f,
            c"node_head_%u [shape=none, label=\"\"]\n"
                .as_ptr()
                .cast::<i8>(),
            i,
        );
        fprintf(
            f,
            c"node_head_%u -> node_%p [".as_ptr().cast::<i8>(),
            i,
            head.node as *const c_void,
        );

        if head.status == StackStatus::Paused {
            fprintf(f, c"color=red ".as_ptr().cast::<i8>());
        }
        fprintf(
            f,
            c"label=%u, fontcolor=blue, weight=10000, labeltooltip=\"node_count: %u\nerror_cost: %u".as_ptr().cast::<i8>(),
            i,
            node_count_since_error,
            error_cost,
        );

        if !head.summary.is_null() {
            fprintf(f, c"\nsummary:".as_ptr().cast::<i8>());
            let summary = head.summary.as_ref().unwrap_unchecked();
            for j in 0..summary.size {
                let entry = array_get_ref(summary, j);
                fprintf(f, c" %u".as_ptr().cast::<i8>(), u32::from(entry.state));
            }
        }

        if !head.last_external_token.ptr.is_null() {
            let state = ts_subtree_external_scanner_state(&head.last_external_token);
            let data = ts_external_scanner_state_data(state);
            fprintf(f, c"\nexternal_scanner_state:".as_ptr().cast::<i8>());
            for j in 0..state.length {
                fprintf(
                    f,
                    c" %2X".as_ptr().cast::<i8>(),
                    u32::from(*data.add(j as usize)),
                );
            }
        }

        fprintf(f, c"\"]\n".as_ptr().cast::<i8>());

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
        array_push(&mut stack.iterators, iter);
    }

    loop {
        let mut all_iterators_done = true;

        for i in 0..stack.iterators.size {
            let iterator = ptr::read(array_get_ref(&stack.iterators, i));
            let mut node = iterator.node;

            for j in 0..visited_nodes.size {
                if *array_get_ref(&visited_nodes, j) == node {
                    node = ptr::null_mut();
                    break;
                }
            }

            if node.is_null() {
                continue;
            }
            all_iterators_done = false;
            let node_ref = stack_node_ref(node);

            fprintf(f, c"node_%p [".as_ptr().cast::<i8>(), node as *const c_void);
            if node_ref.state == ERROR_STATE {
                fprintf(f, c"label=\"?\"".as_ptr().cast::<i8>());
            } else if node_ref.link_count == 1
                && !node_ref.links[0].subtree.ptr.is_null()
                && ts_subtree_extra(node_ref.links[0].subtree)
            {
                fprintf(f, c"shape=point margin=0 label=\"\"".as_ptr().cast::<i8>());
            } else {
                fprintf(
                    f,
                    c"label=\"%d\"".as_ptr().cast::<i8>(),
                    i32::from(node_ref.state),
                );
            }

            fprintf(
                f,
                c" tooltip=\"position: %u,%u\nnode_count:%u\nerror_cost: %u\ndynamic_precedence: %d\"];\n".as_ptr().cast::<i8>(),
                node_ref.position.extent.row + 1,
                node_ref.position.extent.column,
                node_ref.node_count,
                node_ref.error_cost,
                node_ref.dynamic_precedence,
            );

            for j in 0..node_ref.link_count as usize {
                let link = node_ref.links[j];
                fprintf(
                    f,
                    c"node_%p -> node_%p [".as_ptr().cast::<i8>(),
                    node as *const c_void,
                    link.node as *const c_void,
                );
                if link.is_pending {
                    fprintf(f, c"style=dashed ".as_ptr().cast::<i8>());
                }
                if !link.subtree.ptr.is_null() && ts_subtree_extra(link.subtree) {
                    fprintf(f, c"fontcolor=gray ".as_ptr().cast::<i8>());
                }

                if link.subtree.ptr.is_null() {
                    fprintf(f, c"color=red".as_ptr().cast::<i8>());
                } else {
                    fprintf(f, c"label=\"".as_ptr().cast::<i8>());
                    let quoted =
                        ts_subtree_visible(link.subtree) && !ts_subtree_named(link.subtree);
                    if quoted {
                        fprintf(f, c"'".as_ptr().cast::<i8>());
                    }
                    ts_language_write_symbol_as_dot_string(
                        language,
                        f,
                        ts_subtree_symbol(link.subtree),
                    );
                    if quoted {
                        fprintf(f, c"'".as_ptr().cast::<i8>());
                    }
                    fprintf(f, c"\"".as_ptr().cast::<i8>());
                    fprintf(
                        f,
                        c"labeltooltip=\"error_cost: %u\ndynamic_precedence: %d\""
                            .as_ptr()
                            .cast::<i8>(),
                        ts_subtree_error_cost(link.subtree),
                        ts_subtree_dynamic_precedence(link.subtree),
                    );
                }

                fprintf(f, c"];\n".as_ptr().cast::<i8>());

                let next_iterator = if j == 0 {
                    array_get_mut(&mut stack.iterators, i)
                } else {
                    array_push(&mut stack.iterators, ptr::read(&iterator));
                    array_back_mut(&mut stack.iterators)
                };
                next_iterator.node = link.node;
            }

            array_push(&mut visited_nodes, node);
        }
        if all_iterators_done {
            break;
        }
    }

    fprintf(f, c"}\n".as_ptr().cast::<i8>());

    array_delete(&mut visited_nodes);
    true
}
