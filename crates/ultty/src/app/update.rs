//! Main update loop: process terminal events, poll servers, and schedule renders.

use std::time::{Duration, Instant};

use super::App;
use ultty::event_loop::TerminalEvent;
use ultty::tmux::TmuxController;

impl App {
    /// Process terminal events and render. Returns true if should exit.
    pub(super) fn update(&mut self) -> bool {
        // Check for config file changes (hot-reload)
        if let Some(ref mut watcher) = self.config_watcher {
            if watcher.poll() {
                tracing::info!("Config file changed, reloading...");
                self.reload_config();
            }
        }

        // Poll native protocol server for TUI app messages
        if let Some(ref mut server) = self.protocol_server {
            if let Err(e) = server.poll(Some(Duration::ZERO)) {
                tracing::warn!("Protocol server poll error: {}", e);
            }
            // Tick scroll animations
            if server.update_animations() {
                self.needs_redraw = true;
            }

            // Handle clipboard operations from native sessions
            let gets = server.pending_clipboard_gets();
            for key in gets {
                let text = self
                    .clipboard
                    .as_mut()
                    .and_then(|cb| cb.get_text().ok());
                server.send_clipboard_content(key, text);
            }
            let sets = server.pending_clipboard_sets();
            for (_key, text) in sets {
                if let Some(ref mut cb) = self.clipboard {
                    if let Err(e) = cb.set_text(text) {
                        tracing::warn!("Failed to set clipboard from native session: {}", e);
                    }
                }
            }
        }

        if let Some(ref mut server) = self.control_server {
            if let Err(e) = server.poll(Some(Duration::ZERO)) {
                tracing::warn!("Control server poll error: {}", e);
            }
            self.handle_control_requests();
        }

        // Update cursor blink state
        let now = Instant::now();
        if now.duration_since(self.last_cursor_blink) >= super::CURSOR_BLINK_INTERVAL {
            self.cursor_blink_visible = !self.cursor_blink_visible;
            self.last_cursor_blink = now;
            self.needs_redraw = true;
        }

        // Get the focused terminal ID for title updates
        let focused_id = self.terminal_manager.focused_terminal().map(|t| t.id);

        // Process all terminals
        match self.terminal_manager.process_all() {
            Ok(all_events) => {
                let mut content_changed = false;
                for (terminal_id, events) in all_events {
                    for event in events {
                        match event {
                            TerminalEvent::Redraw => {
                                self.needs_redraw = true;
                                // Only update URL detection for focused terminal
                                if Some(terminal_id) == focused_id {
                                    content_changed = true;
                                }
                            }
                            TerminalEvent::ChildExited { status } => {
                                tracing::info!(
                                    "Terminal {:?} exited with status {}",
                                    terminal_id,
                                    status
                                );
                            }
                            TerminalEvent::Bell => {
                                if Some(terminal_id) == focused_id {
                                    // Visual bell - flash border for 100ms
                                    if self.config.behavior.bell_visual {
                                        self.bell_active_until =
                                            Some(Instant::now() + Duration::from_millis(100));
                                        self.needs_redraw = true;
                                    }
                                    // Audio bell (if enabled)
                                    if self.config.behavior.bell_sound {
                                        // System bell - print BEL character to trigger terminal bell
                                        print!("\x07");
                                    }
                                    tracing::debug!("Bell");
                                }
                            }
                            TerminalEvent::TitleChanged(title) => {
                                // Only set window title from focused terminal
                                if Some(terminal_id) == focused_id {
                                    self.window.set_title(&title);
                                }
                            }
                            TerminalEvent::SetClipboard { clipboard: _, data } => {
                                // Set clipboard content (OSC 52)
                                // Only handle from focused terminal for security
                                if Some(terminal_id) == focused_id {
                                    // If running inside tmux, use passthrough to reach outer terminal
                                    if self.tmux.is_nested() && self.config.tmux.passthrough {
                                        // Send OSC 52 wrapped in tmux passthrough
                                        use base64::Engine;
                                        let encoded =
                                            base64::engine::general_purpose::STANDARD.encode(&data);
                                        let osc52 = format!("\x1b]52;c;{}\x07", encoded);
                                        let passthrough =
                                            TmuxController::wrap_passthrough(osc52.as_bytes());
                                        // Write to stdout to reach outer terminal
                                        use std::io::Write;
                                        let _ = std::io::stdout().write_all(&passthrough);
                                        let _ = std::io::stdout().flush();
                                        tracing::debug!(
                                            "Set clipboard via OSC 52 passthrough: {} chars",
                                            data.len()
                                        );
                                    } else if let Some(ref mut cb) = self.clipboard {
                                        if let Err(e) = cb.set_text(&data) {
                                            tracing::warn!("Failed to set clipboard: {}", e);
                                        } else {
                                            tracing::debug!(
                                                "Set clipboard via OSC 52: {} chars",
                                                data.len()
                                            );
                                        }
                                    }
                                }
                            }
                            TerminalEvent::Notification { title, body } => {
                                // Desktop notification (OSC 9/777)
                                // Only show from focused terminal
                                if Some(terminal_id) == focused_id {
                                    Self::show_notification(&title, &body);
                                }
                            }
                        }
                    }
                }

                // Clean up exited terminals
                self.terminal_manager.cleanup_exited();
                self.update_tab_bar();

                // Check if all terminals have exited
                if self.terminal_manager.all_exited() {
                    tracing::info!("All terminals exited");
                    return true;
                }

                // Update URL detection when content changes
                if content_changed {
                    self.update_url_detection();
                }

                // Check for column mode resize requests (DECCOLM - 132/80 column mode)
                self.handle_column_resize_requests();
            }
            Err(e) => {
                tracing::error!("Terminal error: {}", e);
                return true;
            }
        }

        // Check if bell animation needs to continue
        if let Some(until) = self.bell_active_until {
            if Instant::now() < until {
                self.needs_redraw = true;
            } else {
                // Bell expired, clear it and redraw to remove border
                self.bell_active_until = None;
                self.needs_redraw = true;
            }
        }

        // Render if needed (limit to ~60fps)
        // Suppress rendering during synchronized update (mode 2026) to prevent
        // partial frames, but enforce a 200ms safety timeout to avoid indefinite
        // suppression if the app never sends ESU.
        let now = Instant::now();
        let sync_active = self
            .terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal().modes().synchronized_update)
            .unwrap_or(false);
        let frame_elapsed = now.duration_since(self.last_render);

        if self.needs_redraw
            && frame_elapsed >= Duration::from_millis(16)
            && (!sync_active || frame_elapsed >= Duration::from_millis(200))
        {
            self.render();
            self.last_render = now;
            self.needs_redraw = false;
        }

        false
    }
}
