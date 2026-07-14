//! Language metadata and parse-table access.
//!
//! This module provides:
//! - Parse-table lookup and generated-language layout types
//! - Exported functions that access `TSLanguage` fields
//! - Internal helpers for generated language tables
//!
//! `TSLanguage` itself is defined in parser.h and created by generated parsers.
//! We access it as an opaque `repr(C)` struct via raw pointers.

use core::ffi::c_void;
use core::ptr;

use crate::ffi::{TSFieldId, TSLanguage, TSStateId, TSSymbol};

use super::subtree::{TSSymbolMetadata, TS_BUILTIN_SYM_ERROR, TS_BUILTIN_SYM_ERROR_REPEAT};

mod lookahead;
pub use lookahead::{language_lookaheads, lookahead_iterator_next};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const LANGUAGE_VERSION_WITH_RESERVED_WORDS: u32 = 15;
pub const LANGUAGE_VERSION_WITH_PRIMARY_STATES: u32 = 14;

pub type TSSymbolType = u32;
pub const TS_SYMBOL_TYPE_REGULAR: TSSymbolType = 0;
pub const TS_SYMBOL_TYPE_ANONYMOUS: TSSymbolType = 1;
pub const TS_SYMBOL_TYPE_SUPERTYPE: TSSymbolType = 2;

pub const TS_SYMBOL_TYPE_AUXILIARY: TSSymbolType = 3;

mod generated;
pub use generated::*;

// ---------------------------------------------------------------------------
// Internal types from language.h
// ---------------------------------------------------------------------------

/// Result of looking up a parse table entry for a (state, symbol) pair.
pub struct TableEntry {
    /// Pointer into `TSLanguageFull::parse_actions`.
    pub actions: *const TSParseAction,
    /// Number of actions for this state/symbol.
    pub action_count: u32,
    /// Whether a token from another lex state can be reused here.
    pub is_reusable: bool,
}

impl TableEntry {
    #[inline]
    pub const fn empty() -> Self {
        Self {
            actions: ptr::null(),
            action_count: 0,
            is_reusable: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: cast TSLanguage to our full layout mirror
// ---------------------------------------------------------------------------

#[inline(always)]
pub const unsafe fn language_full<'a>(self_: *const TSLanguage) -> &'a TSLanguageFull {
    &*self_.cast::<TSLanguageFull>()
}

#[inline(always)]
const unsafe fn lang<'a>(self_: *const TSLanguage) -> &'a TSLanguageFull {
    language_full(self_)
}

#[inline]
unsafe fn parse_action_entry(language: &TSLanguageFull, index: usize) -> &TSParseActionEntry {
    language
        .parse_actions
        .add(index)
        .as_ref()
        .unwrap_unchecked()
}

#[inline]
const unsafe fn parse_action_at(language: &TSLanguageFull, index: usize) -> *const TSParseAction {
    language.parse_actions.add(index).cast::<TSParseAction>()
}

// ---------------------------------------------------------------------------
// Extern C declarations for functions we call from other C modules
// ---------------------------------------------------------------------------

extern "C" {
    fn fputc(c: i32, stream: *mut c_void) -> i32;
    fn fputs(s: *const i8, stream: *mut c_void) -> i32;
}

unsafe fn c_string_prefix_cmp(
    left: *const i8,
    right: *const i8,
    len: usize,
) -> core::cmp::Ordering {
    for i in 0..len {
        let left_byte = *left.add(i) as u8;
        let right_byte = *right.add(i) as u8;
        match left_byte.cmp(&right_byte) {
            core::cmp::Ordering::Equal if left_byte == 0 => return core::cmp::Ordering::Equal,
            core::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    core::cmp::Ordering::Equal
}

// ===========================================================================
// Parse-table helpers
// ===========================================================================

/// Look up the table value for a given symbol and state.
/// For non-terminal symbols → successor state.
/// For terminal symbols → index into actions table.
#[inline]
pub unsafe fn language_lookup(self_: *const TSLanguage, state: TSStateId, symbol: TSSymbol) -> u16 {
    let l = lang(self_);
    if u32::from(state) >= l.large_state_count {
        let index = *l
            .small_parse_table_map
            .add(state as usize - l.large_state_count as usize);
        let mut data = l.small_parse_table.add(index as usize);
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
        *l.parse_table
            .add(state as usize * l.symbol_count as usize + symbol as usize)
    }
}

/// Get the parse actions for a (state, symbol) pair.
#[inline]
pub unsafe fn language_actions(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
    count: &mut u32,
) -> *const TSParseAction {
    let mut entry = TableEntry::empty();
    language_table_entry(self_, state, symbol, &mut entry);
    *count = entry.action_count;
    entry.actions
}

/// Check if a (state, symbol) has a reduce action.
#[inline]
pub unsafe fn language_has_reduce_action(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> bool {
    let mut entry = TableEntry::empty();
    language_table_entry(self_, state, symbol, &mut entry);
    entry.action_count > 0 && (*entry.actions).type_ == TSPARSE_ACTION_TYPE_REDUCE
}

/// Check if a (state, symbol) has any actions.
#[inline]
pub unsafe fn language_has_actions(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> bool {
    language_lookup(self_, state, symbol) != 0
}

/// Whether the state is a "primary state" (ABI >= 14).
#[inline]
pub const unsafe fn language_state_is_primary(self_: *const TSLanguage, state: TSStateId) -> bool {
    let l = lang(self_);
    if l.abi_version >= LANGUAGE_VERSION_WITH_PRIMARY_STATES {
        state == *l.primary_state_ids.add(state as usize)
    } else {
        true
    }
}

/// Get enabled external tokens for a given external scanner state.
#[inline]
pub const unsafe fn language_enabled_external_tokens(
    self_: *const TSLanguage,
    external_scanner_state: u32,
) -> *const bool {
    let l = lang(self_);
    if external_scanner_state == 0 {
        ptr::null()
    } else {
        l.external_scanner
            .states
            .add(l.external_token_count as usize * external_scanner_state as usize)
    }
}

/// Borrow the alias symbols for a production.
///
/// Generated language tables have static storage duration. An empty slice
/// means that the production has no aliases. Raw table access stays confined
/// to this language boundary.
#[inline]
pub const unsafe fn language_alias_sequence_slice(
    self_: *const TSLanguage,
    production_id: u32,
) -> &'static [TSSymbol] {
    if production_id == 0 {
        return &[];
    }
    let l = lang(self_);
    core::slice::from_raw_parts(
        l.alias_sequences
            .add(production_id as usize * l.max_alias_sequence_length as usize),
        l.max_alias_sequence_length as usize,
    )
}

/// Get the alias at a specific position in a production's alias sequence.
#[inline]
pub const unsafe fn language_alias_at(
    self_: *const TSLanguage,
    production_id: u32,
    child_index: u32,
) -> TSSymbol {
    let aliases = language_alias_sequence_slice(self_, production_id);
    if child_index < aliases.len() as u32 {
        aliases[child_index as usize]
    } else {
        0
    }
}

/// Borrow the field mappings for a production.
///
/// Generated languages store these mappings in a shared table. This helper
/// contains the raw table access so callers can use an ordinary Rust slice.
#[inline]
pub const unsafe fn language_field_map_slice<'a>(
    self_: *const TSLanguage,
    production_id: u32,
) -> &'a [TSFieldMapEntry] {
    let l = lang(self_);
    if l.field_count == 0 {
        return &[];
    }
    let slice = *l.field_map_slices.add(production_id as usize);
    core::slice::from_raw_parts(
        l.field_map_entries.add(slice.index as usize),
        slice.length as usize,
    )
}

/// Legacy pointer-range form used by the inactive C-port query implementation.
#[inline]
pub unsafe fn language_field_map(
    self_: *const TSLanguage,
    production_id: u32,
    start: *mut *const TSFieldMapEntry,
    end: *mut *const TSFieldMapEntry,
) {
    let field_map = language_field_map_slice(self_, production_id);
    if field_map.is_empty() {
        *start = ptr::null();
        *end = ptr::null();
    } else {
        *start = field_map.as_ptr();
        *end = field_map.as_ptr().add(field_map.len());
    }
}

/// Get all aliases for a symbol.
#[inline]
pub unsafe fn language_aliases_for_symbol(
    self_: *const TSLanguage,
    original_symbol: TSSymbol,
    start: *mut *const TSSymbol,
    end: *mut *const TSSymbol,
) {
    let l = lang(self_);
    *start = l.public_symbol_map.add(original_symbol as usize);
    *end = (*start).add(1);

    let mut idx: usize = 0;
    loop {
        let symbol = *l.alias_map.add(idx);
        idx += 1;
        if symbol == 0 || symbol > original_symbol {
            break;
        }
        let count = *l.alias_map.add(idx);
        idx += 1;
        if symbol == original_symbol {
            *start = l.alias_map.add(idx).cast::<TSSymbol>();
            *end = l.alias_map.add(idx + count as usize).cast::<TSSymbol>();
            break;
        }
        idx += count as usize;
    }
}

/// Write a symbol name with escaping to a FILE*.
#[inline]
pub unsafe fn language_write_symbol_as_dot_string(
    self_: *const TSLanguage,
    f: *mut c_void,
    symbol: TSSymbol,
) {
    let name = ts_language_symbol_name(self_, symbol);
    let mut chr = name;
    while *chr != 0 {
        match *chr as u8 {
            b'"' | b'\\' => {
                fputc(i32::from(b'\\'), f);
                fputc(i32::from(*chr), f);
            }
            b'\n' => {
                fputs(c"\\n".as_ptr().cast::<i8>(), f);
            }
            b'\t' => {
                fputs(c"\\t".as_ptr().cast::<i8>(), f);
            }
            _ => {
                fputc(i32::from(*chr), f);
            }
        }
        chr = chr.add(1);
    }
}

// ===========================================================================
// Exported language functions
// ===========================================================================

#[no_mangle]
pub const unsafe extern "C" fn ts_language_symbol_count(self_: *const TSLanguage) -> u32 {
    let l = lang(self_);
    l.symbol_count + l.alias_count
}

#[no_mangle]
pub const unsafe extern "C" fn ts_language_state_count(self_: *const TSLanguage) -> u32 {
    lang(self_).state_count
}

/// Raw `token_count` table field (terminal symbols come before this index).
/// Distinct from any public symbol count; used by query analysis.
pub const unsafe fn language_token_count(self_: *const TSLanguage) -> u32 {
    lang(self_).token_count
}

/// Raw `symbol_count` table field (excludes aliases, unlike the public
/// `ts_language_symbol_count`). Used by query analysis.
pub const unsafe fn language_symbol_count(self_: *const TSLanguage) -> u32 {
    lang(self_).symbol_count
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_supertypes(
    self_: *const TSLanguage,
    length: *mut u32,
) -> *const TSSymbol {
    let l = lang(self_);
    if l.abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
        *length = l.supertype_count;
        l.supertype_symbols
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
    if l.abi_version < LANGUAGE_VERSION_WITH_RESERVED_WORDS
        || !ts_language_symbol_metadata(self_, supertype).supertype
    {
        *length = 0;
        return ptr::null();
    }
    let slice = *l.supertype_map_slices.add(supertype as usize);
    *length = u32::from(slice.length);
    l.supertype_map_entries.add(slice.index as usize)
}

#[no_mangle]
pub const unsafe extern "C" fn ts_language_abi_version(self_: *const TSLanguage) -> u32 {
    lang(self_).abi_version
}

#[no_mangle]
pub const unsafe extern "C" fn ts_language_metadata(
    self_: *const TSLanguage,
) -> *const TSLanguageMetadata {
    let l = lang(self_);
    if l.abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
        ptr::addr_of!(l.metadata)
    } else {
        ptr::null()
    }
}

#[no_mangle]
pub const unsafe extern "C" fn ts_language_name(self_: *const TSLanguage) -> *const i8 {
    let l = lang(self_);
    if l.abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
        l.name
    } else {
        ptr::null()
    }
}

#[no_mangle]
pub const unsafe extern "C" fn ts_language_field_count(self_: *const TSLanguage) -> u32 {
    lang(self_).field_count
}

pub unsafe fn language_table_entry(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
    result: &mut TableEntry,
) {
    let l = lang(self_);
    if symbol == TS_BUILTIN_SYM_ERROR || symbol == TS_BUILTIN_SYM_ERROR_REPEAT {
        result.action_count = 0;
        result.is_reusable = false;
        result.actions = ptr::null();
    } else {
        let language = l;
        debug_assert!(u32::from(symbol) < language.token_count);
        let action_index = language_lookup(self_, state, symbol) as usize;
        let entry = parse_action_entry(language, action_index);
        result.action_count = u32::from(entry.entry.count);
        result.is_reusable = entry.entry.reusable;
        result.actions = parse_action_at(language, action_index + 1);
    }
}

pub const unsafe fn language_lex_mode_for_state(
    self_: *const TSLanguage,
    state: TSStateId,
) -> TSLexerMode {
    let l = lang(self_);
    if l.abi_version < 15 {
        let mode = *l.lex_modes.cast::<TSLexMode>().add(state as usize);
        TSLexerMode {
            lex_state: mode.lex_state,
            external_lex_state: mode.external_lex_state,
            reserved_word_set_id: 0,
        }
    } else {
        *l.lex_modes.add(state as usize)
    }
}

pub unsafe fn language_is_reserved_word(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> bool {
    let l = lang(self_);
    let lex_mode = language_lex_mode_for_state(self_, state);
    if lex_mode.reserved_word_set_id > 0 {
        let start =
            u32::from(lex_mode.reserved_word_set_id) * u32::from(l.max_reserved_word_set_size);
        let end = start + u32::from(l.max_reserved_word_set_size);
        for i in start..end {
            let w = *l.reserved_words.add(i as usize);
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
pub const unsafe extern "C" fn ts_language_symbol_metadata(
    self_: *const TSLanguage,
    symbol: TSSymbol,
) -> TSSymbolMetadata {
    if symbol == TS_BUILTIN_SYM_ERROR {
        TSSymbolMetadata {
            visible: true,
            named: true,
            supertype: false,
        }
    } else if symbol == TS_BUILTIN_SYM_ERROR_REPEAT {
        TSSymbolMetadata {
            visible: false,
            named: false,
            supertype: false,
        }
    } else {
        *lang(self_).symbol_metadata.add(symbol as usize)
    }
}

pub const unsafe fn language_public_symbol(self_: *const TSLanguage, symbol: TSSymbol) -> TSSymbol {
    if symbol == TS_BUILTIN_SYM_ERROR {
        symbol
    } else {
        *lang(self_).public_symbol_map.add(symbol as usize)
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_next_state(
    self_: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> TSStateId {
    let l = lang(self_);
    if symbol == TS_BUILTIN_SYM_ERROR || symbol == TS_BUILTIN_SYM_ERROR_REPEAT {
        0
    } else if u32::from(symbol) < l.token_count {
        let mut count: u32 = 0;
        let actions = language_actions(self_, state, symbol, &mut count);
        if count > 0 {
            let action = *actions.add(count as usize - 1);
            if action.type_ == TSPARSE_ACTION_TYPE_SHIFT {
                return if action.shift.extra {
                    state
                } else {
                    action.shift.state
                };
            }
        }
        0
    } else {
        language_lookup(self_, state, symbol)
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_symbol_name(
    self_: *const TSLanguage,
    symbol: TSSymbol,
) -> *const i8 {
    if symbol == TS_BUILTIN_SYM_ERROR {
        c"ERROR".as_ptr().cast::<i8>()
    } else if symbol == TS_BUILTIN_SYM_ERROR_REPEAT {
        c"_ERROR".as_ptr().cast::<i8>()
    } else if u32::from(symbol) < ts_language_symbol_count(self_) {
        *lang(self_).symbol_names.add(symbol as usize)
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
    if is_named
        && c_string_prefix_cmp(string, c"ERROR".as_ptr().cast::<i8>(), length as usize).is_eq()
    {
        return TS_BUILTIN_SYM_ERROR;
    }
    let count = ts_language_symbol_count(self_) as u16;
    let l = lang(self_);
    for i in 0..count {
        let metadata = ts_language_symbol_metadata(self_, i);
        if (!metadata.visible && !metadata.supertype) || metadata.named != is_named {
            continue;
        }
        let symbol_name = *l.symbol_names.add(i as usize);
        if c_string_prefix_cmp(symbol_name, string, length as usize).is_eq()
            && *symbol_name.add(length as usize) == 0
        {
            return *l.public_symbol_map.add(i as usize);
        }
    }
    0
}

#[no_mangle]
pub const unsafe extern "C" fn ts_language_symbol_type(
    self_: *const TSLanguage,
    symbol: TSSymbol,
) -> TSSymbolType {
    let metadata = ts_language_symbol_metadata(self_, symbol);
    if metadata.named && metadata.visible {
        TS_SYMBOL_TYPE_REGULAR
    } else if metadata.visible {
        TS_SYMBOL_TYPE_ANONYMOUS
    } else if metadata.supertype {
        TS_SYMBOL_TYPE_SUPERTYPE
    } else {
        TS_SYMBOL_TYPE_AUXILIARY
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_language_field_name_for_id(
    self_: *const TSLanguage,
    id: TSFieldId,
) -> *const i8 {
    let count = ts_language_field_count(self_);
    if count > 0 && u32::from(id) <= count {
        *lang(self_).field_names.add(id as usize)
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
        let field_name = *l.field_names.add(i as usize);
        match c_string_prefix_cmp(name, field_name, name_length as usize) {
            core::cmp::Ordering::Equal if *field_name.add(name_length as usize) == 0 => return i,
            core::cmp::Ordering::Less => return 0,
            _ => {}
        }
    }
    0
}
