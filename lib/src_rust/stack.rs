//! GLR parse stack and version management.
//!
//! This module implements the branching parse stack used by the GLR parser.
//! Multiple "versions" of the stack can exist simultaneously, representing
//! different parse paths. Versions can be merged when they reach the same
//! state, enabling efficient ambiguity handling.
//!
//! Stack values never cross the C ABI. Their structures intentionally use
//! Rust layout; only exported parser and cursor handles need fixed layouts.

use core::ffi::c_void;
use core::ptr;
use core::ptr::NonNull;

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

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

pub type StackVersion = u32;
pub const STACK_VERSION_NONE: StackVersion = u32::MAX;

/// Directed edge from a stack node to one predecessor.
///
/// The subtree is the syntax node that was shifted/reduced between the
/// predecessor and the current node. Multiple links model GLR ambiguity: the
/// same parse state/position can be reached through different child lists.
#[derive(Clone, Copy)]
pub struct StackLink {
    node: *mut StackNode,
    subtree: Subtree,
}

/// Node in the persistent GLR stack graph.
///
/// A parser version points at one `StackNode` head. Pushing creates a new node
/// linked to the previous head; popping walks backward through links and may
/// fork when a node has multiple predecessors. Cached aggregate fields describe
/// the best path through the node and are used for pruning and merging.
pub struct StackNode {
    /// Parse state at this stack depth.
    state: TSStateId,
    /// Source position reached by the best path to this node.
    position: Length,
    /// Inline predecessor links. Ambiguous nodes can carry several links.
    links: [StackLink; MAX_LINK_COUNT],
    /// Number of initialized entries in `links`.
    link_count: u16,
    /// Intrusive reference count from stack heads and successor links.
    ref_count: u32,
    /// Accumulated parse error cost for pruning worse versions.
    error_cost: u32,
    /// Approximate visible node count since the last error.
    node_count: u32,
    /// Accumulated dynamic precedence along the best path.
    dynamic_precedence: i32,
}

/// DFS cursor used by stack pop operations.
pub struct StackIterator {
    /// Current graph node being visited.
    node: *mut StackNode,
    /// Child subtrees collected so far along this pop path.
    subtrees: SubtreeArray,
    /// Number of non-extra stack entries traversed.
    subtree_count: u32,
}

pub type StackNodeArray = Array<*mut StackNode>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StackStatus {
    Active,
    Paused,
    Halted,
}

pub struct StackSlice {
    pub(super) subtrees: SubtreeArray,
    pub(super) version: StackVersion,
}

pub type StackSliceArray = Array<StackSlice>;

#[derive(Clone, Copy)]
pub struct StackSummaryEntry {
    pub(super) position: Length,
    pub(super) depth: u32,
    pub(super) state: TSStateId,
}

pub type StackSummary = Array<StackSummaryEntry>;

pub struct StackHead {
    /// Current top node for this parser version.
    node: *mut StackNode,
    /// Optional recovery summary, recorded lazily.
    summary: Option<NonNull<StackSummary>>,
    /// Node-count checkpoint used by recovery progress heuristics.
    node_count_at_last_error: u32,
    /// Last token carrying external scanner state for this version.
    last_external_token: Subtree,
    /// Lookahead saved when this version is paused for error recovery.
    lookahead_when_paused: Subtree,
    /// Active versions parse normally; paused versions wait for recovery;
    /// halted versions are removed by stack condensation.
    status: StackStatus,
}

pub struct Stack {
    /// One head per active/paused/halted GLR version.
    heads: Array<StackHead>,
    /// Scratch pop results returned to the parser.
    slices: StackSliceArray,
    /// Reusable DFS iterators for pop operations.
    iterators: Array<StackIterator>,
    /// Free list for recently released stack nodes.
    node_pool: StackNodeArray,
    /// Number of heads whose status is `Halted`.
    halted_version_count: u32,
    /// Initial root node shared by all versions.
    base_node: *mut StackNode,
    /// Parser-owned subtree pool used when releasing link subtrees.
    subtree_pool: *mut SubtreePool,
}

#[derive(Clone, Copy)]
enum StackIterationAction {
    Continue,
    Stop,
    Pop,
    PopAndStop,
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
    #[allow(non_snake_case)]
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
                if !link.subtree.is_null() {
                    subtree_release(subtree_pool, link.subtree);
                }
                stack_node_release(ptr_mut(link.node), pool, subtree_pool);
            }
            let link = node.links[0];
            if !link.subtree.is_null() {
                subtree_release(subtree_pool, link.subtree);
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
unsafe fn stack_subtree_node_count(subtree: Subtree) -> u32 {
    let mut count = subtree_visible_descendant_count(subtree);
    if subtree_visible(subtree) {
        count += 1;
    }
    if subtree_symbol(subtree) == TS_BUILTIN_SYM_ERROR_REPEAT {
        count += 1;
    }
    count
}

/// Allocate a new stack node, reusing from pool if available.
unsafe fn stack_node_new(
    previous_node: *mut StackNode,
    subtree: Subtree,
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
                subtree: NULL_SUBTREE,
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
            subtree,
        };

        (*node).position = (*previous_node).position;
        (*node).error_cost = (*previous_node).error_cost;
        (*node).dynamic_precedence = (*previous_node).dynamic_precedence;
        (*node).node_count = (*previous_node).node_count;

        if !subtree.is_null() {
            (*node).error_cost += subtree_error_cost(subtree);
            (*node).position = length_add((*node).position, subtree_total_size(subtree));
            (*node).node_count += stack_subtree_node_count(subtree);
            (*node).dynamic_precedence += subtree_dynamic_precedence(subtree);
        }
    }

    node
}

/// Check if two subtrees are equivalent for merging purposes.
unsafe fn stack_subtree_is_equivalent(left: Subtree, right: Subtree) -> bool {
    if left == right {
        return true;
    }
    if left.is_null() || right.is_null() {
        return false;
    }

    let left_symbol = subtree_symbol(left);
    let right_symbol = subtree_symbol(right);
    if left_symbol != right_symbol {
        return false;
    }

    let left_error_cost = subtree_error_cost(left);
    let right_error_cost = subtree_error_cost(right);
    if left_error_cost > 0 && right_error_cost > 0 {
        return true;
    }

    let left_child_count = subtree_child_count(left);
    let right_child_count = subtree_child_count(right);
    subtree_padding(left).bytes == subtree_padding(right).bytes
        && subtree_size(left).bytes == subtree_size(right).bytes
        && left_child_count == right_child_count
        && subtree_extra(left) == subtree_extra(right)
        && subtree_external_scanner_state_eq(&left, &right)
}

/// Add one predecessor edge to a stack node, merging equivalent paths.
///
/// If an equivalent edge already exists, the function either keeps the existing
/// subtree or replaces it when the new path has higher dynamic precedence. If
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
        if stack_subtree_is_equivalent(existing_link.subtree, link.subtree) {
            if existing_link.node == link.node {
                if subtree_dynamic_precedence(link.subtree)
                    > subtree_dynamic_precedence(existing_link.subtree)
                {
                    subtree_retain(link.subtree);
                    subtree_release(subtree_pool, existing_link.subtree);
                    existing_link.subtree = link.subtree;
                    self_.dynamic_precedence = ptr_ref(link.node).dynamic_precedence
                        + subtree_dynamic_precedence(link.subtree);
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
                if !link.subtree.is_null() {
                    dynamic_precedence += subtree_dynamic_precedence(link.subtree);
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

    if !link.subtree.is_null() {
        subtree_retain(link.subtree);
        node_count += stack_subtree_node_count(link.subtree);
        dynamic_precedence += subtree_dynamic_precedence(link.subtree);
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
        if !self_.last_external_token.is_null() {
            subtree_release(subtree_pool, self_.last_external_token);
        }
        if !self_.lookahead_when_paused.is_null() {
            subtree_release(subtree_pool, self_.lookahead_when_paused);
        }
        if let Some(mut summary) = self_.summary.take() {
            array_delete(summary.as_mut());
            free(summary.as_ptr().cast::<c_void>());
        }
        stack_node_release(ptr_mut(self_.node), pool, subtree_pool);
    }
}

mod pop;
use pop::{pop_all_action, pop_count_action, pop_error_action, stack_iter, summarize_stack_action};

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
            halted_version_count: 0,
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
    stack.base_node = stack_node_new(ptr::null_mut(), NULL_SUBTREE, 1, &mut stack.node_pool);
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
pub const fn stack_halted_version_count(self_: &Stack) -> u32 {
    self_.halted_version_count
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
    if !token.is_null() {
        subtree_retain(token);
    }
    if !head.last_external_token.is_null() {
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
        || (node.state == ERROR_STATE && node.links[0].subtree.is_null())
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
    state: TSStateId,
) {
    let heads = &mut stack.heads;
    let node_pool = &mut stack.node_pool;
    let head = array_get_mut(heads, version);
    let new_node = stack_node_new(head.node, subtree, state, node_pool);
    if subtree.is_null() {
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
    stack_iter(
        self_,
        version,
        |iterator| pop_count_action(iterator, count),
        Some(count),
    )
}

/// Pop an error from the top of a version.
pub unsafe fn stack_pop_error(self_: &mut Stack, version: StackVersion) -> SubtreeArray {
    let node = stack_head(self_, version).node;
    for i in 0..(*node).link_count as usize {
        let subtree = (*node).links[i].subtree;
        if !subtree.is_null() && subtree_is_error(subtree) {
            let mut found_error = false;
            let pop = stack_iter(
                self_,
                version,
                |iterator| pop_error_action(iterator, &mut found_error),
                Some(1),
            );
            if pop.size > 0 {
                debug_assert_eq!(pop.size, 1);
                let first_pop = array_get_ref(&pop, 0);
                stack_renumber_version(self_, first_pop.version, version);
                return ptr::read(&first_pop.subtrees);
            }
            break;
        }
    }
    array_new()
}

/// Pop all entries from a version.
pub unsafe fn stack_pop_all(self_: &mut Stack, version: StackVersion) -> StackSliceArray {
    stack_iter(self_, version, |iterator| pop_all_action(iterator), Some(0))
}

/// Record a summary of parse states near the top of a version.
pub unsafe fn stack_record_summary(self_: &mut Stack, version: StackVersion, max_depth: u32) {
    let mut summary = array_new();
    stack_iter(
        self_,
        version,
        |iterator| summarize_stack_action(iterator, &mut summary, max_depth),
        None,
    );
    let summary_ptr =
        NonNull::new_unchecked(malloc(core::mem::size_of::<StackSummary>()).cast::<StackSummary>());
    ptr::write(summary_ptr.as_ptr(), summary);
    let head = stack_head_mut(self_, version);
    if let Some(mut previous) = head.summary.replace(summary_ptr) {
        array_delete(previous.as_mut());
        free(previous.as_ptr().cast::<c_void>());
    }
}

/// Get the recorded summary for a version.
pub unsafe fn stack_get_summary(stack: &Stack, version: StackVersion) -> Option<&StackSummary> {
    stack_head(stack, version)
        .summary
        .map(|summary| summary.as_ref())
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
            let subtree = (*node).links[0].subtree;
            if !subtree.is_null() {
                if subtree_total_bytes(subtree) > 0 {
                    return true;
                } else if (*node).node_count > head.node_count_at_last_error
                    && subtree_error_cost(subtree) == 0
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
    if array_get_ref(heads, version).status == StackStatus::Halted {
        self_.halted_version_count -= 1;
    }
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
    if target_head.status == StackStatus::Halted {
        stack.halted_version_count -= 1;
    }
    if source_head.summary.is_none() {
        source_head.summary = target_head.summary.take();
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
    if !head.last_external_token.is_null() {
        subtree_retain(head.last_external_token);
    }
    if head.status == StackStatus::Halted {
        stack.halted_version_count += 1;
    }
    head.summary = None;
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
    if stack_head(self_, version).status != StackStatus::Halted {
        self_.halted_version_count += 1;
        stack_head_mut(self_, version).status = StackStatus::Halted;
    }
}

/// Pause a version with a lookahead token.
pub unsafe fn stack_pause(stack: &mut Stack, version: StackVersion, lookahead: Subtree) {
    if stack_head(stack, version).status == StackStatus::Halted {
        stack.halted_version_count -= 1;
    }
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
    self_.halted_version_count = 0;
    array_push(
        heads,
        StackHead {
            node: self_.base_node,
            status: StackStatus::Active,
            last_external_token: NULL_SUBTREE,
            lookahead_when_paused: NULL_SUBTREE,
            summary: None,
            node_count_at_last_error: 0,
        },
    );
}

mod debug;
pub use debug::stack_print_dot_graph;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_impl::subtree::{subtree_pool_delete, subtree_pool_new};

    #[test]
    fn halted_version_count_tracks_status_changes() {
        unsafe {
            let mut pool = subtree_pool_new(0);
            let stack = stack_new(&mut pool);
            let stack = ptr_mut(stack);

            assert_eq!(stack_halted_version_count(stack), 0);

            let halted = stack_copy_version(stack, 0);
            stack_halt(stack, halted);
            assert_eq!(stack_halted_version_count(stack), 1);

            let halted_copy = stack_copy_version(stack, halted);
            assert_eq!(stack_halted_version_count(stack), 2);

            stack_pause(stack, halted_copy, NULL_SUBTREE);
            assert_eq!(stack_halted_version_count(stack), 1);
            let _ = stack_resume(stack, halted_copy);
            assert_eq!(stack_halted_version_count(stack), 1);

            stack_halt(stack, halted_copy);
            assert_eq!(stack_halted_version_count(stack), 2);
            stack_remove_version(stack, halted_copy);
            assert_eq!(stack_halted_version_count(stack), 1);

            stack_clear(stack);
            assert_eq!(stack_halted_version_count(stack), 0);

            stack_delete(stack);
            subtree_pool_delete(&mut pool);
        }
    }
}
