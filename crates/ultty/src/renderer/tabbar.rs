//! Tab bar rendering.
//!
//! Builds vertex data for the tab bar background, individual tab backgrounds,
//! active tab indicators, and tab title text.

use std::borrow::Cow;

use super::{Renderer, Rgba, Vertex};
use crate::tabs::{
    MenuItem, TabBarState, TabState, TitleButton, CHROME_BUTTON_SIZE, MENU_PADDING,
    TAB_CLOSE_BUTTON_SIZE, TAB_PADDING, TAB_ROW_HEIGHT, TITLE_BAR_HEIGHT, WINDOW_BUTTON_WIDTH,
};

const CHROME_BUTTON_RADIUS: f32 = 6.0;
const CHROME_BUTTON_BORDER: f32 = 1.0;

/// Columns the `->>> ultty_` title takes: `-`, three `>`, a space, `ultty`,
/// and the cursor.
const TITLE_LOGO_COLUMNS: f32 = 11.0;

/// Left edge of the title logo: right of the menu button, as far from it as
/// buttons are from each other.
fn title_logo_x() -> f32 {
    crate::tabs::CHROME_EDGE_INSET
        + WINDOW_BUTTON_WIDTH
        + (WINDOW_BUTTON_WIDTH - CHROME_BUTTON_SIZE) / 2.0
}

/// Fit `path` and ` git:(branch)` into `max_chars` columns. The branch is
/// kept whole when it fits; the path loses characters from the left, behind
/// an ellipsis, so its last components stay visible. Returns the path part
/// and the branch suffix.
fn fit_title_location(path: &str, branch: Option<&str>, max_chars: usize) -> (String, String) {
    let suffix = branch.map(|b| format!(" git:({b})")).unwrap_or_default();
    let suffix_len = suffix.chars().count();
    let path_len = path.chars().count();
    if path_len + suffix_len <= max_chars {
        return (path.to_string(), suffix);
    }
    let (room, suffix) = if suffix_len + 2 <= max_chars {
        (max_chars - suffix_len, suffix)
    } else {
        (max_chars, String::new())
    };
    if path_len <= room {
        return (path.to_string(), suffix);
    }
    if room == 0 {
        return (String::new(), suffix);
    }
    let tail: String = path.chars().skip(path_len - (room - 1)).collect();
    (format!("…{tail}"), suffix)
}

fn lighten(color: Rgba, amount: f32) -> Rgba {
    Rgba::new(
        (color.r + amount).min(1.0),
        (color.g + amount).min(1.0),
        (color.b + amount).min(1.0),
        color.a,
    )
}

fn dim(color: Rgba, amount: f32) -> Rgba {
    Rgba::new(
        (color.r * amount).min(1.0),
        (color.g * amount).min(1.0),
        (color.b * amount).min(1.0),
        color.a,
    )
}

fn mix(a: Rgba, b: Rgba, amount: f32) -> Rgba {
    let amount = amount.clamp(0.0, 1.0);
    Rgba::new(
        a.r + (b.r - a.r) * amount,
        a.g + (b.g - a.g) * amount,
        a.b + (b.b - a.b) * amount,
        a.a + (b.a - a.a) * amount,
    )
}

fn chrome_symbol_color(foreground: Rgba, accent: Rgba, is_hovered: bool) -> Rgba {
    if is_hovered {
        mix(foreground, accent, 0.22)
    } else {
        dim(foreground, 0.9)
    }
}

fn close_symbol_color(foreground: Rgba, is_hovered: bool) -> Rgba {
    if is_hovered {
        Rgba::new(1.0, 0.88, 0.86, 1.0)
    } else {
        dim(foreground, 0.9)
    }
}

fn chrome_button_rect(button_x: f32, button_width: f32) -> (f32, f32, f32, f32) {
    let size = CHROME_BUTTON_SIZE
        .min(button_width)
        .clamp(0.0, TITLE_BAR_HEIGHT);
    (
        button_x + (button_width - size) / 2.0,
        (TITLE_BAR_HEIGHT - size) / 2.0,
        size,
        size,
    )
}

fn chrome_button_inner_rect(x: f32, y: f32, width: f32, height: f32) -> (f32, f32, f32, f32) {
    (
        x + CHROME_BUTTON_BORDER,
        y + CHROME_BUTTON_BORDER,
        (width - CHROME_BUTTON_BORDER * 2.0).max(0.0),
        (height - CHROME_BUTTON_BORDER * 2.0).max(0.0),
    )
}

fn vertex_bounds(vertices: &[Vertex]) -> Option<(f32, f32, f32, f32)> {
    let first = vertices.first()?;
    let mut min_x = first.position[0];
    let mut max_x = first.position[0];
    let mut min_y = first.position[1];
    let mut max_y = first.position[1];

    for vertex in vertices.iter().skip(1) {
        min_x = min_x.min(vertex.position[0]);
        max_x = max_x.max(vertex.position[0]);
        min_y = min_y.min(vertex.position[1]);
        max_y = max_y.max(vertex.position[1]);
    }

    Some((min_x, max_x, min_y, max_y))
}

fn center_vertices_in_rect(vertices: &mut [Vertex], x: f32, y: f32, width: f32, height: f32) {
    let Some((min_x, max_x, min_y, max_y)) = vertex_bounds(vertices) else {
        return;
    };

    let glyph_width = max_x - min_x;
    let glyph_height = max_y - min_y;
    let dx = x + (width - glyph_width) / 2.0 - min_x;
    let dy = y + (height - glyph_height) / 2.0 - min_y;

    for vertex in vertices {
        vertex.position[0] += dx;
        vertex.position[1] += dy;
    }
}

fn truncate_tab_title(title: &str, max_width: f32, cell_width: f32) -> Cow<'_, str> {
    if title.is_empty() || max_width <= 0.0 || cell_width <= 0.0 {
        return Cow::Borrowed("");
    }

    let max_cells = (max_width / cell_width).floor() as usize;
    if max_cells == 0 {
        return Cow::Borrowed("");
    }

    let char_count = title.chars().count();
    if char_count <= max_cells {
        return Cow::Borrowed(title);
    }

    if max_cells == 1 {
        return Cow::Borrowed("…");
    }

    let mut truncated = String::with_capacity(title.len().min(max_cells));
    truncated.extend(title.chars().take(max_cells - 1));
    truncated.push('…');
    Cow::Owned(truncated)
}

fn centered_tab_title_x(tab: &TabState, title: &str, cell_width: f32) -> f32 {
    let label_x = tab.x + TAB_PADDING;
    let label_width = tab.max_text_width().max(0.0);
    if title.is_empty() || label_width <= 0.0 || cell_width <= 0.0 {
        return label_x;
    }

    let title_width = title.chars().count() as f32 * cell_width;
    label_x + ((label_width - title_width).max(0.0) / 2.0)
}

impl Renderer {
    /// Three bars: the main menu button's icon.
    fn emit_menu_icon(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        (x, y, w, h): (f32, f32, f32, f32),
        color: Rgba,
    ) {
        let (icon_w, stroke, gap) = (14.0_f32, 2.0_f32, 3.0_f32);
        let left = (x + (w - icon_w) / 2.0).round();
        let top = (y + (h - (3.0 * stroke + 2.0 * gap)) / 2.0).round();
        for i in 0..3 {
            let bar_y = top + i as f32 * (stroke + gap);
            self.emit_rect(vertices, indices, left, bar_y, icon_w, stroke, color);
        }
    }

    /// The open main menu: a panel under the menu button with one row per
    /// item, a hover highlight, and rules between groups.
    pub(super) fn build_main_menu_vertices(
        &mut self,
        tab_bar: &TabBarState,
    ) -> (Vec<Vertex>, Vec<u32>, Vec<Vertex>, Vec<u32>) {
        let (mut bg_v, mut bg_i, mut text_v, mut text_i) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        if !tab_bar.visible || !tab_bar.menu_open {
            return (bg_v, bg_i, text_v, text_i);
        }
        let layout = tab_bar.menu_layout(self.cell_width, self.cell_height);
        let bg = self.color_scheme.background;
        let panel = lighten(bg, 0.045);
        let border = lighten(bg, 0.17);
        let hover = lighten(bg, 0.11);
        let rule = lighten(bg, 0.1);
        let fg = self.color_scheme.foreground;
        let height = layout.height();
        self.emit_rounded_rect(
            &mut bg_v,
            &mut bg_i,
            layout.x,
            layout.y,
            layout.width,
            height,
            CHROME_BUTTON_RADIUS + 1.0,
            border,
        );
        self.emit_rounded_rect(
            &mut bg_v,
            &mut bg_i,
            layout.x + 1.0,
            layout.y + 1.0,
            layout.width - 2.0,
            height - 2.0,
            CHROME_BUTTON_RADIUS,
            panel,
        );
        for (index, item) in MenuItem::ALL.iter().enumerate() {
            let item_y = layout.y + MENU_PADDING + index as f32 * layout.item_height;
            if item.starts_group() {
                self.emit_rect(
                    &mut bg_v,
                    &mut bg_i,
                    layout.x + 8.0,
                    item_y,
                    layout.width - 16.0,
                    1.0,
                    rule,
                );
            }
            if tab_bar.hovered_menu_item == Some(*item) {
                self.emit_rounded_rect(
                    &mut bg_v,
                    &mut bg_i,
                    layout.x + 4.0,
                    item_y + 2.0,
                    layout.width - 8.0,
                    layout.item_height - 4.0,
                    CHROME_BUTTON_RADIUS - 2.0,
                    hover,
                );
            }
            let text_y = item_y + (layout.item_height - self.cell_height) / 2.0;
            let text_x = layout.x + 2.0 * self.cell_width;
            let (v, idx) = self.text_renderer.build_label_vertices(
                item.label(),
                text_x,
                text_y,
                layout.width - 3.0 * self.cell_width,
                fg,
            );
            let base = text_v.len() as u32;
            text_v.extend(v);
            text_i.extend(idx.iter().map(|i| i + base));
        }
        (bg_v, bg_i, text_v, text_i)
    }

    /// Draw the main menu over everything else, when it is open.
    pub(super) fn render_main_menu(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        tab_bar: &TabBarState,
    ) {
        let (bg_v, bg_i, text_v, text_i) = self.build_main_menu_vertices(tab_bar);
        self.draw_overlay_layers(encoder, view, (&bg_v, &bg_i), (&text_v, &text_i));
    }

    /// A pane outline with a divider: vertical for split right, horizontal
    /// for split down. Drawn with rectangles so it needs no icon font.
    fn emit_split_icon(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        (x, y, w, h): (f32, f32, f32, f32),
        side_by_side: bool,
        color: Rgba,
    ) {
        let (icon_w, icon_h, stroke) = (14.0_f32, 12.0_f32, 1.5_f32);
        let left = (x + (w - icon_w) / 2.0).round();
        let top = (y + (h - icon_h) / 2.0).round();
        for (rx, ry, rw, rh) in [
            (left, top, icon_w, stroke),
            (left, top + icon_h - stroke, icon_w, stroke),
            (left, top, stroke, icon_h),
            (left + icon_w - stroke, top, stroke, icon_h),
        ] {
            self.emit_rect(vertices, indices, rx, ry, rw, rh, color);
        }
        if side_by_side {
            let mid = (left + (icon_w - stroke) / 2.0).round();
            self.emit_rect(vertices, indices, mid, top, stroke, icon_h, color);
        } else {
            let mid = (top + (icon_h - stroke) / 2.0).round();
            self.emit_rect(vertices, indices, left, mid, icon_w, stroke, color);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_chrome_button(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        fill: Rgba,
        border: Rgba,
    ) {
        self.emit_rounded_rect(
            vertices,
            indices,
            x,
            y,
            width,
            height,
            CHROME_BUTTON_RADIUS,
            border,
        );

        let (inner_x, inner_y, inner_w, inner_h) = chrome_button_inner_rect(x, y, width, height);
        self.emit_rounded_rect(
            vertices,
            indices,
            inner_x,
            inner_y,
            inner_w,
            inner_h,
            CHROME_BUTTON_RADIUS - CHROME_BUTTON_BORDER,
            fill,
        );
    }

    /// Build tab bar vertex data.
    pub(super) fn build_tab_bar_vertices(&self, tab_bar: &TabBarState) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        if !tab_bar.visible {
            return (vertices, indices);
        }

        let height = tab_bar.height();
        let show_tabs = tab_bar.shows_tab_row();
        let bg = self.color_scheme.background;
        let title_bg = lighten(bg, 0.012);
        let tab_row_bg = lighten(bg, 0.035);
        let rail_line = lighten(bg, 0.075);
        let active_tab_bg = lighten(bg, 0.058);
        let inactive_tab_bg = lighten(bg, 0.02);
        let inactive_hover_bg = lighten(bg, 0.055);
        let accent_color = dim(self.color_scheme.cursor, 0.92);
        let active_edge = dim(self.color_scheme.foreground, 0.18);
        let control_bg = lighten(bg, 0.085);
        let control_hover_bg = lighten(bg, 0.19);
        let control_border = lighten(bg, 0.17);
        let control_hover_border = lighten(bg, 0.26);
        let destructive_hover_bg = Rgba::new(0.55, 0.16, 0.14, 1.0);
        let destructive_hover_border = Rgba::new(0.78, 0.25, 0.23, 1.0);

        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            0.0,
            self.config.width as f32,
            TITLE_BAR_HEIGHT,
            title_bg,
        );

        if show_tabs {
            self.emit_rect(
                &mut vertices,
                &mut indices,
                0.0,
                TITLE_BAR_HEIGHT,
                self.config.width as f32,
                TAB_ROW_HEIGHT,
                tab_row_bg,
            );
        }

        // The rail/content divider is drawn before tabs so the selected tab can
        // bridge over it and feel attached to the terminal surface.
        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            height - 1.0,
            self.config.width as f32,
            1.0,
            rail_line,
        );

        // Draw individual tabs (only when 2+ tabs)
        for (tab_idx, tab) in tab_bar.tabs.iter().enumerate() {
            if !show_tabs {
                continue;
            }
            let tab_bg = if tab.is_active {
                active_tab_bg
            } else if tab.is_hovered {
                inactive_hover_bg
            } else {
                inactive_tab_bg
            };

            let margin_x = 1.0;
            let tab_y = TITLE_BAR_HEIGHT + if tab.is_active { 1.0 } else { 3.0 };
            let tab_height = if tab.is_active {
                height - tab_y
            } else {
                height - tab_y - 1.0
            };
            self.emit_rect(
                &mut vertices,
                &mut indices,
                tab.x + margin_x,
                tab_y,
                tab.width - margin_x * 2.0,
                tab_height,
                tab_bg,
            );

            if tab.is_active {
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    tab.x + margin_x,
                    tab_y,
                    tab.width - margin_x * 2.0,
                    3.0,
                    accent_color,
                );
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    tab.x + margin_x,
                    tab_y,
                    1.0,
                    tab_height,
                    active_edge,
                );
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    tab.x + tab.width - margin_x - 1.0,
                    tab_y,
                    1.0,
                    tab_height,
                    active_edge,
                );
            }

            // Close button hover background
            if tab_bar.hovered_close_button == Some(tab_idx) {
                let (cx, cy, target_size, _) = tab.close_hover_bounds();
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    cx,
                    cy,
                    target_size,
                    target_size,
                    destructive_hover_bg,
                );
            }
        }

        // Title-band buttons: split right/down, window controls, and "+"
        // while the tab row is hidden.
        for (button, slot_x) in tab_bar.title_buttons() {
            let (button_x, button_y, button_w, button_h) =
                chrome_button_rect(slot_x, WINDOW_BUTTON_WIDTH);
            let is_hovered = tab_bar.hovered_title_button == Some(button)
                || (button == TitleButton::Menu && tab_bar.menu_open);
            let destructive = button == TitleButton::Close;
            let (button_bg, button_border) = match (is_hovered, destructive) {
                (true, true) => (destructive_hover_bg, destructive_hover_border),
                (true, false) => (control_hover_bg, control_hover_border),
                (false, _) => (control_bg, control_border),
            };
            self.emit_chrome_button(
                &mut vertices,
                &mut indices,
                button_x,
                button_y,
                button_w,
                button_h,
                button_bg,
                button_border,
            );
            if button == TitleButton::Menu {
                let color =
                    chrome_symbol_color(self.color_scheme.foreground, accent_color, is_hovered);
                self.emit_menu_icon(
                    &mut vertices,
                    &mut indices,
                    (button_x, button_y, button_w, button_h),
                    color,
                );
            }
            if matches!(button, TitleButton::SplitRight | TitleButton::SplitDown) {
                let color =
                    chrome_symbol_color(self.color_scheme.foreground, accent_color, is_hovered);
                self.emit_split_icon(
                    &mut vertices,
                    &mut indices,
                    (button_x, button_y, button_w, button_h),
                    button == TitleButton::SplitRight,
                    color,
                );
            }
        }
        // Separator between the split buttons and the window controls.
        let separator_height = CHROME_BUTTON_SIZE - 6.0;
        self.emit_rect(
            &mut vertices,
            &mut indices,
            tab_bar.title_group_separator_x(),
            (TITLE_BAR_HEIGHT - separator_height) / 2.0,
            1.0,
            separator_height,
            control_border,
        );

        // "+" at the end of the tab row: a hover highlight only; the glyph is
        // drawn with the titles.
        if show_tabs && tab_bar.new_tab_button_hovered {
            self.emit_rect(
                &mut vertices,
                &mut indices,
                tab_bar.new_tab_button_x + 2.0,
                TITLE_BAR_HEIGHT + 3.0,
                TabBarState::NEW_TAB_BUTTON_WIDTH - 4.0,
                TAB_ROW_HEIGHT - 4.0,
                inactive_hover_bg,
            );
        }

        (vertices, indices)
    }

    /// Build tab title text vertices.
    pub(super) fn build_tab_title_vertices(
        &mut self,
        tab_bar: &TabBarState,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let mut all_vertices = Vec::new();
        let mut all_indices = Vec::new();

        if !tab_bar.visible {
            return (all_vertices, all_indices);
        }

        // Center tab text vertically in the tab row. Chrome symbols are centered
        // by their emitted glyph bounds because symbols have uneven side bearings.
        let tab_text_y = TITLE_BAR_HEIGHT + (TAB_ROW_HEIGHT - self.cell_height) / 2.0;
        let accent_color = dim(self.color_scheme.cursor, 0.92);

        let show_tabs = tab_bar.shows_tab_row();
        for (tab_idx, tab) in tab_bar.tabs.iter().enumerate() {
            if !show_tabs {
                continue;
            }
            let text_color = if tab.is_active {
                self.color_scheme.foreground
            } else {
                dim(self.color_scheme.foreground, 0.62)
            };

            // Calculate text position and max width.
            let max_text_width = tab.max_text_width();
            let title = truncate_tab_title(&tab.title, max_text_width, self.cell_width);
            let text_x = centered_tab_title_x(tab, title.as_ref(), self.cell_width);

            let (vertices, indices) = self.text_renderer.build_label_vertices(
                &title,
                text_x,
                tab_text_y,
                max_text_width,
                text_color,
            );

            // Merge vertices, adjusting indices
            let base_idx = all_vertices.len() as u32;
            all_vertices.extend(vertices);
            all_indices.extend(indices.iter().map(|i| i + base_idx));

            // Close button "x" on the right side of the tab
            let is_close_hovered = tab_bar.hovered_close_button == Some(tab_idx);
            let close_center_x =
                tab.close_button_x() + TAB_CLOSE_BUTTON_SIZE / 2.0 - self.cell_width / 2.0;

            let close_color = if is_close_hovered {
                lighten(self.color_scheme.foreground, 0.08)
            } else {
                dim(self.color_scheme.foreground, 0.36)
            };
            let (v, idx) = self.text_renderer.build_label_vertices(
                "×",
                close_center_x,
                tab_text_y,
                12.0,
                close_color,
            );
            let base_idx = all_vertices.len() as u32;
            all_vertices.extend(v);
            all_indices.extend(idx.iter().map(|i| i + base_idx));
        }

        // Title: `->>> ultty_`, then the
        // focused pane's location centered between it and the buttons.
        {
            let cw = self.cell_width;
            let text_y = (TITLE_BAR_HEIGHT - self.cell_height) / 2.0;
            let green = self.color_scheme.named[2];
            let aqua = self.color_scheme.named[6];
            let fg = self.color_scheme.foreground;
            let x0 = title_logo_x();
            // One label per character so ligature fonts cannot merge `>>>`.
            let mut parts: Vec<(&str, f32, Rgba)> = vec![("-", x0, green)];
            for i in 1..=3 {
                parts.push((">", x0 + i as f32 * cw, aqua));
            }
            // The launch animation types `ultty` letter by letter; with no
            // letters yet the cursor sits right after `->>>`.
            let letters = tab_bar.title_letters.min(5);
            if letters > 0 {
                parts.push((&"ultty"[..letters], x0 + 5.0 * cw, fg));
            }
            let cursor_column = if letters == 0 { 4 } else { 5 + letters };
            parts.push(("_", x0 + cursor_column as f32 * cw, green));

            let region_start = x0 + (TITLE_LOGO_COLUMNS + 2.0) * cw;
            let region_end = tab_bar.title_buttons_start() - 2.0 * cw;
            let max_chars = if cw > 0.0 {
                ((region_end - region_start) / cw).floor().max(0.0) as usize
            } else {
                0
            };
            let (path, suffix) = fit_title_location(
                &tab_bar.title_path,
                tab_bar.title_branch.as_deref(),
                max_chars,
            );
            let text_chars = (path.chars().count() + suffix.chars().count()) as f32;
            let centered = (tab_bar.total_width - text_chars * cw) / 2.0;
            let path_x = centered
                .max(region_start)
                .min((region_end - text_chars * cw).max(region_start));
            let path_owned = path;
            let suffix_owned = suffix;
            if !path_owned.is_empty() {
                parts.push((path_owned.as_str(), path_x, fg));
            }
            if !suffix_owned.is_empty() {
                let suffix_x = path_x + path_owned.chars().count() as f32 * cw;
                parts.push((suffix_owned.as_str(), suffix_x, aqua));
            }
            for (text, x, color) in parts {
                let width = text.chars().count() as f32 * cw + cw;
                let (v, idx) = self
                    .text_renderer
                    .build_label_vertices(text, x, text_y, width, color);
                let base_idx = all_vertices.len() as u32;
                all_vertices.extend(v);
                all_indices.extend(idx.iter().map(|i| i + base_idx));
            }
        }

        // Title-band button symbols (split icons are drawn as shapes).
        for (button, slot_x) in tab_bar.title_buttons() {
            let symbol = match button {
                TitleButton::NewTab => "+",
                TitleButton::Minimize => "−",
                TitleButton::Maximize => "▢",
                TitleButton::Close => "×",
                TitleButton::Menu | TitleButton::SplitRight | TitleButton::SplitDown => continue,
            };
            let (button_x, button_y, button_w, button_h) =
                chrome_button_rect(slot_x, WINDOW_BUTTON_WIDTH);
            let is_hovered = tab_bar.hovered_title_button == Some(button);
            let color = if button == TitleButton::Close {
                close_symbol_color(self.color_scheme.foreground, is_hovered)
            } else {
                chrome_symbol_color(self.color_scheme.foreground, accent_color, is_hovered)
            };
            let (mut v, idx) = self.text_renderer.build_label_vertices(
                symbol,
                button_x,
                button_y,
                WINDOW_BUTTON_WIDTH,
                color,
            );
            center_vertices_in_rect(&mut v, button_x, button_y, button_w, button_h);
            let base_idx = all_vertices.len() as u32;
            all_vertices.extend(v);
            all_indices.extend(idx.iter().map(|i| i + base_idx));
        }

        // "+" at the end of the tab row.
        if show_tabs {
            let plus_color = chrome_symbol_color(
                self.color_scheme.foreground,
                accent_color,
                tab_bar.new_tab_button_hovered,
            );
            let (plus_x, plus_w) = (tab_bar.new_tab_button_x, TabBarState::NEW_TAB_BUTTON_WIDTH);
            let (mut v, idx) = self.text_renderer.build_label_vertices(
                "+",
                plus_x,
                TITLE_BAR_HEIGHT,
                plus_w,
                plus_color,
            );
            center_vertices_in_rect(&mut v, plus_x, TITLE_BAR_HEIGHT, plus_w, TAB_ROW_HEIGHT);
            let base_idx = all_vertices.len() as u32;
            all_vertices.extend(v);
            all_indices.extend(idx.iter().map(|i| i + base_idx));
        }

        (all_vertices, all_indices)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        center_vertices_in_rect, centered_tab_title_x, chrome_button_inner_rect,
        chrome_button_rect, chrome_symbol_color, close_symbol_color, dim, fit_title_location, mix,
        truncate_tab_title, vertex_bounds, CHROME_BUTTON_BORDER, CHROME_BUTTON_SIZE,
        TITLE_BAR_HEIGHT, WINDOW_BUTTON_WIDTH,
    };
    use crate::renderer::{Rgba, Vertex};
    use crate::tabs::TabState;
    use crate::terminal_manager::TabId;

    #[test]
    fn title_location_fits_whole_when_there_is_room() {
        assert_eq!(
            fit_title_location("~/projects/ultty", Some("main"), 40),
            ("~/projects/ultty".to_string(), " git:(main)".to_string())
        );
        assert_eq!(
            fit_title_location("~/projects/ultty", None, 40),
            ("~/projects/ultty".to_string(), String::new())
        );
    }

    #[test]
    fn title_location_trims_the_path_from_the_left_and_keeps_the_branch() {
        let (path, suffix) = fit_title_location("~/projects/tui-terminal", Some("main"), 20);
        assert_eq!(suffix, " git:(main)");
        assert_eq!(path, "…terminal");
        assert_eq!(path.chars().count() + suffix.chars().count(), 20);
    }

    #[test]
    fn title_location_drops_the_branch_before_the_path_vanishes() {
        let (path, suffix) =
            fit_title_location("~/projects/tui-terminal", Some("a-very-long-branch"), 12);
        assert_eq!(suffix, "");
        assert_eq!(path, "…ui-terminal");
        assert_eq!(
            fit_title_location("~/x", Some("main"), 0),
            (String::new(), String::new())
        );
    }

    #[test]
    fn truncate_tab_title_keeps_short_titles() {
        assert_eq!(truncate_tab_title("Tab 1", 80.0, 10.0).as_ref(), "Tab 1");
    }

    #[test]
    fn truncate_tab_title_adds_ellipsis_before_width_limit() {
        assert_eq!(
            truncate_tab_title("long-terminal-session", 50.0, 10.0).as_ref(),
            "long…"
        );
    }

    #[test]
    fn truncate_tab_title_handles_one_cell() {
        assert_eq!(truncate_tab_title("long", 10.0, 10.0).as_ref(), "…");
    }

    #[test]
    fn truncate_tab_title_handles_no_space() {
        assert_eq!(truncate_tab_title("long", 0.0, 10.0).as_ref(), "");
        assert_eq!(truncate_tab_title("long", 10.0, 0.0).as_ref(), "");
    }

    #[test]
    fn centered_tab_title_x_centers_short_titles_in_label_area() {
        let tab = test_tab_state(0.0, 200.0);

        assert_eq!(centered_tab_title_x(&tab, "Tab", 10.0), 76.0);
    }

    #[test]
    fn centered_tab_title_x_keeps_truncated_titles_inside_label_area() {
        let tab = test_tab_state(0.0, 80.0);
        let title = truncate_tab_title("long-title", tab.max_text_width(), 10.0);

        assert_eq!(title.as_ref(), "lo…");
        assert_eq!(centered_tab_title_x(&tab, title.as_ref(), 10.0), 16.0);
    }

    #[test]
    fn centered_tab_title_x_returns_label_start_for_empty_titles() {
        let tab = test_tab_state(20.0, 200.0);

        assert_eq!(centered_tab_title_x(&tab, "", 10.0), 32.0);
    }

    #[test]
    fn mix_interpolates_between_colors() {
        let color = mix(
            Rgba::new(0.0, 0.2, 0.4, 1.0),
            Rgba::new(1.0, 0.6, 0.0, 1.0),
            0.5,
        );

        assert!((color.r - 0.5).abs() < f32::EPSILON);
        assert!((color.g - 0.4).abs() < 0.000_001);
        assert!((color.b - 0.2).abs() < f32::EPSILON);
        assert!((color.a - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn chrome_symbol_hover_moves_toward_accent() {
        let fg = Rgba::new(0.8, 0.8, 0.8, 1.0);
        let accent = Rgba::new(0.2, 0.6, 1.0, 1.0);

        assert_eq!(chrome_symbol_color(fg, accent, false), dim(fg, 0.9));
        assert_eq!(chrome_symbol_color(fg, accent, true), mix(fg, accent, 0.22));
    }

    #[test]
    fn close_symbol_hover_uses_high_contrast_destructive_color() {
        let fg = Rgba::new(0.8, 0.8, 0.8, 1.0);

        assert_eq!(close_symbol_color(fg, false), dim(fg, 0.9));
        assert_eq!(
            close_symbol_color(fg, true),
            Rgba::new(1.0, 0.88, 0.86, 1.0)
        );
    }

    #[test]
    fn chrome_button_rect_centers_square_in_slot() {
        let (x, y, width, height) = chrome_button_rect(100.0, WINDOW_BUTTON_WIDTH);

        assert_eq!(x, 100.0 + (WINDOW_BUTTON_WIDTH - CHROME_BUTTON_SIZE) / 2.0);
        assert_eq!(y, (TITLE_BAR_HEIGHT - CHROME_BUTTON_SIZE) / 2.0);
        assert_eq!(width, CHROME_BUTTON_SIZE);
        assert_eq!(height, CHROME_BUTTON_SIZE);
    }

    #[test]
    fn chrome_button_inner_rect_reserves_even_border() {
        let (x, y, width, height) = chrome_button_inner_rect(100.0, 20.0, 24.0, 18.0);

        assert_eq!(x, 100.0 + CHROME_BUTTON_BORDER);
        assert_eq!(y, 20.0 + CHROME_BUTTON_BORDER);
        assert_eq!(width, 24.0 - CHROME_BUTTON_BORDER * 2.0);
        assert_eq!(height, 18.0 - CHROME_BUTTON_BORDER * 2.0);
    }

    #[test]
    fn center_vertices_in_rect_uses_actual_vertex_bounds() {
        let mut vertices = vec![
            test_vertex(2.0, 4.0),
            test_vertex(6.0, 4.0),
            test_vertex(6.0, 12.0),
            test_vertex(2.0, 12.0),
        ];

        center_vertices_in_rect(&mut vertices, 10.0, 20.0, 20.0, 20.0);

        assert_eq!(vertex_bounds(&vertices), Some((18.0, 22.0, 26.0, 34.0)));
    }

    fn test_tab_state(x: f32, width: f32) -> TabState {
        TabState {
            id: TabId::new(0),
            title: "Tab".to_string(),
            x,
            width,
            is_active: false,
            is_hovered: false,
        }
    }

    fn test_vertex(x: f32, y: f32) -> Vertex {
        Vertex {
            position: [x, y],
            tex_coord: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
            is_color: 0,
        }
    }
}
