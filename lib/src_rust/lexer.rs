//! Rust replacement for lexer.c/h — Input buffering and character decoding.
//!
//! This module implements the `Lexer` struct which wraps `TSLexer` and provides:
//! - Character-by-character reading from an input source (`TSInput`)
//! - Unicode decoding (UTF-8, UTF-16LE, UTF-16BE, custom)
//! - Included range tracking (for injected languages)
//! - Column computation and logging
//!
//! The `TSLexer` vtable (`advance`, `mark_end`, `get_column`, etc.) is populated
//! with function pointers to static functions in this module, so generated
//! parsers can call them without linking against this library.

use core::ffi::{c_char, c_void};
use core::ptr;

use crate::ffi::{
    TSInput, TSInputEncodingUTF16BE, TSInputEncodingUTF16LE, TSInputEncodingUTF8, TSLogger,
    TSPoint, TSRange,
};

use super::alloc::{free, realloc};
use super::language::TSLexer;
use super::length::{length_is_undefined, Length, LENGTH_UNDEFINED};
use super::unicode::{ts_decode_utf16_be, ts_decode_utf16_le, ts_decode_utf8, TS_DECODE_ERROR};
use super::utils::{ptr_mut, ptr_ref};

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

/// Cached column tracking state.
///
/// `TSLexer::get_column` is an expensive callback because byte offsets and
/// columns differ for multi-byte encodings. When the cache is invalid, the
/// lexer rewinds to the start of the current line and advances back to the
/// original byte position to count columns. Normal `advance` calls keep this
/// cache valid as long as they move forward from that known position.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ColumnData {
    /// Last computed column value for `current_position`.
    pub value: u32,
    /// Whether `value` still corresponds to `current_position`.
    pub valid: bool,
}

/// The main lexer state. Contains the `TSLexer` data (with vtable pointers)
/// plus all internal state needed for buffered reading and range tracking.
#[repr(C)]
pub struct Lexer {
    /// Callback surface passed to generated lexers and external scanners.
    pub data: TSLexer,
    /// Current read cursor in bytes and row/column coordinates.
    pub current_position: Length,
    /// Start position of the token currently being scanned.
    pub token_start_position: Length,
    /// Last marked end position for the token currently being scanned.
    pub token_end_position: Length,

    /// Sorted ranges that should be visible to this parse.
    pub included_ranges: *mut TSRange,
    /// Borrowed chunk returned by `TSInput::read`; owned by the caller.
    pub chunk: *const c_char,
    /// Source reader and encoding callbacks.
    pub input: TSInput,
    /// Optional logging callback.
    pub logger: TSLogger,

    /// Number of included ranges. A single default range is the common case.
    pub included_range_count: u32,
    /// Included range containing, or immediately following, `current_position`.
    pub current_included_range_index: u32,
    /// Byte offset where `chunk` starts in the full source document.
    pub chunk_start: u32,
    /// Byte length of the current `chunk`.
    pub chunk_size: u32,
    /// Width in bytes of `data.lookahead`; zero means no lookahead is loaded.
    pub lookahead_size: u32,
    /// Whether the current token asked for column data.
    pub did_get_column: bool,
    /// Cached column value used by `TSLexer::get_column`.
    pub column_data: ColumnData,

    /// Scratch buffer shared with external scanner serialization and logging.
    pub debug_buffer: [u8; TREE_SITTER_SERIALIZATION_BUFFER_SIZE],
}

pub unsafe fn lexer_new() -> Lexer {
    let mut lexer = Lexer {
        data: TSLexer {
            lookahead: 0,
            result_symbol: 0,
            advance: Some(ts_lexer__advance),
            mark_end: Some(ts_lexer__mark_end),
            get_column: Some(ts_lexer__get_column),
            is_at_included_range_start: Some(ts_lexer__is_at_included_range_start),
            eof: Some(ts_lexer__eof),
            log: Some(ts_lexer__log_shim),
        },
        current_position: Length {
            bytes: 0,
            extent: TSPoint { row: 0, column: 0 },
        },
        token_start_position: Length {
            bytes: 0,
            extent: TSPoint { row: 0, column: 0 },
        },
        token_end_position: LENGTH_UNDEFINED,
        included_ranges: ptr::null_mut(),
        chunk: ptr::null(),
        input: TSInput {
            payload: ptr::null_mut(),
            read: None,
            encoding: TSInputEncodingUTF8,
            decode: None,
        },
        logger: TSLogger {
            payload: ptr::null_mut(),
            log: None,
        },
        included_range_count: 0,
        current_included_range_index: 0,
        chunk_start: 0,
        chunk_size: 0,
        lookahead_size: 0,
        did_get_column: false,
        column_data: ColumnData {
            value: 0,
            valid: false,
        },
        debug_buffer: [0; TREE_SITTER_SERIALIZATION_BUFFER_SIZE],
    };
    lexer_set_included_ranges(&mut lexer, ptr::null(), 0);
    lexer
}

// ---------------------------------------------------------------------------
// Compile-time layout assertions
// ---------------------------------------------------------------------------

const _: () = assert!(core::mem::size_of::<ColumnData>() == 8);

// ---------------------------------------------------------------------------
// Internal (static) functions
// ---------------------------------------------------------------------------

#[inline]
unsafe fn lexer_ref<'a>(lexer: *const TSLexer) -> &'a Lexer {
    ptr_ref(lexer.cast::<Lexer>())
}

#[inline]
unsafe fn lexer_mut<'a>(lexer: *mut TSLexer) -> &'a mut Lexer {
    ptr_mut(lexer.cast::<Lexer>())
}

/// Sets the column data to the given value and marks it valid.
fn lexer_set_column_data(self_: &mut Lexer, val: u32) {
    self_.column_data.valid = true;
    self_.column_data.value = val;
}

/// Increments the value of the column data; no-op if invalid.
fn lexer_increment_column_data(self_: &mut Lexer) {
    if self_.column_data.valid {
        self_.column_data.value += 1;
    }
}

/// Marks the column data as invalid.
fn lexer_invalidate_column_data(self_: &mut Lexer) {
    self_.column_data.valid = false;
    self_.column_data.value = 0;
}

/// Check if the lexer has reached EOF.
#[allow(non_snake_case)]
unsafe extern "C" fn ts_lexer__eof(lexer: *const TSLexer) -> bool {
    let self_ = lexer_ref(lexer);
    lexer_is_eof(self_)
}

#[inline]
const fn lexer_is_eof(self_: &Lexer) -> bool {
    self_.current_included_range_index == self_.included_range_count
}

/// Clear the currently stored chunk of source code.
fn lexer_clear_chunk(self_: &mut Lexer) {
    self_.chunk = ptr::null();
    self_.chunk_size = 0;
    self_.chunk_start = 0;
}

unsafe fn lexer_included_range(self_: &Lexer, index: usize) -> &TSRange {
    debug_assert!(index < self_.included_range_count as usize);
    debug_assert!(!self_.included_ranges.is_null());
    ptr_ref(self_.included_ranges.add(index))
}

/// Call the input callback to obtain a new chunk of source code.
unsafe fn lexer_get_chunk(self_: &mut Lexer) {
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
unsafe fn lexer_get_lookahead(self_: &mut Lexer) {
    let position_in_chunk = self_.current_position.bytes - self_.chunk_start;
    let mut size = self_.chunk_size - position_in_chunk;

    if size == 0 {
        self_.lookahead_size = 1;
        self_.data.lookahead = 0;
        return;
    }

    let mut chunk = self_.chunk.cast::<u8>().add(position_in_chunk as usize);
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
        lexer_get_chunk(self_);
        chunk = self_.chunk.cast::<u8>();
        size = self_.chunk_size;
        self_.lookahead_size = decode(chunk, size, &mut self_.data.lookahead);
    }

    if self_.data.lookahead == TS_DECODE_ERROR {
        self_.lookahead_size = 1;
    }
}

/// Move the lexer to a given position, finding the right included range.
unsafe fn lexer_goto(self_: &mut Lexer, position: Length) {
    if position.bytes != self_.current_position.bytes {
        lexer_invalidate_column_data(self_);
    }

    self_.current_position = position;

    if self_.included_range_count == 1 {
        let included_range = *lexer_included_range(self_, 0);
        if included_range.end_byte > self_.current_position.bytes
            && included_range.end_byte > included_range.start_byte
        {
            if included_range.start_byte >= self_.current_position.bytes {
                self_.current_position = Length {
                    bytes: included_range.start_byte,
                    extent: included_range.start_point,
                };
            }
            self_.current_included_range_index = 0;
            if !self_.chunk.is_null()
                && (self_.current_position.bytes < self_.chunk_start
                    || self_.current_position.bytes >= self_.chunk_start + self_.chunk_size)
            {
                lexer_clear_chunk(self_);
            }
            self_.lookahead_size = 0;
            self_.data.lookahead = 0;
        } else {
            self_.current_included_range_index = 1;
            self_.current_position = Length {
                bytes: included_range.end_byte,
                extent: included_range.end_point,
            };
            lexer_clear_chunk(self_);
            self_.lookahead_size = 1;
            self_.data.lookahead = 0;
        }
        return;
    }

    // Move to the first valid position at or after the given position.
    let found_included_range = 'range_search: {
        for i in 0..self_.included_range_count {
            let included_range = lexer_included_range(self_, i as usize);
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
                break 'range_search true;
            }
        }
        false
    };

    if found_included_range {
        // If the current position is outside of the current chunk of text,
        // then clear out the current chunk of text.
        if !self_.chunk.is_null()
            && (self_.current_position.bytes < self_.chunk_start
                || self_.current_position.bytes >= self_.chunk_start + self_.chunk_size)
        {
            lexer_clear_chunk(self_);
        }

        self_.lookahead_size = 0;
        self_.data.lookahead = 0;
    } else {
        // If the given position is beyond any of included ranges, move to the EOF
        // state - past the end of the included ranges.
        self_.current_included_range_index = self_.included_range_count;
        let last_range_index = self_.included_range_count as usize - 1;
        let last_included_range = lexer_included_range(self_, last_range_index);
        self_.current_position = Length {
            bytes: last_included_range.end_byte,
            extent: last_included_range.end_point,
        };
        lexer_clear_chunk(self_);
        self_.lookahead_size = 1;
        self_.data.lookahead = 0;
    }
}

/// Advance byte/point coordinates by the currently loaded lookahead character.
///
/// This step only moves the logical position. It does not load a new input
/// chunk or decode the next character.
fn lexer_advance_position(self_: &mut Lexer) {
    if self_.lookahead_size != 0 {
        if self_.data.lookahead == '\n' as i32 {
            self_.current_position.extent.row += 1;
            self_.current_position.extent.column = 0;
            lexer_set_column_data(self_, 0);
        } else {
            let is_bom =
                self_.current_position.bytes == 0 && self_.data.lookahead == BYTE_ORDER_MARK;
            if !is_bom {
                lexer_increment_column_data(self_);
            }
            self_.current_position.extent.column += self_.lookahead_size;
        }
        self_.current_position.bytes += self_.lookahead_size;
    }
}

/// Move from exhausted included ranges to the next visible range.
///
/// Returns `false` when the lexer has advanced beyond all included ranges and
/// should report EOF. Ranges can be disjoint, so moving across a boundary may
/// jump `current_position` forward without consuming bytes from the input.
unsafe fn lexer_seek_visible_range(self_: &mut Lexer) -> bool {
    loop {
        let range_index = self_.current_included_range_index as usize;
        let current_range = lexer_included_range(self_, range_index);
        if self_.current_position.bytes < current_range.end_byte
            && current_range.end_byte != current_range.start_byte
        {
            break;
        }
        if self_.current_included_range_index < self_.included_range_count {
            self_.current_included_range_index += 1;
        }
        if self_.current_included_range_index < self_.included_range_count {
            let next_range_index = self_.current_included_range_index as usize;
            let next_range = lexer_included_range(self_, next_range_index);
            self_.current_position = Length {
                bytes: next_range.start_byte,
                extent: next_range.start_point,
            };
        } else {
            return false;
        }
    }

    true
}

/// Load the next source chunk if needed, then decode the next lookahead.
unsafe fn lexer_load_next_lookahead(self_: &mut Lexer, has_current_range: bool) {
    if has_current_range {
        if self_.current_position.bytes < self_.chunk_start
            || self_.current_position.bytes >= self_.chunk_start + self_.chunk_size
        {
            lexer_get_chunk(self_);
        }
        lexer_get_lookahead(self_);
    } else {
        lexer_clear_chunk(self_);
        self_.data.lookahead = 0;
        self_.lookahead_size = 1;
    }
}

/// Actually advances the lexer. Does not log anything.
unsafe fn lexer_do_advance(self_: &mut Lexer, skip: bool) {
    lexer_advance_position(self_);
    let has_current_range = lexer_seek_visible_range(self_);

    if skip {
        self_.token_start_position = self_.current_position;
    }

    lexer_load_next_lookahead(self_, has_current_range);
}

/// Advance to the next character (with logging). `TSLexer` vtable callback.
#[allow(non_snake_case)]
unsafe extern "C-unwind" fn ts_lexer__advance(lexer: *mut TSLexer, skip: bool) {
    let self_ = lexer_mut(lexer);
    if self_.chunk.is_null() {
        return;
    }

    if self_.logger.log.is_some() {
        let character = self_.data.lookahead;
        if skip {
            if (32..127).contains(&character) {
                ts_lexer__log_shim(
                    lexer,
                    c"skip character:'%c'".as_ptr().cast::<i8>(),
                    character,
                );
            } else {
                ts_lexer__log_shim(lexer, c"skip character:%d".as_ptr().cast::<i8>(), character);
            }
        } else if (32..127).contains(&character) {
            ts_lexer__log_shim(
                lexer,
                c"consume character:'%c'".as_ptr().cast::<i8>(),
                character,
            );
        } else {
            ts_lexer__log_shim(
                lexer,
                c"consume character:%d".as_ptr().cast::<i8>(),
                character,
            );
        }
    }

    lexer_do_advance(self_, skip);
}

/// Mark that a token match has completed. `TSLexer` vtable callback.
#[allow(non_snake_case)]
unsafe extern "C" fn ts_lexer__mark_end(lexer: *mut TSLexer) {
    let self_ = lexer_mut(lexer);
    if !lexer_is_eof(self_) {
        // If the lexer is right at the beginning of included range,
        // then the token should be considered to end at the *end* of the
        // previous included range, rather than here.
        let range_index = self_.current_included_range_index as usize;
        let current_included_range = lexer_included_range(self_, range_index);
        if range_index > 0 && self_.current_position.bytes == current_included_range.start_byte {
            let previous_included_range = lexer_included_range(self_, range_index - 1);
            self_.token_end_position = Length {
                bytes: previous_included_range.end_byte,
                extent: previous_included_range.end_point,
            };
            return;
        }
    }
    self_.token_end_position = self_.current_position;
}

/// Get the current column number. `TSLexer` vtable callback.
#[allow(non_snake_case)]
unsafe extern "C" fn ts_lexer__get_column(lexer: *mut TSLexer) -> u32 {
    let self_ = lexer_mut(lexer);

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
        lexer_goto(self_, start_of_col);
        lexer_set_column_data(self_, 0);
        lexer_get_chunk(self_);

        if !lexer_is_eof(self_) {
            lexer_get_lookahead(self_);

            // Advance to the recorded position
            while self_.current_position.bytes < goal_byte
                && !lexer_is_eof(self_)
                && !self_.chunk.is_null()
            {
                lexer_do_advance(self_, false);
                if lexer_is_eof(self_) {
                    break;
                }
            }
        }
    }

    self_.column_data.value
}

/// Is the lexer at a boundary between two disjoint included ranges?
/// `TSLexer` vtable callback.
#[allow(non_snake_case)]
unsafe extern "C" fn ts_lexer__is_at_included_range_start(lexer: *const TSLexer) -> bool {
    let self_ = lexer_ref(lexer);
    if self_.current_included_range_index < self_.included_range_count {
        let range_index = self_.current_included_range_index as usize;
        let current_range = lexer_included_range(self_, range_index);
        self_.current_position.bytes == current_range.start_byte
    } else {
        false
    }
}

// The variadic log function is defined in lexer_log_shim.c because
// Rust stable cannot define C-variadic functions. It's imported here
// and assigned to TSLexer::log in lexer_init.
//
// `C-unwind`: the log callback may be a host function (e.g. a JS logger) that
// throws/unwinds. Without this the unwind would hit a `nounwind` boundary and
// abort instead of propagating out of the parse.
extern "C-unwind" {
    #[allow(non_snake_case)]
    fn ts_lexer__log_shim(_self: *const TSLexer, fmt: *const i8, ...);
}

// ===========================================================================
// Parser-facing lexer functions.
// ===========================================================================

/// Free the lexer's `included_ranges` allocation.
pub unsafe fn lexer_delete(self_: &mut Lexer) {
    free(self_.included_ranges.cast::<c_void>());
}

/// Set the input source for the lexer.
pub unsafe fn lexer_set_input(self_: &mut Lexer, input: TSInput) {
    self_.input = input;
    lexer_clear_chunk(self_);
    lexer_goto(self_, self_.current_position);
}

/// Move the lexer to the given position (no-op if already there).
pub unsafe fn lexer_reset(self_: &mut Lexer, position: Length) {
    if position.bytes != self_.current_position.bytes {
        lexer_goto(self_, position);
    }
}

/// Prepare the lexer to start scanning a new token.
pub unsafe fn lexer_start(self_: &mut Lexer) {
    self_.token_start_position = self_.current_position;
    self_.token_end_position = LENGTH_UNDEFINED;
    self_.data.result_symbol = 0;
    self_.did_get_column = false;
    if !lexer_is_eof(self_) {
        if self_.chunk_size == 0 {
            lexer_get_chunk(self_);
        }
        if self_.lookahead_size == 0 {
            lexer_get_lookahead(self_);
        }
        if self_.current_position.bytes == 0 {
            if self_.data.lookahead == BYTE_ORDER_MARK {
                ts_lexer__advance(&mut self_.data, true);
            }
            lexer_set_column_data(self_, 0);
        }
    }
}

/// Finalize the current token scan.
pub unsafe fn lexer_finish(self_: &mut Lexer, lookahead_end_byte: &mut u32) {
    if length_is_undefined(self_.token_end_position) {
        ts_lexer__mark_end(&mut self_.data);
    }

    // If the token ended at an included range boundary, then its end position
    // will have been reset to the end of the preceding range. Reset the start
    // position to match.
    if self_.token_end_position.bytes < self_.token_start_position.bytes {
        self_.token_start_position = self_.token_end_position;
    }

    let mut current_lookahead_end_byte = self_.current_position.bytes + 1;

    // In order to determine that a byte sequence is invalid UTF8 or UTF16,
    // the character decoding algorithm may have looked at the following byte.
    if self_.data.lookahead == TS_DECODE_ERROR {
        current_lookahead_end_byte += 4;
    }

    if current_lookahead_end_byte > *lookahead_end_byte {
        *lookahead_end_byte = current_lookahead_end_byte;
    }
}

/// Mark the end of the current token.
pub unsafe fn lexer_mark_end(self_: &mut Lexer) {
    ts_lexer__mark_end(&mut self_.data);
}

/// Set the included ranges for the lexer. Returns false if ranges are invalid.
pub unsafe fn lexer_set_included_ranges(
    self_: &mut Lexer,
    mut ranges: *const TSRange,
    mut count: u32,
) -> bool {
    if count == 0 || ranges.is_null() {
        ranges = &DEFAULT_RANGE;
        count = 1;
    } else {
        let mut previous_byte: u32 = 0;
        for range in core::slice::from_raw_parts(ranges, count as usize) {
            if range.start_byte < previous_byte || range.end_byte < range.start_byte {
                return false;
            }
            previous_byte = range.end_byte;
        }
    }

    let count = count as usize;
    self_.included_ranges = realloc(
        self_.included_ranges.cast::<c_void>(),
        count * core::mem::size_of::<TSRange>(),
    )
    .cast::<TSRange>();
    core::ptr::copy_nonoverlapping(ranges, self_.included_ranges, count);
    self_.included_range_count = count as u32;
    lexer_goto(self_, self_.current_position);
    true
}

/// Get the current included ranges.
pub unsafe fn lexer_included_ranges(self_: &Lexer, count: *mut u32) -> *mut TSRange {
    *count = self_.included_range_count;
    self_.included_ranges
}
