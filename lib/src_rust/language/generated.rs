//! C layouts emitted by generated parsers.
//!
//! These types intentionally preserve `parser.h` layout. In particular, the
//! parse-action unions are required because generated language tables contain
//! their C representations directly.

use core::ffi::c_void;

use crate::ffi::{TSStateId, TSSymbol};

use super::super::subtree::TSSymbolMetadata;
pub use super::super::subtree::{TSFieldMapEntry, TSMapSlice};

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

#[repr(C)]
pub struct TSLexer {
    pub lookahead: i32,
    pub result_symbol: TSSymbol,
    pub advance: Option<unsafe extern "C-unwind" fn(*mut Self, bool)>,
    pub mark_end: Option<unsafe extern "C" fn(*mut Self)>,
    pub get_column: Option<unsafe extern "C" fn(*mut Self) -> u32>,
    pub is_at_included_range_start: Option<unsafe extern "C" fn(*const Self) -> bool>,
    pub eof: Option<unsafe extern "C" fn(*const Self) -> bool>,
    pub log: Option<unsafe extern "C-unwind" fn(*const Self, *const i8, ...)>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLanguageMetadata {
    pub major_version: u8,
    pub minor_version: u8,
    pub patch_version: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLexMode {
    pub lex_state: u16,
    pub external_lex_state: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLexerMode {
    pub lex_state: u16,
    pub external_lex_state: u16,
    pub reserved_word_set_id: u16,
}

pub const TSPARSE_ACTION_TYPE_SHIFT: u8 = 0;
pub const TSPARSE_ACTION_TYPE_REDUCE: u8 = 1;
pub const TSPARSE_ACTION_TYPE_ACCEPT: u8 = 2;
pub const TSPARSE_ACTION_TYPE_RECOVER: u8 = 3;

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

const _: () = assert!(core::mem::size_of::<TSLexMode>() == 4);
const _: () = assert!(core::mem::size_of::<TSLexerMode>() == 6);
const _: () = assert!(core::mem::size_of::<TSParseActionReduce>() == 8);
const _: () = assert!(core::mem::size_of::<TSParseActionShift>() == 6);
const _: () = assert!(core::mem::size_of::<TSParseAction>() == 8);
const _: () = assert!(core::mem::size_of::<TSParseActionEntryData>() == 2);
const _: () = assert!(core::mem::size_of::<TSParseActionEntry>() == 8);
const _: () = assert!(core::mem::size_of::<TSLanguageMetadata>() == 3);
const _: () = assert!(core::mem::size_of::<TSMapSlice>() == 4);
