#![allow(dead_code)]

use crate::ffi::{TSInputEdit, TSPoint};

pub const POINT_ZERO: TSPoint = TSPoint { row: 0, column: 0 };
pub const POINT_MAX: TSPoint = TSPoint {
    row: u32::MAX,
    column: u32::MAX,
};

#[inline]
pub const fn point_new(row: u32, column: u32) -> TSPoint {
    TSPoint { row, column }
}

#[inline]
pub const fn point_add(a: TSPoint, b: TSPoint) -> TSPoint {
    if b.row > 0 {
        point_new(a.row + b.row, b.column)
    } else {
        point_new(a.row, a.column + b.column)
    }
}

#[inline]
pub const fn point_sub(a: TSPoint, b: TSPoint) -> TSPoint {
    if a.row > b.row {
        point_new(a.row - b.row, a.column)
    } else {
        point_new(0, a.column.saturating_sub(b.column))
    }
}

#[inline]
pub const fn point_lte(a: TSPoint, b: TSPoint) -> bool {
    (a.row < b.row) || (a.row == b.row && a.column <= b.column)
}

#[inline]
pub const fn point_lt(a: TSPoint, b: TSPoint) -> bool {
    (a.row < b.row) || (a.row == b.row && a.column < b.column)
}

#[inline]
pub const fn point_gt(a: TSPoint, b: TSPoint) -> bool {
    (a.row > b.row) || (a.row == b.row && a.column > b.column)
}

#[inline]
pub const fn point_gte(a: TSPoint, b: TSPoint) -> bool {
    (a.row > b.row) || (a.row == b.row && a.column >= b.column)
}

#[inline]
pub const fn point_eq(a: TSPoint, b: TSPoint) -> bool {
    a.row == b.row && a.column == b.column
}

fn ts_point_edit_ref(point: &mut TSPoint, byte: &mut u32, edit: &TSInputEdit) {
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
    let point = &mut *point;
    let byte = &mut *byte;
    let edit = &*edit;

    ts_point_edit_ref(point, byte, edit);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit() -> TSInputEdit {
        TSInputEdit {
            start_byte: 5,
            old_end_byte: 10,
            new_end_byte: 12,
            start_point: point_new(0, 5),
            old_end_point: point_new(0, 10),
            new_end_point: point_new(1, 2),
        }
    }

    fn assert_point_eq(actual: TSPoint, expected: TSPoint) {
        assert_eq!(actual.row, expected.row);
        assert_eq!(actual.column, expected.column);
    }

    #[test]
    fn edit_point_after_changed_range() {
        let mut point = point_new(0, 14);
        let mut byte = 14;

        ts_point_edit_ref(&mut point, &mut byte, &edit());

        assert_eq!(byte, 16);
        assert_point_eq(point, point_new(1, 6));
    }

    #[test]
    fn edit_point_inside_changed_range() {
        let mut point = point_new(0, 7);
        let mut byte = 7;

        ts_point_edit_ref(&mut point, &mut byte, &edit());

        assert_eq!(byte, 12);
        assert_point_eq(point, point_new(1, 2));
    }

    #[test]
    fn edit_point_before_changed_range() {
        let mut point = point_new(0, 4);
        let mut byte = 4;

        ts_point_edit_ref(&mut point, &mut byte, &edit());

        assert_eq!(byte, 4);
        assert_point_eq(point, point_new(0, 4));
    }
}
