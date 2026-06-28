#![allow(non_snake_case)]

//! Rust replacement for stack.c/h — GLR parse stack with version management.
//!
//! This module implements the branching parse stack used by the GLR parser.
//! Multiple "versions" of the stack can exist simultaneously, representing
//! different parse paths. Versions can be merged when they reach the same
//! state, enabling efficient ambiguity handling.

use core::ffi::c_void;
use core::ptr;

use crate::ffi::{TSLanguage, TSStateId};

use super::alloc::{free, malloc};
use super::error_costs::{ERROR_COST_PER_RECOVERY, ERROR_STATE};
use super::language::language_write_symbol_as_dot_string;
use super::length::{length_add, length_zero, Length};
use super::subtree::{
    external_scanner_state_data, subtree_alloc_size, subtree_child_count,
    subtree_dynamic_precedence, subtree_error_cost, subtree_external_scanner_state,
    subtree_external_scanner_state_eq, subtree_extra, subtree_is_error, subtree_named,
    subtree_padding, subtree_release, subtree_retain, subtree_size, subtree_symbol,
    subtree_total_bytes, subtree_total_size, subtree_visible, subtree_visible_descendant_count,
    Subtree, SubtreeArray, SubtreePool, NULL_SUBTREE, TS_BUILTIN_SYM_ERROR_REPEAT,
};
use super::subtree::{subtree_array_copy, subtree_array_delete, subtree_array_reverse};
use super::utils::{
    array_back_mut, array_back_ref, array_clear, array_delete, array_erase, array_get_mut,
    array_get_ref, array_insert, array_new, array_pop, array_push, array_reserve, Array,
};
use super::utils::{ptr_mut, ptr_ref};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_LINK_COUNT: usize = 8;
const MAX_NODE_POOL_SIZE: u32 = 50;
const MAX_ITERATOR_COUNT: u32 = 64;
const STACK_LINK_PAYLOAD_IS_PENDING_LINK: u8 = 1 << 0;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

pub type StackVersion = u32;
pub const STACK_VERSION_NONE: StackVersion = u32::MAX;

/// Payload carried by an edge in the parse stack graph.
#[repr(C)]
#[derive(Clone, Copy)]
pub union StackLinkPayloadValue {
    pub subtree: Subtree,
}

/// Tagged edge payload for a `StackLink`.
///
/// `STACK_LINK_PAYLOAD_IS_PENDING_LINK` means the payload is part of a pending
/// path during stack popping.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StackLinkPayload {
    pub value: StackLinkPayloadValue,
    pub flags: u8,
}

/// Directed edge from a stack node to one predecessor.
///
/// The edge payload is the syntax node that was shifted/reduced between the
/// predecessor and the current node. Multiple links model GLR ambiguity: the
/// same parse state/position can be reached through different child lists.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StackLink {
    pub node: *mut StackNode,
    pub payload: StackLinkPayload,
}

/// Node in the persistent GLR stack graph.
///
/// A parser version points at one `StackNode` head. Pushing creates a new node
/// linked to the previous head; popping walks backward through links and may
/// fork when a node has multiple predecessors. Cached aggregate fields describe
/// the best path through the node and are used for pruning and merging.
#[repr(C)]
pub struct StackNode {
    /// Parse state at this stack depth.
    pub state: TSStateId,
    /// Source position reached by the best path to this node.
    pub position: Length,
    /// Inline predecessor links. Ambiguous nodes can carry several links.
    pub links: [StackLink; MAX_LINK_COUNT],
    /// Number of initialized entries in `links`.
    pub link_count: u16,
    /// Intrusive reference count from stack heads and successor links.
    pub ref_count: u32,
    /// Accumulated parse error cost for pruning worse versions.
    pub error_cost: u32,
    /// Approximate visible node count since the last error.
    pub node_count: u32,
    /// Accumulated dynamic precedence along the best path.
    pub dynamic_precedence: i32,
}

/// DFS cursor used by stack pop operations.
#[repr(C)]
pub struct StackIterator {
    /// Current graph node being visited.
    pub node: *mut StackNode,
    /// Child subtrees collected so far along this pop path.
    pub subtrees: SubtreeArray,
    /// Number of non-null subtree payloads traversed.
    pub subtree_count: u32,
    /// Whether this iterator is walking pending links.
    pub is_pending: bool,
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
pub struct StackSliceSpan {
    pub start: u32,
    pub size: u32,
    pub version: StackVersion,
}

pub type StackSliceSpanArray = Array<StackSliceSpan>;

#[repr(C)]
pub struct StackPopBuilder {
    pub slices: StackSliceSpanArray,
    pub subtrees: SubtreeArray,
}

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
    /// Current top node for this parser version.
    pub node: *mut StackNode,
    /// Optional recovery summary, recorded lazily.
    pub summary: *mut StackSummary,
    /// Node-count checkpoint used by recovery progress heuristics.
    pub node_count_at_last_error: u32,
    /// Last token carrying external scanner state for this version.
    pub last_external_token: Subtree,
    /// Lookahead saved when this version is paused for error recovery.
    pub lookahead_when_paused: Subtree,
    /// Active versions parse normally; paused versions wait for recovery;
    /// halted versions are removed by stack condensation.
    pub status: StackStatus,
}

#[repr(C)]
pub struct Stack {
    /// One head per active/paused/halted GLR version.
    pub heads: Array<StackHead>,
    /// Scratch pop results returned to the parser.
    pub slices: StackSliceArray,
    /// Reusable DFS iterators for pop operations.
    pub iterators: Array<StackIterator>,
    /// Free list for recently released stack nodes.
    pub node_pool: StackNodeArray,
    /// Initial root node shared by all versions.
    pub base_node: *mut StackNode,
    /// Parser-owned subtree pool used when releasing link payloads.
    pub subtree_pool: *mut SubtreePool,
}

// ---------------------------------------------------------------------------
// Compile-time layout assertions (sizes from C on 64-bit)
// ---------------------------------------------------------------------------

#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<StackLink>() == 24);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<StackLinkPayload>() == 16);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<StackNode>() == 232);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<StackIterator>() == 32);
const _: () = assert!(core::mem::size_of::<StackStatus>() == 4);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<StackSlice>() == 24);
const _: () = assert!(core::mem::size_of::<StackSliceSpan>() == 12);
const _: () = assert!(core::mem::size_of::<StackSummaryEntry>() == 20);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<StackHead>() == 48);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<Stack>() == 80);

pub type StackAction = u32;
pub const STACK_ACTION_NONE: StackAction = 0;
pub const STACK_ACTION_STOP: StackAction = 1;
pub const STACK_ACTION_POP: StackAction = 2;

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

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    static stderr: *mut c_void;

    // Windows MSVC has no `stderr` symbol; the CRT exposes the standard streams
    // through __acrt_iob_func (stderr is index 2).
    #[cfg(target_os = "windows")]
    fn __acrt_iob_func(index: u32) -> *mut c_void;
}

#[cfg(target_os = "windows")]
unsafe fn stderr_file() -> *mut c_void {
    __acrt_iob_func(2)
}

#[cfg(not(target_os = "windows"))]
unsafe fn stderr_file() -> *mut c_void {
    stderr
}

pub const fn stack_pop_builder_new() -> StackPopBuilder {
    StackPopBuilder {
        slices: array_new(),
        subtrees: array_new(),
    }
}

pub unsafe fn stack_pop_builder_delete(self_: &mut StackPopBuilder) {
    if !self_.slices.contents.is_null() {
        array_delete(&mut self_.slices);
    }
    if !self_.subtrees.contents.is_null() {
        array_delete(&mut self_.subtrees);
    }
}

fn stack_pop_builder_clear(self_: &mut StackPopBuilder) {
    self_.slices.size = 0;
    self_.subtrees.size = 0;
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

    let heads = core::slice::from_raw_parts_mut(self_.contents, self_.size as usize);
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

// ---------------------------------------------------------------------------
// Internal (static) functions
// ---------------------------------------------------------------------------

/// Retain (increment ref count) a stack node.
fn stack_node_retain(self_: &mut StackNode) {
    debug_assert!(self_.ref_count > 0);
    self_.ref_count += 1;
    debug_assert!(self_.ref_count != 0);
}

/// Release (decrement ref count) a stack node, freeing if zero.
unsafe fn stack_node_release(
    self_: &mut StackNode,
    pool: &mut StackNodeArray,
    subtree_pool: &mut SubtreePool,
) {
    let mut self_ = ptr::from_mut(self_);
    loop {
        let node = ptr_mut(self_);
        debug_assert!(node.ref_count != 0);
        node.ref_count -= 1;
        if node.ref_count > 0 {
            return;
        }

        let first_predecessor = if node.link_count > 0 {
            for i in (1..usize::from(node.link_count)).rev() {
                let link = node.links[i];
                if !stack_link_payload_is_null(link.payload) {
                    stack_link_payload_release(link.payload, subtree_pool);
                }
                stack_node_release(ptr_mut(link.node), pool, subtree_pool);
            }
            let link = node.links[0];
            if !stack_link_payload_is_null(link.payload) {
                stack_link_payload_release(link.payload, subtree_pool);
            }
            link.node
        } else {
            ptr::null_mut()
        };

        if pool.size < MAX_NODE_POOL_SIZE {
            array_push(pool, self_);
        } else {
            free(self_.cast::<c_void>());
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
    let mut count = subtree_visible_descendant_count(subtree);
    if subtree_visible(subtree) {
        count += 1;
    }
    if subtree_symbol(subtree) == TS_BUILTIN_SYM_ERROR_REPEAT {
        count += 1;
    }
    count
}

#[inline]
const fn stack_link_payload_new(subtree: Subtree, is_pending: bool) -> StackLinkPayload {
    StackLinkPayload {
        value: StackLinkPayloadValue { subtree },
        flags: if is_pending {
            STACK_LINK_PAYLOAD_IS_PENDING_LINK
        } else {
            0
        },
    }
}

#[inline]
const unsafe fn stack_link_payload_subtree_raw(payload: StackLinkPayload) -> Subtree {
    payload.value.subtree
}

#[inline]
const fn stack_link_payload_is_pending(payload: StackLinkPayload) -> bool {
    payload.flags & STACK_LINK_PAYLOAD_IS_PENDING_LINK != 0
}

#[inline]
pub const unsafe fn stack_link_payload_subtree(payload: StackLinkPayload) -> Subtree {
    stack_link_payload_subtree_raw(payload)
}

#[inline]
unsafe fn stack_link_payload_is_null(payload: StackLinkPayload) -> bool {
    stack_link_payload_subtree(payload).ptr.is_null()
}

#[inline]
const unsafe fn stack_link_payload_error_cost(payload: StackLinkPayload) -> u32 {
    subtree_error_cost(stack_link_payload_subtree(payload))
}

#[inline]
unsafe fn stack_link_payload_total_size(payload: StackLinkPayload) -> Length {
    subtree_total_size(stack_link_payload_subtree(payload))
}

#[inline]
unsafe fn stack_link_payload_total_bytes(payload: StackLinkPayload) -> u32 {
    subtree_total_bytes(stack_link_payload_subtree(payload))
}

#[inline]
const unsafe fn stack_link_payload_dynamic_precedence(payload: StackLinkPayload) -> i32 {
    subtree_dynamic_precedence(stack_link_payload_subtree(payload))
}

#[inline]
unsafe fn stack_link_payload_node_count(payload: StackLinkPayload) -> u32 {
    stack__subtree_node_count(stack_link_payload_subtree(payload))
}

#[inline]
unsafe fn stack_link_payload_retain_impl(payload: StackLinkPayload) {
    subtree_retain(stack_link_payload_subtree(payload));
}

#[inline]
unsafe fn stack_link_payload_release_impl(
    payload: StackLinkPayload,
    subtree_pool: &mut SubtreePool,
) {
    subtree_release(subtree_pool, stack_link_payload_subtree(payload));
}

#[inline]
pub unsafe fn stack_link_payload_retain(payload: StackLinkPayload) {
    stack_link_payload_retain_impl(payload);
}

#[inline]
pub unsafe fn stack_link_payload_release(
    payload: StackLinkPayload,
    subtree_pool: &mut SubtreePool,
) {
    stack_link_payload_release_impl(payload, subtree_pool);
}

#[inline]
const unsafe fn stack_link_payload_extra(payload: StackLinkPayload) -> bool {
    subtree_extra(stack_link_payload_subtree(payload))
}

/// Allocate a new stack node, reusing from pool if available.
unsafe fn stack_node_new_with_payload(
    previous_node: *mut StackNode,
    payload: StackLinkPayload,
    state: TSStateId,
    pool: &mut StackNodeArray,
) -> *mut StackNode {
    let node: *mut StackNode = if pool.size > 0 {
        array_pop(pool)
    } else {
        malloc(core::mem::size_of::<StackNode>()).cast::<StackNode>()
    };

    ptr::write(
        node,
        StackNode {
            state,
            position: length_zero(),
            links: [StackLink {
                node: ptr::null_mut(),
                payload: stack_link_payload_new(NULL_SUBTREE, false),
            }; MAX_LINK_COUNT],
            link_count: 0,
            ref_count: 1,
            error_cost: 0,
            node_count: 0,
            dynamic_precedence: 0,
        },
    );

    if !previous_node.is_null() {
        (*node).link_count = 1;
        (*node).links[0] = StackLink {
            node: previous_node,
            payload,
        };

        (*node).position = (*previous_node).position;
        (*node).error_cost = (*previous_node).error_cost;
        (*node).dynamic_precedence = (*previous_node).dynamic_precedence;
        (*node).node_count = (*previous_node).node_count;

        if !stack_link_payload_is_null(payload) {
            (*node).error_cost += stack_link_payload_error_cost(payload);
            (*node).position = length_add((*node).position, stack_link_payload_total_size(payload));
            (*node).node_count += stack_link_payload_node_count(payload);
            (*node).dynamic_precedence += stack_link_payload_dynamic_precedence(payload);
        }
    }

    node
}

unsafe fn stack_node_new(
    previous_node: *mut StackNode,
    subtree: Subtree,
    is_pending: bool,
    state: TSStateId,
    pool: &mut StackNodeArray,
) -> *mut StackNode {
    stack_node_new_with_payload(
        previous_node,
        stack_link_payload_new(subtree, is_pending),
        state,
        pool,
    )
}

/// Check if two subtrees are equivalent for merging purposes.
unsafe fn stack__subtree_is_equivalent(left: Subtree, right: Subtree) -> bool {
    if left.ptr == right.ptr {
        return true;
    }
    if left.ptr.is_null() || right.ptr.is_null() {
        return false;
    }

    if subtree_symbol(left) != subtree_symbol(right) {
        return false;
    }

    if subtree_error_cost(left) > 0 && subtree_error_cost(right) > 0 {
        return true;
    }

    subtree_padding(left).bytes == subtree_padding(right).bytes
        && subtree_size(left).bytes == subtree_size(right).bytes
        && subtree_child_count(left) == subtree_child_count(right)
        && subtree_extra(left) == subtree_extra(right)
        && subtree_external_scanner_state_eq(&left, &right)
}

#[inline]
unsafe fn stack_link_payload_is_equivalent(
    left: StackLinkPayload,
    right: StackLinkPayload,
) -> bool {
    stack__subtree_is_equivalent(
        stack_link_payload_subtree(left),
        stack_link_payload_subtree(right),
    ) && stack_link_payload_is_pending(left) == stack_link_payload_is_pending(right)
}

/// Add one predecessor edge to a stack node, merging equivalent paths.
///
/// If an equivalent edge already exists, the function either keeps the existing
/// payload or replaces it when the new path has higher dynamic precedence. If
/// the predecessor node itself represents the same state/position/error cost,
/// its links are folded into the existing predecessor to keep the graph shallow.
/// This is the core local compaction step that prevents GLR branching from
/// turning every ambiguity into a completely separate stack.
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
        if stack_link_payload_is_equivalent(existing_link.payload, link.payload) {
            if existing_link.node == link.node {
                if stack_link_payload_dynamic_precedence(link.payload)
                    > stack_link_payload_dynamic_precedence(existing_link.payload)
                {
                    stack_link_payload_retain(link.payload);
                    stack_link_payload_release(existing_link.payload, subtree_pool);
                    existing_link.payload = link.payload;
                    self_.dynamic_precedence = ptr_ref(link.node).dynamic_precedence
                        + stack_link_payload_dynamic_precedence(link.payload);
                }
                return;
            }

            let existing_node = ptr_ref(existing_link.node);
            let link_node = ptr_ref(link.node);
            if existing_node.state == link_node.state
                && existing_node.position.bytes == link_node.position.bytes
                && existing_node.error_cost == link_node.error_cost
            {
                for j in 0..link_node.link_count as usize {
                    stack_node_add_link(
                        ptr_mut(existing_link.node),
                        link_node.links[j],
                        subtree_pool,
                    );
                }
                let mut dynamic_precedence = link_node.dynamic_precedence;
                if !stack_link_payload_is_null(link.payload) {
                    dynamic_precedence += stack_link_payload_dynamic_precedence(link.payload);
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

    stack_node_retain(ptr_mut(link.node));
    let link_node = ptr_ref(link.node);
    let mut node_count = link_node.node_count;
    let mut dynamic_precedence = link_node.dynamic_precedence;
    self_.links[self_.link_count as usize] = link;
    self_.link_count += 1;

    if !stack_link_payload_is_null(link.payload) {
        stack_link_payload_retain(link.payload);
        node_count += stack_link_payload_node_count(link.payload);
        dynamic_precedence += stack_link_payload_dynamic_precedence(link.payload);
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
            subtree_release(subtree_pool, self_.last_external_token);
        }
        if !self_.lookahead_when_paused.ptr.is_null() {
            subtree_release(subtree_pool, self_.lookahead_when_paused);
        }
        if !self_.summary.is_null() {
            array_delete(ptr_mut(self_.summary));
            free(self_.summary.cast::<c_void>());
        }
        stack_node_release(ptr_mut(self_.node), pool, subtree_pool);
    }
}

/// Add a new version to the stack, cloning metadata from an existing version.
unsafe fn stack__add_version(
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
        subtree_retain(head.last_external_token);
    }
    self_.heads.size - 1
}

/// Add a slice to the stack's slice array, finding or creating a version.
unsafe fn stack__add_slice(
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

    let version = stack__add_version(self_, original_version, node);
    let slice = StackSlice {
        subtrees: ptr::read(subtrees),
        version,
    };
    array_push(&mut self_.slices, slice);
}

unsafe fn stack_pop_builder_reverse_subtrees(builder: &mut StackPopBuilder, start: u32, size: u32) {
    let limit = size / 2;
    for i in 0..limit {
        let reverse_index = start as usize + size as usize - 1 - i as usize;
        let a = builder.subtrees.contents.add(start as usize + i as usize);
        let b = builder.subtrees.contents.add(reverse_index);
        ptr::swap(a, b);
    }
}

unsafe fn stack_pop_builder_append_subtrees(
    builder: &mut StackPopBuilder,
    subtrees: &SubtreeArray,
) -> StackSliceSpan {
    let start = builder.subtrees.size;
    let dest = &mut builder.subtrees;
    array_reserve(dest, start + subtrees.size);
    if subtrees.size > 0 {
        ptr::copy_nonoverlapping(
            subtrees.contents,
            dest.contents.add(start as usize),
            subtrees.size as usize,
        );
    }
    dest.size = start + subtrees.size;
    StackSliceSpan {
        start,
        size: subtrees.size,
        version: STACK_VERSION_NONE,
    }
}

unsafe fn stack_pop_builder_add_slice(
    self_: &mut Stack,
    original_version: StackVersion,
    node: &mut StackNode,
    builder: &mut StackPopBuilder,
    mut slice: StackSliceSpan,
) {
    let node_ptr = ptr::from_mut(node);
    for i in (0..builder.slices.size).rev() {
        let version = array_get_ref(&builder.slices, i).version;
        if stack_head(self_, version).node == node_ptr {
            slice.version = version;
            array_insert(&mut builder.slices, i + 1, slice);
            return;
        }
    }

    slice.version = stack__add_version(self_, original_version, node);
    array_push(&mut builder.slices, slice);
}

/// Fast pop path for an unbranched stack chain.
///
/// The parser asks for `count` non-extra payloads. While every node has exactly
/// one predecessor, this can walk a straight linked list, retain payloads, then
/// reverse the collected child array into left-to-right order. Encountering a
/// branched node returns `None` so the caller can use the full DFS pop path.
unsafe fn stack_pop_count_linear(
    self_: &mut Stack,
    version: StackVersion,
    count: u32,
) -> Option<StackSliceArray> {
    array_clear(&mut self_.slices);

    let mut node = stack_head(self_, version).node;
    let mut subtree_count = 0;
    let mut subtrees = array_new();
    let reserve_count = subtree_alloc_size(count) / core::mem::size_of::<Subtree>();
    array_reserve(&mut subtrees, u32::try_from(reserve_count).unwrap());

    while subtree_count < count {
        let current_node = ptr_ref(node);
        if current_node.link_count != 1 {
            subtree_array_delete(ptr_mut(self_.subtree_pool), &mut subtrees);
            return None;
        }

        let link = current_node.links[0];
        node = link.node;
        let subtree = stack_link_payload_subtree(link.payload);
        if stack_link_payload_is_null(link.payload) {
            subtree_count += 1;
        } else {
            array_push(&mut subtrees, subtree);
            stack_link_payload_retain(link.payload);

            if !stack_link_payload_extra(link.payload) {
                subtree_count += 1;
            }
        }
    }

    subtree_array_reverse(&mut subtrees);
    stack__add_slice(self_, version, ptr_mut(node), &subtrees);
    Some(ptr::read(&self_.slices))
}

/// Builder-writing variant of `stack_pop_count_linear`.
///
/// Fresh parses use parser-owned `StackPopBuilder` scratch storage to avoid
/// allocating a temporary `StackSliceArray` for every reduce. The traversal and
/// fallback condition are the same as `stack_pop_count_linear`.
unsafe fn stack_pop_count_linear_into(
    self_: &mut Stack,
    version: StackVersion,
    count: u32,
    builder: &mut StackPopBuilder,
) -> bool {
    let mut node = stack_head(self_, version).node;
    let mut subtree_count = 0;
    let start = builder.subtrees.size;
    let reserve_count = subtree_alloc_size(count) / core::mem::size_of::<Subtree>();
    array_reserve(
        &mut builder.subtrees,
        start + u32::try_from(reserve_count).unwrap(),
    );

    while subtree_count < count {
        let current_node = ptr_ref(node);
        if current_node.link_count != 1 {
            let subtrees = &mut builder.subtrees;
            for i in start..subtrees.size {
                subtree_release(ptr_mut(self_.subtree_pool), *array_get_ref(subtrees, i));
            }
            subtrees.size = start;
            return false;
        }

        let link = current_node.links[0];
        node = link.node;
        let subtree = stack_link_payload_subtree(link.payload);
        if stack_link_payload_is_null(link.payload) {
            subtree_count += 1;
        } else {
            array_push(&mut builder.subtrees, subtree);
            stack_link_payload_retain(link.payload);

            if !stack_link_payload_extra(link.payload) {
                subtree_count += 1;
            }
        }
    }

    let size = builder.subtrees.size - start;
    stack_pop_builder_reverse_subtrees(builder, start, size);
    let slice = StackSliceSpan {
        start,
        size,
        version: STACK_VERSION_NONE,
    };
    stack_pop_builder_add_slice(self_, version, ptr_mut(node), builder, slice);
    true
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
        subtrees: array_new(),
        subtree_count: 0,
        is_pending: true,
    };

    if let Some(goal_subtree_count) = goal_subtree_count {
        let reserve_count =
            subtree_alloc_size(goal_subtree_count) / core::mem::size_of::<Subtree>();
        let subtrees = &mut new_iterator.subtrees;
        array_reserve(subtrees, u32::try_from(reserve_count).unwrap());
    }
    let include_subtrees = goal_subtree_count.is_some();

    array_push(&mut stack.iterators, new_iterator);

    while stack.iterators.size > 0 {
        let mut i: u32 = 0;
        let mut active_iterator_count = stack.iterators.size;
        while i < active_iterator_count {
            let iterator = array_get_ref(&stack.iterators, i);
            let node = iterator.node;

            let action = callback(payload, iterator);
            let should_pop = (action & STACK_ACTION_POP) != 0;
            let should_stop = (action & STACK_ACTION_STOP) != 0 || (*node).link_count == 0;

            if should_pop {
                let mut subtrees = ptr::read(&array_get_ref(&stack.iterators, i).subtrees);
                if !should_stop {
                    let source_subtrees = ptr::read(&subtrees);
                    subtree_array_copy(&source_subtrees, &mut subtrees);
                }
                subtree_array_reverse(&mut subtrees);
                stack__add_slice(stack, version, ptr_mut(node), &subtrees);
            }

            if should_stop {
                if !should_pop {
                    let iter = array_get_mut(&mut stack.iterators, i);
                    subtree_array_delete(ptr_mut(stack.subtree_pool), &mut iter.subtrees);
                }
                array_erase(&mut stack.iterators, i);
                active_iterator_count -= 1;
                continue;
            }

            // Copy all alternate branches, then reuse the current iterator for
            // link 0 so the common path avoids an extra subtree-array clone.
            let link_count = u32::from((*node).link_count);
            for branch_index in 1..=link_count {
                let next_iterator: &mut StackIterator;
                let link: StackLink;
                if branch_index == link_count {
                    link = (*node).links[0];
                    next_iterator = array_get_mut(&mut stack.iterators, i);
                } else {
                    if stack.iterators.size >= MAX_ITERATOR_COUNT {
                        continue;
                    }
                    link = (*node).links[branch_index as usize];
                    let current_iterator = ptr::read(array_get_ref(&stack.iterators, i));
                    array_push(&mut stack.iterators, current_iterator);
                    next_iterator = array_back_mut(&mut stack.iterators);
                    let source_subtrees = ptr::read(&next_iterator.subtrees);
                    subtree_array_copy(&source_subtrees, &mut next_iterator.subtrees);
                }

                next_iterator.node = link.node;
                let subtree = stack_link_payload_subtree(link.payload);
                if stack_link_payload_is_null(link.payload) {
                    next_iterator.subtree_count += 1;
                    next_iterator.is_pending = false;
                } else {
                    if include_subtrees {
                        let subtrees = &mut next_iterator.subtrees;
                        array_push(subtrees, subtree);
                        stack_link_payload_retain(link.payload);
                    }

                    if !stack_link_payload_extra(link.payload) {
                        next_iterator.subtree_count += 1;
                        if !stack_link_payload_is_pending(link.payload) {
                            next_iterator.is_pending = false;
                        }
                    }
                }
            }
            i = i.wrapping_add(1);
        }
    }

    ptr::read(&stack.slices)
}

// Callbacks for stack__iter
unsafe fn pop_count_callback(payload: *mut c_void, iterator: &StackIterator) -> StackAction {
    let goal_subtree_count = *ptr_ref(payload.cast::<u32>());
    if iterator.subtree_count == goal_subtree_count {
        STACK_ACTION_POP | STACK_ACTION_STOP
    } else {
        STACK_ACTION_NONE
    }
}

const unsafe fn pop_pending_callback(
    _payload: *mut c_void,
    iterator: &StackIterator,
) -> StackAction {
    if iterator.subtree_count >= 1 {
        if iterator.is_pending {
            STACK_ACTION_POP | STACK_ACTION_STOP
        } else {
            STACK_ACTION_STOP
        }
    } else {
        STACK_ACTION_NONE
    }
}

unsafe fn pop_error_callback(payload: *mut c_void, iterator: &StackIterator) -> StackAction {
    if iterator.subtrees.size > 0 {
        let found_error = ptr_mut(payload.cast::<bool>());
        if !*found_error && subtree_is_error(*array_get_ref(&iterator.subtrees, 0)) {
            *found_error = true;
            STACK_ACTION_POP | STACK_ACTION_STOP
        } else {
            STACK_ACTION_STOP
        }
    } else {
        STACK_ACTION_NONE
    }
}

unsafe fn pop_all_callback(_payload: *mut c_void, iterator: &StackIterator) -> StackAction {
    let node = ptr_ref(iterator.node);
    if node.link_count == 0 {
        STACK_ACTION_POP
    } else {
        STACK_ACTION_NONE
    }
}

unsafe fn summarize_stack_callback(payload: *mut c_void, iterator: &StackIterator) -> StackAction {
    let node = ptr_ref(iterator.node);
    let session = ptr_mut(payload.cast::<SummarizeStackSession>());
    let state = node.state;
    let depth = iterator.subtree_count;
    if depth > session.max_depth {
        return STACK_ACTION_STOP;
    }
    let summary = ptr_ref(session.summary);
    for i in (0..summary.size).rev() {
        let entry = array_get_ref(summary, i);
        if entry.depth < depth {
            break;
        }
        if entry.depth == depth && entry.state == state {
            return STACK_ACTION_NONE;
        }
    }
    array_push(
        ptr_mut(session.summary),
        StackSummaryEntry {
            position: node.position,
            depth,
            state,
        },
    );
    STACK_ACTION_NONE
}

// ===========================================================================
// Internal stack helpers used by the Rust parser.
// ===========================================================================

/// Create a new parse stack.
pub unsafe fn stack_new(subtree_pool: &mut SubtreePool) -> *mut Stack {
    let self_ = malloc(core::mem::size_of::<Stack>()).cast::<Stack>();
    ptr::write(
        self_,
        Stack {
            heads: array_new(),
            slices: array_new(),
            iterators: array_new(),
            node_pool: array_new(),
            base_node: ptr::null_mut(),
            subtree_pool,
        },
    );
    let stack = ptr_mut(self_);

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
    stack_clear(stack);

    self_
}

/// Free the parse stack.
pub unsafe fn stack_delete(self_: &mut Stack) {
    if !self_.slices.contents.is_null() {
        array_delete(&mut self_.slices);
    }
    if !self_.iterators.contents.is_null() {
        array_delete(&mut self_.iterators);
    }
    let subtree_pool = ptr_mut(self_.subtree_pool);
    stack_node_release(ptr_mut(self_.base_node), &mut self_.node_pool, subtree_pool);
    let heads = &mut self_.heads;
    let node_pool = &mut self_.node_pool;
    for i in 0..heads.size {
        stack_head_delete(array_get_mut(heads, i), node_pool, subtree_pool);
    }
    array_clear(heads);
    if !node_pool.contents.is_null() {
        for i in 0..node_pool.size {
            free((*array_get_ref(node_pool, i)).cast::<c_void>());
        }
        array_delete(node_pool);
    }
    array_delete(heads);
    free(ptr::from_mut(self_).cast::<c_void>());
}

/// Get the number of versions in the stack.
pub const fn stack_version_count(self_: &Stack) -> u32 {
    self_.heads.size
}

/// Get the number of halted versions.
pub unsafe fn stack_halted_version_count(self_: &Stack) -> u32 {
    let mut count = 0u32;
    for i in 0..self_.heads.size {
        if stack_head(self_, i).status == StackStatus::Halted {
            count += 1;
        }
    }
    count
}

/// Get the state at the top of a version.
pub unsafe fn stack_state(self_: &Stack, version: StackVersion) -> TSStateId {
    ptr_ref(stack_head(self_, version).node).state
}

/// Get the position of a version.
pub unsafe fn stack_position(self_: &Stack, version: StackVersion) -> Length {
    ptr_ref(stack_head(self_, version).node).position
}

/// Get the last external token for a version.
pub unsafe fn stack_last_external_token(self_: &Stack, version: StackVersion) -> Subtree {
    stack_head(self_, version).last_external_token
}

/// Set the last external token for a version.
pub unsafe fn stack_set_last_external_token(
    self_: &mut Stack,
    version: StackVersion,
    token: Subtree,
) {
    let subtree_pool = ptr_mut(self_.subtree_pool);
    let head = array_get_mut(&mut self_.heads, version);
    if !token.ptr.is_null() {
        subtree_retain(token);
    }
    if !head.last_external_token.ptr.is_null() {
        subtree_release(subtree_pool, head.last_external_token);
    }
    head.last_external_token = token;
}

/// Get the error cost for a version.
pub unsafe fn stack_error_cost(self_: &Stack, version: StackVersion) -> u32 {
    let head = stack_head(self_, version);
    let node = ptr_ref(head.node);
    let mut result = node.error_cost;
    if head.status == StackStatus::Paused
        || (node.state == ERROR_STATE && stack_link_payload_is_null(node.links[0].payload))
    {
        result += ERROR_COST_PER_RECOVERY;
    }
    result
}

/// Get the node count since last error for a version.
pub unsafe fn stack_node_count_since_error(self_: &mut Stack, version: StackVersion) -> u32 {
    let head = stack_head_mut(self_, version);
    let node = ptr_ref(head.node);
    if node.node_count < head.node_count_at_last_error {
        head.node_count_at_last_error = node.node_count;
    }
    node.node_count - head.node_count_at_last_error
}

/// Push a subtree onto a version.
pub unsafe fn stack_push(
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
pub unsafe fn stack_pop_count(
    self_: &mut Stack,
    version: StackVersion,
    count: u32,
) -> StackSliceArray {
    if let Some(result) = stack_pop_count_linear(self_, version, count) {
        return result;
    }

    stack__iter(
        self_,
        version,
        pop_count_callback,
        ptr::addr_of!(count).cast_mut().cast::<c_void>(),
        Some(count),
    )
}

/// Pop a given number of entries from a version into a caller-owned builder.
pub unsafe fn stack_pop_count_into(
    self_: &mut Stack,
    version: StackVersion,
    count: u32,
    builder: &mut StackPopBuilder,
) {
    stack_pop_builder_clear(builder);
    if stack_pop_count_linear_into(self_, version, count, builder) {
        return;
    }

    let pop = stack__iter(
        self_,
        version,
        pop_count_callback,
        ptr::addr_of!(count).cast_mut().cast::<c_void>(),
        Some(count),
    );
    for i in 0..pop.size {
        let mut slice = ptr::read(array_get_ref(&pop, i));
        let mut span = stack_pop_builder_append_subtrees(builder, &slice.subtrees);
        span.version = slice.version;
        array_push(&mut builder.slices, span);
        array_delete(&mut slice.subtrees);
    }
}

/// Pop an error from the top of a version.
pub unsafe fn stack_pop_error(self_: &mut Stack, version: StackVersion) -> SubtreeArray {
    let node = stack_head(self_, version).node;
    for i in 0..(*node).link_count as usize {
        let payload = (*node).links[i].payload;
        let subtree = stack_link_payload_subtree(payload);
        if !stack_link_payload_is_null(payload) && subtree_is_error(subtree) {
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
                stack_renumber_version(self_, first_pop.version, version);
                return ptr::read(&first_pop.subtrees);
            }
            break;
        }
    }
    array_new()
}

/// Pop pending entries from a version.
pub unsafe fn stack_pop_pending(self_: &mut Stack, version: StackVersion) -> StackSliceArray {
    let mut pop = stack__iter(
        self_,
        version,
        pop_pending_callback,
        ptr::null_mut(),
        Some(0),
    );
    if pop.size > 0 {
        let first_pop = array_get_mut(&mut pop, 0);
        stack_renumber_version(self_, first_pop.version, version);
        first_pop.version = version;
    }
    pop
}

/// Pop all entries from a version.
pub unsafe fn stack_pop_all(self_: &mut Stack, version: StackVersion) -> StackSliceArray {
    stack__iter(self_, version, pop_all_callback, ptr::null_mut(), Some(0))
}

/// Record a summary of parse states near the top of a version.
pub unsafe fn stack_record_summary(self_: &mut Stack, version: StackVersion, max_depth: u32) {
    let summary = malloc(core::mem::size_of::<StackSummary>()).cast::<StackSummary>();
    ptr::write(summary, array_new());
    let mut session = SummarizeStackSession { summary, max_depth };
    stack__iter(
        self_,
        version,
        summarize_stack_callback,
        ptr::from_mut(&mut session).cast::<c_void>(),
        None,
    );
    let head = stack_head_mut(self_, version);
    if !head.summary.is_null() {
        array_delete(ptr_mut(head.summary));
        free(head.summary.cast::<c_void>());
    }
    head.summary = session.summary;
}

/// Get the recorded summary for a version.
pub unsafe fn stack_get_summary(stack: &Stack, version: StackVersion) -> *mut StackSummary {
    stack_head(stack, version).summary
}

/// Get the dynamic precedence of a version.
pub unsafe fn stack_dynamic_precedence(self_: &Stack, version: StackVersion) -> i32 {
    stack_head(self_, version)
        .node
        .as_ref()
        .unwrap_unchecked()
        .dynamic_precedence
}

/// Check if a version has advanced since the last error.
pub unsafe fn stack_has_advanced_since_error(self_: &Stack, version: StackVersion) -> bool {
    let head = stack_head(self_, version);
    let mut node = head.node;
    if (*node).error_cost == 0 {
        return true;
    }
    loop {
        if (*node).link_count > 0 {
            let payload = (*node).links[0].payload;
            if !stack_link_payload_is_null(payload) {
                if stack_link_payload_total_bytes(payload) > 0 {
                    return true;
                } else if (*node).node_count > head.node_count_at_last_error
                    && stack_link_payload_error_cost(payload) == 0
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
pub unsafe fn stack_remove_version(self_: &mut Stack, version: StackVersion) {
    let heads = &mut self_.heads;
    let node_pool = &mut self_.node_pool;
    let subtree_pool = ptr_mut(self_.subtree_pool);
    stack_head_delete(array_get_mut(heads, version), node_pool, subtree_pool);
    array_erase(heads, version);
}

/// Renumber version v1 to v2 (move v1 into v2's slot, removing v2).
pub unsafe fn stack_renumber_version(stack: &mut Stack, v1: StackVersion, v2: StackVersion) {
    if v1 == v2 {
        return;
    }
    debug_assert!(v2 < v1);
    debug_assert!(v1 < stack.heads.size);

    let heads = &mut stack.heads;
    let node_pool = &mut stack.node_pool;
    let subtree_pool = ptr_mut(stack.subtree_pool);
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
pub unsafe fn stack_swap_versions(stack: &mut Stack, v1: StackVersion, v2: StackVersion) {
    let temp = ptr::read(array_get_ref(&stack.heads, v1));
    let other = ptr::read(array_get_ref(&stack.heads, v2));
    ptr::write(array_get_mut(&mut stack.heads, v1), other);
    ptr::write(array_get_mut(&mut stack.heads, v2), temp);
}

/// Copy a version, creating a new one.
pub unsafe fn stack_copy_version(stack: &mut Stack, version: StackVersion) -> StackVersion {
    debug_assert!(version < stack.heads.size);
    let version_head = ptr::read(array_get_ref(&stack.heads, version));
    array_push(&mut stack.heads, version_head);
    let head = array_back_mut(&mut stack.heads);
    stack_node_retain(ptr_mut(head.node));
    if !head.last_external_token.ptr.is_null() {
        subtree_retain(head.last_external_token);
    }
    head.summary = ptr::null_mut();
    stack.heads.size - 1
}

/// Merge two versions if possible.
pub unsafe fn stack_merge(
    stack: &mut Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    if !stack_can_merge(stack, version1, version2) {
        return false;
    }
    {
        let stack_heads = &mut stack.heads;
        let subtree_pool = ptr_mut(stack.subtree_pool);
        let (head1, head2) = stack_head_array_pair_mut(stack_heads, version1, version2);
        let head2_node = ptr_ref(head2.node);
        for i in 0..head2_node.link_count as usize {
            stack_node_add_link(ptr_mut(head1.node), head2_node.links[i], subtree_pool);
        }
        let head1_node = ptr_ref(head1.node);
        if head1_node.state == ERROR_STATE {
            head1.node_count_at_last_error = head1_node.node_count;
        }
    }
    stack_remove_version(stack, version2);
    true
}

/// Check if two versions can be merged.
pub unsafe fn stack_can_merge(
    stack: &Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    let head1 = stack_head(stack, version1);
    let head2 = stack_head(stack, version2);
    let node1 = ptr_ref(head1.node);
    let node2 = ptr_ref(head2.node);
    head1.status == StackStatus::Active
        && head2.status == StackStatus::Active
        && node1.state == node2.state
        && node1.position.bytes == node2.position.bytes
        && node1.error_cost == node2.error_cost
        && subtree_external_scanner_state_eq(&head1.last_external_token, &head2.last_external_token)
}

/// Halt a version.
pub unsafe fn stack_halt(self_: &mut Stack, version: StackVersion) {
    stack_head_mut(self_, version).status = StackStatus::Halted;
}

/// Pause a version with a lookahead token.
pub unsafe fn stack_pause(stack: &mut Stack, version: StackVersion, lookahead: Subtree) {
    let head = stack_head_mut(stack, version);
    head.status = StackStatus::Paused;
    head.lookahead_when_paused = lookahead;
    head.node_count_at_last_error = ptr_ref(head.node).node_count;
}

/// Check if a version is active.
pub unsafe fn stack_is_active(self_: &Stack, version: StackVersion) -> bool {
    stack_head(self_, version).status == StackStatus::Active
}

/// Check if a version is halted.
pub unsafe fn stack_is_halted(self_: &Stack, version: StackVersion) -> bool {
    stack_head(self_, version).status == StackStatus::Halted
}

/// Check if a version is paused.
pub unsafe fn stack_is_paused(self_: &Stack, version: StackVersion) -> bool {
    stack_head(self_, version).status == StackStatus::Paused
}

/// Resume a paused version, returning its stored lookahead.
pub unsafe fn stack_resume(stack: &mut Stack, version: StackVersion) -> Subtree {
    let head = stack_head_mut(stack, version);
    debug_assert!(head.status == StackStatus::Paused);
    let result = head.lookahead_when_paused;
    head.status = StackStatus::Active;
    head.lookahead_when_paused = NULL_SUBTREE;
    result
}

/// Clear all versions, resetting to initial state.
pub unsafe fn stack_clear(self_: &mut Stack) {
    stack_node_retain(ptr_mut(self_.base_node));
    let heads = &mut self_.heads;
    let node_pool = &mut self_.node_pool;
    let subtree_pool = ptr_mut(self_.subtree_pool);
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
pub unsafe fn stack_print_dot_graph(
    stack: &mut Stack,
    language: *const TSLanguage,
    mut f: *mut c_void,
) -> bool {
    array_reserve(&mut stack.iterators, 32);
    if f.is_null() {
        f = stderr_file();
    }

    fprintf(f, c"digraph stack {\n".as_ptr().cast::<i8>());
    fprintf(f, c"rankdir=\"RL\";\n".as_ptr().cast::<i8>());
    fprintf(f, c"edge [arrowhead=none]\n".as_ptr().cast::<i8>());

    let mut visited_nodes: Array<*mut StackNode> = array_new();

    array_clear(&mut stack.iterators);
    for i in 0..stack.heads.size {
        if stack_head(stack, i).status == StackStatus::Halted {
            continue;
        }
        let node_count_since_error = stack_node_count_since_error(stack, i);
        let error_cost = stack_error_cost(stack, i);
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
            let summary = ptr_ref(head.summary);
            for j in 0..summary.size {
                let entry = array_get_ref(summary, j);
                fprintf(f, c" %u".as_ptr().cast::<i8>(), u32::from(entry.state));
            }
        }

        if !head.last_external_token.ptr.is_null() {
            let state = subtree_external_scanner_state(&head.last_external_token);
            let data = external_scanner_state_data(state);
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
            subtrees: array_new(),
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
            let node_ref = ptr_ref(node);

            fprintf(f, c"node_%p [".as_ptr().cast::<i8>(), node as *const c_void);
            if node_ref.state == ERROR_STATE {
                fprintf(f, c"label=\"?\"".as_ptr().cast::<i8>());
            } else if node_ref.link_count == 1
                && !stack_link_payload_is_null(node_ref.links[0].payload)
                && stack_link_payload_extra(node_ref.links[0].payload)
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
                if stack_link_payload_is_pending(link.payload) {
                    fprintf(f, c"style=dashed ".as_ptr().cast::<i8>());
                }
                let subtree = stack_link_payload_subtree(link.payload);
                if !stack_link_payload_is_null(link.payload)
                    && stack_link_payload_extra(link.payload)
                {
                    fprintf(f, c"fontcolor=gray ".as_ptr().cast::<i8>());
                }

                if stack_link_payload_is_null(link.payload) {
                    fprintf(f, c"color=red".as_ptr().cast::<i8>());
                } else {
                    fprintf(f, c"label=\"".as_ptr().cast::<i8>());
                    let quoted = subtree_visible(subtree) && !subtree_named(subtree);
                    if quoted {
                        fprintf(f, c"'".as_ptr().cast::<i8>());
                    }
                    language_write_symbol_as_dot_string(language, f, subtree_symbol(subtree));
                    if quoted {
                        fprintf(f, c"'".as_ptr().cast::<i8>());
                    }
                    fprintf(f, c"\"".as_ptr().cast::<i8>());
                    fprintf(
                        f,
                        c"labeltooltip=\"error_cost: %u\ndynamic_precedence: %d\""
                            .as_ptr()
                            .cast::<i8>(),
                        stack_link_payload_error_cost(link.payload),
                        stack_link_payload_dynamic_precedence(link.payload),
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
