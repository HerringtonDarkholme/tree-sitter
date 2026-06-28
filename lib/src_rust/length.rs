use crate::ffi::TSPoint;

use super::point::{point_add, point_sub};

/// Combined byte and point distance.
///
/// Tree-sitter tracks both absolute byte offsets and `(row, column)` points.
/// A `Length` represents a span in both coordinate systems so callers can
/// update source positions without rescanning text.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Length {
    /// Byte distance.
    pub bytes: u32,
    /// Row/column distance.
    pub extent: TSPoint,
}

impl PartialEq for Length {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
            && self.extent.row == other.extent.row
            && self.extent.column == other.extent.column
    }
}

impl Eq for Length {}

pub const LENGTH_UNDEFINED: Length = Length {
    bytes: 0,
    extent: TSPoint { row: 0, column: 1 },
};

/// Sentinel larger than any concrete source length.
pub const LENGTH_MAX: Length = Length {
    bytes: u32::MAX,
    extent: TSPoint {
        row: u32::MAX,
        column: u32::MAX,
    },
};

#[inline]
pub const fn length_is_undefined(length: Length) -> bool {
    length.bytes == 0 && length.extent.column != 0
}

#[inline]
pub const fn length_min(len1: Length, len2: Length) -> Length {
    if len1.bytes < len2.bytes {
        len1
    } else {
        len2
    }
}

#[inline]
pub const fn length_add(len1: Length, len2: Length) -> Length {
    Length {
        bytes: len1.bytes + len2.bytes,
        extent: point_add(len1.extent, len2.extent),
    }
}

#[inline]
pub const fn length_sub(len1: Length, len2: Length) -> Length {
    Length {
        bytes: len1.bytes.saturating_sub(len2.bytes),
        extent: point_sub(len1.extent, len2.extent),
    }
}

#[inline]
pub const fn length_zero() -> Length {
    Length {
        bytes: 0,
        extent: TSPoint { row: 0, column: 0 },
    }
}

#[inline]
pub const fn length_saturating_sub(len1: Length, len2: Length) -> Length {
    if len1.bytes > len2.bytes {
        length_sub(len1, len2)
    } else {
        length_zero()
    }
}
