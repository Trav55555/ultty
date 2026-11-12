//! User-triggered actions: config reload, font sizing, screenshots, debug dumps,
//! theme cycling, and desktop notifications.

use super::App;
use ultty::config::Config;
use ultty::protocol::ProtocolServer;
use ultty::renderer::OverlayState;
use ultty::tabs::TabBarState;
use ultty::url::DetectedUrl;
use ultty::vim::CopyMode;

pub(super) struct ScreenshotOverlayState {
    copy_mode: Option<CopyMode>,
    urls: Vec<DetectedUrl>,
    tab_bar: Option<TabBarState>,
    bell_active: bool,
    shortcuts_visible: bool,
    keybindings: Vec<(String, String)>,
    cursor_blink_visible: bool,
}

impl ScreenshotOverlayState {
    pub(super) fn as_overlay(&self) -> OverlayState<'_> {
        OverlayState {
            copy_mode: self.copy_mode.as_ref(),
            urls: &self.urls,
            ctrl_held: false,
            tab_bar: self.tab_bar.as_ref(),
            bell_active: self.bell_active,
            shortcuts_visible: self.shortcuts_visible,
            keybindings: self
                .shortcuts_visible
                .then_some(self.keybindings.as_slice()),
            protocol_status: None,
            cursor_blink_visible: self.cursor_blink_visible,
            ime_preedit: None,
        }
    }
}

impl App {
    pub(super) fn build_screenshot_overlay_state(&self) -> ScreenshotOverlayState {
        ScreenshotOverlayState {
            copy_mode: if self.copy_mode.is_active() {
                Some(self.copy_mode.clone())
            } else {
                None
            },
            urls: self.url_detector.urls().to_vec(),
            tab_bar: if self.tab_bar.visible {
                Some(self.tab_bar.clone())
            } else {
                None
            },
            bell_active: self
                .bell_active_until
                .map(|until| std::time::Instant::now() < until)
                .unwrap_or(false),
            shortcuts_visible: self.shortcuts_visible,
            keybindings: self.shortcuts_list.clone(),
            cursor_blink_visible: self.cursor_blink.visible(),
        }
    }

    /// Capture every pane of the active tab to `path`, as the window shows
    /// them. The caller must not hold any pane's terminal lock.
    pub(super) fn capture_panes_screenshot(
        &mut self,
        path: &std::path::Path,
    ) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let overlay_state = self.build_screenshot_overlay_state();
        let overlay = overlay_state.as_overlay();
        let locked = super::render::lock_active_panes(
            &self.terminal_manager,
            self.renderer.surface_size().into(),
            self.tab_bar.content_top(),
        );
        let panes = super::render::pane_draws(&locked, self.protocol_server.as_ref());
        self.renderer
            .capture_panes_screenshot(&panes, &overlay, path)
    }

    fn notify_screenshot_saved(&self, path: &std::path::Path) {
        tracing::info!("Screenshot saved to {}", path.display());
        // Show notification if possible
        if let Ok(notif) = notify_rust::Notification::new()
            .summary("Screenshot Saved")
            .body(&format!("Saved to {}", path.display()))
            .show()
        {
            drop(notif);
        }
    }

    /// Build the list of keyboard shortcuts for display.
    pub(super) fn build_shortcuts_list(
        keybindings: &ultty::config::KeybindingConfig,
    ) -> Vec<(String, String)> {
        vec![
            (keybindings.copy.clone(), "Copy selection".to_string()),
            (
                keybindings.paste.clone(),
                "Paste from clipboard".to_string(),
            ),
            (keybindings.new_tab.clone(), "New tab".to_string()),
            (keybindings.close_tab.clone(), "Close tab".to_string()),
            (keybindings.next_tab.clone(), "Next tab".to_string()),
            (keybindings.prev_tab.clone(), "Previous tab".to_string()),
            (
                keybindings.split_horizontal.clone(),
                "Split horizontal".to_string(),
            ),
            (
                keybindings.split_vertical.clone(),
                "Split vertical".to_string(),
            ),
            (
                keybindings.focus_pane_left.clone(),
                "Focus pane left".to_string(),
            ),
            (
                keybindings.focus_pane_right.clone(),
                "Focus pane right".to_string(),
            ),
            (
                keybindings.focus_pane_up.clone(),
                "Focus pane up".to_string(),
            ),
            (
                keybindings.focus_pane_down.clone(),
                "Focus pane down".to_string(),
            ),
            (keybindings.close_pane.clone(), "Close pane".to_string()),
            (keybindings.vim_mode.clone(), "Toggle vim mode".to_string()),
            (
                keybindings.scroll_page_up.clone(),
                "Scroll back a page".to_string(),
            ),
            (
                keybindings.scroll_page_down.clone(),
                "Scroll forward a page".to_string(),
            ),
            (
                keybindings.reload_config.clone(),
                "Reload config".to_string(),
            ),
            (
                keybindings.show_shortcuts.clone(),
                "Show/hide shortcuts".to_string(),
            ),
            (
                keybindings.protocol_status.clone(),
                "Show/hide protocol status".to_string(),
            ),
            (
                keybindings.cycle_theme.clone(),
                "Cycle color theme".to_string(),
            ),
            (
                keybindings.font_increase.clone(),
                "Increase font size".to_string(),
            ),
            (
                keybindings.font_decrease.clone(),
                "Decrease font size".to_string(),
            ),
            (
                keybindings.font_reset.clone(),
                "Reset font size".to_string(),
            ),
            (
                keybindings.screenshot.clone(),
                "Take screenshot".to_string(),
            ),
            (keybindings.debug_dump.clone(), "Debug dump".to_string()),
        ]
    }

    /// Build native protocol status rows for the in-terminal status overlay.
    pub(super) fn build_protocol_status_rows(&self) -> Vec<(String, String)> {
        Self::format_protocol_status_rows(self.protocol_server.as_ref())
    }

    pub(super) fn format_protocol_status_rows(
        server: Option<&ProtocolServer>,
    ) -> Vec<(String, String)> {
        let Some(server) = server else {
            return vec![
                ("Socket".to_string(), "disabled".to_string()),
                ("Sessions".to_string(), "0 connected".to_string()),
                ("Recent errors".to_string(), "none".to_string()),
            ];
        };

        let mut rows = vec![
            (
                "Socket dir".to_string(),
                server.socket_dir().display().to_string(),
            ),
            (
                "Sessions".to_string(),
                format!("{} connected", server.session_count()),
            ),
        ];

        if server.session_count() == 0 {
            rows.push(("Native state".to_string(), "no active sessions".to_string()));
            rows.push(("Recent errors".to_string(), "none".to_string()));
            return rows;
        }

        for session in server.sessions() {
            let (viewport_w, viewport_h) = session.viewport();
            let region_count = session.scene().iter_depth_first().count();
            let focused = session
                .focused_region()
                .map(|id| id.0.to_string())
                .unwrap_or_else(|| "none".to_string());
            let layout = session
                .last_layout_duration()
                .map(format_duration)
                .unwrap_or_else(|| "not computed".to_string());
            let recent_errors = {
                let errors = session.recent_protocol_errors().collect::<Vec<_>>();
                if errors.is_empty() {
                    "none".to_string()
                } else {
                    errors.join(" | ")
                }
            };

            rows.push((
                format!("Session {}", session.session_id()),
                format!(
                    "tab={} v{} caps=0x{:08x} viewport={:.0}x{:.0} regions={}",
                    session.tab_id().raw(),
                    session.protocol_version(),
                    session.capabilities(),
                    viewport_w,
                    viewport_h,
                    region_count
                ),
            ));
            rows.push((
                format!("Focused {}", session.session_id()),
                format!("region {}", focused),
            ));
            rows.push((
                format!("Dirty {}", session.session_id()),
                format!(
                    "render={} scroll={} dirty_regions={}",
                    session.is_render_dirty(),
                    session.is_scroll_dirty(),
                    session.dirty_regions().len()
                ),
            ));
            rows.push((format!("Layout {}", session.session_id()), layout));
            rows.push((format!("Errors {}", session.session_id()), recent_errors));
        }

        rows
    }

    /// Reload configuration from disk.
    pub(super) fn reload_config(&mut self) {
        let mut new_config = Config::load();
        if new_config.behavior.keep_start_only(&self.config.behavior) {
            tracing::info!("behavior.accessibility changed; it takes effect after Ultty restarts");
        }

        // Update color scheme
        self.renderer
            .set_color_scheme(ultty::renderer::ColorScheme::from_config(
                &new_config.colors,
            ));
        self.terminal_manager
            .set_theme_colors(new_config.colors.theme_colors());
        self.terminal_manager
            .set_scrollback_lines(new_config.behavior.scrollback_lines);
        self.terminal_manager
            .set_close_on_exit(new_config.behavior.close_on_exit);
        self.terminal_manager
            .set_inherit_working_directory(new_config.behavior.inherit_working_directory);

        // Update opacity
        self.renderer.set_opacity(new_config.behavior.opacity);

        // Update ligatures setting
        self.renderer.set_ligatures(new_config.font.ligatures);

        // Update URL detection setting
        self.url_detector
            .set_enabled(new_config.behavior.url_detection);

        self.config = new_config;
        self.needs_redraw = true;

        tracing::info!("Configuration reloaded");
    }

    /// Change font size (increase or decrease).
    pub(super) fn change_font_size(&mut self, increase: bool) {
        let new_size = if increase {
            self.renderer.increase_font_size()
        } else {
            self.renderer.decrease_font_size()
        };

        tracing::info!("Font size: {:.1}pt", new_size);

        // Cell size changed: re-size every pane's grid.
        self.relayout_terminals();

        self.needs_redraw = true;
    }

    /// Reset font size to default.
    pub(super) fn reset_font_size(&mut self) {
        let default_size = self.config.font.size * self.scale_factor;
        let new_size = self.renderer.set_font_size(default_size);

        tracing::info!("Font size reset to: {:.1}pt", new_size);

        // Cell size changed: re-size every pane's grid.
        self.relayout_terminals();

        self.needs_redraw = true;
    }

    /// Take a screenshot of the terminal.
    pub(super) fn take_screenshot(&mut self) {
        // Generate filename with timestamp
        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let filename = format!("ultty-screenshot-{}.png", timestamp);

        // Try Pictures directory first, then current directory
        let path = dirs::picture_dir()
            .map(|p| p.join("Screenshots").join(&filename))
            .unwrap_or_else(|| std::path::PathBuf::from(&filename));

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if self.terminal_manager.pane_count() > 1 {
            match self.capture_panes_screenshot(&path) {
                Ok(()) => self.notify_screenshot_saved(&path),
                Err(e) => tracing::error!("Failed to capture screenshot: {}", e),
            }
            return;
        }

        // Get terminal reference
        let Some(terminal) = self
            .terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal())
        else {
            tracing::warn!("Cannot take screenshot: no focused terminal");
            return;
        };

        let overlay_state = self.build_screenshot_overlay_state();
        let overlay = overlay_state.as_overlay();

        let native_sessions: Vec<&ultty::protocol::NativeSession> = self
            .protocol_server
            .as_ref()
            .map(|s| s.active_tab_sessions().collect())
            .unwrap_or_default();

        match self.renderer.capture_screenshot_with_native(
            &terminal,
            &overlay,
            &path,
            &native_sessions,
        ) {
            Ok(()) => self.notify_screenshot_saved(&path),
            Err(e) => {
                tracing::error!("Failed to capture screenshot: {}", e);
            }
        }
    }

    /// Dump terminal state to a file for debugging.
    pub(super) fn debug_dump_terminal(&self) {
        use std::io::Write;

        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let filename = format!("/tmp/ultty-debug-{}.txt", timestamp);

        let Some(terminal) = self.terminal() else {
            return;
        };
        let grid = terminal.grid();
        let dims = grid.dimensions();
        let cursor = grid.cursor();

        let mut output = String::new();

        // Header
        output.push_str("=== Ultty Terminal Debug Dump ===\n");
        output.push_str(&format!("Timestamp: {}\n", timestamp));
        output.push_str(&format!(
            "Dimensions: {}x{} (cols x rows)\n",
            dims.cols, dims.rows
        ));
        output.push_str(&format!(
            "Cursor: row={}, col={}\n",
            cursor.position.row, cursor.position.col
        ));
        output.push_str(&format!(
            "Modes: origin={}, autowrap={}\n",
            terminal.modes().origin_mode,
            terminal.modes().auto_wrap
        ));
        if let Some(region) = grid.scroll_region() {
            output.push_str(&format!(
                "Scroll region: top={}, bottom={}\n",
                region.top, region.bottom
            ));
        }
        output.push_str("\n=== Grid Content ===\n");

        // Dump grid content with row numbers
        for row_idx in 0..dims.rows {
            output.push_str(&format!("{:3}|", row_idx));
            if let Some(row) = grid.row(row_idx) {
                for col_idx in 0..dims.cols {
                    if let Some(cell) = row.cell(col_idx) {
                        let ch = cell.char();
                        // Replace control characters with visible representation
                        let display_char = if ch.is_control() || ch == '\0' {
                            '·'
                        } else if ch == ' ' {
                            ' '
                        } else {
                            ch
                        };
                        output.push(display_char);
                    } else {
                        output.push('·');
                    }
                }
            }
            output.push_str("|\n");
        }

        output.push_str("\n=== End of Dump ===\n");

        // Write to file
        match std::fs::File::create(&filename) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(output.as_bytes()) {
                    tracing::error!("Failed to write debug dump: {}", e);
                } else {
                    tracing::info!("Debug dump saved to {}", filename);
                    // Also show notification
                    let _ = notify_rust::Notification::new()
                        .summary("Debug Dump Saved")
                        .body(&format!("Saved to {}", filename))
                        .show();
                }
            }
            Err(e) => {
                tracing::error!("Failed to create debug dump file: {}", e);
            }
        }
    }

    /// Cycle through available color themes.
    pub(super) fn cycle_theme(&mut self) {
        let theme_names: Vec<String> = self.config.colors.theme_names();
        if theme_names.is_empty() {
            return;
        }

        // Find current theme index
        let current = self.config.colors.theme.as_deref().unwrap_or("");
        let current_idx = theme_names.iter().position(|n| n == current).unwrap_or(0);

        // Move to next theme (wrap around)
        let next_idx = (current_idx + 1) % theme_names.len();
        let next_name = &theme_names[next_idx];

        // Apply the theme
        if let Some(theme) = self.config.colors.get_theme(next_name).cloned() {
            self.config.colors.apply_theme(&theme);
            self.config.colors.theme = Some(next_name.clone());

            // Update renderer
            self.renderer
                .set_color_scheme(ultty::renderer::ColorScheme::from_config(
                    &self.config.colors,
                ));
            self.terminal_manager
                .set_theme_colors(self.config.colors.theme_colors());

            self.needs_redraw = true;
            tracing::info!("Switched to theme: {}", theme.name);
        }
    }

    /// Show a desktop notification via a dedicated background thread.
    ///
    /// Uses a single long-lived thread with a channel to avoid spawning
    /// a new thread per notification.
    pub(super) fn show_notification(title: &str, body: &str) {
        use std::sync::{mpsc, OnceLock};

        static SENDER: OnceLock<mpsc::Sender<(String, String)>> = OnceLock::new();

        let sender = SENDER.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<(String, String)>();
            std::thread::Builder::new()
                .name("notification".into())
                .spawn(move || {
                    use notify_rust::Notification;
                    for (title, body) in rx {
                        if let Err(e) = Notification::new()
                            .summary(&title)
                            .body(&body)
                            .appname("Ultty")
                            .timeout(5000)
                            .show()
                        {
                            tracing::warn!("Failed to show notification: {}", e);
                        } else {
                            tracing::debug!("Showed notification: {} - {}", title, body);
                        }
                    }
                })
                .expect("failed to spawn notification thread");
            tx
        });

        let title = if title.is_empty() { "Ultty" } else { title };
        let _ = sender.send((title.to_string(), body.to_string()));
    }
}

fn format_duration(duration: std::time::Duration) -> String {
    let micros = duration.as_micros();
    if micros >= 1_000 {
        format!("{:.2} ms", duration.as_secs_f64() * 1_000.0)
    } else {
        format!("{} us", micros)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultty::tabs::TabBarState;

    #[test]
    fn screenshot_overlay_snapshot_exposes_owned_chrome_state() {
        let mut tab_bar = TabBarState::new();
        tab_bar.total_width = 800.0;

        let snapshot = ScreenshotOverlayState {
            copy_mode: None,
            urls: Vec::new(),
            tab_bar: Some(tab_bar),
            bell_active: true,
            shortcuts_visible: true,
            keybindings: vec![("ctrl+s".to_string(), "Screenshot".to_string())],
            cursor_blink_visible: false,
        };

        let overlay = snapshot.as_overlay();

        assert!(overlay.tab_bar.is_some());
        assert!(overlay.bell_active);
        assert!(overlay.shortcuts_visible);
        assert_eq!(
            overlay.keybindings.unwrap(),
            &[("ctrl+s".to_string(), "Screenshot".to_string())]
        );
        assert!(!overlay.cursor_blink_visible);
    }

    #[test]
    fn protocol_status_rows_cover_disabled_server() {
        let rows = App::format_protocol_status_rows(None);

        assert!(rows
            .iter()
            .any(|(key, value)| key == "Socket" && value == "disabled"));
        assert!(rows
            .iter()
            .any(|(key, value)| key == "Sessions" && value == "0 connected"));
    }

    /// Server with one open tab in a private temp dir; the dir is removed on drop.
    struct StatusServer {
        server: ProtocolServer,
        socket_path: std::path::PathBuf,
        dir: std::path::PathBuf,
    }

    impl StatusServer {
        fn new(tag: &str) -> Self {
            let dir = ultty::testing::short_socket_test_dir(tag);
            let _ = std::fs::remove_dir_all(&dir);
            let mut server = ProtocolServer::new(&dir).expect("server should start");
            let tab = ultty::terminal_manager::TabId::new(0);
            let socket_path = server.open_tab(tab).expect("tab socket opens");
            server.set_active_tab(Some(tab));
            Self {
                server,
                socket_path,
                dir,
            }
        }
    }

    impl Drop for StatusServer {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn protocol_status_rows_cover_empty_server() {
        let status = StatusServer::new("empty");

        let rows = App::format_protocol_status_rows(Some(&status.server));

        assert!(rows
            .iter()
            .any(|(key, value)| key == "Sessions" && value == "0 connected"));
        assert!(rows
            .iter()
            .any(|(key, value)| key == "Native state" && value == "no active sessions"));
    }

    #[test]
    fn protocol_status_rows_cover_connected_session() {
        let mut status = StatusServer::new("session");
        let _client =
            std::os::unix::net::UnixStream::connect(&status.socket_path).expect("client connects");

        status
            .server
            .poll(Some(std::time::Duration::from_millis(0)))
            .expect("server accepts client");
        let rows = App::format_protocol_status_rows(Some(&status.server));

        assert!(rows
            .iter()
            .any(|(key, value)| key == "Sessions" && value == "1 connected"));
        assert!(rows.iter().any(|(key, value)| {
            key.starts_with("Session ")
                && value.contains("tab=0")
                && value.contains("caps=0x")
                && value.contains("regions=")
        }));
        assert!(rows
            .iter()
            .any(|(key, value)| key.starts_with("Dirty ") && value.contains("render=")));
    }
}
