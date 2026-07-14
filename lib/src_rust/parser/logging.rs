use super::{
    c_char, fmt, fprintf, fputc, fputs, ptr_mut, stack_print_dot_graph, subtree_print_dot_graph,
    ts_language_symbol_name, CStr, Stack, Subtree, TSLanguage, TSLogTypeParse, TSParser, TSSymbol,
    Write,
};

// ---------------------------------------------------------------------------
// Internal helpers — logging & breakdown
// ---------------------------------------------------------------------------

pub(super) struct ParserLogBuffer<'a> {
    bytes: &'a mut [u8],
    len: usize,
}

impl ParserLogBuffer<'_> {
    fn write_bytes(&mut self, bytes: &[u8]) {
        let available = self.bytes.len().saturating_sub(self.len + 1);
        let count = available.min(bytes.len());
        self.bytes[self.len..self.len + count].copy_from_slice(&bytes[..count]);
        self.len += count;
    }
}

impl Write for ParserLogBuffer<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.write_bytes(value.as_bytes());
        Ok(())
    }
}

pub(super) struct DisplayCStr(pub(super) *const c_char);

impl fmt::Display for DisplayCStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut bytes = unsafe { CStr::from_ptr(self.0) }.to_bytes();
        while !bytes.is_empty() {
            match core::str::from_utf8(bytes) {
                Ok(value) => return formatter.write_str(value),
                Err(error) => {
                    let valid = error.valid_up_to();
                    formatter
                        .write_str(unsafe { core::str::from_utf8_unchecked(&bytes[..valid]) })?;
                    formatter.write_char(char::REPLACEMENT_CHARACTER)?;
                    bytes = &bytes[valid + error.error_len().unwrap_or(1)..];
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) struct ParserLogContext {
    pub(super) language: *const TSLanguage,
    pub(super) stack: *mut Stack,
}

pub(super) unsafe fn parser_log(
    self_: &mut TSParser,
    write_message: impl FnOnce(ParserLogContext, &mut ParserLogBuffer<'_>) -> fmt::Result,
) {
    if self_.lexer.logger.log.is_none() && self_.dot_graph_file.is_null() {
        return;
    }

    {
        let context = ParserLogContext {
            language: self_.language,
            stack: self_.stack,
        };
        let mut buffer = ParserLogBuffer {
            bytes: &mut self_.lexer.debug_buffer,
            len: 0,
        };
        let _ = write_message(context, &mut buffer);
        buffer.bytes[buffer.len] = 0;
    }

    parser_emit_log(self_);
}

pub(super) unsafe fn parser_log_stack(self_: &TSParser) {
    if !self_.dot_graph_file.is_null() {
        stack_print_dot_graph(ptr_mut(self_.stack), self_.language, self_.dot_graph_file);
        fputs(c"\n\n".as_ptr().cast::<i8>(), self_.dot_graph_file);
    }
}

pub(super) unsafe fn parser_log_tree(self_: &TSParser, tree: Subtree) {
    if !self_.dot_graph_file.is_null() {
        subtree_print_dot_graph(tree, self_.language, self_.dot_graph_file);
        fputs(c"\n".as_ptr().cast::<i8>(), self_.dot_graph_file);
    }
}

pub(super) unsafe fn parser_symbol_name(
    language: *const TSLanguage,
    symbol: TSSymbol,
) -> *const c_char {
    ts_language_symbol_name(language, symbol)
}

pub(super) unsafe fn parser_tree_name(language: *const TSLanguage, tree: Subtree) -> *const c_char {
    parser_symbol_name(language, tree.symbol())
}

pub(super) unsafe fn parser_log_lookahead(self_: &mut TSParser, symbol: *const c_char, size: u32) {
    parser_log(self_, |_, buffer| {
        buffer.write_str("lexed_lookahead sym:")?;
        for byte in CStr::from_ptr(symbol).to_bytes() {
            match *byte {
                b'\t' => buffer.write_str("\\t")?,
                b'\n' => buffer.write_str("\\n")?,
                0x0b => buffer.write_str("\\v")?,
                0x0c => buffer.write_str("\\f")?,
                b'\r' => buffer.write_str("\\r")?,
                b'\\' => buffer.write_str("\\\\")?,
                _ => buffer.write_bytes(core::slice::from_ref(byte)),
            }
        }
        write!(buffer, ", size:{size}")
    });
}

unsafe fn parser_emit_log(self_: &mut TSParser) {
    if let Some(log_fn) = self_.lexer.logger.log {
        log_fn(
            self_.lexer.logger.payload,
            TSLogTypeParse,
            self_.lexer.debug_buffer.as_ptr().cast::<c_char>(),
        );
    }

    if !self_.dot_graph_file.is_null() {
        fprintf(
            self_.dot_graph_file,
            c"graph {\nlabel=\"".as_ptr().cast::<i8>(),
        );
        let mut chr = self_.lexer.debug_buffer.as_ptr();
        while *chr != 0 {
            if *chr == b'"' || *chr == b'\\' {
                fputc(i32::from(b'\\'), self_.dot_graph_file);
            }
            fputc(i32::from(*chr), self_.dot_graph_file);
            chr = chr.add(1);
        }
        fprintf(self_.dot_graph_file, c"\"\n}\n\n".as_ptr().cast::<i8>());
    }
}
