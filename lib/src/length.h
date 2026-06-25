#ifndef TREE_SITTER_LENGTH_H_
#define TREE_SITTER_LENGTH_H_

#include <stdint.h>
#include "tree_sitter/api.h"

typedef struct {
  uint32_t bytes;
  TSPoint extent;
} Length;

#endif
