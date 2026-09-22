mod data;
mod sparkline;
mod views;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ulttui::event::Key;
use ulttui::prelude::*;

use data::{ProcessInfo, SystemCollector, SystemSnapshot};

// ── Messages ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum Msg {
    Tick,
    SystemData(SystemSnapshot),
    Resized(u16),
    SelectUp,
    SelectDown,
    CycleSort,
    ReverseSort,
    Quit,
}

// ── Sort ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum SortColumn {
    Pid,
    Name,
    Cpu,
    Mem,
}

impl SortColumn {
    fn next(&self) -> Self {
        match self {
            SortColumn::Pid => SortColumn::Name,
            SortColumn::Name => SortColumn::Cpu,
            SortColumn::Cpu => SortColumn::Mem,
            SortColumn::Mem => SortColumn::Pid,
        }
    }
}

// ── App State ─────────────────────────────────────────────────────────

struct SysMon {
    collector: Arc<Mutex<SystemCollector>>,
    hostname: String,
    uptime_secs: u64,
    cpu_percent: f32,
    cpu_history: Vec<f32>,
    mem_percent: f32,
    mem_total: u64,
    mem_used: u64,
    processes: Vec<ProcessInfo>,
    selected_process: usize,
    sort_column: SortColumn,
    sort_ascending: bool,
    net_rx: u64,
    net_tx: u64,
    disks: Vec<data::DiskInfo>,
    term_width: u16,
}

impl SysMon {
    fn new() -> Self {
        let mut collector = SystemCollector::new();
        let snap = collector.collect();
        let mut app = Self {
            collector: Arc::new(Mutex::new(collector)),
            hostname: snap.hostname,
            uptime_secs: snap.uptime_secs,
            cpu_percent: snap.cpu_percent,
            cpu_history: vec![snap.cpu_percent],
            mem_percent: snap.mem_percent,
            mem_total: snap.mem_total,
            mem_used: snap.mem_used,
            processes: snap.processes,
            selected_process: 0,
            sort_column: SortColumn::Cpu,
            sort_ascending: false,
            net_rx: snap.net_rx_bytes,
            net_tx: snap.net_tx_bytes,
            disks: snap.disks,
            term_width: 80,
        };
        app.sort_processes();
        app
    }

    fn apply_snapshot(&mut self, snap: SystemSnapshot) {
        self.hostname = snap.hostname;
        self.uptime_secs = snap.uptime_secs;
        self.cpu_percent = snap.cpu_percent;
        self.cpu_history.push(snap.cpu_percent);
        if self.cpu_history.len() > 120 {
            self.cpu_history.remove(0);
        }
        self.mem_percent = snap.mem_percent;
        self.mem_total = snap.mem_total;
        self.mem_used = snap.mem_used;
        self.processes = snap.processes;
        self.net_rx = snap.net_rx_bytes;
        self.net_tx = snap.net_tx_bytes;
        self.disks = snap.disks;
        self.sort_processes();
        // Clamp selection
        if self.selected_process >= self.processes.len() {
            self.selected_process = self.processes.len().saturating_sub(1);
        }
    }

    fn sort_processes(&mut self) {
        let asc = self.sort_ascending;
        match self.sort_column {
            SortColumn::Pid => self.processes.sort_by(|a, b| {
                if asc {
                    a.pid.cmp(&b.pid)
                } else {
                    b.pid.cmp(&a.pid)
                }
            }),
            SortColumn::Name => self.processes.sort_by(|a, b| {
                if asc {
                    a.name.to_lowercase().cmp(&b.name.to_lowercase())
                } else {
                    b.name.to_lowercase().cmp(&a.name.to_lowercase())
                }
            }),
            SortColumn::Cpu => self.processes.sort_by(|a, b| {
                let ord = a
                    .cpu_percent
                    .partial_cmp(&b.cpu_percent)
                    .unwrap_or(std::cmp::Ordering::Equal);
                if asc {
                    ord
                } else {
                    ord.reverse()
                }
            }),
            SortColumn::Mem => self.processes.sort_by(|a, b| {
                if asc {
                    a.mem_bytes.cmp(&b.mem_bytes)
                } else {
                    b.mem_bytes.cmp(&a.mem_bytes)
                }
            }),
        }
    }
}

// ── Application Trait ─────────────────────────────────────────────────

impl Application for SysMon {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Tick => {
                let collector = Arc::clone(&self.collector);
                Cmd::spawn(async move {
                    let snap = collector.lock().unwrap().collect();
                    Msg::SystemData(snap)
                })
            }
            Msg::SystemData(snap) => {
                self.apply_snapshot(snap);
                Cmd::none()
            }
            Msg::SelectUp => {
                if self.selected_process > 0 {
                    self.selected_process -= 1;
                }
                Cmd::none()
            }
            Msg::SelectDown => {
                if self.selected_process + 1 < self.processes.len() {
                    self.selected_process += 1;
                }
                Cmd::none()
            }
            Msg::CycleSort => {
                self.sort_column = self.sort_column.next();
                self.sort_processes();
                Cmd::none()
            }
            Msg::ReverseSort => {
                self.sort_ascending = !self.sort_ascending;
                self.sort_processes();
                Cmd::none()
            }
            Msg::Resized(w) => {
                self.term_width = w;
                Cmd::none()
            }
            Msg::Quit => Cmd::Quit,
        }
    }

    fn subscriptions(&self) -> Sub<Msg> {
        Sub::every(Duration::from_secs(1), || Msg::Tick)
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        match event {
            Event::Key(key) => match &key.key {
                Key::Char('q') => Some(Msg::Quit),
                Key::Char('j') | Key::Down => Some(Msg::SelectDown),
                Key::Char('k') | Key::Up => Some(Msg::SelectUp),
                Key::Char('s') => Some(Msg::CycleSort),
                Key::Char('r') => Some(Msg::ReverseSort),
                _ => None,
            },
            Event::Resize { width, .. } => Some(Msg::Resized(*width)),
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        let w = self.term_width as usize;
        let bar_width = (w / 2).saturating_sub(15).clamp(10, 50);
        let chart_width = w.saturating_sub(8).clamp(20, 100);
        let proc_rows = 20.min(self.processes.len());

        Column::new()
            .child(views::header_view(&self.hostname, self.uptime_secs))
            .child(Divider::horizontal())
            .child(views::resource_bars(
                self.cpu_percent,
                self.mem_percent,
                self.mem_used,
                self.mem_total,
                bar_width,
            ))
            .child(views::cpu_chart(&self.cpu_history, chart_width))
            .child(Divider::horizontal())
            .child(views::process_list(
                &self.processes,
                self.selected_process,
                &self.sort_column,
                self.sort_ascending,
                proc_rows,
            ))
            .child(Divider::horizontal())
            .child(views::footer_view(self.net_rx, self.net_tx, &self.disks))
            .padding(1.0)
    }
}

// ── Main ──────────────────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    SysMon::new().run()
}
