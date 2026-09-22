use ulttui::prelude::*;
use ulttui::widgets::RichText;

use crate::data::{DiskInfo, ProcessInfo};
use crate::sparkline::{bar, chart, format_bytes, format_uptime};
use crate::SortColumn;

// ── Colors ────────────────────────────────────────────────────────────

const CYAN: Color = Color::Rgb(100, 220, 220);
const GREEN: Color = Color::Rgb(120, 220, 120);
const YELLOW: Color = Color::Rgb(220, 200, 80);
const RED: Color = Color::Rgb(220, 100, 100);
const DIM: Color = Color::Rgb(100, 100, 110);
const ACCENT: Color = Color::Rgb(140, 160, 255);
const WHITE: Color = Color::Rgb(210, 210, 220);

fn load_color(percent: f32) -> Color {
    if percent > 80.0 {
        RED
    } else if percent > 50.0 {
        YELLOW
    } else {
        GREEN
    }
}

// ── Views ─────────────────────────────────────────────────────────────

pub fn header_view(hostname: &str, uptime_secs: u64) -> Row {
    Row::new()
        .child(
            RichText::new()
                .span(
                    "System Monitor",
                    ultty_protocol::Style::new().fg(ACCENT).bold(),
                )
                .fill_width(),
        )
        .child(RichText::new().span(
            format!("{}  up {}", hostname, format_uptime(uptime_secs)),
            ultty_protocol::Style::new().fg(DIM),
        ))
}

pub fn resource_bars(
    cpu_percent: f32,
    mem_percent: f32,
    mem_used: u64,
    mem_total: u64,
    bar_width: usize,
) -> Column {
    let cpu_bar = bar(cpu_percent, bar_width);
    let mem_bar = bar(mem_percent, bar_width);

    Column::new()
        .child(
            RichText::new()
                .span("CPU  ", ultty_protocol::Style::new().fg(DIM))
                .span(
                    &cpu_bar,
                    ultty_protocol::Style::new().fg(load_color(cpu_percent)),
                )
                .span(
                    format!("  {:.0}%", cpu_percent),
                    ultty_protocol::Style::new()
                        .fg(load_color(cpu_percent))
                        .bold(),
                ),
        )
        .child(
            RichText::new()
                .span("MEM  ", ultty_protocol::Style::new().fg(DIM))
                .span(
                    &mem_bar,
                    ultty_protocol::Style::new().fg(load_color(mem_percent)),
                )
                .span(
                    format!(
                        "  {:.0}%  {}/{}",
                        mem_percent,
                        format_bytes(mem_used),
                        format_bytes(mem_total)
                    ),
                    ultty_protocol::Style::new()
                        .fg(load_color(mem_percent))
                        .bold(),
                ),
        )
}

pub fn cpu_chart(cpu_history: &[f32], width: usize) -> Column {
    let rows = chart(cpu_history, width, 6);
    let mut col = Column::new().child(Text::new(""));
    for row in rows {
        col = col.child(RichText::new().span(row, ultty_protocol::Style::new().fg(GREEN)));
    }
    col
}

pub fn process_list(
    processes: &[ProcessInfo],
    selected: usize,
    sort_col: &SortColumn,
    sort_asc: bool,
    max_rows: usize,
) -> Column {
    let arrow = if sort_asc { "^" } else { "v" };
    let si = |this: &SortColumn| {
        if sort_col == this {
            arrow
        } else {
            " "
        }
    };

    let header = format!(
        "  {:>7}  {:<18} {:>6}  {:>7}  {}",
        format!("PID{}", si(&SortColumn::Pid)),
        format!("NAME{}", si(&SortColumn::Name)),
        format!("CPU{}", si(&SortColumn::Cpu)),
        format!("MEM{}", si(&SortColumn::Mem)),
        "STATUS"
    );

    let mut col = Column::new()
        .child(
            RichText::new()
                .span("Processes", ultty_protocol::Style::new().fg(ACCENT).bold())
                .span(
                    format!(" ({})  ", processes.len()),
                    ultty_protocol::Style::new().fg(DIM),
                )
                .span(
                    "[s]ort [r]everse [q]uit",
                    ultty_protocol::Style::new().fg(DIM),
                ),
        )
        .child(RichText::new().span(header, ultty_protocol::Style::new().fg(DIM).bold()));

    for (i, p) in processes.iter().take(max_rows).enumerate() {
        let indicator = if i == selected { ">" } else { " " };
        let line = format!(
            "{} {:>7}  {:<18} {:>5.1}%  {:>7}  {}",
            indicator,
            p.pid,
            truncate_name(&p.name, 18),
            p.cpu_percent,
            format_bytes(p.mem_bytes),
            p.status,
        );
        let style = if i == selected {
            ultty_protocol::Style::new().fg(CYAN).bold()
        } else {
            ultty_protocol::Style::new().fg(WHITE)
        };
        col = col.child(RichText::new().span(line, style));
    }

    col
}

fn truncate_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        name.to_string()
    } else {
        format!("{}...", &name[..max - 3])
    }
}

pub fn footer_view(net_rx: u64, net_tx: u64, disks: &[DiskInfo]) -> Row {
    let disk_str = if let Some(root) = disks.iter().find(|d| d.mount == "/") {
        format!(
            "DISK / {:.0}% {}/{}",
            root.percent,
            format_bytes(root.used),
            format_bytes(root.total),
        )
    } else if let Some(first) = disks.first() {
        format!("DISK {} {:.0}%", first.mount, first.percent)
    } else {
        "DISK --".into()
    };

    Row::new()
        .child(
            RichText::new()
                .span("NET ", ultty_protocol::Style::new().fg(DIM))
                .span(
                    format!("rx {}  tx {}", format_bytes(net_rx), format_bytes(net_tx)),
                    ultty_protocol::Style::new().fg(GREEN),
                )
                .fill_width(),
        )
        .child(RichText::new().span(disk_str, ultty_protocol::Style::new().fg(DIM)))
}
