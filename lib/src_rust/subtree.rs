#![allow(dead_code)]

// Stub for subtree.c/h — Central node representation.
// This is the most complex module: inline/heap subtree union,
// bitfields, atomic ref counting, memory pooling.
//
// Will be implemented in Tier 1 of the rewrite.

/// Placeholder for the Subtree type.
/// In the C code, Subtree is a union of an inline representation (for small
/// leaf nodes) and a pointer to a heap-allocated SubtreeHeapData.
pub struct Subtree {
    _placeholder: u64,
}

/// Placeholder for the mutable subtree handle.
pub struct MutableSubtree {
    _placeholder: u64,
}

/// Placeholder for the subtree pool used to recycle allocations.
pub struct SubtreePool {
    _placeholder: u8,
}
