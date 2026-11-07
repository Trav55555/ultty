//! Environment metadata recorded with every result.
//!
//! Reads only `/proc`, `/sys` and `git`; nothing here needs root. Values that
//! cannot be read are recorded as `null` rather than guessed.

use serde_json::{json, Value};
use std::process::Command;

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// One-minute load average.
pub fn load_average() -> Option<f64> {
    read_trimmed("/proc/loadavg")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn cpu_model() -> Option<String> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    info.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}

/// The CPUs this process may run on, as the kernel prints them ("5,11").
pub fn allowed_cpus() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with("Cpus_allowed_list:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}

/// First CPU of the allowed set: governor and EPP are read for it.
fn first_allowed_cpu() -> u32 {
    allowed_cpus()
        .and_then(|list| {
            list.split([',', '-'])
                .next()
                .and_then(|s| s.trim().parse().ok())
        })
        .unwrap_or(0)
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Revision under test: `ULTTY_BENCH_REV` when a driver script sets it
/// (it knows which worktree it built), else `git` in the current directory.
pub fn revision() -> (Option<String>, Option<bool>) {
    if let Ok(rev) = std::env::var("ULTTY_BENCH_REV") {
        let dirty = std::env::var("ULTTY_BENCH_DIRTY").ok().map(|v| v == "1");
        return (Some(rev), dirty);
    }
    let rev = git(&["rev-parse", "--short=12", "HEAD"]);
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).map(|s| !s.is_empty());
    (rev, dirty)
}

/// Snapshot taken at the start of a session. `load_end` is added when the
/// session finishes.
pub fn snapshot() -> Value {
    let cpu = first_allowed_cpu();
    let cpufreq = format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq");
    let (rev, dirty) = revision();
    json!({
        "rev": rev,
        "dirty": dirty,
        "rustc": env!("ULTTY_BENCH_RUSTC"),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "cpu": cpu_model(),
        "kernel": read_trimmed("/proc/sys/kernel/osrelease"),
        "governor": read_trimmed(&format!("{cpufreq}/scaling_governor")),
        "epp": read_trimmed(&format!("{cpufreq}/energy_performance_preference")),
        "boost": read_trimmed("/sys/devices/system/cpu/cpufreq/boost"),
        "pin": allowed_cpus(),
        "nproc": std::thread::available_parallelism().map(|n| n.get()).ok(),
        "load_start": load_average(),
    })
}
