//! Rust replacement for language.c/h — Language metadata and parse table access.
//!
//! This module provides:
//! - `TableEntry` / `LookaheadIterator` internal types (from language.h)
//! - Exported functions that access `TSLanguage` fields (from language.c)
//! - Static-inline helper functions re-implemented from language.h
//!
//! `TSLanguage` itself is defined in parser.h and created by generated parsers.
//! We access it as an opaque `repr(C)` struct via raw pointers.

use core::ffi::c_void;
use core::ptr;

use crate::ffi::{TSFieldId, TSLanguage, TSStateId, TSSymbol};

// Re-use types already defined in subtree.rs
use super::alloc::{free, malloc};
use super::subtree::TSSymbolMetadata;
use super::utils::ptr_mut;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const LANGUAGE_VERSION_WITH_RESERVED_WORDS: u32 = 15;
pub const LANGUAGE_VERSION_WITH_PRIMARY_STATES: u32 = 14;

const TS_BUILTIN_SYM_ERROR: TSSymbol = u16::MAX;
const TS_BUILTIN_SYM_ERROR_REPEAT: TSSymbol = TS_BUILTIN_SYM_ERROR - 1;

pub type TSSymbolType = u32;
pub const TS_SYMBOL_TYPE_REGULAR: TSSymbolType = 0;
pub const TS_SYMBOL_TYPE_ANONYMOUS: TSSymbolType = 1;
pub const TS_SYMBOL_TYPE_SUPERTYPE: TSSymbolType = 2;

pub const TS_SYMBOL_TYPE_AUXILIARY: TSSymbolType = 3;

// ---------------------------------------------------------------------------
// TSLanguage field access
// ---------------------------------------------------------------------------
//
// TSLanguage is defined in parser.h (C) and generated parsers emit it.
// We must read its fields at known offsets. We define a full repr(C) mirror
// struct here so we can cast `*const TSLanguage` → `TSLanguageFull`
// and access every field.
//
// This replaces the earlier partial language-layout mirrors.
// ---------------------------------------------------------------------------

/// Mirrors the `external_scanner` sub-struct inside `TSLanguage`.
#[repr(C)]
pub struct TSExternalScanner {
    /// Per-external-lex-state bitmap of valid external tokens.
    pub states: *const bool,
    /// Maps external scanner result indices to grammar symbols.
    pub symbol_map: *const TSSymbol,
    /// Optional scanner instance constructor.
    pub create: Option<unsafe extern "C" fn() -> *mut c_void>,
    /// Optional scanner instance destructor.
    pub destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Main external scanning callback.
    pub scan: Option<unsafe extern "C" fn(*mut c_void, *mut TSLexer, *const bool) -> bool>,
    /// Serialize scanner state into the parser's fixed buffer.
    pub serialize: Option<unsafe extern "C" fn(*mut c_void, *mut i8) -> u32>,
    /// Restore scanner state before scanning a stack version.
    pub deserialize: Option<unsafe extern "C" fn(*mut c_void, *const i8, u32)>,
}

/// Generated-lexer callback surface from `parser.h`.
///
/// Generated lexers and external scanners mutate this struct while scanning a
/// token. The first fields carry token state; the function pointers route back
/// into `lexer.rs` for input movement, column calculation, EOF checks, and
/// optional logging.
#[repr(C)]
pub struct TSLexer {
    /// Current decoded codepoint, or zero at EOF.
    pub lookahead: i32,
    /// Symbol selected by the generated lexer or external scanner.
    pub result_symbol: TSSymbol,
    /// Consume or skip the current lookahead. `C-unwind` so a throwing host
    /// logger invoked during advance can unwind out instead of aborting.
    pub advance: Option<unsafe extern "C-unwind" fn(*mut Self, bool)>,
    /// Mark the current source position as the end of the token.
    pub mark_end: Option<unsafe extern "C" fn(*mut Self)>,
    /// Compute the current column, potentially by rescanning the line.
    pub get_column: Option<unsafe extern "C" fn(*mut Self) -> u32>,
    /// Report whether the lexer is at an included-range boundary.
    pub is_at_included_range_start: Option<unsafe extern "C" fn(*const Self) -> bool>,
    /// Report whether the visible input is exhausted.
    pub eof: Option<unsafe extern "C" fn(*const Self) -> bool>,
    /// Optional variadic logger. `C-unwind` so a throwing host logger can
    /// unwind out of the parse instead of aborting at this boundary.
    pub log: Option<unsafe extern "C-unwind" fn(*const Self, *const i8, ...)>,
}

/// `TSLanguageMetadata` (from parser.h)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLanguageMetadata {
    pub major_version: u8,
    pub minor_version: u8,
    pub patch_version: u8,
}

/// `TSLexMode` (older ABI < 15, without `reserved_word_set_id`)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLexMode {
    pub lex_state: u16,
    pub external_lex_state: u16,
}

/// `TSLexerMode` (ABI >= 15, with `reserved_word_set_id`)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLexerMode {
    pub lex_state: u16,
    pub external_lex_state: u16,
    pub reserved_word_set_id: u16,
}

/// `TSParseActionType` enum
pub const TSPARSE_ACTION_TYPE_SHIFT: u8 = 0;
pub const TSPARSE_ACTION_TYPE_REDUCE: u8 = 1;
pub const TSPARSE_ACTION_TYPE_ACCEPT: u8 = 2;
pub const TSPARSE_ACTION_TYPE_RECOVER: u8 = 3;

/// `TSParseAction` — a union in C. We use repr(C) with manual field access.
/// The C union has:
///   shift: { type: u8, state: u16, extra: bool, repetition: bool }
///   reduce: { type: u8, `child_count`: u8, symbol: u16, `dynamic_precedence`: i16, `production_id`: u16 }
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

/// `TSParseActionEntry` — a union in C:
///   action: `TSParseAction`
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

/// `TSMapSlice` (from parser.h, also defined in subtree.rs — re-import from there)
pub use super::subtree::TSMapSlice;

/// `TSFieldMapEntry` (from parser.h, also defined in subtree.rs)
pub use super::subtree::TSFieldMapEntry;

/// Full repr(C) mirror of the `TSLanguage` struct from parser.h.
/// Used to read fields at correct offsets via pointer cast.
#[repr(C)]
pub struct TSLanguageFull {
    /// ABI version used to interpret optional/legacy fields.
    pub abi_version: u32,
    /// Total number of symbols in the grammar.
    pub symbol_count: u32,
    /// Number of alias symbols.
    pub alias_count: u32,
    /// Number of terminal symbols.
    pub token_count: u32,
    /// Number of external scanner tokens.
    pub external_token_count: u32,
    /// Number of parse states.
    pub state_count: u32,
    /// States below this count use the dense parse table.
    pub large_state_count: u32,
    /// Number of productions with field/alias metadata.
    pub production_id_count: u32,
    /// Number of named fields.
    pub field_count: u32,
    /// Maximum alias sequence length for any production.
    pub max_alias_sequence_length: u16,
    /// Dense large-state parse table.
    pub parse_table: *const u16,
    /// Compressed small-state parse table entries.
    pub small_parse_table: *const u16,
    /// Offset/count map into `small_parse_table` per small state.
    pub small_parse_table_map: *const u32,
    /// Shared parse action lists addressed by table values.
    pub parse_actions: *const TSParseActionEntry,
    /// Null-terminated symbol names.
    pub symbol_names: *const *const i8,
    /// Null-terminated field names.
    pub field_names: *const *const i8,
    /// Production-to-field-map slice table.
    pub field_map_slices: *const TSMapSlice,
    /// Field map entries for named children.
    pub field_map_entries: *const TSFieldMapEntry,
    /// Visibility/name/supertype metadata per symbol.
    pub symbol_metadata: *const TSSymbolMetadata,
    /// Map from internal to public symbol ids.
    pub public_symbol_map: *const TSSymbol,
    /// Production alias-map slice table.
    pub alias_map: *const u16,
    /// Alias symbols for production children.
    pub alias_sequences: *const TSSymbol,
    /// Lex state metadata per parse state.
    pub lex_modes: *const TSLexerMode,
    /// Generated main lexer callback.
    pub lex_fn: Option<unsafe extern "C" fn(*mut TSLexer, TSStateId) -> bool>,
    /// Generated keyword lexer callback.
    pub keyword_lex_fn: Option<unsafe extern "C" fn(*mut TSLexer, TSStateId) -> bool>,
    /// Default word token that keyword lexing can refine.
    pub keyword_capture_token: TSSymbol,
    /// Optional external scanner descriptor.
    pub external_scanner: TSExternalScanner,
    /// Primary states for public node state APIs.
    pub primary_state_ids: *const TSStateId,
    /// Language name.
    pub name: *const i8,
    /// Reserved-word table for ABI >= 15.
    pub reserved_words: *const TSSymbol,
    /// Width of each reserved-word set.
    pub max_reserved_word_set_size: u16,
    /// Number of supertype symbols.
    pub supertype_count: u32,
    /// Supertype symbol list.
    pub supertype_symbols: *const TSSymbol,
    /// Supertype-to-subtype slice table.
    pub supertype_map_slices: *const TSMapSlice,
    /// Supertype subtype entries.
    pub supertype_map_entries: *const TSSymbol,
    /// Language semantic version metadata.
    pub metadata: TSLanguageMetadata,
}

// ---------------------------------------------------------------------------
// Internal types from language.h
// ---------------------------------------------------------------------------

/// Result of looking up a parse table entry for a (state, symbol) pair.
#[repr(C)]
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

/// Iterator over valid lookahead symbols for a given parse state.
#[repr(C)]
pub struct LookaheadIterator {
    /// Language whose tables are being scanned.
    pub language: *const TSLanguage,
    /// Current raw table cursor.
    pub data: *const u16,
    /// End of the current small-state symbol group.
    pub group_end: *const u16,
    /// Parse state being inspected.
    pub state: TSStateId,
    /// Parse-table value for the current symbol.
    pub table_value: u16,
    /// Section index in the small parse table.
    pub section_index: u16,
    /// Remaining grouped-symbol count in the current section.
    pub group_count: u16,
    /// Whether this iterator is scanning small-state data.
    pub is_small_state: bool,

    /// Current symbol's action list.
    pub actions: *const TSParseAction,
    /// Current lookahead symbol.
    pub symbol: TSSymbol,
    /// Shift/goto state for current symbol when applicable.
    pub next_state: TSStateId,
    /// Number of current actions.
    pub action_count: u16,
}

// ---------------------------------------------------------------------------
// Compile-time layout assertions
// ---------------------------------------------------------------------------

const _: () = assert!(core::mem::size_of::<TSLexMode>() == 4);
const _: () = assert!(core::mem::size_of::<TSLexerMode>() == 6);
const _: () = assert!(core::mem::size_of::<TSParseActionReduce>() == 8);
const _: () = assert!(core::mem::size_of::<TSParseActionShift>() == 6);
const _: () = assert!(core::mem::size_of::<TSParseAction>() == 8);
const _: () = assert!(core::mem::size_of::<TSParseActionEntryData>() == 2);
const _: () = assert!(core::mem::size_of::<TSParseActionEntry>() == 8);
const _: () = assert!(core::mem::size_of::<TSLanguageMetadata>() == 3);
const _: () = assert!(core::mem::size_of::<TSMapSlice>() == 4);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<TableEntry>() == 16);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<LookaheadIterator>() == 56);

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
// Static inline re-implementations from language.h
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

/// Create a lookahead iterator for a given state.
#[inline]
pub unsafe fn language_lookaheads(self_: *const TSLanguage, state: TSStateId) -> LookaheadIterator {
    let l = lang(self_);
    let is_small_state = u32::from(state) >= l.large_state_count;
    let (data, group_end, group_count): (*const u16, *const u16, u16) = if is_small_state {
        let index = *l
            .small_parse_table_map
            .add(state as usize - l.large_state_count as usize);
        let data = l.small_parse_table.add(index as usize);
        (data, data.add(1), *data)
    } else {
        (
            l.parse_table
                .add(state as usize * l.symbol_count as usize)
                .sub(1),
            ptr::null(),
            0,
        )
    };
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
pub unsafe fn lookahead_iterator_next(self_: &mut LookaheadIterator) -> bool {
    let l = lang(self_.language);

    if self_.is_small_state {
        self_.data = self_.data.add(1);
        if self_.data == self_.group_end {
            if self_.group_count == 0 {
                return false;
            }
            self_.group_count -= 1;
            self_.table_value = *self_.data;
            self_.data = self_.data.add(1);
            let symbol_count = *self_.data;
            self_.data = self_.data.add(1);
            self_.group_end = self_.data.add(symbol_count as usize);
            self_.symbol = *self_.data;
        } else {
            self_.symbol = *self_.data;
            return true;
        }
    } else {
        loop {
            self_.data = self_.data.add(1);
            self_.symbol = self_.symbol.wrapping_add(1);
            if self_.symbol >= l.symbol_count as u16 {
                return false;
            }
            self_.table_value = *self_.data;
            if self_.table_value != 0 {
                break;
            }
        }
    }

    // Depending on if the symbol is terminal or non-terminal, the table value
    // either represents a list of actions or a successor state.
    let language = l;
    if u32::from(self_.symbol) < language.token_count {
        let entry = parse_action_entry(language, self_.table_value as usize);
        self_.action_count = u16::from(entry.entry.count);
        self_.actions = parse_action_at(language, self_.table_value as usize + 1);
        self_.next_state = 0;
    } else {
        self_.action_count = 0;
        self_.next_state = self_.table_value;
    }
    true
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

/// Get the alias sequence for a production ID.
#[inline]
pub const unsafe fn language_alias_sequence(
    self_: *const TSLanguage,
    production_id: u32,
) -> *const TSSymbol {
    if production_id != 0 {
        let l = lang(self_);
        l.alias_sequences
            .add(production_id as usize * l.max_alias_sequence_length as usize)
    } else {
        ptr::null()
    }
}

/// Get the alias at a specific position in a production's alias sequence.
#[inline]
pub const unsafe fn language_alias_at(
    self_: *const TSLanguage,
    production_id: u32,
    child_index: u32,
) -> TSSymbol {
    if production_id != 0 {
        let l = lang(self_);
        *l.alias_sequences.add(
            production_id as usize * l.max_alias_sequence_length as usize + child_index as usize,
        )
    } else {
        0
    }
}

/// Get the field map (start, end) for a production ID.
#[inline]
pub unsafe fn language_field_map(
    self_: *const TSLanguage,
    production_id: u32,
    start: *mut *const TSFieldMapEntry,
    end: *mut *const TSFieldMapEntry,
) {
    let l = lang(self_);
    if l.field_count == 0 {
        *start = ptr::null();
        *end = ptr::null();
        return;
    }
    let slice = *l.field_map_slices.add(production_id as usize);
    *start = l.field_map_entries.add(slice.index as usize);
    *end = l
        .field_map_entries
        .add(slice.index as usize + slice.length as usize);
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
// Exported functions from language.c
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

// ---------------------------------------------------------------------------
// Lookahead iterator public API
// ---------------------------------------------------------------------------

/// `TSLookaheadIterator` is an opaque handle = `LookaheadIterator` allocated on heap.
#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_new(
    self_: *const TSLanguage,
    state: TSStateId,
) -> *mut LookaheadIterator {
    if u32::from(state) >= lang(self_).state_count {
        return ptr::null_mut();
    }
    let iterator = malloc(core::mem::size_of::<LookaheadIterator>()).cast::<LookaheadIterator>();
    ptr::write(iterator, language_lookaheads(self_, state));
    iterator
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_delete(self_: *mut LookaheadIterator) {
    free(self_.cast::<c_void>());
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_reset_state(
    self_: *mut LookaheadIterator,
    state: TSStateId,
) -> bool {
    if u32::from(state) >= lang((*self_).language).state_count {
        return false;
    }
    *self_ = language_lookaheads((*self_).language, state);
    true
}

#[no_mangle]
pub const unsafe extern "C" fn ts_lookahead_iterator_language(
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
    if u32::from(state) >= lang(language).state_count {
        return false;
    }
    *self_ = language_lookaheads(language, state);
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_next(self_: *mut LookaheadIterator) -> bool {
    lookahead_iterator_next(ptr_mut(self_))
}

#[no_mangle]
pub const unsafe extern "C" fn ts_lookahead_iterator_current_symbol(
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
