//! `cat` a file through a real PTY into the event loop's I/O thread with no
//! renderer, and report MiB/s: the end-to-end ceiling before drawing.
//!
//! cargo run --release -p ultty --example pty_throughput -- <file>
use std::time::{Duration, Instant};
use ultty::event_loop::{EventLoop, TerminalEvent};

fn main() -> std::io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: pty_throughput <file>");
    let bytes = std::fs::metadata(&path)?.len() as f64;
    for run in 1..=3 {
        let start = Instant::now();
        let mut event_loop = EventLoop::with_command("/bin/cat", &["/bin/cat", &path], 120, 40)?;
        'outer: loop {
            for event in event_loop.wait(Duration::from_millis(50))? {
                if let TerminalEvent::ChildExited { .. } = event {
                    break 'outer;
                }
            }
        }
        let secs = start.elapsed().as_secs_f64();
        println!(
            "run {run}: {:.0} ms, {:.0} MiB/s",
            secs * 1e3,
            bytes / secs / 1048576.0
        );
    }
    Ok(())
}
