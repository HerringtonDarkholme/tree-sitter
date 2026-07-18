//! GLR parser state, lifecycle, and top-level parse loop.
//!
//! [`TSParser`] owns the mutable objects used during a parse: the lexer, the
//! graph-structured [`Stack`], subtree allocation pools, external-scanner
//! state, and the best accepted tree. [`ts_parser_parse`] is the outer driver.
//! It advances every active stack version, condenses compatible or inferior
//! versions, invokes recovery when all useful versions are paused, and finally
//! balances and returns the accepted tree.
//!
//! The action-level work is divided by purpose:
//!
//! - `lexing` obtains or reuses a lookahead token and manages external scanners;
//! - `advance` interprets parse-table entries and manages versions;
//! - `actions` executes shift, reduce, and accept stack/tree mutations;
//! - `recovery` searches for a useful continuation after invalid input;
//! - `balancing` prepares the accepted subtree for long-lived navigation; and
//! - `logging` renders parser, stack, and tree diagnostics.
//!
//! Generated languages and the public API enter through C-compatible types,
//! but parser-owned state is internal and uses Rust layout.

use core::ffi::{c_char, c_void};
use core::fmt::Write;
use core::ptr;

use crate::ffi::{
    TSInput, TSInputEncoding, TSInputEncodingUTF8, TSLanguage, TSLogger, TSParseOptions,
    TSParseState, TSPoint, TSRange, TREE_SITTER_LANGUAGE_VERSION,
    TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION,
};

use super::alloc::{free, malloc};
use super::error_costs::ERROR_COST_PER_SKIPPED_TREE;
use super::language::language_full;
use super::length::length_zero;
use super::lexer::{
    lexer_delete, lexer_included_ranges, lexer_included_ranges_slice, lexer_new, lexer_reset,
    lexer_set_included_ranges, lexer_set_input, Lexer,
};
use super::reduce_action::ReduceActionSet;
use super::stack::{stack_clear, stack_delete, stack_new, Stack, StackVersion};
use super::subtree::{
    subtree_pool_delete, subtree_pool_new, subtree_pool_prepare_for_parse,
    subtree_pool_retain_arena, subtree_publish, Subtree, SubtreeArray, SubtreePool, NULL_SUBTREE,
};
use super::tree::TSTree;
use super::utils::Array;
use super::utils::{ptr_mut, ptr_ref};

// ---------------------------------------------------------------------------
// Extern C functions
// ---------------------------------------------------------------------------

extern "C" {
    // libc
    fn fprintf(f: *mut c_void, fmt: *const i8, ...) -> i32;
    fn fputs(s: *const i8, f: *mut c_void) -> i32;
    fn fputc(c: i32, f: *mut c_void) -> i32;
    // `fdopen` is spelled `_fdopen` on Windows (declared at the call site);
    // `fclose` keeps its name on all platforms.
    #[cfg(not(target_os = "windows"))]
    fn fdopen(fd: i32, mode: *const i8) -> *mut c_void;
    fn fclose(f: *mut c_void) -> i32;
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_VERSION_COUNT: u32 = 6;
const MAX_VERSION_COUNT_OVERFLOW: u32 = 4;
const MAX_SUMMARY_DEPTH: u32 = 16;
const MAX_COST_DIFFERENCE: u32 = 18 * ERROR_COST_PER_SKIPPED_TREE;
const OP_COUNT_PER_PARSER_CALLBACK_CHECK: u32 = 100;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One-token cache shared by stack versions at the same byte offset.
///
/// GLR versions often ask the lexer for the same position and external scanner
/// state. The cache stores the concrete token plus the last external token that
/// determined scanner state, so another version can reuse it only when scanner
/// state is equivalent.
struct TokenCache {
    /// Retained lookahead token.
    token: Subtree,
    /// Retained token carrying the external scanner state used for `token`.
    last_external_token: Subtree,
    /// Byte offset where `token` was lexed.
    byte_index: u32,
}

/// Summary used to compare and prune stack versions.
#[derive(Clone, Copy)]
struct ErrorStatus {
    /// Accumulated recovery/error cost.
    cost: u32,
    /// Number of visible nodes since the last error.
    node_count: u32,
    /// Dynamic precedence for tie-breaking.
    dynamic_precedence: i32,
    /// Whether the version is currently in error recovery.
    is_in_error: bool,
}

/// `ErrorComparison`
#[derive(PartialEq, Eq)]
enum ErrorComparison {
    TakeLeft,
    PreferLeft,
    None,
    PreferRight,
    TakeRight,
}

/// `TSStringInput` — for string-based parsing
struct TSStringInput {
    string: *const c_char,
    length: u32,
}

/// Main parser runtime state.
///
/// One `TSParser` owns all mutable state for a parse: lexer callbacks, GLR
/// stack versions, parser scratch arrays, external scanner state, and the final
/// accepted tree. The public C API only observes pointers to this opaque type,
/// so its fields deliberately use Rust layout.
pub struct TSParser {
    /// Input adapter and `TSLexer` callback surface.
    lexer: Lexer,
    /// Persistent GLR parse stack.
    stack: *mut Stack,
    /// Free lists used while releasing or mutating subtrees.
    tree_pool: SubtreePool,
    /// Active language tables and callbacks.
    language: *const TSLanguage,
    /// Scratch set of reductions considered during recovery.
    reduce_actions: ReduceActionSet,
    /// Best accepted root found so far.
    finished_tree: Subtree,
    /// Scratch arrays for stripping and comparing trailing extras.
    trailing_extras: SubtreeArray,
    trailing_extras2: SubtreeArray,
    /// Scratch child array used for subtree comparisons.
    scratch_trees: SubtreeArray,
    /// Cached lexer result for repeated same-position lookups.
    token_cache: TokenCache,
    /// Language-owned external scanner payload.
    external_scanner_payload: *mut c_void,
    /// Optional parse debug graph output.
    dot_graph_file: *mut c_void,
    /// Number of accepted trees seen in this parse.
    accept_count: u32,
    /// Progress-callback operation counter.
    operation_count: u32,
    /// Public parse cancellation/progress options.
    parse_options: TSParseOptions,
    /// Mutable status passed to the progress callback.
    parse_state: TSParseState,
    /// Set when balancing was canceled by the progress callback.
    canceled_balancing: bool,
    /// Set once any accepted tree contains an error.
    has_error: bool,
}

#[inline]
fn parse_options_none() -> TSParseOptions {
    TSParseOptions {
        payload: ptr::null_mut(),
        progress_callback: None,
    }
}

#[inline]
const fn parse_state_empty() -> TSParseState {
    TSParseState {
        payload: ptr::null_mut(),
        current_byte_offset: 0,
        has_error: false,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — StringInput
// ---------------------------------------------------------------------------

unsafe extern "C" fn ts_string_input_read(
    payload: *mut c_void,
    byte: u32,
    _point: TSPoint,
    length: *mut u32,
) -> *const c_char {
    let input = ptr_ref(payload.cast::<TSStringInput>());
    if byte >= input.length {
        *length = 0;
        c"".as_ptr()
    } else {
        *length = input.length - byte;
        input.string.add(byte as usize)
    }
}

mod logging;
use logging::{parser_log, parser_log_stack, parser_log_tree};

mod advance;
use advance::{parser_advance, parser_condense_stack};

mod lexing;
use lexing::{
    parser_external_scanner_create, parser_external_scanner_destroy, parser_set_cached_token,
};

mod actions;
mod recovery;

mod balancing;
use balancing::parser_balance_subtree;

unsafe fn parser_has_outstanding_parse(self_: &TSParser) -> bool {
    self_.canceled_balancing
        || !self_.external_scanner_payload.is_null()
        || ptr_ref(self_.stack).head(0).state() != 1
        || ptr_mut(self_.stack).node_count_since_error(0) != 0
}

unsafe fn parser_take_finished_tree(self_: &mut TSParser) -> *mut TSTree {
    subtree_publish(self_.tree_pool.arena());
    let arena = subtree_pool_retain_arena(&mut self_.tree_pool);
    let result = TSTree::new(
        self_.finished_tree,
        arena,
        self_.language,
        lexer_included_ranges_slice(&self_.lexer),
    );
    self_.finished_tree = NULL_SUBTREE;
    result
}

// ---------------------------------------------------------------------------
// Exported functions — lifecycle
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_parser_new() -> *mut TSParser {
    let self_ = malloc(core::mem::size_of::<TSParser>()).cast::<TSParser>();
    ptr::write(
        self_,
        TSParser {
            lexer: lexer_new(),
            stack: ptr::null_mut(),
            tree_pool: subtree_pool_new(32),
            language: ptr::null(),
            reduce_actions: Array::new(),
            finished_tree: NULL_SUBTREE,
            trailing_extras: SubtreeArray::new(),
            trailing_extras2: SubtreeArray::new(),
            scratch_trees: SubtreeArray::new(),
            token_cache: TokenCache {
                token: NULL_SUBTREE,
                last_external_token: NULL_SUBTREE,
                byte_index: 0,
            },
            external_scanner_payload: ptr::null_mut(),
            dot_graph_file: ptr::null_mut(),
            accept_count: 0,
            operation_count: 0,
            parse_options: parse_options_none(),
            parse_state: parse_state_empty(),
            canceled_balancing: false,
            has_error: false,
        },
    );
    let parser = ptr_mut(self_);
    parser.reduce_actions.reserve(4);
    parser.stack = stack_new(&mut parser.tree_pool);
    parser_set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    self_
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_delete(self_: *mut TSParser) {
    if self_.is_null() {
        return;
    }

    ts_parser_reset(self_);
    let parser = ptr_mut(self_);
    stack_delete(ptr_mut(parser.stack));
    if !parser.reduce_actions.contents.is_null() {
        parser.reduce_actions.delete();
    }
    lexer_delete(&mut parser.lexer);
    parser_set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    subtree_pool_delete(&mut parser.tree_pool);
    parser.trailing_extras.delete();
    parser.trailing_extras2.delete();
    parser.scratch_trees.delete();
    free(self_.cast::<c_void>());
}

// ---------------------------------------------------------------------------
// Exported functions — configuration
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ts_parser_language(self_: *const TSParser) -> *const TSLanguage {
    let parser = ptr_ref(self_);
    parser.language
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_language(
    self_: *mut TSParser,
    language: *const TSLanguage,
) -> bool {
    ts_parser_reset(self_);
    let parser = ptr_mut(self_);
    parser.language = ptr::null();
    if !language.is_null() {
        let language_data = language_full(language);
        if language_data.abi_version > TREE_SITTER_LANGUAGE_VERSION
            || language_data.abi_version < TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION
        {
            return false;
        }
    }

    parser.language = language;
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_logger(self_: *const TSParser) -> TSLogger {
    let parser = ptr_ref(self_);
    ptr::read(&parser.lexer.logger)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_logger(self_: *mut TSParser, logger: TSLogger) {
    let parser = ptr_mut(self_);
    parser.lexer.logger = logger;
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_print_dot_graphs(self_: *mut TSParser, fd: i32) {
    let parser = ptr_mut(self_);
    if !parser.dot_graph_file.is_null() {
        fclose(parser.dot_graph_file);
    }

    if fd >= 0 {
        #[cfg(target_os = "windows")]
        {
            extern "C" {
                fn _fdopen(fd: i32, mode: *const i8) -> *mut c_void;
            }
            parser.dot_graph_file = _fdopen(fd, c"a".as_ptr().cast::<i8>());
        }
        #[cfg(not(target_os = "windows"))]
        {
            parser.dot_graph_file = fdopen(fd, c"a".as_ptr().cast::<i8>());
        }
    } else {
        parser.dot_graph_file = ptr::null_mut();
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_set_included_ranges(
    self_: *mut TSParser,
    ranges: *const TSRange,
    count: u32,
) -> bool {
    let parser = ptr_mut(self_);
    lexer_set_included_ranges(&mut parser.lexer, ranges, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_included_ranges(
    self_: *const TSParser,
    count: *mut u32,
) -> *const TSRange {
    let parser = ptr_ref(self_);
    lexer_included_ranges(&parser.lexer, count)
}

#[no_mangle]
pub unsafe extern "C" fn ts_parser_reset(self_: *mut TSParser) {
    let parser = ptr_mut(self_);
    parser_external_scanner_destroy(parser);

    lexer_reset(&mut parser.lexer, length_zero());
    stack_clear(ptr_mut(parser.stack));
    parser_set_cached_token(parser, 0, NULL_SUBTREE, NULL_SUBTREE);
    if !parser.finished_tree.is_null() {
        parser.finished_tree.release(&mut parser.tree_pool);
        parser.finished_tree = NULL_SUBTREE;
    }
    parser.accept_count = 0;
    parser.has_error = false;
    parser.canceled_balancing = false;
    parser.parse_options = parse_options_none();
    parser.parse_state = parse_state_empty();
}

// ---------------------------------------------------------------------------
// Exported functions — parsing
// ---------------------------------------------------------------------------

#[no_mangle]
/// Parse one input document and return a new tree.
///
/// The driver owns the outer GLR loop:
/// - initialize the lexer and external scanner;
/// - process every active stack version until none can advance normally;
/// - condense/merge/prune stack versions;
/// - recover when all versions are paused at errors;
/// - balance the accepted tree and transfer its root into `TSTree`.
///
/// Returning null means parsing was canceled. Parser-owned scratch state is
/// reset before returning unless the parse is intentionally resumable.
pub unsafe extern "C-unwind" fn ts_parser_parse(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    input: TSInput,
) -> *mut TSTree {
    let _ = old_tree;
    let parser = ptr_mut(self_);
    if parser.language.is_null() || input.read.is_none() {
        return ptr::null_mut();
    }

    lexer_set_input(&mut parser.lexer, input);
    parser.operation_count = 0;

    if parser_has_outstanding_parse(parser) {
        parser_log(parser, |_, log| log.write_str("resume_parsing"));
        if parser.canceled_balancing {
            debug_assert!(!parser.finished_tree.is_null());
            if !parser_balance_subtree(parser) {
                parser.canceled_balancing = true;
                return ptr::null_mut();
            }
            parser.canceled_balancing = false;
            parser_log(parser, |_, log| log.write_str("done"));
            parser_log_tree(parser, parser.finished_tree);

            let result = parser_take_finished_tree(parser);

            ts_parser_reset(self_);
            return result;
        }
    } else {
        subtree_pool_prepare_for_parse(&mut parser.tree_pool);
        parser_external_scanner_create(parser);
        parser_log(parser, |_, log| log.write_str("new_parse"));
    }

    let mut last_position: u32 = 0;
    let mut version_count: StackVersion;
    loop {
        let mut version: StackVersion = 0;
        loop {
            version_count = ptr_ref(parser.stack).version_count();
            if version >= version_count {
                break;
            }

            while ptr_ref(parser.stack).head(version).is_active() {
                parser_log(parser, |context, log| {
                    let stack = ptr_ref(context.stack);
                    let head = stack.head(version);
                    write!(
                        log,
                        "process version:{version}, version_count:{}, state:{}, row:{}, col:{}",
                        stack.version_count(),
                        i32::from(head.state()),
                        head.position().extent.row,
                        head.position().extent.column
                    )
                });

                if !parser_advance(parser, version) {
                    return ptr::null_mut();
                }

                parser_log_stack(parser);

                let position = ptr_ref(parser.stack).head(version).position().bytes;
                if position > last_position || (version > 0 && position == last_position) {
                    last_position = position;
                    break;
                }
            }
            version += 1;
        }

        // After advancing each version of the stack, re-sort the versions by their cost,
        // removing any versions that are no longer worth pursuing.
        let min_error_cost = parser_condense_stack(parser);

        // If there's already a finished parse tree that's better than any in-progress version,
        // then terminate parsing. Clear the parse stack to remove any extra references to subtrees
        // within the finished tree, ensuring that these subtrees can be safely mutated in-place
        // for rebalancing.
        if !parser.finished_tree.is_null()
            && parser.finished_tree.error_cost(parser.tree_pool.arena()) < min_error_cost
        {
            stack_clear(ptr_mut(parser.stack));
            break;
        }

        if version_count == 0 {
            break;
        }
    }

    // balance:
    debug_assert!(!parser.finished_tree.is_null());
    if !parser_balance_subtree(parser) {
        parser.canceled_balancing = true;
        return ptr::null_mut();
    }
    parser.canceled_balancing = false;
    parser_log(parser, |_, log| log.write_str("done"));
    parser_log_tree(parser, parser.finished_tree);

    let result = parser_take_finished_tree(parser);

    // exit:
    ts_parser_reset(self_);
    result
}

#[no_mangle]
pub unsafe extern "C-unwind" fn ts_parser_parse_with_options(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    input: TSInput,
    parse_options: TSParseOptions,
) -> *mut TSTree {
    {
        let parser = ptr_mut(self_);
        parser.parse_options = parse_options;
        parser.parse_state.payload = parse_options.payload;
    }
    let result = ts_parser_parse(self_, old_tree, input);
    // Reset parser options before further parse calls.
    let parser = ptr_mut(self_);
    parser.parse_options = parse_options_none();
    result
}

#[no_mangle]
pub unsafe extern "C-unwind" fn ts_parser_parse_string(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    string: *const i8,
    length: u32,
) -> *mut TSTree {
    ts_parser_parse_string_encoding(self_, old_tree, string, length, TSInputEncodingUTF8)
}

#[no_mangle]
pub unsafe extern "C-unwind" fn ts_parser_parse_string_encoding(
    self_: *mut TSParser,
    old_tree: *const TSTree,
    string: *const i8,
    length: u32,
    encoding: TSInputEncoding,
) -> *mut TSTree {
    let input = TSStringInput {
        string: string.cast::<c_char>(),
        length,
    };
    ts_parser_parse(
        self_,
        old_tree,
        TSInput {
            payload: core::ptr::addr_of!(input) as *mut c_void,
            read: Some(ts_string_input_read),
            encoding,
            decode: None,
        },
    )
}
