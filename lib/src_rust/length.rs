#![allow(dead_code)]

use crate::ffi::TSPoint;

use super::point::{point_add, point_sub};

#[derive(Clone, Copy, Debug)]
pub struct Length {
    pub bytes: u32,
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

pub const LENGTH_MAX: Length = Length {
    bytes: u32::MAX,
    extent: TSPoint {
        row: u32::MAX,
        column: u32::MAX,
    },
};

#[inline]
pub fn length_is_undefined(length: Length) -> bool {
    length.bytes == 0 && length.extent.column != 0
}

#[inline]
pub fn length_min(len1: Length, len2: Length) -> Length {
    if len1.bytes < len2.bytes {
        len1
    } else {
        len2
    }
}

#[inline]
pub fn length_add(len1: Length, len2: Length) -> Length {
    Length {
        bytes: len1.bytes + len2.bytes,
        extent: point_add(len1.extent, len2.extent),
    }
}

#[inline]
pub fn length_sub(len1: Length, len2: Length) -> Length {
    Length {
        bytes: if len1.bytes >= len2.bytes {
            len1.bytes - len2.bytes
        } else {
            0
        },
        extent: point_sub(len1.extent, len2.extent),
    }
}

#[inline]
pub fn length_zero() -> Length {
    Length {
        bytes: 0,
        extent: TSPoint { row: 0, column: 0 },
    }
}

#[inline]
pub fn length_saturating_sub(len1: Length, len2: Length) -> Length {
    if len1.bytes > len2.bytes {
        length_sub(len1, len2)
    } else {
        length_zero()
    }
}
