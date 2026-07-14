use crate::ffi::{TSInputEdit, TSRange};

use super::super::point::{point_add, point_sub, POINT_MAX};
use super::super::utils::array_push;
use super::{Length, TSRangeArray};

pub fn range_edit_ref(range: &mut TSRange, edit: &TSInputEdit) {
    if range.end_byte >= edit.old_end_byte {
        if range.end_byte != u32::MAX {
            range.end_byte = edit.new_end_byte + (range.end_byte - edit.old_end_byte);
            range.end_point = point_add(
                edit.new_end_point,
                point_sub(range.end_point, edit.old_end_point),
            );
            if range.end_byte < edit.new_end_byte {
                range.end_byte = u32::MAX;
                range.end_point = POINT_MAX;
            }
        }
    } else if range.end_byte > edit.start_byte {
        range.end_byte = edit.start_byte;
        range.end_point = edit.start_point;
    }

    if range.start_byte >= edit.old_end_byte {
        range.start_byte = edit.new_end_byte + (range.start_byte - edit.old_end_byte);
        range.start_point = point_add(
            edit.new_end_point,
            point_sub(range.start_point, edit.old_end_point),
        );
        if range.start_byte < edit.new_end_byte {
            range.start_byte = u32::MAX;
            range.start_point = POINT_MAX;
        }
    } else if range.start_byte > edit.start_byte {
        range.start_byte = edit.start_byte;
        range.start_point = edit.start_point;
    }
}

pub unsafe fn range_array_intersects_ref(
    ranges: &TSRangeArray,
    start_index: u32,
    start_byte: u32,
    end_byte: u32,
) -> bool {
    for range in ranges.as_slice().iter().skip(start_index as usize) {
        if range.end_byte > start_byte {
            if range.start_byte >= end_byte {
                break;
            }
            return true;
        }
    }
    false
}

pub(super) unsafe fn range_array_add(ranges: &mut TSRangeArray, start: Length, end: Length) {
    if let Some(last_range) = ranges.as_mut_slice().last_mut() {
        if start.bytes <= last_range.end_byte {
            last_range.end_byte = end.bytes;
            last_range.end_point = end.extent;
            return;
        }
    }

    if start.bytes < end.bytes {
        array_push(
            ranges,
            TSRange {
                start_point: start.extent,
                end_point: end.extent,
                start_byte: start.bytes,
                end_byte: end.bytes,
            },
        );
    }
}
