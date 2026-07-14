//! Lifecycle and calls for a language's external scanner.
//!
//! External scanners own language-specific mutable state outside the generated
//! lexer. Before scanning a GLR version, the parser restores the state stored
//! on that version's last external token. After a successful scan, the caller
//! serializes the new state into the produced subtree so another version can
//! reproduce the same lexical context.

use core::ptr;

use crate::ffi::TSStateId;

use super::super::language::{language_enabled_external_tokens, language_full};
use super::super::lexer::TREE_SITTER_SERIALIZATION_BUFFER_SIZE;
use super::super::subtree::Subtree;
use super::TSParser;

pub(super) unsafe fn parser_external_scanner_create(parser: &mut TSParser) {
    if parser.language.is_null() {
        return;
    }

    let language = language_full(parser.language);
    if language.external_scanner.states.is_null() {
        return;
    }

    if let Some(create) = language.external_scanner.create {
        parser.external_scanner_payload = create();
    }
}

pub(super) unsafe fn parser_external_scanner_destroy(parser: &mut TSParser) {
    if !parser.language.is_null() && !parser.external_scanner_payload.is_null() {
        let language = language_full(parser.language);
        if let Some(destroy) = language.external_scanner.destroy {
            destroy(parser.external_scanner_payload);
        }
    }
    parser.external_scanner_payload = ptr::null_mut();
}

pub(super) unsafe fn parser_external_scanner_serialize(parser: &mut TSParser) -> u32 {
    let serialize = language_full(parser.language)
        .external_scanner
        .serialize
        .unwrap();
    let length = serialize(
        parser.external_scanner_payload,
        parser.lexer.debug_buffer.as_mut_ptr().cast::<i8>(),
    );
    debug_assert!(length as usize <= TREE_SITTER_SERIALIZATION_BUFFER_SIZE);
    length
}

pub(super) unsafe fn parser_external_scanner_deserialize(
    parser: &mut TSParser,
    external_token: Subtree,
) {
    let (data, length) = if !external_token.is_null() {
        let state = external_token.external_scanner_state();
        (state.as_bytes().as_ptr(), state.length)
    } else {
        (ptr::null(), 0)
    };

    let deserialize = language_full(parser.language)
        .external_scanner
        .deserialize
        .unwrap();
    deserialize(parser.external_scanner_payload, data.cast::<i8>(), length);
}

pub(super) unsafe fn parser_external_scanner_scan(
    parser: &mut TSParser,
    external_lex_state: TSStateId,
) -> bool {
    let language = language_full(parser.language);
    let valid_tokens =
        language_enabled_external_tokens(parser.language, u32::from(external_lex_state));
    (language.external_scanner.scan.unwrap())(
        parser.external_scanner_payload,
        &mut parser.lexer.data,
        valid_tokens,
    )
}
