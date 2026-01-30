use anyhow::Result;
use crossterm::{
    cursor,
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{self, Stdout};

/// A wrapper around the low-level terminal interface.
/// Handles raw mode, alternate screen, and mouse capture.
pub struct Terminal {
    stdout: Stdout,
}

impl Terminal {
    /// Initialize the terminal in "Alternate Mode" (full screen, raw input).
    pub fn init() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;
        
        Ok(Self { stdout })
    }

    /// Restore the terminal to its original state (Standard Buffer).
    /// This flushes the "Alternate Screen" away and returns the user to their shell.
    pub fn restore(&mut self) -> Result<()> {
        execute!(
            self.stdout,
            LeaveAlternateScreen,
            DisableMouseCapture,
            cursor::Show
        )?;
        disable_raw_mode()?;
        Ok(())
    }

    /// Access the underlying stdout for rendering.
    pub fn backend(&mut self) -> &mut Stdout {
        &mut self.stdout
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Ensure terminal is restored even on panic
        let _ = self.restore();
    }
}
