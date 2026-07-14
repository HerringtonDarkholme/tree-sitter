use super::super::subtree::subtree_external_scanner_state;
use super::{
    external_scanner_state_eq, language_full, language_has_actions, language_is_reserved_word,
    language_lex_mode_for_state, language_table_entry, length_sub, length_zero, lexer_advance,
    lexer_finish, lexer_is_eof, lexer_reset, lexer_start, parser_call_keyword_lex_fn,
    parser_call_main_lex_fn, parser_external_scanner_deserialize, parser_external_scanner_scan,
    parser_external_scanner_serialize, parser_log, parser_log_lookahead, parser_symbol_name,
    ptr_ref, stack_has_advanced_since_error, stack_last_external_token, stack_position,
    subtree_child_count, subtree_external_scanner_state_eq, subtree_is_keyword, subtree_new_error,
    subtree_new_leaf, subtree_parse_state, subtree_release, subtree_retain,
    subtree_set_external_scanner_state, subtree_size, subtree_symbol, subtree_to_mut_unsafe,
    subtree_total_size, ts_language_next_state, DisplayCStr, Length, StackVersion, Subtree,
    TSParser, TSStateId, TSSymbol, TableEntry, Write, ERROR_STATE, NULL_SUBTREE,
    TS_BUILTIN_SYM_END, TS_BUILTIN_SYM_ERROR,
};

// ---------------------------------------------------------------------------
// Internal helpers — token reuse & lexing
// ---------------------------------------------------------------------------

unsafe fn parser_can_reuse_token(
    self_: &TSParser,
    state: TSStateId,
    token: Subtree,
    table_entry: &TableEntry,
) -> bool {
    debug_assert_eq!(subtree_child_count(token), 0);
    let token_symbol = subtree_symbol(token);
    let current_lex_mode = language_lex_mode_for_state(self_.language, state);

    // At the end of a non-terminal extra node, the lexer normally returns
    // NULL, which indicates that the parser should look for a reduce action
    // at symbol `0`. Avoid reusing tokens in this situation.
    if current_lex_mode.lex_state == u16::MAX {
        return false;
    }

    // If the token was created in a state with the same set of lookaheads, it is reusable.
    if table_entry.action_count > 0 {
        let token_state = subtree_parse_state(token);
        let token_lex_mode = language_lex_mode_for_state(self_.language, token_state);
        if token_lex_mode.lex_state == current_lex_mode.lex_state
            && token_lex_mode.external_lex_state == current_lex_mode.external_lex_state
            && token_lex_mode.reserved_word_set_id == current_lex_mode.reserved_word_set_id
        {
            let lang = language_full(self_.language);
            if token_symbol != lang.keyword_capture_token
                || (!subtree_is_keyword(token) && subtree_parse_state(token) == state)
            {
                return true;
            }
        }
    }

    // Empty tokens are not reusable in states with different lookaheads.
    if subtree_size(token).bytes == 0 && token_symbol != TS_BUILTIN_SYM_END {
        return false;
    }

    // If the current state allows external tokens or other tokens that conflict with this
    // token, this token is not reusable.
    current_lex_mode.external_lex_state == 0 && table_entry.is_reusable
}

/// Build the error token produced after skipping unrecognized input.
unsafe fn parser_new_error_lookahead(
    self_: &mut TSParser,
    parse_state: TSStateId,
    start_position: Length,
    error_start_position: Length,
    error_end_position: Length,
    lookahead_end_byte: u32,
    first_error_character: i32,
) -> Subtree {
    let padding = length_sub(error_start_position, start_position);
    let size = length_sub(error_end_position, error_start_position);
    let lookahead_bytes = lookahead_end_byte - error_end_position.bytes;
    subtree_new_error(
        &mut self_.tree_pool,
        first_error_character,
        padding,
        size,
        lookahead_bytes,
        parse_state,
        self_.language,
    )
}

/// Resolve the public symbol for a token found by internal or external lexing.
///
/// External scanners return an index into their symbol map. Internal lexing may
/// return the grammar's word token, in which case the keyword lexer gets one
/// chance to refine it to a reserved word that is valid in the current state.
unsafe fn parser_resolve_lexed_symbol(
    self_: &mut TSParser,
    parse_state: TSStateId,
    found_external_token: bool,
) -> (TSSymbol, bool) {
    let lang = language_full(self_.language);
    let mut symbol = self_.lexer.data.result_symbol;
    let mut is_keyword = false;

    if found_external_token {
        symbol = *lang.external_scanner.symbol_map.add(symbol as usize);
    } else if symbol == lang.keyword_capture_token && symbol != 0 {
        let end_byte = self_.lexer.token_end_position.bytes;
        let token_start_position = self_.lexer.token_start_position;
        lexer_reset(&mut self_.lexer, token_start_position);
        lexer_start(&mut self_.lexer);

        is_keyword = parser_call_keyword_lex_fn(self_);

        if is_keyword
            && self_.lexer.token_end_position.bytes == end_byte
            && (language_has_actions(self_.language, parse_state, self_.lexer.data.result_symbol)
                || language_is_reserved_word(
                    self_.language,
                    parse_state,
                    self_.lexer.data.result_symbol,
                ))
        {
            symbol = self_.lexer.data.result_symbol;
        }
    }

    (symbol, is_keyword)
}

/// Build the concrete leaf token after the lexing loop succeeds.
#[allow(clippy::too_many_arguments)]
unsafe fn parser_new_leaf_lookahead(
    self_: &mut TSParser,
    parse_state: TSStateId,
    start_position: Length,
    lookahead_end_byte: u32,
    found_external_token: bool,
    called_get_column: bool,
    external_scanner_state_len: u32,
    external_scanner_state_changed: bool,
) -> Subtree {
    let padding = length_sub(self_.lexer.token_start_position, start_position);
    let size = length_sub(
        self_.lexer.token_end_position,
        self_.lexer.token_start_position,
    );
    let lookahead_bytes = lookahead_end_byte - self_.lexer.token_end_position.bytes;
    let (symbol, is_keyword) =
        parser_resolve_lexed_symbol(self_, parse_state, found_external_token);

    let result = subtree_new_leaf(
        &mut self_.tree_pool,
        symbol,
        padding,
        size,
        lookahead_bytes,
        parse_state,
        found_external_token,
        called_get_column,
        is_keyword,
        self_.language,
    );

    if found_external_token {
        let mut_result = subtree_to_mut_unsafe(result);
        subtree_set_external_scanner_state(
            mut_result,
            self_.lexer.debug_buffer.as_ptr(),
            external_scanner_state_len,
        );
        mut_result
            .heap_data_mut()
            .set_has_external_scanner_state_change(external_scanner_state_changed);
    }

    result
}

/// Scan from the current stack position and return one lookahead subtree.
///
/// The scanner first gives an external scanner a chance when the parse state
/// enables one, then falls back to the generated lexer. If normal lexing fails,
/// it switches to the error lex mode and consumes bytes until it can produce an
/// error token or EOF.
unsafe fn parser_lex(
    self_: &mut TSParser,
    version: StackVersion,
    parse_state: TSStateId,
) -> Subtree {
    let lang = language_full(self_.language);
    let mut lex_mode = language_lex_mode_for_state(self_.language, parse_state);
    if lex_mode.lex_state == u16::MAX {
        parser_log(self_, |_, log| {
            log.write_str("no_lookahead_after_non_terminal_extra")
        });
        return NULL_SUBTREE;
    }

    let stack = ptr_ref(self_.stack);
    let start_position = stack_position(stack, version);
    let external_token = stack_last_external_token(stack, version);

    let mut found_external_token = false;
    let mut error_mode = parse_state == ERROR_STATE;
    let mut skipped_error = false;
    let mut called_get_column = false;
    let mut first_error_character: i32 = 0;
    let mut error_start_position = length_zero();
    let mut error_end_position = length_zero();
    let mut lookahead_end_byte: u32 = 0;
    let mut external_scanner_state_len: u32 = 0;
    let mut external_scanner_state_changed = false;
    lexer_reset(&mut self_.lexer, start_position);

    loop {
        let mut found_token;
        let current_position = self_.lexer.current_position;
        let column_data = self_.lexer.column_data;

        if lex_mode.external_lex_state != 0 {
            parser_log(self_, |_, log| {
                write!(
                    log,
                    "lex_external state:{}, row:{}, column:{}",
                    i32::from(lex_mode.external_lex_state),
                    current_position.extent.row,
                    current_position.extent.column
                )
            });
            lexer_start(&mut self_.lexer);
            parser_external_scanner_deserialize(self_, external_token);
            found_token = parser_external_scanner_scan(self_, lex_mode.external_lex_state);
            lexer_finish(&mut self_.lexer, &mut lookahead_end_byte);

            if found_token {
                external_scanner_state_len = parser_external_scanner_serialize(self_);
                let external_scanner_state = subtree_external_scanner_state(&external_token);
                external_scanner_state_changed = !external_scanner_state_eq(
                    external_scanner_state,
                    self_.lexer.debug_buffer.as_ptr(),
                    external_scanner_state_len,
                );

                if self_.lexer.token_end_position.bytes <= current_position.bytes
                    && !external_scanner_state_changed
                {
                    let symbol = *lang
                        .external_scanner
                        .symbol_map
                        .add(self_.lexer.data.result_symbol as usize);
                    let next_parse_state =
                        ts_language_next_state(self_.language, parse_state, symbol);
                    let token_is_extra = next_parse_state == parse_state;
                    if error_mode
                        || !stack_has_advanced_since_error(ptr_ref(self_.stack), version)
                        || token_is_extra
                    {
                        parser_log(self_, |context, log| {
                            write!(
                                log,
                                "ignore_empty_external_token symbol:{}",
                                DisplayCStr(parser_symbol_name(context.language, symbol))
                            )
                        });
                        found_token = false;
                    }
                }
            }

            if found_token {
                found_external_token = true;
                called_get_column = self_.lexer.did_get_column;
                break;
            }

            lexer_reset(&mut self_.lexer, current_position);
            self_.lexer.column_data = column_data;
        }

        parser_log(self_, |_, log| {
            write!(
                log,
                "lex_internal state:{}, row:{}, column:{}",
                i32::from(lex_mode.lex_state),
                current_position.extent.row,
                current_position.extent.column
            )
        });
        lexer_start(&mut self_.lexer);
        found_token = parser_call_main_lex_fn(self_, lex_mode);
        lexer_finish(&mut self_.lexer, &mut lookahead_end_byte);
        if found_token {
            break;
        }

        if !error_mode {
            error_mode = true;
            lex_mode = language_lex_mode_for_state(self_.language, ERROR_STATE);
            lexer_reset(&mut self_.lexer, start_position);
            continue;
        }

        if !skipped_error {
            parser_log(self_, |_, log| log.write_str("skip_unrecognized_character"));
            skipped_error = true;
            error_start_position = self_.lexer.token_start_position;
            error_end_position = self_.lexer.token_start_position;
            first_error_character = self_.lexer.data.lookahead;
        }

        if self_.lexer.current_position.bytes == error_end_position.bytes {
            if lexer_is_eof(&self_.lexer) {
                self_.lexer.data.result_symbol = TS_BUILTIN_SYM_ERROR;
                break;
            }
            lexer_advance(&mut self_.lexer, false);
        }

        error_end_position = self_.lexer.current_position;
    }

    let result = if skipped_error {
        parser_new_error_lookahead(
            self_,
            parse_state,
            start_position,
            error_start_position,
            error_end_position,
            lookahead_end_byte,
            first_error_character,
        )
    } else {
        parser_new_leaf_lookahead(
            self_,
            parse_state,
            start_position,
            lookahead_end_byte,
            found_external_token,
            called_get_column,
            external_scanner_state_len,
            external_scanner_state_changed,
        )
    };

    parser_log_lookahead(
        self_,
        parser_symbol_name(self_.language, subtree_symbol(result)),
        subtree_total_size(result).bytes,
    );
    result
}

unsafe fn parser_get_cached_token(
    self_: &TSParser,
    state: TSStateId,
    position: usize,
    last_external_token: Subtree,
) -> Option<(Subtree, TableEntry)> {
    let cache = &self_.token_cache;
    if !cache.token.is_null()
        && cache.byte_index == position as u32
        && subtree_external_scanner_state_eq(&cache.last_external_token, &last_external_token)
    {
        let mut table_entry = TableEntry::empty();
        language_table_entry(
            self_.language,
            state,
            subtree_symbol(cache.token),
            &mut table_entry,
        );
        if parser_can_reuse_token(self_, state, cache.token, &table_entry) {
            subtree_retain(cache.token);
            return Some((cache.token, table_entry));
        }
    }
    None
}

pub(super) unsafe fn parser_set_cached_token(
    self_: &mut TSParser,
    byte_index: u32,
    last_external_token: Subtree,
    token: Subtree,
) {
    let cache = &mut self_.token_cache;
    if !token.is_null() {
        subtree_retain(token);
    }
    if !last_external_token.is_null() {
        subtree_retain(last_external_token);
    }
    if !cache.token.is_null() {
        subtree_release(&mut self_.tree_pool, cache.token);
    }
    if !cache.last_external_token.is_null() {
        subtree_release(&mut self_.tree_pool, cache.last_external_token);
    }
    cache.token = token;
    cache.byte_index = byte_index;
    cache.last_external_token = last_external_token;
}

/// Find the initial lookahead for one stack version.
///
/// The parser tries sources in cheapest-to-most-expensive order:
///
/// 1. Reuse the parser's one-token cache for another version at this position.
/// 2. Ask the lexer to scan a fresh token.
///
/// The returned `needs_lex` flag tells `parser_advance` whether step 2 is
/// still required.
pub(super) unsafe fn parser_get_initial_lookahead(
    self_: &mut TSParser,
    state: TSStateId,
    position: u32,
    last_external_token: Subtree,
) -> (Subtree, TableEntry, bool) {
    let (lookahead, table_entry) =
        parser_get_cached_token(self_, state, position as usize, last_external_token)
            .unwrap_or((NULL_SUBTREE, TableEntry::empty()));

    let needs_lex = lookahead.is_null();
    (lookahead, table_entry, needs_lex)
}

/// Lex a token for the current stack version and prepare its parse-table entry.
///
/// A null lookahead is meaningful when parsing a non-terminal extra: it asks the
/// parser to consult the EOF entry for a forced reduction, after which lexing
/// resumes from the new parse state.
pub(super) unsafe fn parser_lex_lookahead(
    self_: &mut TSParser,
    version: StackVersion,
    state: TSStateId,
    position: u32,
    last_external_token: Subtree,
    lookahead: &mut Subtree,
    table_entry: &mut TableEntry,
) {
    *lookahead = parser_lex(self_, version, state);

    if !lookahead.is_null() {
        parser_set_cached_token(self_, position, last_external_token, *lookahead);
        language_table_entry(
            self_.language,
            state,
            subtree_symbol(*lookahead),
            table_entry,
        );
    } else {
        language_table_entry(self_.language, state, TS_BUILTIN_SYM_END, table_entry);
    }
}

// ---------------------------------------------------------------------------
