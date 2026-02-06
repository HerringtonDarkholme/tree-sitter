#![allow(dead_code, unused_variables, unused_imports, non_upper_case_globals, non_snake_case)]

//! Rust replacement for language.c/h — Language metadata and parse table access.
//!
//! This module provides:
//! - `TableEntry` / `LookaheadIterator` internal types (from language.h)
//! - Exported functions that access TSLanguage fields (from language.c)
//! - Static-inline helper functions re-implemented from language.h
//!
//! TSLanguage itself is defined in parser.h and created by generated parsers.
//! We access it as an opaque `repr(C)` struct via raw pointers.

use std::ffi::c_void;
use std::ptr;

use crate::ffi::{TSLanguage, TSPoint, TSStateId, TSSymbol, TSFieldId};

// Re-use types already defined in subtree.rs
use super::subtree::TSSymbolMetadata;
use super::alloc::{ts_malloc, ts_free};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const LANGUAGE_VERSION_WITH_RESERVED_WORDS: u32 = 15;
pub const LANGUAGE_VERSION_WITH_PRIMARY_STATES: u32 = 14;

const ts_builtin_sym_error: TSSymbol = u16::MAX;
const ts_builtin_sym_error_repeat: TSSymbol = ts_builtin_sym_error - 1;

pub type TSSymbolType = u32;
pub const TSSymbolTypeRegular: TSSymbolType = 0;
pub const TSSymbolTypeAnonymous: TSSymbolType = 1;
pub const TSSymbolTypeSupertype: TSSymbolType = 2;
pub const TSSymbolTypeAuxiliary: TSSymbolType = 3;

// ---------------------------------------------------------------------------
// TSLanguage field access
// ---------------------------------------------------------------------------
//
// TSLanguage is defined in parser.h (C) and generated parsers emit it.
// We must read its fields at known offsets. We define a full repr(C) mirror
// struct here so we can cast `*const TSLanguage` → `*const TSLanguageFull`
// and access every field.
//
// This replaces the partial `TSLanguageData` in subtree.rs.
// ---------------------------------------------------------------------------

/// Mirrors the `external_scanner` sub-struct inside TSLanguage.
#[repr(C)]
pub struct TSExternalScanner {
    pub states: *const bool,
    pub symbol_map: *const TSSymbol,
    pub create: Option<unsafe extern "C" fn() -> *mut c_void>,
    pub destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    pub scan: Option<unsafe extern "C" fn(*mut c_void, *mut TSLexer, *const bool) -> bool>,
    pub serialize: Option<unsafe extern "C" fn(*mut c_void, *mut i8) -> u32>,
    pub deserialize: Option<unsafe extern "C" fn(*mut c_void, *const i8, u32)>,
}

/// TSLexer struct (from parser.h). Needed for function pointer types.
#[repr(C)]
pub struct TSLexer {
    pub lookahead: i32,
    pub result_symbol: TSSymbol,
    pub advance: Option<unsafe extern "C" fn(*mut TSLexer, bool)>,
    pub mark_end: Option<unsafe extern "C" fn(*mut TSLexer)>,
    pub get_column: Option<unsafe extern "C" fn(*mut TSLexer) -> u32>,
    pub is_at_included_range_start: Option<unsafe extern "C" fn(*const TSLexer) -> bool>,
    pub eof: Option<unsafe extern "C" fn(*const TSLexer) -> bool>,
    pub log: Option<unsafe extern "C" fn(*const TSLexer, *const i8, ...)>,
}

/// TSLanguageMetadata (from parser.h)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLanguageMetadata {
    pub major_version: u8,
    pub minor_version: u8,
    pub patch_version: u8,
}

/// TSLexMode (older ABI < 15, without reserved_word_set_id)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLexMode {
    pub lex_state: u16,
    pub external_lex_state: u16,
}

/// TSLexerMode (ABI >= 15, with reserved_word_set_id)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLexerMode {
    pub lex_state: u16,
    pub external_lex_state: u16,
    pub reserved_word_set_id: u16,
}

/// TSParseActionType enum
pub const TSParseActionTypeShift: u8 = 0;
pub const TSParseActionTypeReduce: u8 = 1;
pub const TSParseActionTypeAccept: u8 = 2;
pub const TSParseActionTypeRecover: u8 = 3;

/// TSParseAction — a union in C. We use repr(C) with manual field access.
/// The C union has:
///   shift: { type: u8, state: u16, extra: bool, repetition: bool }
///   reduce: { type: u8, child_count: u8, symbol: u16, dynamic_precedence: i16, production_id: u16 }
///   type: u8
///
/// Total size is 8 bytes (the `reduce` variant is largest).
#[repr(C)]
#[derive(Clone, Copy)]
pub union TSParseAction {
    pub shift: TSParseActionShift,
    pub reduce: TSParseActionReduce,
    pub type_: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSParseActionShift {
    pub type_: u8,
    pub state: TSStateId,
    pub extra: bool,
    pub repetition: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSParseActionReduce {
    pub type_: u8,
    pub child_count: u8,
    pub symbol: TSSymbol,
    pub dynamic_precedence: i16,
    pub production_id: u16,
}

/// TSParseActionEntry — a union in C:
///   action: TSParseAction
///   entry: { count: u8, reusable: bool }
#[repr(C)]
#[derive(Clone, Copy)]
pub union TSParseActionEntry {
    pub action: TSParseAction,
    pub entry: TSParseActionEntryData,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSParseActionEntryData {
    pub count: u8,
    pub reusable: bool,
}

/// TSMapSlice (from parser.h, also defined in subtree.rs — re-import from there)
pub use super::subtree::TSMapSlice;

/// TSFieldMapEntry (from parser.h, also defined in subtree.rs)
pub use super::subtree::TSFieldMapEntry;

/// Full repr(C) mirror of the TSLanguage struct from parser.h.
/// Used to read fields at correct offsets via pointer cast.
#[repr(C)]
pub struct TSLanguageFull {
    pub abi_version: u32,
    pub symbol_count: u32,
    pub alias_count: u32,
    pub token_count: u32,
    pub external_token_count: u32,
    pub state_count: u32,
    pub large_state_count: u32,
    pub production_id_count: u32,
    pub field_count: u32,
    pub max_alias_sequence_length: u16,
    pub parse_table: *const u16,
    pub small_parse_table: *const u16,
    pub small_parse_table_map: *const u32,
    pub parse_actions: *const TSParseActionEntry,
    pub symbol_names: *const *const i8,
    pub field_names: *const *const i8,
    pub field_map_slices: *const TSMapSlice,
    pub field_map_entries: *const TSFieldMapEntry,
    pub symbol_metadata: *const TSSymbolMetadata,
    pub public_symbol_map: *const TSSymbol,
    pub alias_map: *const u16,
    pub alias_sequences: *const TSSymbol,
    pub lex_modes: *const TSLexerMode,
    pub lex_fn: Option<unsafe extern "C" fn(*mut TSLexer, TSStateId) -> bool>,
    pub keyword_lex_fn: Option<unsafe extern "C" fn(*mut TSLexer, TSStateId) -> bool>,
    pub keyword_capture_token: TSSymbol,
    pub external_scanner: TSExternalScanner,
    pub primary_state_ids: *const TSStateId,
    pub name: *const i8,
    pub reserved_words: *const TSSymbol,
    pub max_reserved_word_set_size: u16,
    pub supertype_count: u32,
    pub supertype_symbols: *const TSSymbol,
    pub supertype_map_slices: *const TSMapSlice,
    pub supertype_map_entries: *const TSSymbol,
    pub metadata: TSLanguageMetadata,
}

// ---------------------------------------------------------------------------
// Internal types from language.h
// ---------------------------------------------------------------------------

/// Result of looking up a parse table entry for a (state, symbol) pair.
#[repr(C)]
pub struct TableEntry {
    pub actions: *const TSParseAction,
    pub action_count: u32,
    pub is_reusable: bool,
}

/// Iterator over valid lookahead symbols for a given parse state.
#[repr(C)]
pub struct LookaheadIterator {
    pub language: *const TSLanguage,
    pub data: *const u16,
    pub group_end: *const u16,
    pub state: TSStateId,
    pub table_value: u16,
    pub section_index: u16,
    pub group_count: u16,
    pub is_small_state: bool,

    pub actions: *const TSParseAction,
    pub symbol: TSSymbol,
    pub next_state: TSStateId,
    pub action_count: u16,
}

// ---------------------------------------------------------------------------
// Compile-time layout assertions
// ---------------------------------------------------------------------------

const _: () = assert!(std::mem::size_of::<TSLexMode>() == 4);
const _: () = assert!(std::mem::size_of::<TSLexerMode>() == 6);
const _: () = assert!(std::mem::size_of::<TSParseActionReduce>() == 8);
const _: () = assert!(std::mem::size_of::<TSParseActionShift>() == 6);
const _: () = assert!(std::mem::size_of::<TSParseAction>() == 8);
const _: () = assert!(std::mem::size_of::<TSParseActionEntryData>() == 2);
const _: () = assert!(std::mem::size_of::<TSParseActionEntry>() == 8);
const _: () = assert!(std::mem::size_of::<TSLanguageMetadata>() == 3);
const _: () = assert!(std::mem::size_of::<TSMapSlice>() == 4);
const _: () = assert!(std::mem::size_of::<TableEntry>() == 16);
const _: () = assert!(std::mem::size_of::<LookaheadIterator>() == 56);

// ---------------------------------------------------------------------------
// Helper: cast TSLanguage to our full layout mirror
// ---------------------------------------------------------------------------

#[inline(always)]
unsafe fn lang(self_: *const TSLanguage) -> *const TSLanguageFull {
    self_ as *const TSLanguageFull
}

// ---------------------------------------------------------------------------
// Extern C declarations for functions we call from other C modules
// ---------------------------------------------------------------------------

extern "C" {
    // wasm_store.c — only called when wasm feature is active
    fn ts_language_is_wasm(self_: *const TSLanguage) -> bool;
    fn ts_wasm_language_retain(self_: *const TSLanguage);
    fn ts_wasm_language_release(self_: *const TSLanguage);

    // libc
    fn strncmp(s1: *const i8, s2: *const i8, n: usize) -> i32;
    fn fputc(c: i32, stream: *mut c_void) -> i32;
    fn fputs(s: *const i8, stream: *mut c_void) -> i32;
}

// ===========================================================================
// Static inline re-implementations from language.h
// ===========================================================================

/// Look up the table value for a given symbol and state.
/// For non-terminal symbols → successor state.
/// For terminal symbols → index into actions table.
#[inline]
pub unsafe fn ts_language_lookup(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> u16 {
    let l = lang(self_);
    if (state as u32) >= (*l).large_state_count {
        let index = *(*l).small_parse_table_map.add(state as usize - (*l).large_state_count as usize);
        let mut data = (*l).small_parse_table.add(index as usize);
        let group_count = *data;
        data = data.add(1);
        for _ in 0..group_count {
            let section_value = *data;
            data = data.add(1);
            let symbol_count = *data;
            data = data.add(1);
            for _ in 0..symbol_count {
                if *data == symbol {
                    return section_value;
                }
                data = data.add(1);
            }
        }
        0
    } else {
        *(*l).parse_table.add(state as usize * (*l).symbol_count as usize + symbol as usize)
    }
}

/// Get the parse actions for a (state, symbol) pair.
#[inline]
pub unsafe fn ts_language_actions(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
    count: *mut u32,
) -> *const TSParseAction {
    let mut entry = std::mem::zeroed::<TableEntry>();
    ts_language_table_entry(self_, state, symbol, &mut entry);
    *count = entry.action_count;
    entry.actions
}

/// Check if a (state, symbol) has a reduce action.
#[inline]
pub unsafe fn ts_language_has_reduce_action(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> bool {
    let mut entry = std::mem::zeroed::<TableEntry>();
    ts_language_table_entry(self_, state, symbol, &mut entry);
    entry.action_count > 0 && (*entry.actions).type_ == TSParseActionTypeReduce
}

/// Check if a (state, symbol) has any actions.
#[inline]
pub unsafe fn ts_language_has_actions(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> bool {
    ts_language_lookup(self_, state, symbol) != 0
}

/// Create a lookahead iterator for a given state.
#[inline]
pub unsafe fn ts_language_lookaheads(
    self_: *const TSLanguage,
    state: TSStateId,
) -> LookaheadIterator {
    let l = lang(self_);
    let is_small_state = (state as u32) >= (*l).large_state_count;
    let data;
    let mut group_end: *const u16 = ptr::null();
    let mut group_count: u16 = 0;
    if is_small_state {
        let index = *(*l).small_parse_table_map.add(state as usize - (*l).large_state_count as usize);
        data = (*l).small_parse_table.add(index as usize);
        group_end = data.add(1);
        group_count = *data;
    } else {
        data = (*l).parse_table.add(state as usize * (*l).symbol_count as usize).sub(1);
    }
    LookaheadIterator {
        language: self_,
        data,
        group_end,
        state: 0,
        table_value: 0,
        section_index: 0,
        group_count,
        is_small_state,
        actions: ptr::null(),
        symbol: u16::MAX,
        next_state: 0,
        action_count: 0,
    }
}

/// Advance a lookahead iterator to the next valid symbol.
#[inline]
pub unsafe fn ts_lookahead_iterator__next(self_: *mut LookaheadIterator) -> bool {
    let l = lang((*self_).language);

    if (*self_).is_small_state {
        (*self_).data = (*self_).data.add(1);
        if (*self_).data == (*self_).group_end {
            if (*self_).group_count == 0 {
                return false;
            }
            (*self_).group_count -= 1;
            (*self_).table_value = *(*self_).data;
            (*self_).data = (*self_).data.add(1);
            let symbol_count = *(*self_).data;
            (*self_).data = (*self_).data.add(1);
            (*self_).group_end = (*self_).data.add(symbol_count as usize);
            (*self_).symbol = *(*self_).data;
        } else {
            (*self_).symbol = *(*self_).data;
            return true;
        }
    } else {
        loop {
            (*self_).data = (*self_).data.add(1);
            (*self_).symbol = (*self_).symbol.wrapping_add(1);
            if (*self_).symbol >= (*l).symbol_count as u16 {
                return false;
            }
            (*self_).table_value = *(*self_).data;
            if (*self_).table_value != 0 {
                break;
            }
        }
    }

    // Depending on if the symbol is terminal or non-terminal, the table value
    // either represents a list of actions or a successor state.
    if ((*self_).symbol as u32) < (*l).token_count {
        let entry = &*(*l).parse_actions.add((*self_).table_value as usize);
        (*self_).action_count = entry.entry.count as u16;
        (*self_).actions = (*l).parse_actions.add((*self_).table_value as usize + 1) as *const TSParseAction;
        (*self_).next_state = 0;
    } else {
        (*self_).action_count = 0;
        (*self_).next_state = (*self_).table_value;
    }
    true
}

/// Whether the state is a "primary state" (ABI >= 14).
#[inline]
pub unsafe fn ts_language_state_is_primary(
    self_: *const TSLanguage,
    state: TSStateId,
) -> bool {
    let l = lang(self_);
    if (*l).abi_version >= LANGUAGE_VERSION_WITH_PRIMARY_STATES {
        state == *(*l).primary_state_ids.add(state as usize)
    } else {
        true
    }
}

/// Get enabled external tokens for a given external scanner state.
#[inline]
pub unsafe fn ts_language_enabled_external_tokens(
    self_: *const TSLanguage,
    external_scanner_state: u32,
) -> *const bool {
    let l = lang(self_);
    if external_scanner_state == 0 {
        ptr::null()
    } else {
        (*l).external_scanner.states.add((*l).external_token_count as usize * external_scanner_state as usize)
    }
}

/// Get the alias sequence for a production ID.
#[inline]
pub unsafe fn ts_language_alias_sequence(
    self_: *const TSLanguage,
    production_id: u32,
) -> *const TSSymbol {
    let l = lang(self_);
    if production_id != 0 {
        (*l).alias_sequences.add(production_id as usize * (*l).max_alias_sequence_length as usize)
    } else {
        ptr::null()
    }
}

/// Get the alias at a specific position in a production's alias sequence.
#[inline]
pub unsafe fn ts_language_alias_at(
    self_: *const TSLanguage,
    production_id: u32,
    child_index: u32,
) -> TSSymbol {
    let l = lang(self_);
    if production_id != 0 {
        *(*l).alias_sequences.add(production_id as usize * (*l).max_alias_sequence_length as usize + child_index as usize)
    } else {
        0
    }
}

/// Get the field map (start, end) for a production ID.
#[inline]
pub unsafe fn ts_language_field_map(
    self_: *const TSLanguage,
    production_id: u32,
    start: *mut *const TSFieldMapEntry,
    end: *mut *const TSFieldMapEntry,
) {
    let l = lang(self_);
    if (*l).field_count == 0 {
        *start = ptr::null();
        *end = ptr::null();
        return;
    }
    let slice = *(*l).field_map_slices.add(production_id as usize);
    *start = (*l).field_map_entries.add(slice.index as usize);
    *end = (*l).field_map_entries.add(slice.index as usize + slice.length as usize);
}

/// Get all aliases for a symbol.
#[inline]
pub unsafe fn ts_language_aliases_for_symbol(
    self_: *const TSLanguage,
    original_symbol: TSSymbol,
    start: *mut *const TSSymbol,
    end: *mut *const TSSymbol,
) {
    let l = lang(self_);
    *start = (*l).public_symbol_map.add(original_symbol as usize);
    *end = (*start).add(1);

    let mut idx: usize = 0;
    loop {
        let symbol = *(*l).alias_map.add(idx);
        idx += 1;
        if symbol == 0 || symbol > original_symbol {
            break;
        }
        let count = *(*l).alias_map.add(idx);
        idx += 1;
        if symbol == original_symbol {
            *start = (*l).alias_map.add(idx) as *const TSSymbol;
            *end = (*l).alias_map.add(idx + count as usize) as *const TSSymbol;
            break;
        }
        idx += count as usize;
    }
}

/// Write a symbol name with escaping to a FILE*.
#[inline]
pub unsafe fn ts_language_write_symbol_as_dot_string(
    self_: *const TSLanguage,
    f: *mut c_void,
    symbol: TSSymbol,
) {
    let name = ts_language_symbol_name(self_, symbol);
    let mut chr = name;
    while *chr != 0 {
        match *chr as u8 {
            b'"' | b'\\' => {
                fputc(b'\\' as i32, f);
                fputc(*chr as i32, f);
            }
            b'\n' => {
                fputs(b"\\n\0".as_ptr() as *const i8, f);
            }
            b'\t' => {
                fputs(b"\\t\0".as_ptr() as *const i8, f);
            }
            _ => {
                fputc(*chr as i32, f);
            }
        }
        chr = chr.add(1);
    }
}

// ===========================================================================
// Exported functions from language.c
// ===========================================================================

#[no_mangle]
pub unsafe extern "C" fn ts_language_copy(
    self_: *const TSLanguage,
) -> *const TSLanguage {
    if !self_.is_null() && ts_language_is_wasm(self_) {
        ts_wasm_language_retain(self_);
    }
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_delete(self_: *const TSLanguage) {
    if !self_.is_null() && ts_language_is_wasm(self_) {
        ts_wasm_language_release(self_);
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_symbol_count(
    self_: *const TSLanguage,
) -> u32 {
    let l = lang(self_);
    (*l).symbol_count + (*l).alias_count
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_state_count(
    self_: *const TSLanguage,
) -> u32 {
    (*lang(self_)).state_count
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_supertypes(
    self_: *const TSLanguage,
    length: *mut u32,
) -> *const TSSymbol {
    let l = lang(self_);
    if (*l).abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
        *length = (*l).supertype_count;
        (*l).supertype_symbols
    } else {
        *length = 0;
        ptr::null()
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_subtypes(
    self_: *const TSLanguage,
    supertype: TSSymbol,
    length: *mut u32,
) -> *const TSSymbol {
    let l = lang(self_);
    if (*l).abi_version < LANGUAGE_VERSION_WITH_RESERVED_WORDS
        || !ts_language_symbol_metadata(self_, supertype).supertype
    {
        *length = 0;
        return ptr::null();
    }
    let slice = *(*l).supertype_map_slices.add(supertype as usize);
    *length = slice.length as u32;
    (*l).supertype_map_entries.add(slice.index as usize)
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_abi_version(
    self_: *const TSLanguage,
) -> u32 {
    (*lang(self_)).abi_version
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_metadata(
    self_: *const TSLanguage,
) -> *const TSLanguageMetadata {
    let l = lang(self_);
    if (*l).abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
        &(*l).metadata
    } else {
        ptr::null()
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_name(
    self_: *const TSLanguage,
) -> *const i8 {
    let l = lang(self_);
    if (*l).abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
        (*l).name
    } else {
        ptr::null()
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_field_count(
    self_: *const TSLanguage,
) -> u32 {
    (*lang(self_)).field_count
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_table_entry(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
    result: *mut TableEntry,
) {
    let l = lang(self_);
    if symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat {
        (*result).action_count = 0;
        (*result).is_reusable = false;
        (*result).actions = ptr::null();
    } else {
        debug_assert!((symbol as u32) < (*l).token_count);
        let action_index = ts_language_lookup(self_, state, symbol) as usize;
        let entry = &*(*l).parse_actions.add(action_index);
        (*result).action_count = entry.entry.count as u32;
        (*result).is_reusable = entry.entry.reusable;
        (*result).actions = (*l).parse_actions.add(action_index + 1) as *const TSParseAction;
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_lex_mode_for_state(
    self_: *const TSLanguage,
    state: TSStateId,
) -> TSLexerMode {
    let l = lang(self_);
    if (*l).abi_version < 15 {
        let mode = *((*l).lex_modes as *const TSLexMode).add(state as usize);
        TSLexerMode {
            lex_state: mode.lex_state,
            external_lex_state: mode.external_lex_state,
            reserved_word_set_id: 0,
        }
    } else {
        *(*l).lex_modes.add(state as usize)
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_is_reserved_word(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> bool {
    let l = lang(self_);
    let lex_mode = ts_language_lex_mode_for_state(self_, state);
    if lex_mode.reserved_word_set_id > 0 {
        let start = lex_mode.reserved_word_set_id as u32 * (*l).max_reserved_word_set_size as u32;
        let end = start + (*l).max_reserved_word_set_size as u32;
        for i in start..end {
            let w = *(*l).reserved_words.add(i as usize);
            if w == symbol {
                return true;
            }
            if w == 0 {
                break;
            }
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_symbol_metadata(
    self_: *const TSLanguage,
    symbol: TSSymbol,
) -> TSSymbolMetadata {
    if symbol == ts_builtin_sym_error {
        TSSymbolMetadata { visible: true, named: true, supertype: false }
    } else if symbol == ts_builtin_sym_error_repeat {
        TSSymbolMetadata { visible: false, named: false, supertype: false }
    } else {
        *(*lang(self_)).symbol_metadata.add(symbol as usize)
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_public_symbol(
    self_: *const TSLanguage,
    symbol: TSSymbol,
) -> TSSymbol {
    if symbol == ts_builtin_sym_error {
        symbol
    } else {
        *(*lang(self_)).public_symbol_map.add(symbol as usize)
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_next_state(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> TSStateId {
    let l = lang(self_);
    if symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat {
        0
    } else if (symbol as u32) < (*l).token_count {
        let mut count: u32 = 0;
        let actions = ts_language_actions(self_, state, symbol, &mut count);
        if count > 0 {
            let action = *actions.add(count as usize - 1);
            if action.type_ == TSParseActionTypeShift {
                return if action.shift.extra { state } else { action.shift.state };
            }
        }
        0
    } else {
        ts_language_lookup(self_, state, symbol)
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_symbol_name(
    self_: *const TSLanguage,
    symbol: TSSymbol,
) -> *const i8 {
    if symbol == ts_builtin_sym_error {
        b"ERROR\0".as_ptr() as *const i8
    } else if symbol == ts_builtin_sym_error_repeat {
        b"_ERROR\0".as_ptr() as *const i8
    } else if (symbol as u32) < ts_language_symbol_count(self_) {
        *(*lang(self_)).symbol_names.add(symbol as usize)
    } else {
        ptr::null()
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_symbol_for_name(
    self_: *const TSLanguage,
    string: *const i8,
    length: u32,
    is_named: bool,
) -> TSSymbol {
    if is_named && strncmp(string, b"ERROR\0".as_ptr() as *const i8, length as usize) == 0 {
        return ts_builtin_sym_error;
    }
    let count = ts_language_symbol_count(self_) as u16;
    let l = lang(self_);
    for i in 0..count {
        let metadata = ts_language_symbol_metadata(self_, i);
        if (!metadata.visible && !metadata.supertype) || metadata.named != is_named {
            continue;
        }
        let symbol_name = *(*l).symbol_names.add(i as usize);
        if strncmp(symbol_name, string, length as usize) == 0
            && *symbol_name.add(length as usize) == 0
        {
            return *(*l).public_symbol_map.add(i as usize);
        }
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_symbol_type(
    self_: *const TSLanguage,
    symbol: TSSymbol,
) -> TSSymbolType {
    let metadata = ts_language_symbol_metadata(self_, symbol);
    if metadata.named && metadata.visible {
        TSSymbolTypeRegular
    } else if metadata.visible {
        TSSymbolTypeAnonymous
    } else if metadata.supertype {
        TSSymbolTypeSupertype
    } else {
        TSSymbolTypeAuxiliary
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_field_name_for_id(
    self_: *const TSLanguage,
    id: TSFieldId,
) -> *const i8 {
    let count = ts_language_field_count(self_);
    if count > 0 && (id as u32) <= count {
        *(*lang(self_)).field_names.add(id as usize)
    } else {
        ptr::null()
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_field_id_for_name(
    self_: *const TSLanguage,
    name: *const i8,
    name_length: u32,
) -> TSFieldId {
    let l = lang(self_);
    let count = ts_language_field_count(self_) as u16;
    for i in 1..=count {
        match strncmp(name, *(*l).field_names.add(i as usize), name_length as usize) {
            0 => {
                if *(*(*l).field_names.add(i as usize)).add(name_length as usize) == 0 {
                    return i;
                }
            }
            n if n < 0 => return 0,
            _ => {}
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Lookahead iterator public API
// ---------------------------------------------------------------------------

/// TSLookaheadIterator is an opaque handle = LookaheadIterator allocated on heap.
#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_new(
    self_: *const TSLanguage,
    state: TSStateId,
) -> *mut LookaheadIterator {
    if (state as u32) >= (*lang(self_)).state_count {
        return ptr::null_mut();
    }
    let iterator = ts_malloc(std::mem::size_of::<LookaheadIterator>()) as *mut LookaheadIterator;
    *iterator = ts_language_lookaheads(self_, state);
    iterator
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_delete(
    self_: *mut LookaheadIterator,
) {
    ts_free(self_ as *mut c_void);
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_reset_state(
    self_: *mut LookaheadIterator,
    state: TSStateId,
) -> bool {
    if (state as u32) >= (*lang((*self_).language)).state_count {
        return false;
    }
    *self_ = ts_language_lookaheads((*self_).language, state);
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_language(
    self_: *const LookaheadIterator,
) -> *const TSLanguage {
    (*self_).language
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_reset(
    self_: *mut LookaheadIterator,
    language: *const TSLanguage,
    state: TSStateId,
) -> bool {
    if (state as u32) >= (*lang(language)).state_count {
        return false;
    }
    *self_ = ts_language_lookaheads(language, state);
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_next(
    self_: *mut LookaheadIterator,
) -> bool {
    ts_lookahead_iterator__next(self_)
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_current_symbol(
    self_: *const LookaheadIterator,
) -> TSSymbol {
    (*self_).symbol
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_current_symbol_name(
    self_: *const LookaheadIterator,
) -> *const i8 {
    ts_language_symbol_name((*self_).language, (*self_).symbol)
}
