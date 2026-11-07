//! Main update loop: process terminal events, poll servers, and schedule renders.

use std::time::{Duration, Instant};

use super::App;
use ultty::config::{BehaviorConfig, ClipboardWrite};
use ultty::event_loop::TerminalEvent;

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

        if let Some(toggle) = self.cursor_blink.next_toggle() {
            deadline = deadline.min(toggle);
        }

        if let Some(freeze) = self.tab_width_freeze {
            deadline = deadline.min(freeze.until);
        }

        if let Some(until) = self.bell_active_until {
            deadline = deadline.min(until);
        }

        if let Some(next) =
            ultty::tabs::title_next_change(now.saturating_duration_since(self.launched_at))
        {
            deadline = deadline.min(self.launched_at + next);
        }

        deadline.max(now)
    }

    fn render_delay_for_sync_state(&self, sync_active: bool) -> std::time::Duration {
        render_delay_for_sync_state(sync_active)
    }

    /// Move in-band protocol data between each tab's PTY and the protocol
    /// server (ADR 0018).
    ///
    /// Only a tab with a single pane can host an in-band session: the
    /// native scene covers the whole tab, and a pane's app cannot own it.
    /// Data from any other pane is answered with nothing, which releases the
    /// held query replies so the app's probe sees DA1 first and falls back
    /// to ANSI.
    fn pump_in_band(&mut self) {
        let tabs: Vec<(
            ultty::terminal_manager::TabId,
            Vec<ultty::terminal_manager::TerminalId>,
        )> = self
            .terminal_manager
            .tabs()
            .iter()
            .map(|tab| (tab.id, tab.terminals.clone()))
            .collect();
        for (tab_id, terminals) in tabs {
            let single = terminals.len() == 1;
            for terminal_id in terminals {
                let Some(terminal) = self.terminal_manager.terminal(terminal_id) else {
                    continue;
                };
                let event_loop = &terminal.event_loop;
                let Some(input) = event_loop.take_in_band() else {
                    continue;
                };
                let reply = match self.protocol_server.as_mut() {
                    Some(server) if single => {
                        server.in_band_input(
                            tab_id,
                            event_loop.foreground_pgrp(),
                            || event_loop.foreground_pids(),
                            &input.bytes,
                            input.broken,
                        );
                        server.take_tab_in_band_output(tab_id)
                    }
                    _ => Vec::new(),
                };
                if let Err(e) = event_loop.finish_in_band(&reply) {
                    tracing::warn!(
                        "Failed to answer in-band data on tab {}: {}",
                        tab_id.raw(),
                        e
                    );
                }
            }
        }

        let Some(server) = self.protocol_server.as_mut() else {
            return;
        };
        for tab_id in server.in_band_tabs() {
            let terminal = self
                .terminal_manager
                .tabs()
                .iter()
                .find(|tab| tab.id == tab_id)
                .filter(|tab| tab.terminals.len() == 1)
                .and_then(|tab| self.terminal_manager.terminal(tab.terminals[0]));
            let Some(terminal) = terminal else {
                // The tab was split or closed under the session.
                server.end_in_band_session(tab_id);
                continue;
            };
            let event_loop = &terminal.event_loop;
            server.end_in_band_if_app_gone(tab_id, event_loop.foreground_pgrp());
            let out = server.take_tab_in_band_output(tab_id);
            if let Err(e) = event_loop.send_in_band(&out) {
                tracing::warn!("Failed to send in-band data to tab {}: {}", tab_id.raw(), e);
                server.end_in_band_session(tab_id);
            }
        }
    }

    /// Whether the focused pane's cursor should blink: the config allows
    /// it, the window has focus, and the cursor is shown in a blinking
    /// style. Anything else draws a steady cursor and runs no blink timer.
    fn cursor_blinks(&self) -> bool {
        self.config.behavior.cursor_blink
            && self.window.has_focus()
            && self.terminal().is_some_and(|terminal| {
                let cursor = terminal.buffer().grid().cursor();
                cursor.visible && cursor.blinking
            })
    }

    fn sync_update_active(&self) -> bool {
        self.terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal().modes().synchronized_update)
            .unwrap_or(false)
    }

    /// Process terminal events and render. Returns true if should exit.
    pub(super) fn update(&mut self) -> bool {
        let letters = ultty::tabs::title_letters_at(self.launched_at.elapsed());
        if letters != self.tab_bar.title_letters {
            self.tab_bar.title_letters = letters;
            self.needs_redraw = true;
        }
        self.refresh_runtime_wakeup_fds();

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
            // Redraw when a visible native session has new content
            if server.active_tab_sessions().any(|s| s.is_render_dirty()) {
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

        self.pump_in_band();

        if let Some(ref mut server) = self.control_server {
            if let Err(e) = server.poll(Some(Duration::ZERO)) {
                tracing::warn!("Control server poll error: {}", e);
            }
            self.handle_control_requests();
        }

        self.refresh_runtime_wakeup_fds();

        // Update cursor blink state
        let now = Instant::now();
        self.expire_tab_width_freeze(now);

        if self.cursor_blink.update(self.cursor_blinks(), now) {
            self.needs_redraw = true;
        }

        // Get the focused terminal ID for title updates
        let focused_id = self.terminal_manager.focused_terminal().map(|t| t.id);
        let tab_state_before_cleanup = (
            self.terminal_manager.tab_count(),
            self.terminal_manager.active_tab().map(|tab| tab.id),
        );

        // Process all terminals
        match self.terminal_manager.process_all() {
            Ok(all_events) => {
                let mut content_changed = false;
                let mut tab_titles_changed = false;
                for (terminal_id, events) in all_events {
                    for event in events {
                        match event {
                            TerminalEvent::Redraw => {
                                self.needs_redraw = true;
                                // Only update URL detection for focused terminal
                                if Some(terminal_id) == focused_id {
                                    content_changed = true;
                                    self.output_since_input = self.input_sent_at.is_some();
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
                                let bell = bell_response(
                                    &self.config.behavior,
                                    Some(terminal_id) == focused_id,
                                    self.window.has_focus(),
                                );
                                if bell.flash {
                                    // Flash the border for 100ms
                                    self.bell_active_until =
                                        Some(Instant::now() + Duration::from_millis(100));
                                    self.needs_redraw = true;
                                }
                                if bell.urgent {
                                    // Cleared when the window gains focus.
                                    self.window.request_user_attention(Some(
                                        winit::window::UserAttentionType::Informational,
                                    ));
                                }
                                tracing::debug!("Bell");
                            }
                            TerminalEvent::TitleChanged(title) => {
                                tab_titles_changed = true;
                                // Only set window title from focused terminal
                                if Some(terminal_id) == focused_id {
                                    self.window.set_title(&title);
                                }
                            }
                            TerminalEvent::SetClipboard { clipboard: _, data } => {
                                // Set clipboard content (OSC 52), if the config allows
                                // it, and only from the focused terminal
                                if self.config.behavior.clipboard_write == ClipboardWrite::Deny {
                                    tracing::debug!(
                                        "Ignored OSC 52 clipboard write: behavior.clipboard_write = \"deny\""
                                    );
                                } else if Some(terminal_id) == focused_id {
                                    // Ultty owns a window and talks to the system clipboard
                                    // directly. It never forwards OSC 52 to its own stdout,
                                    // even when it was started from inside tmux.
                                    if let Some(ref mut cb) = self.clipboard {
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
                let tab_state_after_cleanup = (
                    self.terminal_manager.tab_count(),
                    self.terminal_manager.active_tab().map(|tab| tab.id),
                );
                let tab_structure_changed = tab_state_before_cleanup != tab_state_after_cleanup;
                if tab_title_refresh_due(
                    now,
                    self.last_tab_title_refresh,
                    tab_titles_changed || tab_structure_changed,
                ) {
                    self.update_tab_bar();
                }

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

        self.complete_pending_control_waits();

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
        let echo_after = self
            .input_sent_at
            .filter(|_| self.output_since_input)
            .map(|sent| now.saturating_duration_since(sent));

        if should_render(self.needs_redraw, frame_elapsed, sync_active, echo_after) {
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
            if self.output_since_input {
                self.input_sent_at = None;
                self.output_since_input = false;
            }
            // A frame that skipped glyphs (atlas overflow) must draw again
            // once the deferred eviction has run.
            self.needs_redraw = self.renderer.frame_incomplete();
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

/// Whether to draw a frame now.
///
/// Frames are paced to at most one per [`FRAME_INTERVAL`](super::FRAME_INTERVAL)
/// since the last one, so a flood of output is drawn in coalesced frames
/// instead of one per chunk. Waiting out that interval would add up to a
/// frame of latency to every keystroke, though, so the output that follows
/// user input is the exception: `echo_after` is how long after the input
/// the focused terminal's output arrived (`None` if none has since), and if
/// that is within [`INPUT_ECHO_WINDOW`](super::INPUT_ECHO_WINDOW) the frame
/// is drawn at once. That is one early frame per input, so a command that
/// floods output after Enter is paced again from its second frame on.
/// A synchronized update (mode 2026) holds every frame, the echo included.
fn should_render(
    needs_redraw: bool,
    frame_elapsed: std::time::Duration,
    sync_active: bool,
    echo_after: Option<std::time::Duration>,
) -> bool {
    if !needs_redraw {
        return false;
    }
    let echo_due = !sync_active && echo_after.is_some_and(|d| d <= super::INPUT_ECHO_WINDOW);
    echo_due || frame_elapsed >= render_delay_for_sync_state(sync_active)
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

fn tab_title_refresh_due(now: Instant, last_refresh: Instant, force: bool) -> bool {
    if force {
        return true;
    }

    match now.checked_duration_since(last_refresh) {
        Some(elapsed) => elapsed >= super::TAB_TITLE_REFRESH_INTERVAL,
        None => false,
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

/// What a BEL from a pane does.
#[derive(Debug, PartialEq, Eq)]
struct BellResponse {
    /// Flash the window border.
    flash: bool,
    /// Ask the window manager to mark the window as wanting attention.
    urgent: bool,
}

/// The response to a BEL. Only the focused pane flashes; any pane marks an
/// unfocused window as wanting attention.
fn bell_response(
    behavior: &BehaviorConfig,
    from_focused_pane: bool,
    window_focused: bool,
) -> BellResponse {
    BellResponse {
        flash: behavior.bell_visual && from_focused_pane,
        urgent: behavior.bell_urgent && !window_focused,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        bell_response, catchup_poll_deadline, render_delay_for_sync_state, should_render,
        tab_title_refresh_due, BellResponse,
    };
    use ultty::config::BehaviorConfig;

    #[test]
    fn bell_flashes_only_the_focused_pane() {
        let behavior = BehaviorConfig::default();
        assert_eq!(
            bell_response(&behavior, true, true),
            BellResponse {
                flash: true,
                urgent: false
            }
        );
        assert_eq!(
            bell_response(&behavior, false, true),
            BellResponse {
                flash: false,
                urgent: false
            }
        );
    }

    #[test]
    fn bell_in_an_unfocused_window_asks_for_attention() {
        let behavior = BehaviorConfig::default();
        assert!(bell_response(&behavior, false, false).urgent);

        let quiet = BehaviorConfig {
            bell_visual: false,
            bell_urgent: false,
            ..BehaviorConfig::default()
        };
        assert_eq!(
            bell_response(&quiet, true, false),
            BellResponse {
                flash: false,
                urgent: false
            }
        );
    }

    #[test]
    fn should_render_when_dirty_and_frame_interval_elapsed() {
        assert!(should_render(true, Duration::from_millis(16), false, None));
        assert!(!should_render(true, Duration::from_millis(15), false, None));
    }

    #[test]
    fn should_not_render_when_redraw_is_not_dirty() {
        assert!(!should_render(false, Duration::from_secs(1), false, None));
        let echo = Some(Duration::from_millis(1));
        assert!(!should_render(false, Duration::ZERO, false, echo));
    }

    #[test]
    fn should_hold_synchronized_update_until_timeout() {
        assert!(!should_render(true, Duration::from_millis(199), true, None));
        assert!(should_render(true, Duration::from_millis(200), true, None));
    }

    #[test]
    fn echo_of_input_renders_without_waiting_for_the_frame_interval() {
        let just_drew = Duration::from_millis(2);
        let echo = |ms| Some(Duration::from_millis(ms));
        assert!(should_render(true, just_drew, false, echo(3)));
        assert!(should_render(true, just_drew, false, echo(50)));
    }

    #[test]
    fn output_long_after_input_is_paced_like_any_output() {
        let late = Some(Duration::from_millis(51));
        assert!(!should_render(true, Duration::from_millis(2), false, late));
        assert!(should_render(true, Duration::from_millis(16), false, late));
    }

    #[test]
    fn synchronized_update_holds_the_echo_too() {
        let echo = Some(Duration::from_millis(3));
        assert!(!should_render(true, Duration::from_millis(2), true, echo));
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
    fn tab_title_refresh_due_respects_interval() {
        let now = Instant::now();

        assert!(!tab_title_refresh_due(
            now,
            now - super::super::TAB_TITLE_REFRESH_INTERVAL + Duration::from_millis(1),
            false
        ));
        assert!(tab_title_refresh_due(
            now,
            now - super::super::TAB_TITLE_REFRESH_INTERVAL,
            false
        ));
    }

    #[test]
    fn tab_title_refresh_due_honors_force() {
        let now = Instant::now();

        assert!(tab_title_refresh_due(now, now, true));
    }

    #[test]
    fn tab_title_refresh_due_ignores_future_last_refresh() {
        let now = Instant::now();

        assert!(!tab_title_refresh_due(
            now,
            now + Duration::from_millis(1),
            false
        ));
    }

    #[test]
    fn instant_min_deadline_does_not_move_back_before_now() {
        let now = Instant::now();
        let earlier = now - Duration::from_millis(1);
        assert_eq!(earlier.max(now), now);
    }
}
