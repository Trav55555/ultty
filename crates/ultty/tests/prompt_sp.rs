//! Test for zsh PROMPT_SP overwrite issue

use ultty::grid::{Dimensions, Terminal, TerminalHandler};
use ultty::protocol::ansi::AnsiParser;

#[test]
fn test_exact_zsh_sequence() {
    // Create terminal with same width as actual (88 cols from ulttyctl info)
    let mut terminal = Terminal::new(Dimensions::new(88, 35));
    let mut parser = AnsiParser::new();
    
    // Exact sequence from PTY log:
    // [1m[7m%[27m[1m[0m then 80+ spaces, then \r \r
    let mut input = Vec::new();
    
    // [1m - bold
    input.extend_from_slice(b"\x1b[1m");
    // [7m - reverse
    input.extend_from_slice(b"\x1b[7m");
    // % character
    input.push(b'%');
    // [27m - no reverse
    input.extend_from_slice(b"\x1b[27m");
    // [1m - bold again
    input.extend_from_slice(b"\x1b[1m");
    // [0m - reset
    input.extend_from_slice(b"\x1b[0m");
    // Fill with spaces to end of line (87 more spaces to fill 88-wide line)
    for _ in 0..87 {
        input.push(b' ');
    }
    // CR
    input.push(b'\r');
    // Space to overwrite %
    input.push(b' ');
    // CR 
    input.push(b'\r');
    
    {
        let mut handler = TerminalHandler::new(&mut terminal);
        parser.feed(&input, |action| handler.handle(action));
    }
    
    let cell = terminal.grid().cell(0, 0).unwrap();
    println!("Cell (0,0) char='{}' expecting ' '", cell.char());
    
    // Check what's in the first few cells
    for col in 0..5 {
        if let Some(c) = terminal.grid().cell(col, 0) {
            println!("  Cell (0,{}): '{}'", col, c.char());
        }
    }
    
    assert_eq!(cell.char(), ' ', "Cell (0,0) should be space, not %");
}

#[test] 
fn test_cursor_position_after_spaces() {
    let mut terminal = Terminal::new(Dimensions::new(88, 35));
    let mut parser = AnsiParser::new();
    
    // Write % then fill line with spaces
    let mut input = Vec::new();
    input.push(b'%');
    for _ in 0..87 {
        input.push(b' ');
    }
    
    {
        let mut handler = TerminalHandler::new(&mut terminal);
        parser.feed(&input, |action| handler.handle(action));
    }
    
    let cursor = terminal.grid().cursor();
    println!("After writing % + 87 spaces, cursor at ({}, {})", 
             cursor.position.row, cursor.position.col);
    
    // Should wrap to next line since we wrote 88 chars on 88-wide terminal
    // Actually no - cursor should be at end of line or wrapped
}

#[test]
fn test_cr_then_space() {
    let mut terminal = Terminal::new(Dimensions::new(88, 35));
    let mut parser = AnsiParser::new();
    
    // Write X, fill rest of line, CR, write Y
    let mut input = Vec::new();
    input.push(b'X');
    for _ in 0..87 {
        input.push(b'-');
    }
    input.push(b'\r');  // Go back to column 0
    input.push(b'Y');   // Should overwrite X
    
    {
        let mut handler = TerminalHandler::new(&mut terminal);
        parser.feed(&input, |action| handler.handle(action));
    }
    
    let cell = terminal.grid().cell(0, 0).unwrap();
    println!("After X + dashes + CR + Y, cell (0,0) = '{}'", cell.char());
    assert_eq!(cell.char(), 'Y', "Y should overwrite X");
}
