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

use crate::ffi::TSStateId;

use super::alloc::{free, malloc};
use super::error_costs::{ERROR_COST_PER_RECOVERY, ERROR_STATE};
use super::length::{length_add, Length};
use super::subtree::{subtree_alloc_size, Subtree, SubtreeArray, SubtreePool, NULL_SUBTREE};
use super::utils::ptr_mut;
use super::utils::Array;

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
/// Logically, the edge is the forward LR step
/// `predecessor --subtree--> current`; it is stored backward so reductions can
/// pop toward older states. The subtree is the syntax value shifted or reduced
/// during that step.
///
/// A newly pushed node has one link. When compatible GLR versions reconverge,
/// `stack_merge` copies their predecessor links into one surviving current
/// node. Multiple links therefore preserve different pasts while sharing a
/// parser configuration with the same future behavior.
#[derive(Clone, Copy)]
pub struct StackLink {
    /// Parser configuration before `subtree` was recognized.
    node: NonNull<StackNode>,
    /// Syntax value recognized between the predecessor and current states.
    subtree: Subtree,
}

/// DFS cursor used by stack pop operations.
pub struct StackIterator {
    /// Current graph node being visited.
    node: NonNull<StackNode>,
    /// Child subtrees collected so far along this pop path.
    subtrees: SubtreeArray,
    /// Number of non-extra stack entries traversed.
    subtree_count: u32,
}

type StackNodeArray = Array<NonNull<StackNode>>;

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

/// One unmaterialized step in the deterministic suffix of the parse stack.
///
/// The cumulative fields deliberately mirror [`StackNode`]. This makes
/// materialization a field-for-field conversion while avoiding a 160-byte
/// graph node and its unused alternate-link slots for ordinary LR steps.
#[derive(Clone, Copy)]
struct WindowEntry {
    state: TSStateId,
    position: Length,
    error_cost: u32,
    node_count: u32,
    dynamic_precedence: i32,
    subtree: Subtree,
}

/// Current parser configuration and per-version recovery state.
pub(super) struct StackHead {
    /// Current top node for this parser version.
    node: NonNull<StackNode>,
    /// Cached logical top fields. While the deterministic window is nonempty,
    /// these describe its last entry; otherwise they match `node`.
    state: TSStateId,
    position: Length,
    error_cost: u32,
    node_count: u32,
    dynamic_precedence: i32,
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

impl StackHead {
    pub(super) const fn state(&self) -> TSStateId {
        self.state
    }

    pub(super) const fn position(&self) -> Length {
        self.position
    }

    pub(super) const fn last_external_token(&self) -> Subtree {
        self.last_external_token
    }

    pub(super) const fn is_active(&self) -> bool {
        matches!(self.status, StackStatus::Active)
    }

    pub(super) const fn is_halted(&self) -> bool {
        matches!(self.status, StackStatus::Halted)
    }

    pub(super) const fn is_paused(&self) -> bool {
        matches!(self.status, StackStatus::Paused)
    }
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
    /// Linear suffix used while there is exactly one deterministic version.
    window: Array<WindowEntry>,
    /// Whether pushes and reductions may use `window`. Materialization clears
    /// this flag until stack condensation establishes a linear configuration.
    window_enabled: bool,
    /// Number of heads whose status is `Halted`.
    halted_version_count: u32,
    /// Initial root node shared by all versions.
    base_node: NonNull<StackNode>,
    /// Parser-owned subtree pool used when releasing link subtrees.
    subtree_pool: *mut SubtreePool,
}

impl Stack {
    pub const fn version_count(&self) -> u32 {
        self.heads.size
    }

    pub const fn halted_version_count(&self) -> u32 {
        self.halted_version_count
    }

    /// Return the head for one GLR stack version.
    ///
    /// # Safety
    ///
    /// `version` must identify a current stack version.
    pub(super) unsafe fn head(&self, version: StackVersion) -> &StackHead {
        self.heads.get_unchecked(version)
    }

    unsafe fn head_mut(&mut self, version: StackVersion) -> &mut StackHead {
        self.heads.get_unchecked_mut(version)
    }

    unsafe fn materialize_window(&mut self) {
        if !self.window_enabled {
            return;
        }
        debug_assert_eq!(self.heads.size, 1);

        let mut node = self.head(0).node;
        for i in 0..self.window.size {
            let entry = self.window.get_unchecked(i);
            node = stack_node_new(Some(node), entry.subtree, entry.state, &mut self.node_pool);
            let materialized = node.as_ref();
            debug_assert_eq!(materialized.position, entry.position);
            debug_assert_eq!(materialized.error_cost, entry.error_cost);
            debug_assert_eq!(materialized.node_count, entry.node_count);
            debug_assert_eq!(materialized.dynamic_precedence, entry.dynamic_precedence);
        }
        self.window.clear();
        self.head_mut(0).node = node;
        self.window_enabled = false;
    }

    unsafe fn release_window(&mut self) {
        let subtree_pool = ptr_mut(self.subtree_pool);
        for entry in self.window.as_slice() {
            if !entry.subtree.is_null() {
                entry.subtree.release(subtree_pool);
            }
        }
        self.window.clear();
        self.window_enabled = false;
    }

    unsafe fn sync_head_from_node(&mut self, version: StackVersion) {
        let node = self.head(version).node.as_ref();
        let (state, position, error_cost, node_count, dynamic_precedence) = (
            node.state,
            node.position,
            node.error_cost,
            node.node_count,
            node.dynamic_precedence,
        );
        let head = self.head_mut(version);
        head.state = state;
        head.position = position;
        head.error_cost = error_cost;
        head.node_count = node_count;
        head.dynamic_precedence = dynamic_precedence;
    }

    pub unsafe fn set_last_external_token(&mut self, version: StackVersion, token: Subtree) {
        let subtree_pool = ptr_mut(self.subtree_pool);
        let head = self.heads.get_unchecked_mut(version);
        if !token.is_null() {
            token.retain();
        }
        if !head.last_external_token.is_null() {
            head.last_external_token.release(subtree_pool);
        }
        head.last_external_token = token;
    }

    pub unsafe fn error_cost(&self, version: StackVersion) -> u32 {
        let head = self.head(version);
        let mut result = head.error_cost;
        if head.status == StackStatus::Paused
            || (head.state == ERROR_STATE
                && self.window.is_empty()
                && head.node.as_ref().links[0].subtree.is_null())
        {
            result += ERROR_COST_PER_RECOVERY;
        }
        result
    }

    pub unsafe fn dynamic_precedence(&self, version: StackVersion) -> i32 {
        if self.window_enabled {
            debug_assert_eq!(version, 0);
            self.head(version).dynamic_precedence
        } else {
            self.head(version).node.as_ref().dynamic_precedence
        }
    }

    pub unsafe fn node_count_since_error(&mut self, version: StackVersion) -> u32 {
        let node_count = if self.window_enabled {
            debug_assert_eq!(version, 0);
            self.head(version).node_count
        } else {
            self.head(version).node.as_ref().node_count
        };
        let head = self.head_mut(version);
        if node_count < head.node_count_at_last_error {
            head.node_count_at_last_error = node_count;
        }
        node_count - head.node_count_at_last_error
    }

    pub unsafe fn has_advanced_since_error(&self, version: StackVersion) -> bool {
        let head = self.head(version);
        if head.error_cost == 0 {
            return true;
        }

        if self.window_enabled {
            let mut node_count = head.node_count;
            for entry in self.window.as_slice().iter().rev() {
                let subtree = entry.subtree;
                if subtree.total_bytes() > 0 {
                    return true;
                }
                if node_count > head.node_count_at_last_error && subtree.error_cost() == 0 {
                    node_count =
                        node_count.saturating_sub(stack_node::stack_subtree_node_count(subtree));
                    continue;
                }
                return false;
            }
        }

        let mut node = head.node;
        if node.as_ref().error_cost == 0 {
            return true;
        }
        loop {
            let node_ref = node.as_ref();
            if node_ref.link_count > 0 {
                let subtree = node_ref.links[0].subtree;
                if !subtree.is_null() {
                    if subtree.total_bytes() > 0 {
                        return true;
                    } else if node_ref.node_count > head.node_count_at_last_error
                        && subtree.error_cost() == 0
                    {
                        node = node_ref.links[0].node;
                        continue;
                    }
                }
            }
            return false;
        }
    }

    pub unsafe fn halt(&mut self, version: StackVersion) {
        if !self.head(version).is_halted() {
            self.halted_version_count += 1;
            self.head_mut(version).status = StackStatus::Halted;
        }
    }

    pub unsafe fn pause(&mut self, version: StackVersion, lookahead: Subtree) {
        self.materialize_window();
        if self.head(version).is_halted() {
            self.halted_version_count -= 1;
        }
        let head = self.head_mut(version);
        head.status = StackStatus::Paused;
        head.lookahead_when_paused = lookahead;
        head.node_count_at_last_error = head.node_count;
    }

    pub unsafe fn resume(&mut self, version: StackVersion) -> Subtree {
        let head = self.head_mut(version);
        debug_assert!(head.status == StackStatus::Paused);
        let result = head.lookahead_when_paused;
        head.status = StackStatus::Active;
        head.lookahead_when_paused = NULL_SUBTREE;
        result
    }
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
unsafe fn stack_head_array_pair_mut(
    heads: &mut Array<StackHead>,
    first: StackVersion,
    second: StackVersion,
) -> (&mut StackHead, &mut StackHead) {
    debug_assert_ne!(first, second);
    debug_assert!((first as usize) < heads.len());
    debug_assert!((second as usize) < heads.len());

    let heads = heads.as_mut_slice();
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

mod stack_node;
use stack_node::{
    stack_head_delete, stack_node_add_link, stack_node_new, stack_node_release, stack_node_retain,
    StackNode,
};
mod pop;
use pop::{pop_all_action, pop_count_action, pop_error_action, stack_iter, summarize_stack_action};

// ===========================================================================
// Internal stack helpers used by the Rust parser.
// ===========================================================================

/// Create a new parse stack.
pub unsafe fn stack_new(subtree_pool: &mut SubtreePool) -> *mut Stack {
    let mut node_pool = Array::new();
    node_pool.reserve(MAX_NODE_POOL_SIZE);
    let base_node = stack_node_new(None, NULL_SUBTREE, 1, &mut node_pool);

    let self_ = malloc(core::mem::size_of::<Stack>()).cast::<Stack>();
    ptr::write(
        self_,
        Stack {
            heads: Array::new(),
            slices: Array::new(),
            iterators: Array::new(),
            node_pool,
            window: Array::new(),
            window_enabled: false,
            halted_version_count: 0,
            base_node,
            subtree_pool,
        },
    );
    let stack = ptr_mut(self_);

    stack.heads.reserve(4);
    stack.slices.reserve(4);
    stack.iterators.reserve(4);
    stack.window.reserve(32);

    stack_clear(stack);

    self_
}

/// Free the parse stack.
pub unsafe fn stack_delete(self_: &mut Stack) {
    self_.release_window();
    if !self_.slices.contents.is_null() {
        self_.slices.delete();
    }
    if !self_.iterators.contents.is_null() {
        self_.iterators.delete();
    }
    if !self_.window.contents.is_null() {
        self_.window.delete();
    }
    let subtree_pool = ptr_mut(self_.subtree_pool);
    stack_node_release(self_.base_node, &mut self_.node_pool, subtree_pool);
    let heads = &mut self_.heads;
    let node_pool = &mut self_.node_pool;
    for head in heads.as_mut_slice() {
        stack_head_delete(head, node_pool, subtree_pool);
    }
    heads.clear();
    if !node_pool.contents.is_null() {
        for node in node_pool.as_slice() {
            free(node.as_ptr().cast::<c_void>());
        }
        node_pool.delete();
    }
    heads.delete();
    free(ptr::from_mut(self_).cast::<c_void>());
}

/// Set the last external token for a version.
/// Push a subtree onto a version.
pub unsafe fn stack_push(
    stack: &mut Stack,
    version: StackVersion,
    subtree: Subtree,
    state: TSStateId,
) {
    if stack.window_enabled && version == 0 && !subtree.is_null() {
        debug_assert_eq!(stack.heads.size, 1);
        let head = stack.head(0);
        let mut entry = WindowEntry {
            state,
            position: head.position,
            error_cost: head.error_cost,
            node_count: head.node_count,
            dynamic_precedence: head.dynamic_precedence,
            subtree,
        };
        entry.error_cost += subtree.error_cost();
        entry.position = length_add(entry.position, subtree.total_size());
        entry.node_count += stack_node::stack_subtree_node_count(subtree);
        entry.dynamic_precedence += subtree.dynamic_precedence();
        stack.window.push(entry);

        let head = stack.head_mut(0);
        head.state = state;
        head.position = entry.position;
        head.error_cost = entry.error_cost;
        head.node_count = entry.node_count;
        head.dynamic_precedence = entry.dynamic_precedence;
        return;
    }

    stack.materialize_window();
    let heads = &mut stack.heads;
    let node_pool = &mut stack.node_pool;
    let head = heads.get_unchecked_mut(version);
    let new_node = stack_node_new(Some(head.node), subtree, state, node_pool);
    if subtree.is_null() {
        head.node_count_at_last_error = new_node.as_ref().node_count;
    }
    head.node = new_node;
    head.state = new_node.as_ref().state;
    head.position = new_node.as_ref().position;
    head.error_cost = new_node.as_ref().error_cost;
    head.node_count = new_node.as_ref().node_count;
    head.dynamic_precedence = new_node.as_ref().dynamic_precedence;
}

/// Move a deterministic suffix into a reduction child array.
///
/// Returns `None` when the reduction straddles the materialized base. Extras
/// are included in the moved suffix but do not contribute to `count`, exactly
/// matching graph-stack pop semantics.
pub unsafe fn stack_pop_count_from_window(
    stack: &mut Stack,
    version: StackVersion,
    count: u32,
) -> Option<SubtreeArray> {
    if !stack.window_enabled || version != 0 || stack.heads.size != 1 {
        return None;
    }

    let mut start = stack.window.size;
    let mut remaining = count;
    while remaining > 0 {
        if start == 0 {
            return None;
        }
        start -= 1;
        let subtree = stack.window.get_unchecked(start).subtree;
        if subtree.is_null() || !subtree.extra() {
            remaining -= 1;
        }
    }

    let physical_count = stack.window.size - start;
    let mut children = SubtreeArray::new();
    let capacity = subtree_alloc_size(physical_count) / core::mem::size_of::<Subtree>();
    children.reserve(u32::try_from(capacity).unwrap());
    for i in start..stack.window.size {
        children.push(stack.window.get_unchecked(i).subtree);
    }
    stack.window.size = start;

    if let Some(&entry) = stack.window.as_slice().last() {
        let head = stack.head_mut(0);
        head.state = entry.state;
        head.position = entry.position;
        head.error_cost = entry.error_cost;
        head.node_count = entry.node_count;
        head.dynamic_precedence = entry.dynamic_precedence;
    } else {
        stack.sync_head_from_node(0);
    }
    Some(children)
}

/// Materialize the deterministic suffix and remain in ordinary GLR mode.
pub unsafe fn stack_materialize(stack: &mut Stack) {
    stack.materialize_window();
}

/// Re-enter deterministic mode after condensation leaves one active version.
pub unsafe fn stack_try_enable_window(stack: &mut Stack) {
    if stack.window_enabled {
        return;
    }
    if stack.heads.size == 1 && stack.head(0).is_active() && stack.head(0).state != ERROR_STATE {
        debug_assert!(stack.window.is_empty());
        stack.sync_head_from_node(0);
        stack.window_enabled = true;
    }
}

/// Pop a given number of entries from a version.
pub unsafe fn stack_pop_count(
    self_: &mut Stack,
    version: StackVersion,
    count: u32,
) -> StackSliceArray {
    self_.materialize_window();
    stack_iter(
        self_,
        version,
        |iterator| pop_count_action(iterator, count),
        Some(count),
    )
}

/// Pop an error from the top of a version.
pub unsafe fn stack_pop_error(self_: &mut Stack, version: StackVersion) -> SubtreeArray {
    self_.materialize_window();
    let node = self_.head(version).node;
    for link in &node.as_ref().links[..node.as_ref().link_count as usize] {
        let subtree = link.subtree;
        if !subtree.is_null() && subtree.is_error() {
            let mut found_error = false;
            let pop = stack_iter(
                self_,
                version,
                |iterator| pop_error_action(iterator, &mut found_error),
                Some(1),
            );
            if pop.size > 0 {
                debug_assert_eq!(pop.size, 1);
                let first_pop = pop.get_unchecked(0);
                stack_renumber_version(self_, first_pop.version, version);
                return ptr::read(&first_pop.subtrees);
            }
            break;
        }
    }
    Array::new()
}

/// Pop all entries from a version.
pub unsafe fn stack_pop_all(self_: &mut Stack, version: StackVersion) -> StackSliceArray {
    self_.materialize_window();
    stack_iter(self_, version, |iterator| pop_all_action(iterator), Some(0))
}

/// Record a summary of parse states near the top of a version.
pub unsafe fn stack_record_summary(self_: &mut Stack, version: StackVersion, max_depth: u32) {
    self_.materialize_window();
    let mut summary = Array::new();
    stack_iter(
        self_,
        version,
        |iterator| summarize_stack_action(iterator, &mut summary, max_depth),
        None,
    );
    let summary_ptr =
        NonNull::new_unchecked(malloc(core::mem::size_of::<StackSummary>()).cast::<StackSummary>());
    ptr::write(summary_ptr.as_ptr(), summary);
    let head = self_.head_mut(version);
    if let Some(mut previous) = head.summary.replace(summary_ptr) {
        previous.as_mut().delete();
        free(previous.as_ptr().cast::<c_void>());
    }
}

/// Get the recorded summary for a version.
pub unsafe fn stack_get_summary(stack: &Stack, version: StackVersion) -> Option<&StackSummary> {
    stack.head(version).summary.map(|summary| summary.as_ref())
}

/// Remove a version from the stack.
pub unsafe fn stack_remove_version(self_: &mut Stack, version: StackVersion) {
    self_.materialize_window();
    let heads = &mut self_.heads;
    let node_pool = &mut self_.node_pool;
    let subtree_pool = ptr_mut(self_.subtree_pool);
    if heads.get_unchecked(version).status == StackStatus::Halted {
        self_.halted_version_count -= 1;
    }
    stack_head_delete(heads.get_unchecked_mut(version), node_pool, subtree_pool);
    heads.erase(version);
}

/// Renumber version v1 to v2 (move v1 into v2's slot, removing v2).
pub unsafe fn stack_renumber_version(stack: &mut Stack, v1: StackVersion, v2: StackVersion) {
    if v1 == v2 {
        return;
    }
    stack.materialize_window();
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
    heads.erase(v1);
}

/// Swap two versions.
pub unsafe fn stack_swap_versions(stack: &mut Stack, v1: StackVersion, v2: StackVersion) {
    stack.materialize_window();
    let temp = ptr::read(stack.heads.get_unchecked(v1));
    let other = ptr::read(stack.heads.get_unchecked(v2));
    ptr::write(stack.heads.get_unchecked_mut(v1), other);
    ptr::write(stack.heads.get_unchecked_mut(v2), temp);
}

/// Copy a version, creating a new one.
pub unsafe fn stack_copy_version(stack: &mut Stack, version: StackVersion) -> StackVersion {
    stack.materialize_window();
    debug_assert!(version < stack.heads.size);
    let version_head = ptr::read(stack.heads.get_unchecked(version));
    stack.heads.push(version_head);
    let head = stack.heads.last_unchecked_mut();
    stack_node_retain(head.node);
    if !head.last_external_token.is_null() {
        head.last_external_token.retain();
    }
    if head.status == StackStatus::Halted {
        stack.halted_version_count += 1;
    }
    head.summary = None;
    stack.heads.size - 1
}

/// Merge two versions that have equivalent current parser configurations.
///
/// The current nodes are alternatives for the same future, so this does not
/// link one current node to the other. It copies `version2`'s predecessor links
/// into `version1`'s current node, then removes `version2`. A later graph pop
/// follows every retained link and reconstructs each distinct past.
pub unsafe fn stack_merge(
    stack: &mut Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    stack.materialize_window();
    if !stack_can_merge(stack, version1, version2) {
        return false;
    }
    {
        let stack_heads = &mut stack.heads;
        let subtree_pool = ptr_mut(stack.subtree_pool);
        let (head1, head2) = stack_head_array_pair_mut(stack_heads, version1, version2);
        let head2_node = head2.node.as_ref();
        for i in 0..head2_node.link_count as usize {
            stack_node_add_link(head1.node.as_mut(), head2_node.links[i], subtree_pool);
        }
        let head1_node = head1.node.as_ref();
        if head1_node.state == ERROR_STATE {
            head1.node_count_at_last_error = head1_node.node_count;
        }
    }
    stack_remove_version(stack, version2);
    true
}

/// Check whether two active heads can share one current graph node.
///
/// Equal state and byte position give them the same parse-table future. Equal
/// error cost and external-scanner state ensure that sharing does not erase
/// information used to rank versions or lex their next token.
pub unsafe fn stack_can_merge(
    stack: &Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    let head1 = stack.head(version1);
    let head2 = stack.head(version2);
    head1.status == StackStatus::Active
        && head2.status == StackStatus::Active
        && head1.state == head2.state
        && head1.position.bytes == head2.position.bytes
        && head1.error_cost == head2.error_cost
        && head1
            .last_external_token
            .has_same_external_scanner_state(head2.last_external_token)
}

/// Clear all versions, resetting to initial state.
pub unsafe fn stack_clear(self_: &mut Stack) {
    self_.release_window();
    stack_node_retain(self_.base_node);
    let heads = &mut self_.heads;
    let node_pool = &mut self_.node_pool;
    let subtree_pool = ptr_mut(self_.subtree_pool);
    for head in heads.as_mut_slice() {
        stack_head_delete(head, node_pool, subtree_pool);
    }
    heads.clear();
    self_.halted_version_count = 0;
    heads.push(StackHead {
        node: self_.base_node,
        state: self_.base_node.as_ref().state,
        position: self_.base_node.as_ref().position,
        error_cost: self_.base_node.as_ref().error_cost,
        node_count: self_.base_node.as_ref().node_count,
        dynamic_precedence: self_.base_node.as_ref().dynamic_precedence,
        status: StackStatus::Active,
        last_external_token: NULL_SUBTREE,
        lookahead_when_paused: NULL_SUBTREE,
        summary: None,
        node_count_at_last_error: 0,
    });
    self_.window_enabled = true;
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

            assert_eq!(stack.halted_version_count(), 0);

            let halted = stack_copy_version(stack, 0);
            stack.halt(halted);
            assert_eq!(stack.halted_version_count(), 1);

            let halted_copy = stack_copy_version(stack, halted);
            assert_eq!(stack.halted_version_count(), 2);

            stack.pause(halted_copy, NULL_SUBTREE);
            assert_eq!(stack.halted_version_count(), 1);
            let _ = stack.resume(halted_copy);
            assert_eq!(stack.halted_version_count(), 1);

            stack.halt(halted_copy);
            assert_eq!(stack.halted_version_count(), 2);
            stack_remove_version(stack, halted_copy);
            assert_eq!(stack.halted_version_count(), 1);

            stack_clear(stack);
            assert_eq!(stack.halted_version_count(), 0);

            stack_delete(stack);
            subtree_pool_delete(&mut pool);
        }
    }
}
