#![allow(dead_code)]

// UTF-8 and UTF-16 decoding support.
// Replaces the ICU unicode/*.h headers used by the C library.
//
// The C library uses ICU macros (U8_NEXT, U16_NEXT, etc.) for decoding.
// This module provides equivalent Rust functions.

/// Decode one UTF-8 code point from `string[..length]`.
/// Returns (bytes_consumed, code_point). On error, code_point is -1.
#[inline]
pub fn utf8_next(string: &[u8], offset: usize) -> (u32, i32) {
    if offset >= string.len() {
        return (0, -1);
    }

    let b0 = string[offset];

    // Single byte (ASCII)
    if b0 < 0x80 {
        return (1, i32::from(b0));
    }

    // Determine expected length from lead byte
    let (expected_len, mut code_point) = if b0 < 0xC2 {
        // Invalid lead byte (continuation byte or overlong 2-byte)
        return (1, -1);
    } else if b0 < 0xE0 {
        (2, i32::from(b0 & 0x1F))
    } else if b0 < 0xF0 {
        (3, i32::from(b0 & 0x0F))
    } else if b0 < 0xF5 {
        (4, i32::from(b0 & 0x07))
    } else {
        return (1, -1);
    };

    let remaining = string.len() - offset;
    if remaining < expected_len {
        return (1, -1);
    }

    for i in 1..expected_len {
        let b = string[offset + i];
        if (b & 0xC0) != 0x80 {
            return (1, -1);
        }
        code_point = (code_point << 6) | i32::from(b & 0x3F);
    }

    // Check for overlong encodings and surrogates
    let valid = match expected_len {
        2 => code_point >= 0x80,
        3 => code_point >= 0x800 && !(0xD800..=0xDFFF).contains(&code_point),
        4 => (0x10000..=0x10FFFF).contains(&code_point),
        _ => false,
    };

    if valid {
        (expected_len as u32, code_point)
    } else {
        (1, -1)
    }
}

/// Decode one UTF-16LE code point from `string[..length]`.
/// Returns (bytes_consumed, code_point). On error, code_point is -1.
#[inline]
pub fn utf16le_next(string: &[u8], offset: usize) -> (u32, i32) {
    if offset + 1 >= string.len() {
        return (0, -1);
    }

    let c = u16::from_le_bytes([string[offset], string[offset + 1]]);

    // BMP character (not surrogate)
    if !(0xD800..=0xDFFF).contains(&c) {
        return (2, i32::from(c));
    }

    // Lead surrogate
    if (0xD800..=0xDBFF).contains(&c) {
        if offset + 3 >= string.len() {
            return (2, -1);
        }
        let c2 = u16::from_le_bytes([string[offset + 2], string[offset + 3]]);
        if (0xDC00..=0xDFFF).contains(&c2) {
            let code_point =
                0x10000 + ((i32::from(c) - 0xD800) << 10) + (i32::from(c2) - 0xDC00);
            return (4, code_point);
        }
    }

    (2, -1)
}

/// Decode one UTF-16BE code point from `string[..length]`.
/// Returns (bytes_consumed, code_point). On error, code_point is -1.
#[inline]
pub fn utf16be_next(string: &[u8], offset: usize) -> (u32, i32) {
    if offset + 1 >= string.len() {
        return (0, -1);
    }

    let c = u16::from_be_bytes([string[offset], string[offset + 1]]);

    if !(0xD800..=0xDFFF).contains(&c) {
        return (2, i32::from(c));
    }

    if (0xD800..=0xDBFF).contains(&c) {
        if offset + 3 >= string.len() {
            return (2, -1);
        }
        let c2 = u16::from_be_bytes([string[offset + 2], string[offset + 3]]);
        if (0xDC00..=0xDFFF).contains(&c2) {
            let code_point =
                0x10000 + ((i32::from(c) - 0xD800) << 10) + (i32::from(c2) - 0xDC00);
            return (4, code_point);
        }
    }

    (2, -1)
}

// C-compatible decode functions matching TSDecodeFunction signature:
// uint32_t (*)(const uint8_t *string, uint32_t length, int32_t *code_point)

pub unsafe extern "C" fn ts_decode_utf8(
    string: *const u8,
    length: u32,
    code_point: *mut i32,
) -> u32 {
    if string.is_null() || length == 0 {
        unsafe {
            *code_point = -1;
        }
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts(string, length as usize) };
    let (consumed, cp) = utf8_next(slice, 0);
    unsafe {
        *code_point = cp;
    }
    consumed
}

pub unsafe extern "C" fn ts_decode_utf16le(
    string: *const u8,
    length: u32,
    code_point: *mut i32,
) -> u32 {
    if string.is_null() || length < 2 {
        unsafe {
            *code_point = -1;
        }
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts(string, length as usize) };
    let (consumed, cp) = utf16le_next(slice, 0);
    unsafe {
        *code_point = cp;
    }
    consumed
}

pub unsafe extern "C" fn ts_decode_utf16be(
    string: *const u8,
    length: u32,
    code_point: *mut i32,
) -> u32 {
    if string.is_null() || length < 2 {
        unsafe {
            *code_point = -1;
        }
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts(string, length as usize) };
    let (consumed, cp) = utf16be_next(slice, 0);
    unsafe {
        *code_point = cp;
    }
    consumed
}
