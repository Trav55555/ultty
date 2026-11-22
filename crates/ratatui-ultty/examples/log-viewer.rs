//! Minimal ratatui log viewer opting its log pane into an Ultty native region.
//!
//! Run with `--once` to render one frame and exit, or `--ansi` to force ratatui fallback.

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, KeyCode};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color as RatColor, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui_ultty::{Mode, NativeRegion, UlttyBridge};
use ultty_protocol::Color;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let once = args.iter().any(|a| a == "--once");
    let force_ansi = args.iter().any(|a| a == "--ansi");

    let mut bridge = if force_ansi {
        UlttyBridge::ansi(CrosstermBackend::new(io::stdout()))?
    } else {
        UlttyBridge::connect(CrosstermBackend::new(io::stdout()))?
    };

    let mut logs = (1..=40)
        .map(|i| format!("[12:00:{i:02}] worker event #{i}: processed batch cleanly"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut scroll_y: u16 = 0;

    if bridge.mode() == Mode::Ansi && !once {
        enable_raw_mode()?;
        io::stdout().execute(EnterAlternateScreen)?;
    }

    let run = (|| -> Result<()> {
        let mut tick = 40usize;
        loop {
            let mode_label = match bridge.mode() {
                Mode::Native => "native (Ultty scroll + copy)",
                Mode::Ansi => "ANSI fallback (ratatui)",
            };
            bridge.draw(|frame| {
                let rows = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Min(5),
                    Constraint::Length(1),
                ])
                .split(frame.area());

                let header =
                    Paragraph::new(format!("ratatui-ultty log viewer | mode: {mode_label}"))
                        .style(Style::default().fg(RatColor::Cyan))
                        .block(Block::default().borders(Borders::ALL).title("App"));
                frame.render_widget(header, rows[0]);

                let log_region = NativeRegion::log("log-pane", &logs)
                    .title("Live Log")
                    .border_color(Color::CYAN)
                    .fallback_scroll(scroll_y);
                frame.native_region(rows[1], log_region);

                let footer = Paragraph::new("a: append line | j/k: scroll | q: quit")
                    .style(Style::default().fg(RatColor::DarkGray));
                frame.render_widget(footer, rows[2]);
            })?;

            if once {
                break;
            }

            if let Some(Event::Key(key)) = bridge.poll_event(Duration::from_millis(100))? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('a') => {
                        tick += 1;
                        logs.push_str(&format!("\n[12:01:{tick:02}] appended event #{tick}"));
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        scroll_y = scroll_y.saturating_add(1);
                        bridge.scroll_region("log-pane", f32::from(scroll_y), false)?;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        scroll_y = scroll_y.saturating_sub(1);
                        bridge.scroll_region("log-pane", f32::from(scroll_y), false)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })();

    if bridge.mode() == Mode::Ansi && !once {
        let _ = io::stdout().execute(LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }

    run
}
