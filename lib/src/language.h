#ifndef TREE_SITTER_LANGUAGE_H_
#define TREE_SITTER_LANGUAGE_H_

#ifdef __cplusplus
extern "C" {
#endif

#include "./subtree.h"
#include "./parser.h"

#define ts_builtin_sym_error_repeat (ts_builtin_sym_error - 1)

#define LANGUAGE_VERSION_WITH_RESERVED_WORDS 15
#define LANGUAGE_VERSION_WITH_PRIMARY_STATES 14

typedef struct {
  const TSLanguage *language;
  const uint16_t *data;
  const uint16_t *group_end;
  TSStateId state;
  uint16_t table_value;
  uint16_t section_index;
  uint16_t group_count;
  bool is_small_state;

  const TSParseAction *actions;
  TSSymbol symbol;
  TSStateId next_state;
  uint16_t action_count;
} LookaheadIterator;

TSSymbolMetadata ts_language_symbol_metadata(const TSLanguage *self, TSSymbol symbol);

// Iterate over all of the symbols that are valid in the given state.
//
// For 'large' parse states, this just requires iterating through
// all possible symbols and checking the parse table for each one.
// For 'small' parse states, this exploits the structure of the
// table to only visit the valid symbols.
static inline LookaheadIterator ts_language_lookaheads(
  const TSLanguage *self,
  TSStateId state
) {
  bool is_small_state = state >= self->large_state_count;
  const uint16_t *data;
  const uint16_t *group_end = NULL;
  uint16_t group_count = 0;
  if (is_small_state) {
    uint32_t index = self->small_parse_table_map[state - self->large_state_count];
    data = &self->small_parse_table[index];
    group_end = data + 1;
    group_count = *data;
  } else {
    data = &self->parse_table[state * self->symbol_count] - 1;
  }
  return (LookaheadIterator) {
    .language = self,
    .data = data,
    .group_end = group_end,
    .group_count = group_count,
    .is_small_state = is_small_state,
    .symbol = UINT16_MAX,
    .next_state = 0,
  };
}

static inline bool ts_lookahead_iterator__next(LookaheadIterator *self) {
  // For small parse states, valid symbols are listed explicitly,
  // grouped by their value. There's no need to look up the actions
  // again until moving to the next group.
  if (self->is_small_state) {
    self->data++;
    if (self->data == self->group_end) {
      if (self->group_count == 0) return false;
      self->group_count--;
      self->table_value = *(self->data++);
      unsigned symbol_count = *(self->data++);
      self->group_end = self->data + symbol_count;
      self->symbol = *self->data;
    } else {
      self->symbol = *self->data;
      return true;
    }
  }

  // For large parse states, iterate through every symbol until one
  // is found that has valid actions.
  else {
    do {
      self->data++;
      self->symbol++;
      if (self->symbol >= self->language->symbol_count) return false;
      self->table_value = *self->data;
    } while (!self->table_value);
  }

  // Depending on if the symbols is terminal or non-terminal, the table value either
  // represents a list of actions or a successor state.
  if (self->symbol < self->language->token_count) {
    const TSParseActionEntry *entry = &self->language->parse_actions[self->table_value];
    self->action_count = entry->entry.count;
    self->actions = (const TSParseAction *)(entry + 1);
    self->next_state = 0;
  } else {
    self->action_count = 0;
    self->next_state = self->table_value;
  }
  return true;
}

// Whether the state is a "primary state". If this returns false, it indicates that there exists
// another state that behaves identically to this one with respect to query analysis.
static inline bool ts_language_state_is_primary(
  const TSLanguage *self,
  TSStateId state
) {
  if (self->abi_version >= LANGUAGE_VERSION_WITH_PRIMARY_STATES) {
    return state == self->primary_state_ids[state];
  } else {
    return true;
  }
}

static inline TSSymbol ts_language_alias_at(
  const TSLanguage *self,
  uint32_t production_id,
  uint32_t child_index
) {
  return production_id ?
    self->alias_sequences[production_id * self->max_alias_sequence_length + child_index] :
    0;
}

static inline void ts_language_field_map(
  const TSLanguage *self,
  uint32_t production_id,
  const TSFieldMapEntry **start,
  const TSFieldMapEntry **end
) {
  if (self->field_count == 0) {
    *start = NULL;
    *end = NULL;
    return;
  }

  TSMapSlice slice = self->field_map_slices[production_id];
  *start = &self->field_map_entries[slice.index];
  *end = &self->field_map_entries[slice.index] + slice.length;
}

static inline void ts_language_aliases_for_symbol(
  const TSLanguage *self,
  TSSymbol original_symbol,
  const TSSymbol **start,
  const TSSymbol **end
) {
  *start = &self->public_symbol_map[original_symbol];
  *end = *start + 1;

  unsigned idx = 0;
  for (;;) {
    TSSymbol symbol = self->alias_map[idx++];
    if (symbol == 0 || symbol > original_symbol) break;
    uint16_t count = self->alias_map[idx++];
    if (symbol == original_symbol) {
      *start = &self->alias_map[idx];
      *end = &self->alias_map[idx + count];
      break;
    }
    idx += count;
  }
}

#ifdef __cplusplus
}
#endif

#endif  // TREE_SITTER_LANGUAGE_H_
