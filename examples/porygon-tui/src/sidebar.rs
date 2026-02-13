use ulttui::prelude::*;
use ulttui::widgets::RichText;
use ultty_protocol::{BorderEdges, BorderStyle, BorderType, Color, Dimension};

use crate::PorygonTui;

struct MoonInfo {
    idx: usize,
    name: &'static str,
    days_to_full: u64,
    full_moon_date: String,
}

fn moon_phase() -> MoonInfo {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Jan 6, 2000 ~13:50 UTC (calibrated to match observed phases)
    let known_new_moon = 947_165_400u64;
    let lunation_secs = 2_551_443u64;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let pos = (now - known_new_moon) % lunation_secs;
    let phase = pos as f64 / lunation_secs as f64;
    let names = [
        "New Moon",
        "Waxing Crescent",
        "First Quarter",
        "Waxing Gibbous",
        "Full Moon",
        "Waning Gibbous",
        "Last Quarter",
        "Waning Crescent",
    ];
    let idx = ((phase * 8.0) as usize) % 8;

    // Seconds until next full moon (phase = 0.5)
    let full_pos = lunation_secs / 2;
    let secs_to_full = if pos < full_pos {
        full_pos - pos
    } else {
        lunation_secs - pos + full_pos
    };
    let days_to_full = secs_to_full / 86400;

    // Format the full moon date
    let full_epoch = now + secs_to_full;
    let time_t = full_epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time_t, &mut tm) };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let full_moon_date = format!("{} {}", months[tm.tm_mon as usize], tm.tm_mday,);

    MoonInfo {
        idx,
        name: names[idx],
        days_to_full,
        full_moon_date,
    }
}

fn weather_view(app: &PorygonTui) -> Column {
    let coral = Color::Rgb(255, 93, 110);
    let cyan = Color::Rgb(58, 224, 221);

    let mut panel = Column::new()
        .bg(Color::Rgb(26, 26, 29))
        .border(BorderStyle::double())
        .border_color(coral)
        .padding(1.0)
        .gap(0.0);

    if let Some(w) = &app.weather {
        let city_str = if w.city.is_empty() {
            String::new()
        } else {
            format!(" ({})", w.city)
        };
        // Centered title
        panel = panel.child(
            Column::new()
                .align_center()
                .width(Dimension::Percent(100.0))
                .child(RichText::new().span("WEATHER", Style::new().fg(cyan).bold())),
        );
        panel = panel.child(
            Column::new()
                .align_center()
                .width(Dimension::Percent(100.0))
                .child(
                    RichText::new()
                        .span(
                            format!("{} {}°C {}", w.emoji, w.temp_c, w.description),
                            Style::default(),
                        )
                        .span(
                            city_str,
                            Style {
                                dim: true,
                                ..Style::new()
                            },
                        ),
                ),
        );
        if let Some(today) = w.forecast.first() {
            panel = panel.child(
                Column::new()
                    .align_center()
                    .width(Dimension::Percent(100.0))
                    .child(
                        RichText::new()
                            .span(format!("H:{}° ", today.1), Style::new().fg(coral))
                            .span(format!("L:{}°", today.2), Style::new().fg(cyan)),
                    ),
            );
        }

        // Braille dot plot chart
        if !w.hourly_temps.is_empty() {
            let n = w.hourly_temps.len();
            let chart_width = 21usize; // 7 days × 3 cols each
            let sample_count = chart_width * 2; // 42 points for 21 braille chars

            // Downsample with cosine interpolation
            let mut samples = Vec::with_capacity(sample_count);
            for i in 0..sample_count {
                let t = i as f64 / (sample_count - 1).max(1) as f64 * (n - 1).max(0) as f64;
                let idx = t.floor() as usize;
                let frac = t - idx as f64;
                let a = w.hourly_temps[idx.min(n - 1)];
                let b = w.hourly_temps[(idx + 1).min(n - 1)];
                let mu = (1.0 - (frac * std::f64::consts::PI).cos()) / 2.0;
                samples.push(a * (1.0 - mu) + b * mu);
            }

            let raw_min = samples.iter().cloned().fold(f64::INFINITY, f64::min);
            let raw_max = samples.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let pad = ((raw_max - raw_min) * 0.1).max(2.0); // 10% padding, min 2°
            let data_min = (raw_min - pad).floor();
            let data_max = (raw_max + pad).ceil();
            let data_range = (data_max - data_min).max(1.0);

            let chart_rows = 3usize;
            let y_levels = chart_rows * 4; // 12 vertical positions

            let mut grid = vec![vec![0u8; chart_width]; chart_rows];
            let mut char_temps: Vec<Vec<Vec<f64>>> =
                vec![vec![Vec::new(); chart_width]; chart_rows];

            // Plot data points
            for (i, &temp) in samples.iter().enumerate() {
                let norm = ((temp - data_min) / data_range).clamp(0.0, 1.0);
                let y = ((1.0 - norm) * (y_levels - 1) as f64).round() as usize;
                let y = y.min(y_levels - 1);

                let text_row = y / 4;
                let y_in_char = y % 4;
                let char_col = i / 2;
                let x_in_char = i % 2;

                if char_col >= chart_width {
                    continue;
                }

                let bit = match (x_in_char, y_in_char) {
                    (0, 0) => 0,
                    (0, 1) => 1,
                    (0, 2) => 2,
                    (0, 3) => 6,
                    (1, 0) => 3,
                    (1, 1) => 4,
                    (1, 2) => 5,
                    (1, 3) => 7,
                    _ => unreachable!(),
                };

                grid[text_row][char_col] |= 1 << bit;
                char_temps[text_row][char_col].push(temp);
            }

            // Add vertical separator lines at even day boundaries
            let num_days = n.div_ceil(24); // typically 7
            let cols_per_day = chart_width / num_days.max(1);
            let mut boundary_cols = Vec::new();
            for day in 1..num_days {
                let char_col = day * cols_per_day;
                if char_col > 0 && char_col < chart_width {
                    boundary_cols.push(char_col);
                    // Add left-column dotted line (bits 0,1,2,6)
                    for grid_row in grid.iter_mut().take(chart_rows) {
                        grid_row[char_col] |= 0b0100_0111;
                    }
                }
            }

            // Track which cols are boundaries for coloring
            let boundary_set: std::collections::HashSet<usize> =
                boundary_cols.iter().cloned().collect();

            let dim_line = Color::Rgb(68, 68, 68);

            // Add 0°C horizontal line if it falls within data range
            if data_min < 0.0 && data_max > 0.0 {
                let zero_norm = ((0.0 - data_min) / data_range).clamp(0.0, 1.0);
                let zero_y = ((1.0 - zero_norm) * (y_levels - 1) as f64).round() as usize;
                let zero_y = zero_y.min(y_levels - 1);
                let zero_row = zero_y / 4;
                let zero_y_in_char = zero_y % 4;
                // Set dots across all columns for 0°C line
                for (col, cell) in grid[zero_row].iter_mut().enumerate().take(chart_width) {
                    if boundary_set.contains(&col) {
                        continue; // don't overwrite boundary lines
                    }
                    // Set both left and right column dots at zero_y
                    let left_bit = match zero_y_in_char {
                        0 => 0,
                        1 => 1,
                        2 => 2,
                        3 => 6,
                        _ => unreachable!(),
                    };
                    let right_bit = match zero_y_in_char {
                        0 => 3,
                        1 => 4,
                        2 => 5,
                        3 => 7,
                        _ => unreachable!(),
                    };
                    *cell |= (1 << left_bit) | (1 << right_bit);
                }
            }

            // Scale labels for each row
            // Find which row contains the 0°C line
            let zero_row_idx = if data_min < 0.0 && data_max > 0.0 {
                let zero_norm = ((0.0 - data_min) / data_range).clamp(0.0, 1.0);
                let zero_y = ((1.0 - zero_norm) * (y_levels - 1) as f64).round() as usize;
                Some(zero_y.min(y_levels - 1) / 4)
            } else {
                None
            };

            let row_temps: Vec<String> = (0..chart_rows)
                .map(|row| {
                    if Some(row) == zero_row_idx {
                        format!("{:>3}°", 0)
                    } else {
                        let mid_y = row as f64 * 4.0 + 2.0;
                        let norm = 1.0 - mid_y / (y_levels - 1) as f64;
                        let temp = data_min + norm * data_range;
                        format!("{:>3.0}°", temp)
                    }
                })
                .collect();

            for row in 0..chart_rows {
                let mut rt = RichText::new();

                // Y-axis label
                rt = rt.span(
                    row_temps[row].clone(),
                    Style {
                        dim: true,
                        ..Style::new()
                    },
                );

                for col in 0..chart_width {
                    let ch = char::from_u32(0x2800 + grid[row][col] as u32).unwrap_or(' ');

                    let color = if boundary_set.contains(&col) {
                        dim_line
                    } else {
                        let avg = if char_temps[row][col].is_empty() {
                            data_min
                        } else {
                            char_temps[row][col].iter().sum::<f64>()
                                / char_temps[row][col].len() as f64
                        };
                        let t = ((avg - data_min) / data_range).clamp(0.0, 1.0);
                        let r = (58.0 + t * (255.0 - 58.0)) as u8;
                        let g = (224.0 + t * (93.0 - 224.0)) as u8;
                        let b = (221.0 + t * (110.0 - 221.0)) as u8;
                        Color::Rgb(r, g, b)
                    };

                    rt = rt.span(ch.to_string(), Style::new().fg(color));
                }
                panel = panel.child(rt.fill_width());
            }

            let dim_s = Style {
                dim: true,
                ..Style::new()
            };

            // Day label row below chart (day-of-month numbers)
            {
                // Get today's day-of-month
                use std::time::{SystemTime, UNIX_EPOCH};
                let secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let time_t = secs as libc::time_t;
                let mut tm: libc::tm = unsafe { std::mem::zeroed() };
                unsafe { libc::localtime_r(&time_t, &mut tm) };
                let today_mday = tm.tm_mday as usize;

                let mut edges = vec![0usize];
                for &bc in &boundary_cols {
                    edges.push(bc);
                }
                edges.push(chart_width);

                let mut label_rt = RichText::new();
                // Indent to align with chart (past Y-axis labels)
                label_rt = label_rt.span("    ", dim_s.clone());
                for seg in 0..edges.len() - 1 {
                    let start = edges[seg];
                    let end = edges[seg + 1];
                    let span_w = end - start;
                    let day_num = format!("{}", (today_mday + seg - 1) % 31 + 1);

                    if span_w >= day_num.len() {
                        let pad_left = (span_w - day_num.len()) / 2;
                        let pad_right = span_w - day_num.len() - pad_left;
                        if pad_left > 0 {
                            label_rt = label_rt.span(" ".repeat(pad_left), dim_s.clone());
                        }
                        label_rt = label_rt.span(day_num, dim_s.clone());
                        if pad_right > 0 {
                            label_rt = label_rt.span(" ".repeat(pad_right), dim_s.clone());
                        }
                    } else {
                        label_rt = label_rt.span(day_num[..span_w].to_string(), dim_s.clone());
                    }
                }
                panel = panel.child(label_rt.fill_width());
            }

            // Forecast: one line per day with day number
            for (d, h, l) in w.forecast.iter() {
                let day_num = if d.len() >= 2 {
                    &d[d.len() - 2..]
                } else {
                    d.as_str()
                };
                panel = panel.child(
                    RichText::new()
                        .span(format!("{day_num}  "), dim_s.clone())
                        .span(format!("{h:>3}°"), Style::new().fg(coral))
                        .span(" / ", dim_s.clone())
                        .span(format!("{l:>3}°"), Style::new().fg(cyan))
                        .fill_width(),
                );
            }
        }
    } else {
        panel = panel.child(
            RichText::new()
                .span("WEATHER: ", Style::new().fg(cyan).bold())
                .span(
                    "Loading...",
                    Style {
                        dim: true,
                        ..Style::new()
                    },
                ),
        );
    }

    panel
}

fn moon_view(_app: &PorygonTui) -> Column {
    // Unicode moon phase emoji (each renders as 2 columns wide)
    let emojis = ["🌑", "🌒", "🌓", "🌔", "🌕", "🌖", "🌗", "🌘"];
    let moon = moon_phase();
    let coral = Color::Rgb(255, 93, 110);
    let cyan = Color::Rgb(58, 224, 221);
    let dim_s = Style {
        dim: true,
        ..Style::new()
    };

    // Rotating wheel: 7 visible slots, current phase at center (slot 3)
    // Each slot: emoji (2 cols) + gap (2 spaces) = 4 cols per slot
    // Smooth transition via color blending based on frac
    let total_slots = 7usize;
    let center = total_slots / 2; // slot 3

    // Build both rows with identical structure so they align when centered
    let mut arrow_rt = RichText::new();
    let mut phases_rt = RichText::new();
    for slot in 0..total_slots {
        if slot > 0 {
            arrow_rt = arrow_rt.span("  ", Style::default());
            phases_rt = phases_rt.span("  ", Style::default());
        }
        // Arrow: ▼ only at center, space-width placeholder elsewhere
        // Emoji is 2 cols wide, so use 2 spaces as placeholder for non-center
        if slot == center {
            arrow_rt = arrow_rt.span("▼ ", Style::new().fg(coral).bold());
        } else {
            arrow_rt = arrow_rt.span("  ", Style::default());
        }
        // Emoji: rotate so current phase is at center
        let emoji_idx = (moon.idx + 8 + slot - center) % 8;
        let dist = (slot as isize - center as isize).unsigned_abs();
        let style = if dist == 0 {
            Style::new().fg(coral).bold()
        } else if dist == 1 {
            Style::new().fg(Color::Rgb(249, 226, 175))
        } else {
            dim_s.clone()
        };
        phases_rt = phases_rt.span(emojis[emoji_idx], style);
    }

    // Next full moon info
    let full_rt = RichText::new()
        .span("🌕 ", Style::new().fg(Color::Rgb(249, 226, 175)))
        .span(moon.full_moon_date.to_string(), Style::new().fg(coral))
        .span(format!(" ({}d)", moon.days_to_full), dim_s);

    Column::new()
        .bg(Color::Rgb(26, 26, 29))
        .border(BorderStyle::double())
        .border_color(coral)
        .padding(1.0)
        .gap(0.0)
        .child(
            Column::new()
                .align_center()
                .width(Dimension::Percent(100.0))
                .child(RichText::new().span("MOON", Style::new().fg(cyan).bold())),
        )
        .child(
            Column::new()
                .align_center()
                .width(Dimension::Percent(100.0))
                .child(RichText::new().span(moon.name, Style::default())),
        )
        .child(
            Column::new()
                .align_center()
                .width(Dimension::Percent(100.0))
                .child(arrow_rt)
                .child(phases_rt),
        )
        .child(
            Column::new()
                .align_center()
                .width(Dimension::Percent(100.0))
                .child(full_rt),
        )
}

pub(crate) fn sidebar_view(app: &PorygonTui) -> Column {
    let conn_status = if app.connected {
        RichText::new()
            .span("DAEMON: ", Style::new().fg(Color::Rgb(136, 136, 136)))
            .span("● ", Style::new().fg(Color::Rgb(166, 227, 161)).bold())
            .span("connected", Style::new().fg(Color::Rgb(166, 227, 161)))
    } else {
        RichText::new()
            .span("DAEMON: ", Style::new().fg(Color::Rgb(136, 136, 136)))
            .span("○ ", Style::new().fg(Color::Rgb(255, 93, 110)).bold())
            .span("disconnected", Style::new().fg(Color::Rgb(255, 93, 110)))
    };

    let (ctx_icon, ctx_color, ctx_label) = match app.status.context_quality.as_str() {
        "peak" => ("●", Color::Rgb(166, 227, 161), "PEAK"),
        "good" => ("●", Color::Rgb(58, 224, 221), "GOOD"),
        "degrading" => ("◐", Color::Rgb(249, 226, 175), "DEGRADING"),
        "poor" => ("○", Color::Rgb(255, 93, 110), "POOR"),
        _ => ("◆", Color::Rgb(247, 247, 247), "UNKNOWN"),
    };

    let ratio = if app.status.budget_usd > 0.0 {
        app.status.cost_usd / app.status.budget_usd
    } else {
        0.0
    };
    let bar_w = 10usize;
    let filled = (ratio.clamp(0.0, 1.0) * bar_w as f64).round() as usize;
    let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(bar_w - filled));
    let bar_color = if ratio > 0.8 {
        Color::Rgb(255, 93, 110)
    } else if ratio > 0.5 {
        Color::Rgb(249, 226, 175)
    } else {
        Color::Rgb(58, 224, 221)
    };
    let pct = (ratio * 100.0).round() as u32;

    let dim = Style {
        dim: true,
        ..Style::new()
    };
    let label_style = Style::new().fg(Color::Rgb(136, 136, 136));

    let mut cassette = Column::new()
        .bg(Color::Rgb(26, 26, 29))
        .border(BorderStyle::double())
        .border_color(Color::Rgb(255, 93, 110))
        .padding(1.0)
        .gap(0.0)
        .child(
            Column::new()
                .align_center()
                .width(Dimension::Percent(100.0))
                .child(RichText::new().span(
                    "▚▚ CASSETTE STATUS ▚▚",
                    Style::new().fg(Color::Rgb(255, 93, 110)).bold(),
                )),
        )
        .child(conn_status.fill_width())
        .child(
            RichText::new()
                .span("MODEL:  ", label_style.clone())
                .span(
                    app.status.model.as_str(),
                    Style::new().fg(Color::Rgb(247, 247, 247)),
                )
                .fill_width(),
        )
        .child(
            RichText::new()
                .span("AGENT:  ", label_style.clone())
                .span(
                    app.status.agent_name.as_str(),
                    Style::new().fg(Color::Rgb(58, 224, 221)),
                )
                .fill_width(),
        )
        .child(
            RichText::new()
                .span("MODE:   ", label_style.clone())
                .span("ACTIVE", Style::new().fg(Color::Rgb(255, 93, 110)).bold())
                .fill_width(),
        )
        .child(Divider::horizontal())
        .child(
            RichText::new()
                .span("BUDGET: ", label_style.clone())
                .span(format!("${:.2}", app.status.budget_usd), Style::default())
                .fill_width(),
        )
        .child(
            RichText::new()
                .span("USAGE:  ", label_style.clone())
                .span(&bar, Style::new().fg(bar_color))
                .span(format!(" {pct}%"), dim.clone())
                .fill_width(),
        )
        .child(
            RichText::new()
                .span("CPU     ", label_style.clone())
                .span(
                    format!(
                        "[{}{}]",
                        "█".repeat((app.status.cpu_usage * 10.0).round() as usize),
                        "░".repeat(10 - (app.status.cpu_usage * 10.0).round() as usize)
                    ),
                    Style::new().fg(if app.status.cpu_usage > 0.8 {
                        Color::Rgb(255, 93, 110)
                    } else {
                        Color::Rgb(166, 227, 161)
                    }),
                )
                .span(
                    format!(" {:.0}%", app.status.cpu_usage * 100.0),
                    dim.clone(),
                )
                .fill_width(),
        )
        .child(
            RichText::new()
                .span("RAM     ", label_style.clone())
                .span(
                    format!(
                        "[{}{}]",
                        "█".repeat((app.status.ram_usage * 10.0).round() as usize),
                        "░".repeat(10 - (app.status.ram_usage * 10.0).round() as usize)
                    ),
                    Style::new().fg(if app.status.ram_usage > 0.8 {
                        Color::Rgb(255, 93, 110)
                    } else {
                        Color::Rgb(58, 224, 221)
                    }),
                )
                .span(
                    format!(" {:.0}%", app.status.ram_usage * 100.0),
                    dim.clone(),
                )
                .fill_width(),
        )
        .child(Divider::horizontal())
        .child(
            RichText::new()
                .span("CONTEXT: ", label_style.clone())
                .span(
                    format!("{ctx_icon} {ctx_label}"),
                    Style::new().fg(ctx_color).bold(),
                )
                .fill_width(),
        )
        .child(
            RichText::new()
                .span("STEP:    ", label_style)
                .span(app.status.step.as_str(), dim)
                .fill_width(),
        );

    if app.waiting_response {
        cassette = cassette.child(
            RichText::new()
                .span(app.hex_gauge(), Style::new().fg(Color::Rgb(255, 93, 110)))
                .fill_width(),
        );
    }

    if let Some(err) = &app.error {
        let err_msg = if err.len() > 25 {
            format!("{}...", &err[..22])
        } else {
            err.clone()
        };
        cassette = cassette.child(
            RichText::new()
                .span(
                    format!("ERR: {err_msg}"),
                    Style::new().fg(Color::Rgb(249, 226, 175)),
                )
                .fill_width(),
        );
    }

    Column::new()
        .width(Dimension::Cells(34.0))
        .flex_shrink(0.0)
        .bg(Color::Rgb(37, 37, 40))
        .padding(1.0)
        .border(BorderStyle {
            style: BorderType::Thin,
            edges: BorderEdges {
                top: false,
                right: false,
                bottom: false,
                left: true,
            },
            radius: 0,
        })
        .border_color(Color::Rgb(58, 224, 221))
        .overflow_y_hidden()
        .gap(1.0)
        .child(cassette)
        .child(weather_view(app))
        .child(moon_view(app))
}
