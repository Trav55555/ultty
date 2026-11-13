//! Terminal emulation core for Ultty.
//!
//! This crate holds everything that turns a byte stream into terminal state:
//! the VT/xterm parser ([`ansi`]) and the screen model ([`grid`]: cells,
//! rows with grapheme clusters, scrollback, reflow, modes, selection).
//!
//! It performs no I/O and has no windowing, GPU, PTY, or async
//! dependencies, so it can be embedded, fuzzed, and benchmarked on its own.
//! Feed bytes through [`ansi::AnsiParser`] into a [`grid::TerminalHandler`]
//! wrapping a [`grid::Terminal`]:
//!
//! ```
//! use ultty_vt::ansi::AnsiParser;
//! use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};
//!
//! let mut terminal = Terminal::new(Dimensions::new(80, 24));
//! let mut parser = AnsiParser::new();
//! let mut handler = TerminalHandler::new(&mut terminal);
//! parser.feed(b"hello\x1b[1mworld", |action| handler.handle(action));
//! assert_eq!(terminal.grid().row(0).unwrap().text_trimmed(), "helloworld");
//! ```

pub mod ansi;
pub mod grid;
pub mod program_status;
