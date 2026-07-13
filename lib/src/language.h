#ifndef TREE_SITTER_LANGUAGE_H_
#define TREE_SITTER_LANGUAGE_H_

#ifdef __cplusplus
extern "C" {
#endif

// The language-table logic now lives in the Rust core
// (lib/src_rust/language.rs). These ABI constants remain available to C code;
// the former LookaheadIterator struct and ts_language_* static-inline helpers
// were removed along with the C core.

#define LANGUAGE_VERSION_WITH_RESERVED_WORDS 15
#define LANGUAGE_VERSION_WITH_PRIMARY_STATES 14

#ifdef __cplusplus
}
#endif

#endif  // TREE_SITTER_LANGUAGE_H_
