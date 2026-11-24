//! Main update loop: process terminal events, poll servers, and schedule renders.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::{App, WindowCtx, WindowState};
use ultty::config::{BehaviorConfig, ClipboardWrite};
use ultty::event_loop::TerminalEvent;
use ultty::terminal_manager::{progress_status, WindowKey};
use winit::event_loop::ActiveEventLoop;

impl WindowState {
    fn sync_update_active(&self, manager: &ultty::terminal_manager::TerminalManager) -> bool {
        manager
            .window_focused_terminal(self.key)
            .map(|t| t.event_loop.terminal().modes().synchronized_update)
            .unwrap_or(false)
    }

    fn cursor_blinks(
        &self,
        config: &ultty::config::Config,
        manager: &ultty::terminal_manager::TerminalManager,
    ) -> bool {
        config.behavior.cursor_blink
            && self.window.has_focus()
            && manager.window_focused_terminal(self.key).is_some_and(|t| {
                let term = t.event_loop.terminal();
                let cursor = term.buffer().grid().cursor();
                cursor.visible && cursor.blinking
            })
    }

    pub(super) fn set_occluded(&mut self, occluded: bool) {
        if self.frame_pacing.set_occluded(occluded) {
            self.needs_redraw = true;
        }
    }

    pub(super) fn frame_due(
        &self,
        manager: &ultty::terminal_manager::TerminalManager,
        now: Instant,
    ) -> Option<(Duration, bool)> {
        let sync_active = self.sync_update_active(manager);
        let frame_elapsed = now.duration_since(self.last_render);
        let echo_after = self
            .input_sent_at
            .filter(|_| self.output_since_input)
            .map(|sent| now.saturating_duration_since(sent));
        let needs_redraw = self.needs_redraw && self.can_draw();
        should_render(needs_redraw, frame_elapsed, sync_active, echo_after)
            .then_some((frame_elapsed, sync_active))
    }

    fn next_window_wake(
        &self,
        manager: &ultty::terminal_manager::TerminalManager,
        now: Instant,
    ) -> Option<Instant> {
        WakeState {
            needs_redraw: self.needs_redraw,
            frame_timer: self.frame_pacing.frame_timer_applies() && self.can_draw(),
            last_render: self.last_render,
            sync_active: self.needs_redraw && self.sync_update_active(manager),
            cursor_blink: self.cursor_blink.next_toggle(),
            tab_width_freeze: self.tab_width_freeze.map(|freeze| freeze.until),
            bell: self.bell_active_until,
            launched_at: self.launched_at,
            tab_title_activity: self.tab_title_activity,
            last_tab_title_refresh: self.last_tab_title_refresh,
            config_reload: None,
            native_sessions: 0,
            control_wait: None,
            anchor_release: None,
            search_pending: self.search_job.is_some(),
            accessibility: self
                .accessibility
                .as_ref()
                .and_then(|adapter| adapter.next_deadline()),
        }
        .next_wake(now)
    }
}

impl WindowCtx<'_> {
    /// Draw a frame for `WindowEvent::RedrawRequested`, if one is due.
    pub(super) fn redraw(&mut self) {
        self.reconcile_copy_mode_pane();
        let now = Instant::now();
        let due = self.win.frame_due(&self.app.runtime.manager, now);
        if !self.win.frame_pacing.should_draw(due.is_some()) {
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
        self.win.last_render = now;
        if self.win.output_since_input {
            self.win.input_sent_at = None;
            self.win.output_since_input = false;
        }
        self.win.needs_redraw = self
            .win
            .renderer
            .ready()
            .is_some_and(|renderer| renderer.frame_incomplete());
    }
}

impl App {
    /// When the loop next has timed work to do across every open window and
    /// the shared runtime, or `None` to sleep until input, PTY output, a
    /// socket or the config watcher wakes it.
    pub(super) fn next_update_deadline(&self, now: Instant) -> Option<Instant> {
        let process_wake = WakeState {
            needs_redraw: false,
            frame_timer: false,
            last_render: now,
            sync_active: false,
            cursor_blink: None,
            tab_width_freeze: None,
            bell: None,
            launched_at: now.checked_sub(Duration::from_secs(60)).unwrap_or(now),
            tab_title_activity: None,
            last_tab_title_refresh: now,
            config_reload: self
                .shared
                .config_watcher
                .as_ref()
                .and_then(|watcher| watcher.next_deadline()),
            native_sessions: self
                .shared
                .runtime
                .protocol
                .as_ref()
                .map_or(0, |server| server.session_count()),
            control_wait: self.next_control_wait_deadline(),
            anchor_release: self
                .shared
                .runtime
                .protocol
                .as_ref()
                .and_then(|server| server.next_pairing_deadline()),
            search_pending: false,
            accessibility: None,
        }
        .next_wake(now);

        self.windows
            .values()
            .filter_map(|win| win.next_window_wake(&self.shared.runtime.manager, now))
            .chain(process_wake)
            .min()
    }

    /// Process terminal events across all windows and schedule redraws.
    /// Returns `true` if the application should exit.
    pub(super) fn update(&mut self, event_loop: &ActiveEventLoop) -> bool {
        for win in self.windows.values_mut() {
            let letters = ultty::tabs::title_letters_at(win.launched_at.elapsed());
            if letters != win.tab_bar.title_letters {
                win.tab_bar.title_letters = letters;
                win.needs_redraw = true;
            }
        }
        self.shared.refresh_runtime_wakeup_fds();

        let reload_due = self
            .shared
            .config_watcher
            .as_mut()
            .is_some_and(|watcher| watcher.poll(Instant::now()));
        if reload_due {
            tracing::info!("Config file changed, reloading...");
            self.reload_config();
        }

        match self.shared.runtime.poll_native() {
            Ok(redraw) => {
                if redraw {
                    self.mark_dirty_native_windows();
                }
            }
            Err(error) => tracing::warn!(%error, "Protocol server poll error"),
        }
        if let Some(server) = self.shared.runtime.protocol.as_mut() {
            for key in server.pending_clipboard_gets() {
                let text = self
                    .shared
                    .clipboard
                    .as_mut()
                    .and_then(|cb| cb.get_text().ok());
                server.send_clipboard_content(key, text);
            }
            for (_key, text) in server.pending_clipboard_sets() {
                if let Some(ref mut cb) = self.shared.clipboard {
                    if let Err(error) = cb.set_text(text) {
                        tracing::warn!(%error, "Failed to set clipboard from native session");
                    }
                }
            }
        }

        if let Some(ref mut server) = self.shared.control_server {
            if let Err(e) = server.poll(Some(Duration::ZERO)) {
                tracing::warn!("Control server poll error: {}", e);
            }
            if self.handle_control_requests(event_loop) {
                self.shared.publish_pane_changes();
                return true;
            }
        }

        self.shared.refresh_runtime_wakeup_fds();

        let now = Instant::now();
        for win in self.windows.values_mut() {
            let mut ctx = WindowCtx {
                app: &mut self.shared,
                win,
            };
            ctx.expire_tab_width_freeze(now);
            let blinks = ctx
                .win
                .cursor_blinks(&ctx.app.config, &ctx.app.runtime.manager);
            if ctx.win.cursor_blink.update(blinks, now) {
                ctx.win.needs_redraw = true;
            }
        }

        let tab_state_before: Vec<(WindowKey, usize, Option<ultty::terminal_manager::TabId>)> =
            self.windows
                .keys()
                .copied()
                .map(|key| {
                    (
                        key,
                        self.shared.runtime.manager.window_tab_count(key),
                        self.shared
                            .runtime
                            .manager
                            .window_active_tab(key)
                            .map(|tab| tab.id),
                    )
                })
                .collect();

        match self.shared.runtime.manager.process_all() {
            Ok(all_events) => {
                let mut content_changed_windows: HashSet<WindowKey> = HashSet::new();
                let mut tab_titles_changed_windows: HashSet<WindowKey> = HashSet::new();
                let mut shown_changed_windows: HashSet<WindowKey> = HashSet::new();
                let mut with_output = Vec::new();

                for (terminal_id, events) in all_events {
                    let owner_window = self.shared.runtime.manager.window_for_pane(terminal_id);
                    let is_window_focused_pane = owner_window.is_some_and(|w| {
                        self.shared.runtime.manager.window_focused_terminal_id(w)
                            == Some(terminal_id)
                    });
                    let is_global_focused_pane = self.focused_window.is_some_and(|w| {
                        owner_window == Some(w)
                            && self.shared.runtime.manager.window_focused_terminal_id(w)
                                == Some(terminal_id)
                    });

                    for event in events {
                        self.shared.observe_control_wait_event(terminal_id, &event);
                        if let (Some(server), Some(control_event)) = (
                            self.shared.control_server.as_mut(),
                            super::control_events::control_event(
                                terminal_id,
                                owner_window.map(|w| w.raw() as u64),
                                &event,
                            ),
                        ) {
                            server.publish(&control_event);
                        }
                        match event {
                            TerminalEvent::Redraw => {
                                with_output.push(terminal_id);
                                if let Some(w) = owner_window {
                                    if let Some(win) = self.windows.get_mut(&w) {
                                        if self
                                            .shared
                                            .runtime
                                            .manager
                                            .is_on_screen_in_window(w, terminal_id)
                                        {
                                            win.needs_redraw = true;
                                        }
                                        win.tab_title_activity = Some(now);
                                        if is_window_focused_pane {
                                            content_changed_windows.insert(w);
                                            shown_changed_windows.insert(w);
                                            win.output_since_input = win.input_sent_at.is_some();
                                        }
                                    }
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
                                if let Some(w) = owner_window {
                                    if let Some(win) = self.windows.get_mut(&w) {
                                        let bell = bell_response(
                                            &self.shared.config.behavior,
                                            is_window_focused_pane,
                                            win.window.has_focus(),
                                        );
                                        if bell.flash {
                                            win.bell_active_until =
                                                Some(Instant::now() + Duration::from_millis(100));
                                            win.needs_redraw = true;
                                        }
                                        if bell.urgent {
                                            win.window.request_user_attention(Some(
                                                winit::window::UserAttentionType::Informational,
                                            ));
                                        }
                                    }
                                }
                                tracing::debug!("Bell");
                            }
                            TerminalEvent::TitleChanged(title) => {
                                if let Some(w) = owner_window {
                                    tab_titles_changed_windows.insert(w);
                                    shown_changed_windows.insert(w);
                                    if is_window_focused_pane {
                                        if let Some(win) = self.windows.get_mut(&w) {
                                            win.window.set_title(&title);
                                            if let Some(adapter) = win.accessibility.as_mut() {
                                                adapter.set_window_title(&title);
                                            }
                                        }
                                    }
                                }
                            }
                            TerminalEvent::SetClipboard { clipboard: _, data } => {
                                if self.shared.config.behavior.clipboard_write
                                    == ClipboardWrite::Deny
                                {
                                    tracing::debug!(
                                        "Ignored OSC 52 clipboard write: behavior.clipboard_write = \"deny\""
                                    );
                                } else if is_global_focused_pane {
                                    if let Some(ref mut cb) = self.shared.clipboard {
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
                                if is_window_focused_pane {
                                    Self::show_notification(&title, &body);
                                }
                            }
                            TerminalEvent::Progress(progress) => {
                                if let Some(terminal) =
                                    self.shared.runtime.manager.terminal_mut(terminal_id)
                                {
                                    terminal.progress = progress_status(progress);
                                    if let Some(w) = owner_window {
                                        tab_titles_changed_windows.insert(w);
                                    }
                                }
                            }
                            TerminalEvent::CommandFinished { .. } => {
                                if let Some(terminal) =
                                    self.shared.runtime.manager.terminal_mut(terminal_id)
                                {
                                    if terminal.progress.take().is_some() {
                                        if let Some(w) = owner_window {
                                            tab_titles_changed_windows.insert(w);
                                        }
                                    }
                                }
                            }
                            TerminalEvent::ProgramStatusChanged { .. } => {
                                if let Some(w) = owner_window {
                                    tab_titles_changed_windows.insert(w);
                                }
                            }
                            TerminalEvent::ShellCommandFinished { .. } => {}
                            TerminalEvent::CommandStarted => {}
                        }
                    }
                }

                self.shared.runtime.cleanup_exited();
                let inline_changed = self.shared.runtime.pair_inline_surfaces(&with_output, now);
                if inline_changed {
                    if let Some(w) = self.focused_window {
                        shown_changed_windows.insert(w);
                    }
                }
                self.mark_dirty_native_windows();

                let empty_windows: Vec<WindowKey> = self
                    .windows
                    .keys()
                    .copied()
                    .filter(|&w| self.shared.runtime.manager.window_is_empty(w))
                    .collect();
                for w in empty_windows {
                    tracing::info!("All terminals exited in window {}", w.raw());
                    if self.close_window(w) {
                        return true;
                    }
                }

                if self.windows.is_empty() && cfg!(not(target_os = "macos")) {
                    tracing::info!("All windows closed");
                    return true;
                }

                for (key, before_count, before_active) in tab_state_before {
                    let Some(win) = self.windows.get_mut(&key) else {
                        continue;
                    };
                    let after_count = self.shared.runtime.manager.window_tab_count(key);
                    let after_active = self
                        .shared
                        .runtime
                        .manager
                        .window_active_tab(key)
                        .map(|tab| tab.id);
                    let structure_changed =
                        before_count != after_count || before_active != after_active;
                    let force = tab_titles_changed_windows.contains(&key) || structure_changed;
                    if tab_title_refresh_due(
                        now,
                        win.last_tab_title_refresh,
                        win.tab_title_activity.is_some(),
                        force,
                    ) {
                        let mut ctx = WindowCtx {
                            app: &mut self.shared,
                            win,
                        };
                        ctx.update_tab_bar();
                    }
                }

                for key in content_changed_windows {
                    if let Some(win) = self.windows.get_mut(&key) {
                        let mut ctx = WindowCtx {
                            app: &mut self.shared,
                            win,
                        };
                        ctx.update_url_detection();
                    }
                }

                for win in self.windows.values_mut() {
                    let mut ctx = WindowCtx {
                        app: &mut self.shared,
                        win,
                    };
                    ctx.handle_column_resize_requests();
                }

                self.update_all_accessibility(&shown_changed_windows, Instant::now());
            }
            Err(e) => {
                tracing::error!("Terminal error: {}", e);
                return true;
            }
        }

        for win in self.windows.values_mut() {
            let mut ctx = WindowCtx {
                app: &mut self.shared,
                win,
            };
            ctx.advance_search();
        }
        self.shared.publish_pane_changes();
        self.shared.complete_pending_control_waits();
        self.shared.complete_pending_override_pushes();
        self.shared.complete_pending_semantic_actions();
        self.expire_pending_resizes();

        let now = Instant::now();
        for win in self.windows.values_mut() {
            if let Some(until) = win.bell_active_until {
                if now < until {
                    win.needs_redraw = true;
                } else {
                    win.bell_active_until = None;
                    win.needs_redraw = true;
                }
            }

            let due = win.frame_due(&self.shared.runtime.manager, now).is_some();
            if win.frame_pacing.should_request(due) {
                win.window.request_redraw();
            }
        }

        false
    }

    fn mark_dirty_native_windows(&mut self) {
        let Some(server) = self.shared.runtime.protocol.as_ref() else {
            return;
        };
        for (&key, win) in &mut self.windows {
            if let Some(active_tab) = self
                .shared
                .runtime
                .manager
                .window_active_tab(key)
                .map(|tab| tab.id)
            {
                if server
                    .tab_sessions(active_tab)
                    .any(|session| session.is_render_dirty())
                {
                    win.needs_redraw = true;
                }
            }
        }
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
    /// A scrollback search has rows left to scan.
    search_pending: bool,
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
            self.search_pending.then_some(now),
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
            search_pending: false,
            accessibility: None,
        }
    }

    #[test]
    fn a_search_in_progress_wakes_the_loop_at_once() {
        let now = Instant::now();
        let state = WakeState {
            search_pending: true,
            ..idle(now)
        };
        assert_eq!(state.next_wake(now), Some(now));
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
