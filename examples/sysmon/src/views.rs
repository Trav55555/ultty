use ulttui::widgets::*;

use crate::data::{DiskInfo, ProcessInfo};
use crate::sparkline::{bar, format_bytes, format_uptime, sparkline};
use crate::SortColumn;

pub fn header_view(hostname: &str, uptime_secs: u64) -> Row {
    Row::new()
        .child(Text::new("System Monitor"))
        .child(Text::new(format!(
            "{}  up {}",
            hostname,
            format_uptime(uptime_secs)
        )))
        .gap(1.0)
}

pub fn cpu_mem_row(cpu_percent: f32, mem_percent: f32, mem_used: u64, mem_total: u64) -> Row {
    Row::new()
        .child(Text::new(format!(
            "CPU {} {:.0}%",
            bar(cpu_percent, 16),
            cpu_percent
        )))
        .child(Text::new(format!(
            "MEM {} {:.0}%  {}/{}",
            bar(mem_percent, 16),
            mem_percent,
            format_bytes(mem_used),
            format_bytes(mem_total),
        )))
        .gap(2.0)
}

pub fn sparkline_view(cpu_history: &[f32]) -> Column {
    let spark = sparkline(cpu_history, 60);
    Column::new()
        .child(Text::new("-- CPU History --"))
        .child(Text::new(spark))
}

pub fn process_table(
    processes: &[ProcessInfo],
    _selected: usize,
    sort_col: &SortColumn,
    sort_asc: bool,
) -> Column {
    let arrow = if sort_asc { " ^" } else { " v" };
    let headers: Vec<String> = vec![
        format!(
            "PID{}",
            if matches!(sort_col, SortColumn::Pid) {
                arrow
            } else {
                ""
            }
        ),
        format!(
            "NAME{}",
            if matches!(sort_col, SortColumn::Name) {
                arrow
            } else {
                ""
            }
        ),
        format!(
            "CPU%{}",
            if matches!(sort_col, SortColumn::Cpu) {
                arrow
            } else {
                ""
            }
        ),
        format!(
            "MEM{}",
            if matches!(sort_col, SortColumn::Mem) {
                arrow
            } else {
                ""
            }
        ),
        "STATUS".into(),
    ];

    let rows: Vec<Vec<String>> = processes
        .iter()
        .map(|p| {
            vec![
                p.pid.to_string(),
                truncate_name(&p.name, 20),
                format!("{:.1}", p.cpu_percent),
                format_bytes(p.mem_bytes),
                p.status.clone(),
            ]
        })
        .collect();

    let table = Table::new()
        .headers(headers)
        .column_widths(vec![8.0, 22.0, 8.0, 10.0, 10.0])
        .rows(rows);

    Column::new()
        .child(Text::new(format!("-- Processes ({}) --", processes.len())))
        .child(table)
}

fn truncate_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        name.to_string()
    } else {
        format!("{}...", &name[..max - 3])
    }
}

pub fn footer_view(net_rx: u64, net_tx: u64, disks: &[DiskInfo]) -> Row {
    let net = format!(
        "NET: rx {}  tx {}",
        format_bytes(net_rx),
        format_bytes(net_tx),
    );

    let disk = if let Some(root) = disks.iter().find(|d| d.mount == "/") {
        format!(
            "DISK: / {:.0}% ({}/{})",
            root.percent,
            format_bytes(root.used),
            format_bytes(root.total),
        )
    } else if let Some(first) = disks.first() {
        format!("DISK: {} {:.0}%", first.mount, first.percent,)
    } else {
        "DISK: --".into()
    };

    Row::new()
        .child(Text::new(net))
        .child(Text::new(disk))
        .gap(2.0)
}
