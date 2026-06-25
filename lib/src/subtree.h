#ifndef TREE_SITTER_SUBTREE_H_
#define TREE_SITTER_SUBTREE_H_

#ifdef __cplusplus
extern "C" {
#endif

#include <limits.h>
#include <stdbool.h>
#include "./length.h"
#include "./host.h"
#include "tree_sitter/api.h"
#include "./parser.h"

#define TS_TREE_STATE_NONE USHRT_MAX

// The serialized state of an external scanner.
//
// Every time an external token subtree is created after a call to an
// external scanner, the scanner's `serialize` function is called to
// retrieve a serialized copy of its state. The bytes are then copied
// onto the subtree itself so that the scanner's state can later be
// restored using its `deserialize` function.
//
// Small byte arrays are stored inline, and long ones are allocated
// separately on the heap.
typedef struct {
  union {
    char *long_data;
    char short_data[24];
  };
  uint32_t length;
} ExternalScannerState;

// A compact representation of a subtree.
//
// This representation is used for small leaf nodes that are not
// errors, and were not created by an external scanner.
//
// The idea behind the layout of this struct is that the `is_inline`
// bit will fall exactly into the same location as the least significant
// bit of the pointer in `Subtree` or `MutableSubtree`, respectively.
// Because of alignment, for any valid pointer this will be 0, giving
// us the opportunity to make use of this bit to signify whether to use
// the pointer or the inline struct.
typedef struct SubtreeInlineData SubtreeInlineData;

#define SUBTREE_BITS    \
  bool visible : 1;     \
  bool named : 1;       \
  bool extra : 1;       \
  bool has_changes : 1; \
  bool is_missing : 1;  \
  bool is_keyword : 1;

#define SUBTREE_SIZE           \
  uint8_t padding_columns;     \
  uint8_t padding_rows : 4;    \
  uint8_t lookahead_bytes : 4; \
  uint8_t padding_bytes;       \
  uint8_t size_bytes;

#if TS_BIG_ENDIAN
#if TS_PTR_SIZE == 32

struct SubtreeInlineData {
  uint16_t parse_state;
  uint8_t symbol;
  SUBTREE_BITS
  bool unused : 1;
  bool is_inline : 1;
  SUBTREE_SIZE
};

#else

struct SubtreeInlineData {
  SUBTREE_SIZE
  uint16_t parse_state;
  uint8_t symbol;
  SUBTREE_BITS
  bool unused : 1;
  bool is_inline : 1;
};

#endif
#else

struct SubtreeInlineData {
  bool is_inline : 1;
  SUBTREE_BITS
  uint8_t symbol;
  uint16_t parse_state;
  SUBTREE_SIZE
};

#endif

#undef SUBTREE_BITS
#undef SUBTREE_SIZE

// A heap-allocated representation of a subtree.
//
// This representation is used for parent nodes, external tokens,
// errors, and other leaf nodes whose data is too large to fit into
// the inline representation.
typedef struct {
  volatile uint32_t ref_count;
  Length padding;
  Length size;
  uint32_t lookahead_bytes;
  uint32_t error_cost;
  uint32_t child_count;
  TSSymbol symbol;
  TSStateId parse_state;

  bool visible : 1;
  bool named : 1;
  bool extra : 1;
  bool fragile_left : 1;
  bool fragile_right : 1;
  bool has_changes : 1;
  bool has_external_tokens : 1;
  bool has_external_scanner_state_change : 1;
  bool depends_on_column: 1;
  bool is_missing : 1;
  bool is_keyword : 1;

  union {
    // Non-terminal subtrees (`child_count > 0`)
    struct {
      uint32_t visible_child_count;
      uint32_t named_child_count;
      uint32_t visible_descendant_count;
      int32_t dynamic_precedence;
      uint16_t repeat_depth;
      uint16_t production_id;
      struct {
        TSSymbol symbol;
        TSStateId parse_state;
      } first_leaf;
    };

    // External terminal subtrees (`child_count == 0 && has_external_tokens`)
    ExternalScannerState external_scanner_state;

    // Error terminal subtrees (`child_count == 0 && symbol == ts_builtin_sym_error`)
    int32_t lookahead_char;
  };
} SubtreeHeapData;

// The fundamental building block of a syntax tree.
typedef union {
  SubtreeInlineData data;
  const SubtreeHeapData *ptr;
} Subtree;

static inline TSSymbol ts_subtree_symbol(Subtree self) {
  return self.data.is_inline ? self.data.symbol : self.ptr->symbol;
}

static inline uint32_t ts_subtree_is_repetition(Subtree self) {
  return self.data.is_inline
    ? 0
    : !self.ptr->named && !self.ptr->visible && self.ptr->child_count != 0;
}

#ifdef __cplusplus
}
#endif

#endif  // TREE_SITTER_SUBTREE_H_
