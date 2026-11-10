//! A real app in ANSI mode on a real pty: input bursts larger than one read
//! all arrive, and the event loop sleeps between them.
//!
//! The test binary runs itself again as the app (the `app_on_pty` test,
//! which does nothing unless `ULTTUI_PTY_APP_OUT` is set), with the pty as
//! its controlling terminal. Linux only: it reads the app's CPU time and
//! wakeups from /proc.
#![cfg(target_os = "linux")]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ulttui::app::Application;
use ulttui::cmd::Cmd;
use ulttui::event::{Event, Key};
use ulttui::widgets::{Text, View};

const OUT_ENV: &str = "ULTTUI_PTY_APP_OUT";

/// Collects typed text; on `!` writes it to the file named by `OUT_ENV`.
struct Typist {
    text: String,
    out: PathBuf,
}

impl Application for Typist {
    type Message = Option<char>;

    fn update(&mut self, msg: Option<char>) -> Cmd<Option<char>> {
        match msg {
            Some('!') => {
                let _ = std::fs::write(&self.out, &self.text);
            }
            Some(c) => self.text.push(c),
            None => {}
        }
        Cmd::none()
    }

    fn on_event(&mut self, event: &Event) -> Option<Option<char>> {
        match event {
            Event::Key(key) => match key.key {
                Key::Char(c) => Some(Some(c)),
                Key::Enter => Some(Some('\r')),
                _ => Some(None),
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Text::new(format!("{} chars", self.text.chars().count()))
    }
}

#[test]
fn app_on_pty() {
    let Some(out) = std::env::var_os(OUT_ENV) else {
        return;
    };
    Typist {
        text: String::new(),
        out: out.into(),
    }
    .run()
    .unwrap();
}

/// CPU seconds and context switches the process has used so far.
fn usage(pid: u32) -> (f64, u64) {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<&str> = stat
        .rsplit(')')
        .next()
        .unwrap()
        .split_whitespace()
        .collect();
    // SAFETY: sysconf has no preconditions.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let cpu = (fields[11].parse::<f64>().unwrap() + fields[12].parse::<f64>().unwrap()) / ticks;
    let mut switches = 0;
    for task in std::fs::read_dir(format!("/proc/{pid}/task")).unwrap() {
        let status =
            std::fs::read_to_string(task.unwrap().path().join("status")).unwrap_or_default();
        switches += status
            .lines()
            .filter(|l| l.contains("_ctxt_switches"))
            .map(|l| l.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap())
            .sum::<u64>();
    }
    (cpu, switches)
}

/// Start the app on a new pty; returns it and the pty's master side.
fn spawn_on_pty(out: &std::path::Path) -> (Child, File) {
    let (mut master, mut slave) = (0, 0);
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty writes two descriptors; name and termios may be null.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    // openpty does not set close-on-exec. Without it the app (and any other
    // child spawned meanwhile) would hold the master open, and would never
    // see the hang-up if this test process died.
    for fd in [master, slave] {
        // SAFETY: `fd` was just opened by openpty.
        let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        assert_eq!(rc, 0, "fcntl: {}", std::io::Error::last_os_error());
    }
    // SAFETY: both were just opened and are owned here.
    let (master, slave) = unsafe { (File::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let stdio = || Stdio::from(slave.try_clone().unwrap());
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "app_on_pty", "--nocapture", "--test-threads=1"])
        .env_remove("ULTTY_SOCKET")
        .env_remove("ULTTY_PID")
        .env("ULTTY_IN_BAND", "0")
        .env("TERM", "xterm-256color")
        .env(OUT_ENV, out)
        .stdin(stdio())
        .stdout(stdio())
        .stderr(stdio());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    drop(slave);
    (child, master)
}

/// Kills the app if the test fails before it quits.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_burst_bigger_than_one_read_is_all_delivered_and_the_loop_then_sleeps() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let out = dir.join(format!("ansi-pty-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let (child, master) = spawn_on_pty(&out);
    let mut child = KillOnDrop(child);
    let pid = child.0.id();

    // Drain the app's output so its writes never block, and say when it
    // has switched to the alternate screen: it is up and drawing.
    let mut reader = master.try_clone().unwrap();
    let (started_tx, started) = mpsc::channel();
    std::thread::spawn(move || {
        const ALT_SCREEN: &[u8] = b"\x1b[?1049h";
        let mut seen = Vec::new();
        let mut announced = false;
        let mut buf = [0u8; 65536];
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            if !announced {
                seen.extend_from_slice(&buf[..n]);
                if seen.windows(ALT_SCREEN.len()).any(|w| w == ALT_SCREEN) {
                    announced = true;
                    let _ = started_tx.send(());
                }
            }
        }
    });
    let mut writer = master;
    started
        .recv_timeout(Duration::from_secs(20))
        .expect("the app never started drawing");

    // Started and idle: no CPU, (almost) no wakeups. A moment first for the
    // runtime's threads to park.
    std::thread::sleep(Duration::from_millis(300));
    let (cpu0, switches0) = usage(pid);
    std::thread::sleep(Duration::from_secs(1));
    let (cpu1, switches1) = usage(pid);
    assert!(cpu1 - cpu0 < 0.05, "idle used {:.2} s CPU", cpu1 - cpu0);
    assert!(
        switches1 - switches0 <= 2,
        "idle woke {} times",
        switches1 - switches0
    );

    // A 3000-byte burst, then a paste with lines and multi-byte text.
    let mut expected = "a".repeat(3000);
    expected.push_str("pasted line\rsecond line é€ and more\r");
    let mut input = expected.clone().into_bytes();
    input.push(b'!');
    writer.write_all(&input).unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let received = loop {
        if let Ok(text) = std::fs::read_to_string(&out) {
            break text;
        }
        assert!(Instant::now() < deadline, "the app never saw the end");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(received.len(), expected.len());
    assert_eq!(received, expected);

    // All handled: the loop sleeps again instead of spinning on the fd.
    std::thread::sleep(Duration::from_millis(300));
    let (cpu0, switches0) = usage(pid);
    std::thread::sleep(Duration::from_secs(1));
    let (cpu1, switches1) = usage(pid);
    assert!(
        cpu1 - cpu0 < 0.05,
        "after the burst used {:.2} s CPU",
        cpu1 - cpu0
    );
    assert!(
        switches1 - switches0 <= 2,
        "after the burst woke {} times",
        switches1 - switches0
    );

    // Ctrl+C quits.
    writer.write_all(b"\x03").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            break;
        }
        assert!(Instant::now() < deadline, "the app did not quit");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_file(&out);
}
