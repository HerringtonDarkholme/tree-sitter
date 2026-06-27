#ifndef TREE_SITTER_LANGUAGE_H_
#define TREE_SITTER_LANGUAGE_H_

#ifdef __cplusplus
extern "C" {
#endif

// The language-table logic now lives in the Rust core
// (lib/src_rust/language.rs). Only these ABI version constants are still
// consumed by the remaining C (wasm_store.c); the former LookaheadIterator
// struct and the ts_language_* static-inline helpers were removed along with
// the C core.

#define LANGUAGE_VERSION_WITH_RESERVED_WORDS 15
#define LANGUAGE_VERSION_WITH_PRIMARY_STATES 14

#ifdef __cplusplus
}
#endif

#endif  // TREE_SITTER_LANGUAGE_H_
