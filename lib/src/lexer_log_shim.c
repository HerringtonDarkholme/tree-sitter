// C shim for the variadic ts_lexer__log function.
// Rust cannot define C-variadic functions on stable, so this thin wrapper
// handles the va_list forwarding and is linked from Rust via extern "C".

#include "./lexer.h"
#include <stdarg.h>
#include <stdio.h>

void ts_lexer__log_shim(const TSLexer *_self, const char *fmt, ...) {
  Lexer *self = (Lexer *)_self;
  va_list args;
  va_start(args, fmt);
  if (self->logger.log) {
    vsnprintf(self->debug_buffer, TREE_SITTER_SERIALIZATION_BUFFER_SIZE, fmt, args);
    self->logger.log(self->logger.payload, TSLogTypeLex, self->debug_buffer);
  }
  va_end(args);
}
