//! Main update loop: process terminal events, poll servers, and schedule renders.

use std::time::{Duration, Instant};

use super::App;
use ultty::event_loop::TerminalEvent;
use ultty::tmux::TmuxController;

impl App {
    pub(super) fn next_update_deadline(&self, now: Instant) -> Instant {
        let mut deadline = now + super::MAINTENANCE_POLL_INTERVAL;

        if self.needs_redraw {
            let render_deadline =
                self.last_render + self.render_delay_for_sync_state(self.sync_update_active());
            deadline = deadline.min(render_deadline);
            if let Some(catchup_deadline) =
                catchup_poll_deadline(now, render_deadline, self.needs_redraw)
            {
                deadline = deadline.min(catchup_deadline);
            }
        }

        deadline = deadline.min(self.last_cursor_blink + super::CURSOR_BLINK_INTERVAL);

        if let Some(until) = self.bell_active_until {
            deadline = deadline.min(until);
        }

        deadline.max(now)
    }

    fn render_delay_for_sync_state(&self, sync_active: bool) -> std::time::Duration {
        render_delay_for_sync_state(sync_active)
    }

    fn sync_update_active(&self) -> bool {
        self.terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal().modes().synchronized_update)
            .unwrap_or(false)
    }

    /// Process terminal events and render. Returns true if should exit.
    pub(super) fn update(&mut self) -> bool {
        self.refresh_pty_wakeup_fds();

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
            // Redraw when any native session has new content
            if server.sessions().any(|s| s.is_render_dirty()) {
                self.needs_redraw = true;
            }
            // Check keepalive timeouts
            server.check_keepalives();

            // Handle clipboard operations from native sessions
            let gets = server.pending_clipboard_gets();
            for key in gets {
                let text = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok());
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
        let sync_active = self.sync_update_active();
        let frame_elapsed = now.duration_since(self.last_render);

        if should_render(self.needs_redraw, frame_elapsed, sync_active) {
            if pipeline_trace_enabled() {
                tracing::info!(
                    target: "ultty::perf",
                    frame_elapsed_us = frame_elapsed.as_micros(),
                    sync_active,
                    "pipeline_render_scheduled"
                );
            }
            self.render();
            self.last_render = now;
            self.needs_redraw = false;
        }

        false
    }
}

fn render_delay_for_sync_state(sync_active: bool) -> std::time::Duration {
    if sync_active {
        super::SYNC_UPDATE_TIMEOUT
    } else {
        super::FRAME_INTERVAL
    }
}

fn should_render(
    needs_redraw: bool,
    frame_elapsed: std::time::Duration,
    sync_active: bool,
) -> bool {
    needs_redraw && frame_elapsed >= render_delay_for_sync_state(sync_active)
}

fn catchup_poll_deadline(
    now: Instant,
    render_deadline: Instant,
    needs_redraw: bool,
) -> Option<Instant> {
    if needs_redraw && render_deadline > now {
        Some(now + super::PTY_CATCHUP_POLL_INTERVAL)
    } else {
        None
    }
}

pub(super) fn pipeline_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("ULTTY_TRACE_PIPELINE").is_ok_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{catchup_poll_deadline, render_delay_for_sync_state, should_render};

    #[test]
    fn should_render_when_dirty_and_frame_interval_elapsed() {
        assert!(should_render(true, Duration::from_millis(16), false));
    }

    #[test]
    fn should_not_render_when_redraw_is_not_dirty() {
        assert!(!should_render(false, Duration::from_secs(1), false));
    }

    #[test]
    fn should_hold_synchronized_update_until_timeout() {
        assert!(!should_render(true, Duration::from_millis(199), true));
        assert!(should_render(true, Duration::from_millis(200), true));
    }

    #[test]
    fn render_delay_uses_frame_interval_without_sync() {
        assert_eq!(
            render_delay_for_sync_state(false),
            Duration::from_millis(16)
        );
    }

    #[test]
    fn render_delay_uses_sync_timeout_with_sync() {
        assert_eq!(
            render_delay_for_sync_state(true),
            Duration::from_millis(200)
        );
    }

    #[test]
    fn catchup_poll_deadline_wakes_before_next_render() {
        let now = Instant::now();
        assert_eq!(
            catchup_poll_deadline(now, now + Duration::from_millis(16), true),
            Some(now + Duration::from_millis(1))
        );
    }

    #[test]
    fn catchup_poll_deadline_ignores_clean_or_due_frames() {
        let now = Instant::now();
        assert_eq!(
            catchup_poll_deadline(now, now + Duration::from_millis(16), false),
            None
        );
        assert_eq!(catchup_poll_deadline(now, now, true), None);
    }

    #[test]
    fn instant_min_deadline_does_not_move_back_before_now() {
        let now = Instant::now();
        let earlier = now - Duration::from_millis(1);
        assert_eq!(earlier.max(now), now);
    }
}
