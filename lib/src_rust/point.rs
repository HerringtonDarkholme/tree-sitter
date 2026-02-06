#![allow(dead_code)]

use crate::ffi::{TSInputEdit, TSPoint};

pub const POINT_ZERO: TSPoint = TSPoint { row: 0, column: 0 };
pub const POINT_MAX: TSPoint = TSPoint {
    row: u32::MAX,
    column: u32::MAX,
};

#[inline]
pub fn point_new(row: u32, column: u32) -> TSPoint {
    TSPoint { row, column }
}

#[inline]
pub fn point_add(a: TSPoint, b: TSPoint) -> TSPoint {
    if b.row > 0 {
        point_new(a.row + b.row, b.column)
    } else {
        point_new(a.row, a.column + b.column)
    }
}

#[inline]
pub fn point_sub(a: TSPoint, b: TSPoint) -> TSPoint {
    if a.row > b.row {
        point_new(a.row - b.row, a.column)
    } else {
        point_new(
            0,
            if a.column >= b.column {
                a.column - b.column
            } else {
                0
            },
        )
    }
}

#[inline]
pub fn point_lte(a: TSPoint, b: TSPoint) -> bool {
    (a.row < b.row) || (a.row == b.row && a.column <= b.column)
}

#[inline]
pub fn point_lt(a: TSPoint, b: TSPoint) -> bool {
    (a.row < b.row) || (a.row == b.row && a.column < b.column)
}

#[inline]
pub fn point_gt(a: TSPoint, b: TSPoint) -> bool {
    (a.row > b.row) || (a.row == b.row && a.column > b.column)
}

#[inline]
pub fn point_gte(a: TSPoint, b: TSPoint) -> bool {
    (a.row > b.row) || (a.row == b.row && a.column >= b.column)
}

#[inline]
pub fn point_eq(a: TSPoint, b: TSPoint) -> bool {
    a.row == b.row && a.column == b.column
}

/// C-compatible `ts_point_edit` — exported for use by remaining C code (node.c).
///
/// # Safety
/// `point`, `byte`, and `edit` must be valid, non-null pointers.
#[no_mangle]
pub unsafe extern "C" fn ts_point_edit(
    point: *mut TSPoint,
    byte: *mut u32,
    edit: *const TSInputEdit,
) {
    let point = unsafe { &mut *point };
    let byte = unsafe { &mut *byte };
    let edit = unsafe { &*edit };

    let start_byte = *byte;
    let start_point = *point;

    if start_byte >= edit.old_end_byte {
        *byte = edit.new_end_byte + (start_byte - edit.old_end_byte);
        *point = point_add(
            edit.new_end_point,
            point_sub(start_point, edit.old_end_point),
        );
    } else if start_byte > edit.start_byte {
        *byte = edit.new_end_byte;
        *point = edit.new_end_point;
    }
}
