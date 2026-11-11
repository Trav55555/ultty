//! Main update loop: process terminal events, poll servers, and schedule renders.

use std::time::{Duration, Instant};

use super::App;
use ultty::config::{BehaviorConfig, ClipboardWrite};
use ultty::event_loop::TerminalEvent;
use ultty::terminal_manager::progress_status;

impl App {
    /// When the loop next has timed work to do, or `None` to sleep until
    /// input, PTY output, a socket or the config watcher wakes it.
    pub(super) fn next_update_deadline(&self, now: Instant) -> Option<Instant> {
        WakeState {
            needs_redraw: self.needs_redraw,
            frame_timer: self.frame_pacing.frame_timer_applies(),
            last_render: self.last_render,
            sync_active: self.needs_redraw && self.sync_update_active(),
            cursor_blink: self.cursor_blink.next_toggle(),
            tab_width_freeze: self.tab_width_freeze.map(|freeze| freeze.until),
            bell: self.bell_active_until,
            launched_at: self.launched_at,
            tab_title_activity: self.tab_title_activity,
            last_tab_title_refresh: self.last_tab_title_refresh,
            config_reload: self
                .config_watcher
                .as_ref()
                .and_then(|watcher| watcher.next_deadline()),
            native_sessions: self
                .protocol_server
                .as_ref()
                .map_or(0, |server| server.session_count()),
            control_wait: self.next_control_wait_deadline(),
            anchor_release: self
                .protocol_server
                .as_ref()
                .and_then(|server| server.next_pairing_deadline()),
            accessibility: self
                .accessibility
                .as_ref()
                .and_then(|adapter| adapter.next_deadline()),
        }
        .next_wake(now)
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

    /// Pair native sessions' inline surfaces with the anchors in their
    /// tabs' output (ADR 0020). A terminal gets a pass when it had output
    /// (`with_output`) or still has surfaces to place or follow; the pass
    /// also tells apps when a surface scrolls off or back onto the screen.
    /// Returns whether a surface was written into the focused pane's rows,
    /// which changes what a screen reader is shown.
    fn pair_inline_surfaces(
        &mut self,
        with_output: &[ultty::terminal_manager::TerminalId],
        now: Instant,
    ) -> bool {
        let Some(server) = self.protocol_server.as_mut() else {
            return false;
        };
        let focused = self.terminal_manager.focused_terminal().map(|t| t.id);
        let mut focused_written = false;
        let active = self.terminal_manager.active_tab().map(|tab| tab.id);
        for tab in self.terminal_manager.tabs() {
            let single_pane = tab.terminals.len() == 1;
            for &terminal_id in &tab.terminals {
                if !with_output.contains(&terminal_id) && !server.needs_pairing(tab.id, terminal_id)
                {
                    continue;
                }
                let Some(managed) = self.terminal_manager.terminal(terminal_id) else {
                    continue;
                };
                let mut terminal = managed.event_loop.terminal();
                let wrote = server.pair_inline_surfaces(
                    tab.id,
                    terminal_id,
                    &mut terminal,
                    single_pane,
                    single_pane && Some(tab.id) == active,
                    now,
                );
                focused_written |= wrote && Some(terminal_id) == focused;
            }
        }
        server.expire_unplaced_surfaces(now);
        let terminal_manager = &self.terminal_manager;
        server.retain_pairings(|terminal_id| terminal_manager.terminal(terminal_id).is_some());
        // A surface placed or dropped just now is drawn in this frame.
        if server.active_tab_sessions().any(|s| s.is_render_dirty()) {
            self.needs_redraw = true;
        }
        focused_written
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
            if watcher.poll(Instant::now()) {
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

        // Sessions that just connected learn their pane; panes that were
        // laid out again since the last update size their sessions.
        self.bind_native_sessions();
        self.sync_native_viewports_if_panes_changed();

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
                let mut with_output = Vec::new();
                // What a screen reader is shown changed (ADR 0021 §3).
                let mut shown_changed = false;
                let subscribed = self
                    .control_server
                    .as_ref()
                    .is_some_and(|server| server.has_subscribers());
                for (terminal_id, events) in all_events {
                    for event in events {
                        self.observe_control_wait_event(terminal_id, &event);
                        if subscribed {
                            if let (Some(server), Some(control_event)) = (
                                self.control_server.as_mut(),
                                super::control_events::control_event(terminal_id, &event),
                            ) {
                                server.publish(&control_event);
                            }
                        }
                        match event {
                            TerminalEvent::Redraw => {
                                with_output.push(terminal_id);
                                // A background tab's output is drawn when
                                // the tab is shown.
                                if self.terminal_manager.is_on_screen(terminal_id) {
                                    self.needs_redraw = true;
                                }
                                // Output may come with a new foreground
                                // program or directory; see the tab titles.
                                self.tab_title_activity = Some(now);
                                // Only update URL detection for focused terminal
                                if Some(terminal_id) == focused_id {
                                    content_changed = true;
                                    shown_changed = true;
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
                                shown_changed = true;
                                // Only set window title from focused terminal
                                if Some(terminal_id) == focused_id {
                                    self.window.set_title(&title);
                                    if let Some(adapter) = self.accessibility.as_mut() {
                                        adapter.set_window_title(&title);
                                    }
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
                            TerminalEvent::Progress(progress) => {
                                if let Some(terminal) =
                                    self.terminal_manager.terminal_mut(terminal_id)
                                {
                                    terminal.progress = progress_status(progress);
                                    tab_titles_changed = true;
                                }
                            }
                            TerminalEvent::CommandFinished { .. } => {
                                // A program that died mid-way leaves its
                                // progress behind; the prompt clears it.
                                if let Some(terminal) =
                                    self.terminal_manager.terminal_mut(terminal_id)
                                {
                                    tab_titles_changed |= terminal.progress.take().is_some();
                                }
                            }
                            // Only subscribers hear of this (above).
                            TerminalEvent::CommandStarted => {}
                        }
                    }
                }

                // Clean up exited terminals
                self.terminal_manager.cleanup_exited();
                shown_changed |= self.pair_inline_surfaces(&with_output, now);
                let tab_state_after_cleanup = (
                    self.terminal_manager.tab_count(),
                    self.terminal_manager.active_tab().map(|tab| tab.id),
                );
                let tab_structure_changed = tab_state_before_cleanup != tab_state_after_cleanup;
                if tab_title_refresh_due(
                    now,
                    self.last_tab_title_refresh,
                    self.tab_title_activity.is_some(),
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

                self.update_accessibility(shown_changed, Instant::now());
            }
            Err(e) => {
                tracing::error!("Terminal error: {}", e);
                return true;
            }
        }

        self.publish_pane_changes();
        self.complete_pending_control_waits();
        self.complete_pending_override_pushes();

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

        // Ask for a frame when one is due; it is drawn in `redraw` when the
        // window system delivers `RedrawRequested` (see `pacing`).
        let due = self.frame_due(Instant::now()).is_some();
        if self.frame_pacing.should_request(due) {
            self.window.request_redraw();
        }

        false
    }

    /// Draw a frame for `WindowEvent::RedrawRequested`, if one is due.
    pub(super) fn redraw(&mut self) {
        let now = Instant::now();
        let due = self.frame_due(now);
        if !self.frame_pacing.should_draw(due.is_some()) {
            return;
        }
        if let (true, Some((frame_elapsed, sync_active))) = (pipeline_trace_enabled(), due) {
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

    /// The window was hidden or shown again (`WindowEvent::Occluded`).
    pub(super) fn set_occluded(&mut self, occluded: bool) {
        if self.frame_pacing.set_occluded(occluded) {
            self.needs_redraw = true;
        }
    }

    /// Whether a frame is due at `now`, with the time since the last frame
    /// and whether a synchronized update is active, for the pipeline trace.
    ///
    /// Frames are limited to about 60 per second. A synchronized update
    /// (mode 2026) holds them to prevent partial frames, but only up to a
    /// 200 ms safety timeout in case the app never ends it.
    fn frame_due(&self, now: Instant) -> Option<(Duration, bool)> {
        let sync_active = self.sync_update_active();
        let frame_elapsed = now.duration_since(self.last_render);
        let echo_after = self
            .input_sent_at
            .filter(|_| self.output_since_input)
            .map(|sent| now.saturating_duration_since(sent));
        should_render(self.needs_redraw, frame_elapsed, sync_active, echo_after)
            .then_some((frame_elapsed, sync_active))
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

/// Whether to refresh the tab titles (foreground program, directory and
/// branch in the title band) now. They are looked up, at most once per
/// [`TAB_TITLE_REFRESH_INTERVAL`](super::TAB_TITLE_REFRESH_INTERVAL), only
/// after PTY output (`activity`), or at once when `force`d by a title or tab
/// change. A pane with no output has nothing new worth a periodic wakeup.
fn tab_title_refresh_due(now: Instant, last_refresh: Instant, activity: bool, force: bool) -> bool {
    if force {
        return true;
    }
    activity
        && now
            .checked_duration_since(last_refresh)
            .is_some_and(|elapsed| elapsed >= super::TAB_TITLE_REFRESH_INTERVAL)
}

/// The activity mark left after a title refresh at `now`. Activity within
/// the last interval keeps it, so one more refresh follows a full interval
/// after the output stops: a program started by the line just echoed has
/// had that long to take the foreground.
pub(super) fn title_activity_after_refresh(
    activity: Option<Instant>,
    now: Instant,
) -> Option<Instant> {
    activity.filter(|at| now.saturating_duration_since(*at) < super::TAB_TITLE_REFRESH_INTERVAL)
}

/// What the loop's own timers depend on, gathered from [`App`].
#[derive(Debug, Clone, Copy)]
struct WakeState {
    /// A frame is waiting to be drawn.
    needs_redraw: bool,
    /// The frame is up to the loop's timer, not to a redraw request in
    /// flight or an occluded window; see `FramePacing::frame_timer_applies`.
    frame_timer: bool,
    last_render: Instant,
    /// The focused pane holds frames for a synchronized update.
    sync_active: bool,
    /// The cursor's next blink toggle, if it blinks.
    cursor_blink: Option<Instant>,
    tab_width_freeze: Option<Instant>,
    /// When the visual bell's flash ends.
    bell: Option<Instant>,
    /// When the window opened; the title types itself out after that.
    launched_at: Instant,
    /// PTY output not yet followed by a settled tab title refresh.
    tab_title_activity: Option<Instant>,
    last_tab_title_refresh: Instant,
    /// When a changed config file is due for reload.
    config_reload: Option<Instant>,
    /// Connected native sessions: their keepalives and animations are
    /// checked on the maintenance tick.
    native_sessions: usize,
    /// The earliest `ulttyctl wait-for` timeout or quiet-period end. Text
    /// and events wake the UI on their own, so waits need no tick.
    control_wait: Option<Instant>,
    /// When an inline surface anchor that no surface claimed is released.
    anchor_release: Option<Instant>,
    /// When a coalesced accessibility tree is due; `None` unless a screen
    /// reader is listening and the screen changed.
    accessibility: Option<Instant>,
}

impl WakeState {
    /// The earliest timer, or `None` when nothing is pending. An idle
    /// window with a steady cursor and no native or control clients has no
    /// timer at all.
    fn next_wake(&self, now: Instant) -> Option<Instant> {
        let frame = (self.needs_redraw && self.frame_timer)
            .then(|| self.last_render + render_delay_for_sync_state(self.sync_active));
        let maintenance =
            (self.native_sessions > 0).then(|| now + super::MAINTENANCE_POLL_INTERVAL);
        let title_animation =
            ultty::tabs::title_next_change(now.saturating_duration_since(self.launched_at))
                .map(|next| self.launched_at + next);
        let tab_titles = self
            .tab_title_activity
            .map(|_| self.last_tab_title_refresh + super::TAB_TITLE_REFRESH_INTERVAL);
        [
            frame,
            frame.and_then(|render| catchup_poll_deadline(now, render, true)),
            self.cursor_blink,
            self.tab_width_freeze,
            self.bell,
            title_animation,
            tab_titles,
            self.config_reload,
            maintenance,
            self.control_wait,
            self.anchor_release,
            self.accessibility,
        ]
        .into_iter()
        .flatten()
        .min()
        .map(|deadline| deadline.max(now))
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
        tab_title_refresh_due, title_activity_after_refresh, BellResponse, WakeState,
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

    const TITLE_INTERVAL: Duration = super::super::TAB_TITLE_REFRESH_INTERVAL;

    #[test]
    fn tab_title_refresh_due_respects_interval() {
        let now = Instant::now();

        assert!(!tab_title_refresh_due(
            now,
            now - TITLE_INTERVAL + Duration::from_millis(1),
            true,
            false
        ));
        assert!(tab_title_refresh_due(
            now,
            now - TITLE_INTERVAL,
            true,
            false
        ));
    }

    #[test]
    fn tab_titles_are_not_refreshed_without_output() {
        let now = Instant::now();
        assert!(!tab_title_refresh_due(
            now,
            now - TITLE_INTERVAL * 10,
            false,
            false
        ));
    }

    #[test]
    fn tab_title_refresh_due_honors_force() {
        let now = Instant::now();

        assert!(tab_title_refresh_due(now, now, false, true));
    }

    #[test]
    fn tab_title_refresh_due_ignores_future_last_refresh() {
        let now = Instant::now();

        assert!(!tab_title_refresh_due(
            now,
            now + Duration::from_millis(1),
            true,
            false
        ));
    }

    #[test]
    fn title_refresh_follows_output_by_a_full_interval_then_stops() {
        let output = Instant::now();
        // The refresh the output triggers right away keeps the mark ...
        assert_eq!(
            title_activity_after_refresh(Some(output), output),
            Some(output)
        );
        // ... so one more runs an interval later, which clears it.
        assert_eq!(
            title_activity_after_refresh(Some(output), output + TITLE_INTERVAL),
            None
        );
        assert_eq!(title_activity_after_refresh(None, output), None);
    }

    /// The state of a window that has finished launching, drawn its last
    /// frame and has nothing pending.
    fn idle(now: Instant) -> WakeState {
        WakeState {
            needs_redraw: false,
            frame_timer: true,
            last_render: now - Duration::from_secs(5),
            sync_active: false,
            cursor_blink: None,
            tab_width_freeze: None,
            bell: None,
            launched_at: now - Duration::from_secs(60),
            tab_title_activity: None,
            last_tab_title_refresh: now - Duration::from_secs(5),
            config_reload: None,
            native_sessions: 0,
            control_wait: None,
            anchor_release: None,
            accessibility: None,
        }
    }

    #[test]
    fn an_idle_window_with_a_steady_cursor_never_wakes() {
        let now = Instant::now();
        assert_eq!(idle(now).next_wake(now), None);
    }

    #[test]
    fn a_blinking_cursor_wakes_only_for_its_toggle() {
        let now = Instant::now();
        let toggle = now + Duration::from_millis(300);
        let state = WakeState {
            cursor_blink: Some(toggle),
            ..idle(now)
        };
        assert_eq!(state.next_wake(now), Some(toggle));
    }

    #[test]
    fn the_launch_title_animation_wakes_until_it_has_typed_out() {
        let now = Instant::now();
        let launching = WakeState {
            launched_at: now,
            ..idle(now)
        };
        assert!(launching.next_wake(now).is_some());
    }

    #[test]
    fn output_schedules_one_title_refresh() {
        let now = Instant::now();
        let refreshed = now - Duration::from_millis(100);
        let state = WakeState {
            tab_title_activity: Some(now),
            last_tab_title_refresh: refreshed,
            ..idle(now)
        };
        assert_eq!(state.next_wake(now), Some(refreshed + TITLE_INTERVAL));
    }

    #[test]
    fn native_sessions_keep_the_maintenance_tick() {
        let now = Instant::now();
        let session = WakeState {
            native_sessions: 1,
            ..idle(now)
        };
        assert_eq!(
            session.next_wake(now),
            Some(now + super::super::MAINTENANCE_POLL_INTERVAL)
        );
    }

    #[test]
    fn a_pending_wait_wakes_at_its_deadline_not_on_a_tick() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(30);
        let wait = WakeState {
            control_wait: Some(deadline),
            ..idle(now)
        };
        assert_eq!(wait.next_wake(now), Some(deadline));
    }

    /// An anchor no surface claimed is released on time even when nothing
    /// else would wake the window (no native session, so no tick).
    #[test]
    fn a_waiting_anchor_wakes_at_its_release() {
        let now = Instant::now();
        let release = now + Duration::from_millis(1500);
        let state = WakeState {
            anchor_release: Some(release),
            ..idle(now)
        };
        assert_eq!(state.next_wake(now), Some(release));
        assert_eq!(idle(now).next_wake(now), None);
    }

    #[test]
    fn a_coalesced_accessibility_tree_wakes_at_its_deadline() {
        let now = Instant::now();
        let due = now + Duration::from_millis(30);
        let state = WakeState {
            accessibility: Some(due),
            ..idle(now)
        };
        assert_eq!(state.next_wake(now), Some(due));
    }

    #[test]
    fn a_pending_frame_wakes_for_the_frame_and_catchup_polls() {
        let now = Instant::now();
        let state = WakeState {
            needs_redraw: true,
            last_render: now,
            ..idle(now)
        };
        assert_eq!(
            state.next_wake(now),
            Some(now + super::super::PTY_CATCHUP_POLL_INTERVAL)
        );
    }

    #[test]
    fn a_frame_left_to_the_window_system_sets_no_timer() {
        // A redraw request the compositor holds back (hidden window), or an
        // occluded window: the loop sleeps until something else happens.
        let now = Instant::now();
        let state = WakeState {
            needs_redraw: true,
            frame_timer: false,
            last_render: now - Duration::from_secs(5),
            ..idle(now)
        };
        assert_eq!(state.next_wake(now), None);
    }

    #[test]
    fn overdue_timers_wake_now_not_in_the_past() {
        let now = Instant::now();
        let state = WakeState {
            bell: Some(now - Duration::from_millis(5)),
            config_reload: Some(now + Duration::from_millis(5)),
            ..idle(now)
        };
        assert_eq!(state.next_wake(now), Some(now));
    }

    #[test]
    fn instant_min_deadline_does_not_move_back_before_now() {
        let now = Instant::now();
        let earlier = now - Duration::from_millis(1);
        assert_eq!(earlier.max(now), now);
    }
}
