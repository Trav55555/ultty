//! ANSI/VT100 escape sequence parser for legacy applications.

/// ANSI parser state machine.
pub struct AnsiParser {
    // TODO: Parser state
    // Consider using `vte` crate as foundation
}

impl AnsiParser {
    /// Create a new ANSI parser.
    pub fn new() -> Self {
        Self {}
    }

    /// Feed bytes into the parser.
    pub fn feed(&mut self, _data: &[u8]) {
        // TODO: Parse ANSI sequences
    }
}

impl Default for AnsiParser {
    fn default() -> Self {
        Self::new()
    }
}
