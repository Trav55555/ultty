use ulttui::widgets::{Column, RichText, Row};
use ultty_protocol::Color;

use crate::data::{DiskInfo, ProcessInfo};
use crate::sparkline::{bar, format_bytes, format_uptime, sparkline};
use crate::SortColumn;

// ── Colors ────────────────────────────────────────────────────────────

const CYAN: Color = Color::Rgb(100, 220, 220);
const GREEN: Color = Color::Rgb(120, 220, 120);
const YELLOW: Color = Color::Rgb(220, 200, 80);
const RED: Color = Color::Rgb(220, 100, 100);
const DIM: Color = Color::Rgb(120, 120, 130);
const ACCENT: Color = Color::Rgb(140, 160, 255);

fn cpu_color(percent: f32) -> Color {
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

pub fn cpu_mem_row(cpu_percent: f32, mem_percent: f32, mem_used: u64, mem_total: u64) -> Row {
    let cpu_bar = bar(cpu_percent, 20);
    let mem_bar = bar(mem_percent, 20);

    Row::new()
        .child(
            RichText::new()
                .span("CPU ", ultty_protocol::Style::new().fg(DIM))
                .span(
                    &cpu_bar,
                    ultty_protocol::Style::new().fg(cpu_color(cpu_percent)),
                )
                .span(
                    format!(" {:.0}%", cpu_percent),
                    ultty_protocol::Style::new()
                        .fg(cpu_color(cpu_percent))
                        .bold(),
                )
                .fill_width(),
        )
        .child(
            RichText::new()
                .span("MEM ", ultty_protocol::Style::new().fg(DIM))
                .span(
                    &mem_bar,
                    ultty_protocol::Style::new().fg(cpu_color(mem_percent)),
                )
                .span(
                    format!(
                        " {:.0}%  {}/{}",
                        mem_percent,
                        format_bytes(mem_used),
                        format_bytes(mem_total)
                    ),
                    ultty_protocol::Style::new()
                        .fg(cpu_color(mem_percent))
                        .bold(),
                )
                .fill_width(),
        )
}

pub fn sparkline_view(cpu_history: &[f32]) -> Column {
    let spark = sparkline(cpu_history, 60);
    Column::new()
        .child(RichText::new().span(
            " CPU History ",
            ultty_protocol::Style::new().fg(ACCENT).bold(),
        ))
        .child(RichText::new().span(spark, ultty_protocol::Style::new().fg(CYAN)))
}

pub fn process_list(
    processes: &[ProcessInfo],
    selected: usize,
    sort_col: &SortColumn,
    sort_asc: bool,
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
        "  {:<8}{:<22}{:>6} {:>8}  {}",
        format!("PID{}", si(&SortColumn::Pid)),
        format!("NAME{}", si(&SortColumn::Name)),
        format!("CPU%{}", si(&SortColumn::Cpu)),
        format!("MEM{}", si(&SortColumn::Mem)),
        "STATUS"
    );

    let mut col = Column::new()
        .child(RichText::new().span(
            format!("Processes ({})  [s]ort [r]everse [q]uit", processes.len()),
            ultty_protocol::Style::new().fg(ACCENT).bold(),
        ))
        .child(RichText::new().span(header, ultty_protocol::Style::new().fg(DIM).bold()));

    for (i, p) in processes.iter().take(20).enumerate() {
        let indicator = if i == selected { ">" } else { " " };
        let line = format!(
            "{} {:<8}{:<22}{:>5.1} {:>8}  {}",
            indicator,
            p.pid,
            truncate_name(&p.name, 20),
            p.cpu_percent,
            format_bytes(p.mem_bytes),
            p.status,
        );
        let style = if i == selected {
            ultty_protocol::Style::new().fg(CYAN).bold()
        } else {
            ultty_protocol::Style::default()
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
    let disk = if let Some(root) = disks.iter().find(|d| d.mount == "/") {
        format!(
            "DISK / {:.0}% ({}/{})",
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
        .child(RichText::new().span(disk, ultty_protocol::Style::new().fg(DIM)))
}
