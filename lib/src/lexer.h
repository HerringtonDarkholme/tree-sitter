#ifndef TREE_SITTER_LEXER_H_
#define TREE_SITTER_LEXER_H_

#include "./length.h"
#include "./subtree.h"
#include "tree_sitter/api.h"
#include "./parser.h"

typedef struct {
  uint32_t value;
  bool valid;
} ColumnData;

typedef struct {
  TSLexer data;
  Length current_position;
  Length token_start_position;
  Length token_end_position;

  TSRange *included_ranges;
  const char *chunk;
  TSInput input;
  TSLogger logger;

  uint32_t included_range_count;
  uint32_t current_included_range_index;
  uint32_t chunk_start;
  uint32_t chunk_size;
  uint32_t lookahead_size;
  bool did_get_column;
  ColumnData column_data;

  char debug_buffer[TREE_SITTER_SERIALIZATION_BUFFER_SIZE];
} Lexer;

#endif  // TREE_SITTER_LEXER_H_
