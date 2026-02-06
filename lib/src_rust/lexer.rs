#![allow(dead_code, unused_variables, unused_imports, non_upper_case_globals, non_snake_case)]

//! Rust replacement for lexer.c/h — Input buffering and character decoding.
//!
//! This module implements the `Lexer` struct which wraps `TSLexer` and provides:
//! - Character-by-character reading from an input source (`TSInput`)
//! - Unicode decoding (UTF-8, UTF-16LE, UTF-16BE, custom)
//! - Included range tracking (for injected languages)
//! - Column computation and logging
//!
//! The `TSLexer` vtable (advance, mark_end, get_column, etc.) is populated
//! with function pointers to static functions in this module, so generated
//! parsers can call them without linking against this library.

use std::ffi::c_void;
use std::ptr;

use crate::ffi::{
    TSInput, TSInputEncoding, TSInputEncodingCustom, TSInputEncodingUTF16BE,
    TSInputEncodingUTF16LE, TSInputEncodingUTF8, TSLogger, TSLogType, TSLogTypeLex, TSPoint,
    TSRange, TSSymbol,
};

use super::alloc::{ts_free, ts_realloc};
use super::language::TSLexer;
use super::length::{length_is_undefined, Length, LENGTH_UNDEFINED};
use super::unicode::{ts_decode_utf8, ts_decode_utf16_le, ts_decode_utf16_be, TS_DECODE_ERROR};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const TREE_SITTER_SERIALIZATION_BUFFER_SIZE: usize = 1024;

const BYTE_ORDER_MARK: i32 = 0xFEFF;

static DEFAULT_RANGE: TSRange = TSRange {
    start_point: TSPoint { row: 0, column: 0 },
    end_point: TSPoint {
        row: u32::MAX,
        column: u32::MAX,
    },
    start_byte: 0,
    end_byte: u32::MAX,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Column tracking state — tracks the column position, which requires
/// backtracking to the start of the line to count characters.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ColumnData {
    pub value: u32,
    pub valid: bool,
}

/// The main lexer state. Contains the `TSLexer` data (with vtable pointers)
/// plus all internal state needed for buffered reading and range tracking.
#[repr(C)]
pub struct Lexer {
    pub data: TSLexer,
    pub current_position: Length,
    pub token_start_position: Length,
    pub token_end_position: Length,

    pub included_ranges: *mut TSRange,
    pub chunk: *const i8,
    pub input: TSInput,
    pub logger: TSLogger,

    pub included_range_count: u32,
    pub current_included_range_index: u32,
    pub chunk_start: u32,
    pub chunk_size: u32,
    pub lookahead_size: u32,
    pub did_get_column: bool,
    pub column_data: ColumnData,

    pub debug_buffer: [u8; TREE_SITTER_SERIALIZATION_BUFFER_SIZE],
}

// ---------------------------------------------------------------------------
// Compile-time layout assertions
// ---------------------------------------------------------------------------

const _: () = assert!(std::mem::size_of::<ColumnData>() == 8);

// ---------------------------------------------------------------------------
// Extern C declarations
// ---------------------------------------------------------------------------

extern "C" {
    // libc
    fn snprintf(s: *mut i8, n: usize, format: *const i8, ...) -> i32;
    fn vsnprintf(s: *mut i8, n: usize, format: *const i8, args: *mut c_void) -> i32;
    fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
}

// ---------------------------------------------------------------------------
// Internal (static) functions
// ---------------------------------------------------------------------------

/// Sets the column data to the given value and marks it valid.
fn ts_lexer__set_column_data(self_: &mut Lexer, val: u32) {
    self_.column_data.valid = true;
    self_.column_data.value = val;
}

/// Increments the value of the column data; no-op if invalid.
fn ts_lexer__increment_column_data(self_: &mut Lexer) {
    if self_.column_data.valid {
        self_.column_data.value += 1;
    }
}

/// Marks the column data as invalid.
fn ts_lexer__invalidate_column_data(self_: &mut Lexer) {
    self_.column_data.valid = false;
    self_.column_data.value = 0;
}

/// Check if the lexer has reached EOF.
unsafe extern "C" fn ts_lexer__eof(_self: *const TSLexer) -> bool {
    let self_ = _self as *const Lexer;
    (*self_).current_included_range_index == (*self_).included_range_count
}

/// Clear the currently stored chunk of source code.
fn ts_lexer__clear_chunk(self_: &mut Lexer) {
    self_.chunk = ptr::null();
    self_.chunk_size = 0;
    self_.chunk_start = 0;
}

/// Call the input callback to obtain a new chunk of source code.
unsafe fn ts_lexer__get_chunk(self_: &mut Lexer) {
    self_.chunk_start = self_.current_position.bytes;
    self_.chunk = (self_.input.read.unwrap_unchecked())(
        self_.input.payload,
        self_.current_position.bytes,
        self_.current_position.extent,
        &mut self_.chunk_size,
    );
    if self_.chunk_size == 0 {
        self_.current_included_range_index = self_.included_range_count;
        self_.chunk = ptr::null();
    }
}

/// Decode the next unicode character in the current chunk.
unsafe fn ts_lexer__get_lookahead(self_: &mut Lexer) {
    let position_in_chunk = self_.current_position.bytes - self_.chunk_start;
    let mut size = self_.chunk_size - position_in_chunk;

    if size == 0 {
        self_.lookahead_size = 1;
        self_.data.lookahead = 0;
        return;
    }

    let mut chunk = (self_.chunk as *const u8).add(position_in_chunk as usize);
    let decode: unsafe extern "C" fn(*const u8, u32, *mut i32) -> u32 =
        if self_.input.encoding == TSInputEncodingUTF8 {
            ts_decode_utf8
        } else if self_.input.encoding == TSInputEncodingUTF16LE {
            ts_decode_utf16_le
        } else if self_.input.encoding == TSInputEncodingUTF16BE {
            ts_decode_utf16_be
        } else {
            self_.input.decode.unwrap_unchecked()
        };

    self_.lookahead_size = decode(chunk, size, &mut self_.data.lookahead);

    // If this chunk ended in the middle of a multi-byte character,
    // try again with a fresh chunk.
    if self_.data.lookahead == TS_DECODE_ERROR && size < 4 {
        ts_lexer__get_chunk(self_);
        chunk = self_.chunk as *const u8;
        size = self_.chunk_size;
        self_.lookahead_size = decode(chunk, size, &mut self_.data.lookahead);
    }

    if self_.data.lookahead == TS_DECODE_ERROR {
        self_.lookahead_size = 1;
    }
}

/// Move the lexer to a given position, finding the right included range.
unsafe fn ts_lexer_goto(self_: &mut Lexer, position: Length) {
    if position.bytes != self_.current_position.bytes {
        ts_lexer__invalidate_column_data(self_);
    }

    self_.current_position = position;

    // Move to the first valid position at or after the given position.
    let mut found_included_range = false;
    for i in 0..self_.included_range_count {
        let included_range = &*self_.included_ranges.add(i as usize);
        if included_range.end_byte > self_.current_position.bytes
            && included_range.end_byte > included_range.start_byte
        {
            if included_range.start_byte >= self_.current_position.bytes {
                self_.current_position = Length {
                    bytes: included_range.start_byte,
                    extent: included_range.start_point,
                };
            }

            self_.current_included_range_index = i;
            found_included_range = true;
            break;
        }
    }

    if found_included_range {
        // If the current position is outside of the current chunk of text,
        // then clear out the current chunk of text.
        if !self_.chunk.is_null()
            && (self_.current_position.bytes < self_.chunk_start
                || self_.current_position.bytes >= self_.chunk_start + self_.chunk_size)
        {
            ts_lexer__clear_chunk(self_);
        }

        self_.lookahead_size = 0;
        self_.data.lookahead = 0;
    } else {
        // If the given position is beyond any of included ranges, move to the EOF
        // state - past the end of the included ranges.
        self_.current_included_range_index = self_.included_range_count;
        let last_included_range =
            &*self_.included_ranges.add(self_.included_range_count as usize - 1);
        self_.current_position = Length {
            bytes: last_included_range.end_byte,
            extent: last_included_range.end_point,
        };
        ts_lexer__clear_chunk(self_);
        self_.lookahead_size = 1;
        self_.data.lookahead = 0;
    }
}

/// Actually advances the lexer. Does not log anything.
unsafe fn ts_lexer__do_advance(self_: &mut Lexer, skip: bool) {
    if self_.lookahead_size != 0 {
        if self_.data.lookahead == '\n' as i32 {
            self_.current_position.extent.row += 1;
            self_.current_position.extent.column = 0;
            ts_lexer__set_column_data(self_, 0);
        } else {
            let is_bom =
                self_.current_position.bytes == 0 && self_.data.lookahead == BYTE_ORDER_MARK;
            if !is_bom {
                ts_lexer__increment_column_data(self_);
            }
            self_.current_position.extent.column += self_.lookahead_size;
        }
        self_.current_position.bytes += self_.lookahead_size;
    }

    let mut current_range_ptr =
        self_.included_ranges.add(self_.current_included_range_index as usize);
    loop {
        let current_range = &*current_range_ptr;
        if self_.current_position.bytes < current_range.end_byte
            && current_range.end_byte != current_range.start_byte
        {
            break;
        }
        if self_.current_included_range_index < self_.included_range_count {
            self_.current_included_range_index += 1;
        }
        if self_.current_included_range_index < self_.included_range_count {
            current_range_ptr = current_range_ptr.add(1);
            let next_range = &*current_range_ptr;
            self_.current_position = Length {
                bytes: next_range.start_byte,
                extent: next_range.start_point,
            };
        } else {
            current_range_ptr = ptr::null_mut();
            break;
        }
    }

    if skip {
        self_.token_start_position = self_.current_position;
    }

    if !current_range_ptr.is_null() {
        if self_.current_position.bytes < self_.chunk_start
            || self_.current_position.bytes >= self_.chunk_start + self_.chunk_size
        {
            ts_lexer__get_chunk(self_);
        }
        ts_lexer__get_lookahead(self_);
    } else {
        ts_lexer__clear_chunk(self_);
        self_.data.lookahead = 0;
        self_.lookahead_size = 1;
    }
}

/// Advance to the next character (with logging). TSLexer vtable callback.
unsafe extern "C" fn ts_lexer__advance(_self: *mut TSLexer, skip: bool) {
    let self_ = _self as *mut Lexer;
    if (*self_).chunk.is_null() {
        return;
    }

    if (*self_).logger.log.is_some() {
        let character = (*self_).data.lookahead;
        if skip {
            if 32 <= character && character < 127 {
                ts_lexer__log_shim(
                    _self,
                    b"skip character:'%c'\0".as_ptr() as *const i8,
                    character,
                );
            } else {
                ts_lexer__log_shim(
                    _self,
                    b"skip character:%d\0".as_ptr() as *const i8,
                    character,
                );
            }
        } else if 32 <= character && character < 127 {
            ts_lexer__log_shim(
                _self,
                b"consume character:'%c'\0".as_ptr() as *const i8,
                character,
            );
        } else {
            ts_lexer__log_shim(
                _self,
                b"consume character:%d\0".as_ptr() as *const i8,
                character,
            );
        }
    }

    ts_lexer__do_advance(&mut *self_, skip);
}

/// Mark that a token match has completed. TSLexer vtable callback.
unsafe extern "C" fn ts_lexer__mark_end(_self: *mut TSLexer) {
    let self_ = _self as *mut Lexer;
    if !ts_lexer__eof(&(*self_).data) {
        // If the lexer is right at the beginning of included range,
        // then the token should be considered to end at the *end* of the
        // previous included range, rather than here.
        let current_included_range = &*(*self_)
            .included_ranges
            .add((*self_).current_included_range_index as usize);
        if (*self_).current_included_range_index > 0
            && (*self_).current_position.bytes == current_included_range.start_byte
        {
            let previous_included_range = &*(*self_)
                .included_ranges
                .add((*self_).current_included_range_index as usize - 1);
            (*self_).token_end_position = Length {
                bytes: previous_included_range.end_byte,
                extent: previous_included_range.end_point,
            };
            return;
        }
    }
    (*self_).token_end_position = (*self_).current_position;
}

/// Get the current column number. TSLexer vtable callback.
unsafe extern "C" fn ts_lexer__get_column(_self: *mut TSLexer) -> u32 {
    let self_ = &mut *(_self as *mut Lexer);

    self_.did_get_column = true;

    if !self_.column_data.valid {
        // Record current position
        let goal_byte = self_.current_position.bytes;

        // Back up to the beginning of the line
        let start_of_col = Length {
            bytes: self_.current_position.bytes - self_.current_position.extent.column,
            extent: TSPoint {
                row: self_.current_position.extent.row,
                column: 0,
            },
        };
        ts_lexer_goto(self_, start_of_col);
        ts_lexer__set_column_data(self_, 0);
        ts_lexer__get_chunk(self_);

        if !ts_lexer__eof(_self) {
            ts_lexer__get_lookahead(self_);

            // Advance to the recorded position
            while self_.current_position.bytes < goal_byte
                && !ts_lexer__eof(_self)
                && !self_.chunk.is_null()
            {
                ts_lexer__do_advance(self_, false);
                if ts_lexer__eof(_self) {
                    break;
                }
            }
        }
    }

    self_.column_data.value
}

/// Is the lexer at a boundary between two disjoint included ranges?
/// TSLexer vtable callback.
unsafe extern "C" fn ts_lexer__is_at_included_range_start(_self: *const TSLexer) -> bool {
    let self_ = _self as *const Lexer;
    if (*self_).current_included_range_index < (*self_).included_range_count {
        let current_range = &*(*self_)
            .included_ranges
            .add((*self_).current_included_range_index as usize);
        (*self_).current_position.bytes == current_range.start_byte
    } else {
        false
    }
}

// The variadic log function is defined in lexer_log_shim.c because
// Rust stable cannot define C-variadic functions. It's imported here
// and assigned to TSLexer::log in ts_lexer_init.
extern "C" {
    fn ts_lexer__log_shim(_self: *const TSLexer, fmt: *const i8, ...);
}

// ===========================================================================
// Exported functions from lexer.h (called by parser.c)
// ===========================================================================

/// Initialize a Lexer, setting up the TSLexer vtable and default state.
#[no_mangle]
pub unsafe extern "C" fn ts_lexer_init(self_: *mut Lexer) {
    let s = &mut *self_;
    s.data.advance = Some(ts_lexer__advance);
    s.data.mark_end = Some(ts_lexer__mark_end);
    s.data.get_column = Some(ts_lexer__get_column);
    s.data.is_at_included_range_start = Some(ts_lexer__is_at_included_range_start);
    s.data.eof = Some(ts_lexer__eof);
    s.data.log = Some(ts_lexer__log_shim);
    s.data.lookahead = 0;
    s.data.result_symbol = 0;
    s.chunk = ptr::null();
    s.chunk_size = 0;
    s.chunk_start = 0;
    s.current_position = Length {
        bytes: 0,
        extent: TSPoint { row: 0, column: 0 },
    };
    s.logger = TSLogger {
        payload: ptr::null_mut(),
        log: None,
    };
    s.included_ranges = ptr::null_mut();
    s.included_range_count = 0;
    s.current_included_range_index = 0;
    s.did_get_column = false;
    s.column_data = ColumnData {
        valid: false,
        value: 0,
    };
    ts_lexer_set_included_ranges(self_, ptr::null(), 0);
}

/// Free the lexer's included_ranges allocation.
#[no_mangle]
pub unsafe extern "C" fn ts_lexer_delete(self_: *mut Lexer) {
    ts_free((*self_).included_ranges as *mut c_void);
}

/// Set the input source for the lexer.
#[no_mangle]
pub unsafe extern "C" fn ts_lexer_set_input(self_: *mut Lexer, input: TSInput) {
    let s = &mut *self_;
    s.input = input;
    ts_lexer__clear_chunk(s);
    ts_lexer_goto(s, s.current_position);
}

/// Move the lexer to the given position (no-op if already there).
#[no_mangle]
pub unsafe extern "C" fn ts_lexer_reset(self_: *mut Lexer, position: Length) {
    let s = &mut *self_;
    if position.bytes != s.current_position.bytes {
        ts_lexer_goto(s, position);
    }
}

/// Prepare the lexer to start scanning a new token.
#[no_mangle]
pub unsafe extern "C" fn ts_lexer_start(self_: *mut Lexer) {
    let s = &mut *self_;
    s.token_start_position = s.current_position;
    s.token_end_position = LENGTH_UNDEFINED;
    s.data.result_symbol = 0;
    s.did_get_column = false;
    if !ts_lexer__eof(&s.data) {
        if s.chunk_size == 0 {
            ts_lexer__get_chunk(s);
        }
        if s.lookahead_size == 0 {
            ts_lexer__get_lookahead(s);
        }
        if s.current_position.bytes == 0 {
            if s.data.lookahead == BYTE_ORDER_MARK {
                ts_lexer__advance(&mut s.data, true);
            }
            ts_lexer__set_column_data(s, 0);
        }
    }
}

/// Finalize the current token scan.
#[no_mangle]
pub unsafe extern "C" fn ts_lexer_finish(
    self_: *mut Lexer,
    lookahead_end_byte: *mut u32,
) {
    let s = &mut *self_;
    if length_is_undefined(s.token_end_position) {
        ts_lexer__mark_end(&mut s.data);
    }

    // If the token ended at an included range boundary, then its end position
    // will have been reset to the end of the preceding range. Reset the start
    // position to match.
    if s.token_end_position.bytes < s.token_start_position.bytes {
        s.token_start_position = s.token_end_position;
    }

    let mut current_lookahead_end_byte = s.current_position.bytes + 1;

    // In order to determine that a byte sequence is invalid UTF8 or UTF16,
    // the character decoding algorithm may have looked at the following byte.
    if s.data.lookahead == TS_DECODE_ERROR {
        current_lookahead_end_byte += 4;
    }

    if current_lookahead_end_byte > *lookahead_end_byte {
        *lookahead_end_byte = current_lookahead_end_byte;
    }
}

/// Mark the end of the current token.
#[no_mangle]
pub unsafe extern "C" fn ts_lexer_mark_end(self_: *mut Lexer) {
    ts_lexer__mark_end(&mut (*self_).data);
}

/// Set the included ranges for the lexer. Returns false if ranges are invalid.
#[no_mangle]
pub unsafe extern "C" fn ts_lexer_set_included_ranges(
    self_: *mut Lexer,
    mut ranges: *const TSRange,
    mut count: u32,
) -> bool {
    let s = &mut *self_;
    if count == 0 || ranges.is_null() {
        ranges = &DEFAULT_RANGE;
        count = 1;
    } else {
        let mut previous_byte: u32 = 0;
        for i in 0..count {
            let range = &*ranges.add(i as usize);
            if range.start_byte < previous_byte || range.end_byte < range.start_byte {
                return false;
            }
            previous_byte = range.end_byte;
        }
    }

    let size = count as usize * std::mem::size_of::<TSRange>();
    s.included_ranges = ts_realloc(s.included_ranges as *mut c_void, size) as *mut TSRange;
    memcpy(
        s.included_ranges as *mut c_void,
        ranges as *const c_void,
        size,
    );
    s.included_range_count = count;
    ts_lexer_goto(s, s.current_position);
    true
}

/// Get the current included ranges.
#[no_mangle]
pub unsafe extern "C" fn ts_lexer_included_ranges(
    self_: *const Lexer,
    count: *mut u32,
) -> *mut TSRange {
    *count = (*self_).included_range_count;
    (*self_).included_ranges
}
