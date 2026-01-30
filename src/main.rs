mod layout;
mod render;
mod terminal;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
    Terminal as RatatuiTerminal,
};
use std::time::{Duration, Instant};

use layout::LayoutEngine;
use terminal::Terminal;

fn main() -> Result<()> {
    // 1. Initialize Terminal (Alt Mode via our wrapper)
    // Note: We need to keep this `term_wrapper` alive so its Drop implementation
    // restores the terminal at the end.
    let mut term_wrapper = Terminal::init()?;
    
    // 2. Initialize Ratatui
    // We use the raw stdout from our wrapper.
    // Ratatui will handle the buffering and diffing.
    let backend = CrosstermBackend::new(term_wrapper.backend());
    let mut terminal = RatatuiTerminal::new(backend)?;

    // 3. Initialize Layout Engine (Taffy)
    let mut layout_engine = LayoutEngine::new();

    // Stats for performance monitoring
    let mut last_frame_time = Instant::now();
    let start_time = Instant::now();
    let mut frame_count = 0;

    // 4. Main Event Loop
    loop {
        let now = Instant::now();
        let delta = now.duration_since(last_frame_time);
        last_frame_time = now;
        frame_count += 1;

        // A. Handle Events (Poll with timeout to cap FPS ~60)
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    _ => {}
                },
                Event::Resize(_, _) => {
                    terminal.autoresize()?;
                }
                _ => {}
            }
        }

        // B. Render Phase
        terminal.draw(|frame| {
            let size = frame.area();

            // 1. Compute Layout (Taffy)
            // In a real implementation, we would update Taffy constraints based on `size`
            // and retrieve computed layout rects for each widget.
            // For this scaffold, we simulate it.
            let _ = layout_engine.compute(size.width, size.height);

            // 2. Draw Widgets
            let uptime = now.duration_since(start_time).as_secs();
            
            // Outer Block
            let block = Block::default()
                .title(format!(" Ultimate TUI Terminal | Frame: {} | FPS: {:.0} | Uptime: {}s ", 
                    frame_count, 
                    1.0 / delta.as_secs_f64().max(0.001),
                    uptime
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));
            frame.render_widget(block, size);

            // Centered Text
            let text = Paragraph::new("Hello from the Hybrid Engine!\n\nArchitecture:\nRust + Ratatui (Diffing) + Taffy (Layout)\n\nPress 'q' to exit.")
                .style(Style::default().fg(Color::White))
                .alignment(ratatui::layout::Alignment::Center)
                .block(Block::default().borders(Borders::ALL).title(" Info "));
            
            // Layout helper (placeholder until Taffy integration is deep)
            let area = centered_rect(size, 60, 40);
            frame.render_widget(text, area);
        })?;
    }

    // 5. Restore Terminal (handled by Drop, but explicit is good)
    drop(terminal); // Drop Ratatui first to release stdout
    term_wrapper.restore()?;
    Ok(())
}

/// Helper to center a rect (using Ratatui constraints)
fn centered_rect(r: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
