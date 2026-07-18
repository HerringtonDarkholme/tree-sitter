//! Human-readable and DOT rendering for subtree hierarchies.
//!
//! The S-expression writer applies language visibility, alias, and field
//! metadata so its output resembles the public syntax tree. The DOT writer
//! exposes the full internal hierarchy for parser diagnostics. Both are kept
//! separate from storage and mutation because they depend on C-style output
//! callbacks and language display metadata.

use core::ffi::c_void;
use core::ptr;

use crate::ffi::{TSLanguage, TSSymbol};

use super::super::alloc::malloc;
use super::super::language::{
    language_alias_sequence_slice, language_field_map_slice, language_full,
    language_write_symbol_as_dot_string, ts_language_symbol_metadata, ts_language_symbol_name,
};
use super::handle::Subtree;
use super::SubtreeArena;

// Subtree string and debug output

extern "C" {
    fn snprintf(s: *mut i8, n: usize, format: *const i8, ...) -> i32;
    fn fprintf(f: *mut c_void, format: *const i8, ...) -> i32;
}

static ROOT_FIELD: &[u8; 9] = b"__ROOT__\0";

unsafe fn subtree_write_char_to_string(s: *mut i8, n: usize, chr: i32) -> usize {
    if chr == -1 {
        snprintf(s, n, c"INVALID".as_ptr().cast::<i8>()) as usize
    } else if chr == 0 {
        snprintf(s, n, c"'\\0'".as_ptr().cast::<i8>()) as usize
    } else if chr == i32::from(b'\n') {
        snprintf(s, n, c"'\\n'".as_ptr().cast::<i8>()) as usize
    } else if chr == i32::from(b'\t') {
        snprintf(s, n, c"'\\t'".as_ptr().cast::<i8>()) as usize
    } else if chr == i32::from(b'\r') {
        snprintf(s, n, c"'\\r'".as_ptr().cast::<i8>()) as usize
    } else if (0x20..0x7F).contains(&chr) {
        snprintf(s, n, c"'%c'".as_ptr().cast::<i8>(), chr) as usize
    } else {
        snprintf(s, n, c"%d".as_ptr().cast::<i8>(), chr) as usize
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn subtree_write_to_string(
    self_: Subtree,
    arena: *mut SubtreeArena,
    string: *mut i8,
    limit: usize,
    language: *const TSLanguage,
    include_all: bool,
    alias_symbol: TSSymbol,
    alias_is_named: bool,
    field_name: *const i8,
) -> usize {
    if self_.is_null() {
        return snprintf(string, limit, c"(NULL)".as_ptr().cast::<i8>()) as usize;
    }

    let mut cursor = string;
    let mut string_measuring = string;
    let writer: *mut *mut i8 = if limit > 1 {
        &mut cursor
    } else {
        &mut string_measuring
    };
    let is_root = field_name == ROOT_FIELD.as_ptr().cast::<i8>();
    let is_visible = include_all
        || self_.missing(arena)
        || (if alias_symbol != 0 {
            alias_is_named
        } else {
            self_.visible(arena) && self_.named(arena)
        });

    if is_visible {
        if !is_root {
            cursor = cursor.add(snprintf(*writer, limit, c" ".as_ptr().cast::<i8>()) as usize);
            if !field_name.is_null() {
                cursor =
                    cursor.add(
                        snprintf(*writer, limit, c"%s: ".as_ptr().cast::<i8>(), field_name)
                            as usize,
                    );
            }
        }

        if self_.is_error(arena)
            && self_.child_count(arena) == 0
            && self_.heap_data(arena).size.bytes > 0
        {
            cursor = cursor
                .add(snprintf(*writer, limit, c"(UNEXPECTED ".as_ptr().cast::<i8>()) as usize);
            cursor = cursor.add(subtree_write_char_to_string(
                *writer,
                limit,
                self_.heap_data(arena).lookahead_char(),
            ));
        } else {
            let symbol = if alias_symbol != 0 {
                alias_symbol
            } else {
                self_.symbol(arena)
            };
            let symbol_name = ts_language_symbol_name(language, symbol);
            if self_.missing(arena) {
                cursor = cursor
                    .add(snprintf(*writer, limit, c"(MISSING ".as_ptr().cast::<i8>()) as usize);
                if alias_is_named || self_.named(arena) {
                    cursor = cursor.add(snprintf(
                        *writer,
                        limit,
                        c"%s".as_ptr().cast::<i8>(),
                        symbol_name,
                    ) as usize);
                } else {
                    cursor = cursor.add(snprintf(
                        *writer,
                        limit,
                        c"\"%s\"".as_ptr().cast::<i8>(),
                        symbol_name,
                    ) as usize);
                }
            } else {
                cursor =
                    cursor.add(
                        snprintf(*writer, limit, c"(%s".as_ptr().cast::<i8>(), symbol_name)
                            as usize,
                    );
            }
        }
    } else if is_root {
        let symbol = if alias_symbol != 0 {
            alias_symbol
        } else {
            self_.symbol(arena)
        };
        let symbol_name = ts_language_symbol_name(language, symbol);
        if self_.child_count(arena) > 0 {
            cursor = cursor
                .add(snprintf(*writer, limit, c"(%s".as_ptr().cast::<i8>(), symbol_name) as usize);
        } else if self_.named(arena) {
            cursor = cursor
                .add(snprintf(*writer, limit, c"(%s)".as_ptr().cast::<i8>(), symbol_name) as usize);
        } else {
            cursor = cursor.add(snprintf(
                *writer,
                limit,
                c"(\"%s\")".as_ptr().cast::<i8>(),
                symbol_name,
            ) as usize);
        }
    }

    if self_.child_count(arena) > 0 {
        let alias_sequence = language_alias_sequence_slice(
            language,
            u32::from(self_.heap_data(arena).children().production_id),
        );
        let field_map = language_field_map_slice(
            language,
            u32::from(self_.heap_data(arena).children().production_id),
        );

        let mut structural_child_index: u32 = 0;
        for child in self_.children(arena) {
            let child = *child;
            if child.extra(arena) {
                cursor = cursor.add(subtree_write_to_string(
                    child,
                    arena,
                    *writer,
                    limit,
                    language,
                    include_all,
                    0,
                    false,
                    ptr::null(),
                ));
            } else {
                let subtree_alias_symbol = alias_sequence
                    .get(structural_child_index as usize)
                    .copied()
                    .unwrap_or(0);
                let subtree_alias_is_named = if subtree_alias_symbol != 0 {
                    ts_language_symbol_metadata(language, subtree_alias_symbol).named
                } else {
                    false
                };

                let mut child_field_name: *const i8 =
                    if is_visible { ptr::null() } else { field_name };
                for map in field_map {
                    if !map.inherited && map.child_index == structural_child_index as u8 {
                        let lang = language_full(language);
                        child_field_name = *lang.field_names.add(map.field_id as usize);
                        break;
                    }
                }

                cursor = cursor.add(subtree_write_to_string(
                    child,
                    arena,
                    *writer,
                    limit,
                    language,
                    include_all,
                    subtree_alias_symbol,
                    subtree_alias_is_named,
                    child_field_name,
                ));
                structural_child_index += 1;
            }
        }
    }

    if is_visible {
        cursor = cursor.add(snprintf(*writer, limit, c")".as_ptr().cast::<i8>()) as usize);
    }

    cursor as usize - string as usize
}

pub unsafe fn subtree_string(
    self_: Subtree,
    arena: *mut SubtreeArena,
    alias_symbol: TSSymbol,
    alias_is_named: bool,
    language: *const TSLanguage,
    include_all: bool,
) -> *mut i8 {
    let mut scratch_string: [i8; 1] = [0];
    let size = subtree_write_to_string(
        self_,
        arena,
        scratch_string.as_mut_ptr(),
        1,
        language,
        include_all,
        alias_symbol,
        alias_is_named,
        ROOT_FIELD.as_ptr().cast::<i8>(),
    ) + 1;
    let result = malloc(size).cast::<i8>();
    subtree_write_to_string(
        self_,
        arena,
        result,
        size,
        language,
        include_all,
        alias_symbol,
        alias_is_named,
        ROOT_FIELD.as_ptr().cast::<i8>(),
    );
    result
}

unsafe fn subtree_print_dot_graph_recursive(
    self_: *const Subtree,
    arena: *mut SubtreeArena,
    start_offset: u32,
    language: *const TSLanguage,
    alias_symbol: TSSymbol,
    f: *mut c_void,
) {
    let tree = *self_;
    let subtree_symbol = tree.symbol(arena);
    let symbol = if alias_symbol != 0 {
        alias_symbol
    } else {
        subtree_symbol
    };
    let end_offset = start_offset + tree.total_bytes(arena);
    fprintf(
        f,
        c"tree_%p [label=\"".as_ptr().cast::<i8>(),
        self_.cast::<c_void>(),
    );
    language_write_symbol_as_dot_string(language, f, symbol);
    fprintf(f, c"\"".as_ptr().cast::<i8>());

    if tree.child_count(arena) == 0 {
        fprintf(f, c", shape=plaintext".as_ptr().cast::<i8>());
    }
    if tree.extra(arena) {
        fprintf(f, c", fontcolor=gray".as_ptr().cast::<i8>());
    }
    if tree.has_changes(arena) {
        fprintf(f, c", color=green, penwidth=2".as_ptr().cast::<i8>());
    }

    fprintf(
        f,
        c", tooltip=\"range: %u - %u\nstate: %d\nerror-cost: %u\nhas-changes: %u\ndepends-on-column: %u\ndescendant-count: %u\nrepeat-depth: %u\nlookahead-bytes: %u".as_ptr().cast::<i8>(),
        start_offset,
        end_offset,
        i32::from(tree.parse_state(arena)),
        tree.error_cost(arena),
        u32::from(tree.has_changes(arena)),
        u32::from(tree.depends_on_column(arena)),
        tree.visible_descendant_count(arena),
        tree.repeat_depth(arena),
        tree.lookahead_bytes(arena),
    );

    if tree.is_error(arena)
        && tree.child_count(arena) == 0
        && tree.heap_data(arena).lookahead_char() != 0
    {
        fprintf(
            f,
            c"\ncharacter: '%c'".as_ptr().cast::<i8>(),
            tree.heap_data(arena).lookahead_char(),
        );
    }

    fprintf(f, c"\"]\n".as_ptr().cast::<i8>());

    let mut child_start_offset = start_offset;
    let lang = language_full(language);
    let mut child_info_offset =
        u32::from(lang.max_alias_sequence_length) * u32::from(tree.production_id(arena));
    for (i, child) in tree.children(arena).iter().enumerate() {
        let child_ptr = ptr::from_ref(child);
        let mut subtree_alias_symbol: TSSymbol = 0;
        if !(*child).extra(arena) && child_info_offset != 0 {
            subtree_alias_symbol = *lang.alias_sequences.add(child_info_offset as usize);
            child_info_offset += 1;
        }
        subtree_print_dot_graph_recursive(
            child_ptr,
            arena,
            child_start_offset,
            language,
            subtree_alias_symbol,
            f,
        );
        fprintf(
            f,
            c"tree_%p -> tree_%p [tooltip=%u]\n".as_ptr().cast::<i8>(),
            self_.cast::<c_void>(),
            child_ptr.cast::<c_void>(),
            i,
        );
        child_start_offset += (*child).total_bytes(arena);
    }
}

pub unsafe fn subtree_print_dot_graph(
    self_: Subtree,
    arena: *mut SubtreeArena,
    language: *const TSLanguage,
    f: *mut c_void,
) {
    fprintf(f, c"digraph tree {\n".as_ptr().cast::<i8>());
    fprintf(f, c"edge [arrowhead=none]\n".as_ptr().cast::<i8>());
    subtree_print_dot_graph_recursive(core::ptr::addr_of!(self_), arena, 0, language, 0, f);
    fprintf(f, c"}\n".as_ptr().cast::<i8>());
}
