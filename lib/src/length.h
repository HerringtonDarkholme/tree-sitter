#ifndef TREE_SITTER_LENGTH_H_
#define TREE_SITTER_LENGTH_H_

#include <stdint.h>
#include "tree_sitter/api.h"

typedef struct {
  uint32_t bytes;
  TSPoint extent;
} Length;

static const Length LENGTH_UNDEFINED = {0, {0, 1}};
static const Length LENGTH_MAX = {UINT32_MAX, {UINT32_MAX, UINT32_MAX}};

#endif
