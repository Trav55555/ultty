//! ADR 0022 prototype harness (feature `proto`): the three arms, the
//! correctness check that they do the same work, and a `pty_flood`-style
//! wall-clock harness that runs all three through the same PTY path.
//!
//! - reference: today's `Terminal` + `TerminalHandler`;
//! - A: packed cells in `Vec<Row>` (`ultty_vt::grid::proto::TerminalA`);
//! - C: screen page + history pages (`ultty_vt::grid::proto::TerminalC`).

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::pty::{openpty, Winsize};
use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg};
use ultty_vt::ansi::{AnsiOutput, AnsiParser};
use ultty_vt::grid::proto::{TerminalA, TerminalC};
use ultty_vt::grid::{Cell, Dimensions, Row, Terminal, TerminalHandler};

/// Bytes handed to the parser per call in the headless passes.
pub const CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    Reference,
    A,
    C,
}

impl Arm {
    pub const ALL: [Arm; 3] = [Arm::Reference, Arm::A, Arm::C];

    pub fn name(self) -> &'static str {
        match self {
            Arm::Reference => "ref",
            Arm::A => "A",
            Arm::C => "C",
        }
    }

    pub fn from_name(name: &str) -> Option<Arm> {
        Arm::ALL
            .into_iter()
            .find(|arm| arm.name().eq_ignore_ascii_case(name))
    }
}

/// One arm's terminal, fed through one parser.
pub enum Target {
    Reference(Box<Terminal>),
    A(Box<TerminalA>),
    C(Box<TerminalC>),
}

impl Target {
    pub fn new(arm: Arm, cols: u16, rows: u16) -> Self {
        match arm {
            Arm::Reference => {
                Target::Reference(Box::new(Terminal::new(Dimensions::new(cols, rows))))
            }
            Arm::A => Target::A(Box::new(TerminalA::new(cols, rows))),
            Arm::C => Target::C(Box::new(TerminalC::new(cols, rows))),
        }
    }

    /// Parse and apply `data`, as `ultty-bench vt` does for the reference.
    pub fn feed(&mut self, parser: &mut AnsiParser, data: &[u8]) {
        match self {
            Target::Reference(terminal) => {
                let mut handler = TerminalHandler::new(terminal);
                parser.feed_with_ascii_spans(data, |out| match out {
                    AnsiOutput::Action(action) => handler.handle(action),
                    AnsiOutput::PrintAscii(bytes) => handler.handle_ascii_span(bytes),
                });
            }
            Target::A(terminal) => terminal.feed(parser, data),
            Target::C(terminal) => terminal.feed(parser, data),
        }
    }

    /// The bench thread's frame: copy every visible row, as the renderer's
    /// row cache does (`Row::copy_from` today, an unpacking copy for the
    /// prototypes, ADR §5).
    pub fn copy_visible(&self, rows: u16, reference_rows: &mut Vec<Row>, cells: &mut [Vec<Cell>]) {
        match self {
            Target::Reference(terminal) => {
                let buffer = terminal.buffer();
                let cols = buffer.dimensions().cols;
                reference_rows.resize_with(usize::from(rows), || Row::new(cols));
                for (index, row) in reference_rows.iter_mut().enumerate() {
                    if let Some(visible) = buffer.visible_row(index as u16) {
                        visible.copy_into(row);
                    }
                }
            }
            Target::A(terminal) => {
                for (index, out) in cells.iter_mut().enumerate().take(usize::from(rows)) {
                    terminal.visible_row(index as u16, out);
                }
            }
            Target::C(terminal) => {
                for (index, out) in cells.iter_mut().enumerate().take(usize::from(rows)) {
                    terminal.visible_row(index as u16, out);
                }
            }
        }
    }

    /// Cursor column, row and pending wrap.
    fn cursor(&self) -> (u16, u16, bool) {
        match self {
            Target::Reference(terminal) => {
                let cursor = terminal.grid().cursor();
                (
                    cursor.position.col,
                    cursor.position.row,
                    cursor.wrap_pending,
                )
            }
            Target::A(terminal) => terminal.cursor(),
            Target::C(terminal) => terminal.cursor(),
        }
    }

    fn history_len(&self) -> usize {
        match self {
            Target::Reference(terminal) => terminal.buffer().history_len(),
            Target::A(terminal) => terminal.history_len(),
            Target::C(terminal) => terminal.history_len(),
        }
    }

    /// Row `index` of history followed by the screen, as cells; whether it
    /// is soft-wrapped.
    fn line(&self, index: usize, out: &mut Vec<Cell>) -> bool {
        let history = self.history_len();
        match self {
            Target::Reference(terminal) => {
                let row = terminal.buffer().row_at(index).expect("row in range");
                out.clear();
                out.extend(row.cells());
                row.is_wrapped()
            }
            Target::A(terminal) if index < history => terminal.history_row(index, out),
            Target::A(terminal) => terminal.visible_row((index - history) as u16, out),
            Target::C(terminal) if index < history => terminal.history_row(index, out),
            Target::C(terminal) => terminal.visible_row((index - history) as u16, out),
        }
    }
}

/// Feed `data` into a fresh `arm` terminal in [`CHUNK`]s.
pub fn filled(arm: Arm, cols: u16, rows: u16, data: &[u8]) -> Target {
    let mut target = Target::new(arm, cols, rows);
    let mut parser = AnsiParser::new();
    for chunk in data.chunks(CHUNK) {
        target.feed(&mut parser, chunk);
    }
    target
}

/// Compare `arm` with the reference after both consumed `data`: cursor,
/// history length, and every history and screen row cell for cell
/// (character, colors, flags, wide structure) plus its wrap flag.
/// Returns the number of rows compared.
pub fn check(arm: Arm, cols: u16, rows: u16, data: &[u8]) -> Result<usize, String> {
    let expected = filled(Arm::Reference, cols, rows, data);
    let actual = filled(arm, cols, rows, data);
    if expected.cursor() != actual.cursor() {
        return Err(format!(
            "cursor {:?}, reference {:?}",
            actual.cursor(),
            expected.cursor()
        ));
    }
    if expected.history_len() != actual.history_len() {
        return Err(format!(
            "history_len {}, reference {}",
            actual.history_len(),
            expected.history_len()
        ));
    }
    let lines = expected.history_len() + usize::from(rows);
    let (mut want, mut got) = (Vec::new(), Vec::new());
    for index in 0..lines {
        let want_wrapped = expected.line(index, &mut want);
        let got_wrapped = actual.line(index, &mut got);
        if want != got || want_wrapped != got_wrapped {
            let text = |cells: &[Cell]| cells.iter().map(Cell::char).collect::<String>();
            return Err(format!(
                "line {index} differs (wrapped {got_wrapped} vs {want_wrapped}):\n  got {:?}\n  ref {:?}",
                text(&got),
                text(&want)
            ));
        }
    }
    Ok(lines)
}

/// What one flood run measured.
#[derive(Debug, Clone, Copy)]
pub struct Flood {
    pub secs: f64,
    pub reads: usize,
    pub frames: usize,
}

/// `pty_flood`'s shape for one arm: `dd` writes `path` into a raw PTY in
/// 64 KiB writes; this thread reads 4 KiB at a time and parses and feeds
/// each read under the terminal mutex; a bench thread takes the mutex once
/// per 16 ms frame to copy the visible rows. Timed from spawn to EOF.
pub fn flood(arm: Arm, path: &Path, cols: u16, rows: u16) -> io::Result<Flood> {
    const FRAME: Duration = Duration::from_millis(16);
    let size = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pty = openpty(&size, None).map_err(io::Error::from)?;
    let mut termios = tcgetattr(&pty.slave).map_err(io::Error::from)?;
    cfmakeraw(&mut termios);
    tcsetattr(&pty.slave, SetArg::TCSANOW, &termios).map_err(io::Error::from)?;

    let target = Arc::new(Mutex::new((
        Target::new(arm, cols, rows),
        AnsiParser::new(),
    )));
    let done = Arc::new(AtomicBool::new(false));
    let bench = {
        let target = Arc::clone(&target);
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut reference_rows = Vec::new();
            let mut cells = vec![Vec::new(); usize::from(rows)];
            let mut frames = 0;
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(FRAME);
                let guard = target.lock().expect("terminal lock");
                guard.0.copy_visible(rows, &mut reference_rows, &mut cells);
                frames += 1;
            }
            std::hint::black_box((&reference_rows, &cells));
            frames
        })
    };

    let start = Instant::now();
    let mut child = {
        let slave = File::from(pty.slave);
        Command::new("dd")
            .arg(format!("if={}", path.display()))
            .arg("bs=65536")
            .arg("status=none")
            .stdin(Stdio::null())
            .stdout(slave)
            .spawn()?
        // The Command, and with it our copy of the slave, drops here, so
        // the master reads EOF (EIO) once dd exits.
    };
    let mut master = File::from(pty.master);
    let mut buf = [0u8; 4096];
    let mut reads = 0;
    loop {
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let mut guard = target.lock().expect("terminal lock");
                let (terminal, parser) = &mut *guard;
                terminal.feed(parser, &buf[..n]);
                reads += 1;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.raw_os_error() == Some(nix::libc::EIO) => break,
            Err(e) => return Err(e),
        }
    }
    let secs = start.elapsed().as_secs_f64();
    child.wait()?;
    done.store(true, Ordering::Relaxed);
    let frames = bench.join().expect("bench thread");
    std::hint::black_box(&target);
    Ok(Flood {
        secs,
        reads,
        frames,
    })
}
