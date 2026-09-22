use sysinfo::{Disks, Networks, System};

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct SystemSnapshot {
    pub hostname: String,
    pub uptime_secs: u64,
    pub cpu_percent: f32,
    pub cpu_per_core: Vec<f32>,
    pub mem_total: u64,
    pub mem_used: u64,
    pub mem_percent: f32,
    pub swap_total: u64,
    pub swap_used: u64,
    pub processes: Vec<ProcessInfo>,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub disks: Vec<DiskInfo>,
}

#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub mem_bytes: u64,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct DiskInfo {
    pub mount: String,
    pub total: u64,
    pub used: u64,
    pub percent: f32,
}

pub struct SystemCollector {
    sys: System,
    networks: Networks,
    disks: Disks,
}

impl SystemCollector {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        Self {
            sys,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
        }
    }

    pub fn collect(&mut self) -> SystemSnapshot {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys
            .refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        self.networks.refresh(true);
        self.disks.refresh(true);

        let cpu_per_core: Vec<f32> = self.sys.cpus().iter().map(|c| c.cpu_usage()).collect();
        let cpu_percent = if cpu_per_core.is_empty() {
            0.0
        } else {
            cpu_per_core.iter().sum::<f32>() / cpu_per_core.len() as f32
        };

        let mut processes: Vec<ProcessInfo> = self
            .sys
            .processes()
            .values()
            .map(|p| ProcessInfo {
                pid: p.pid().as_u32(),
                name: p.name().to_string_lossy().to_string(),
                cpu_percent: p.cpu_usage(),
                mem_bytes: p.memory(),
                status: format!("{:?}", p.status()),
            })
            .collect();
        processes.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        processes.truncate(100);

        let mut net_rx: u64 = 0;
        let mut net_tx: u64 = 0;
        for data in self.networks.values() {
            net_rx += data.received();
            net_tx += data.transmitted();
        }

        let disks: Vec<DiskInfo> = self
            .disks
            .iter()
            .filter(|d| d.total_space() > 0)
            .map(|d| {
                let total = d.total_space();
                let used = total - d.available_space();
                DiskInfo {
                    mount: d.mount_point().to_string_lossy().to_string(),
                    total,
                    used,
                    percent: (used as f32 / total as f32) * 100.0,
                }
            })
            .collect();

        SystemSnapshot {
            hostname: System::host_name().unwrap_or_else(|| "unknown".into()),
            uptime_secs: System::uptime(),
            cpu_percent,
            cpu_per_core,
            mem_total: self.sys.total_memory(),
            mem_used: self.sys.used_memory(),
            mem_percent: (self.sys.used_memory() as f32 / self.sys.total_memory().max(1) as f32)
                * 100.0,
            swap_total: self.sys.total_swap(),
            swap_used: self.sys.used_swap(),
            processes,
            net_rx_bytes: net_rx,
            net_tx_bytes: net_tx,
            disks,
        }
    }
}
