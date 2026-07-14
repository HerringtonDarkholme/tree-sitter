// C shim for the variadic ts_lexer__log function.
// Rust cannot define C-variadic functions on stable, so this thin wrapper
// handles the va_list forwarding and is linked from Rust via extern "C".

#include "./parser.h"
#include <stdarg.h>
#include <stdio.h>

void ts_lexer__emit_log(const TSLexer *, const char *);

void ts_lexer__log_shim(const TSLexer *_self, const char *fmt, ...) {
  char buffer[TREE_SITTER_SERIALIZATION_BUFFER_SIZE];
  va_list args;
  va_start(args, fmt);
  vsnprintf(buffer, sizeof(buffer), fmt, args);
  va_end(args);
  ts_lexer__emit_log(_self, buffer);
}
