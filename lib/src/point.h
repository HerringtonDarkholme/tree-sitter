#ifndef TREE_SITTER_POINT_H_
#define TREE_SITTER_POINT_H_

#include "tree_sitter/api.h"

#define POINT_ZERO ((TSPoint) {0, 0})
#define POINT_MAX ((TSPoint) {UINT32_MAX, UINT32_MAX})

static inline bool point_lte(TSPoint a, TSPoint b) {
  return (a.row < b.row) || (a.row == b.row && a.column <= b.column);
}

static inline bool point_lt(TSPoint a, TSPoint b) {
  return (a.row < b.row) || (a.row == b.row && a.column < b.column);
}

static inline bool point_gt(TSPoint a, TSPoint b) {
  return (a.row > b.row) || (a.row == b.row && a.column > b.column);
}

static inline bool point_gte(TSPoint a, TSPoint b) {
  return (a.row > b.row) || (a.row == b.row && a.column >= b.column);
}

static inline bool point_eq(TSPoint a, TSPoint b) {
  return a.row == b.row && a.column == b.column;
}

#endif
