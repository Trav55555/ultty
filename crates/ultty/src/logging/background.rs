//! Writes log lines on a background thread so the UI never waits on disk.
//!
//! Each tracing event is formatted on the thread that emitted it and handed
//! to the writer thread over a bounded channel. When the channel is full the
//! line is dropped and counted rather than blocking; the writer thread
//! reports how many lines it lost once it catches up.

use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use tracing_subscriber::fmt::MakeWriter;

/// Lines queued for the writer thread before new ones are dropped.
pub const DEFAULT_CAPACITY: usize = 4096;

/// How long [`LogGuard`] waits for queued lines to reach the file at exit.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

enum Message {
    Line(Vec<u8>),
    Flush(SyncSender<()>),
}

/// The sending side, given to the tracing file layer as its writer.
#[derive(Clone)]
pub struct BackgroundWriter {
    sender: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
}

/// Dropping it waits briefly for queued lines to reach the file.
pub struct LogGuard {
    writer: BackgroundWriter,
}

impl BackgroundWriter {
    /// Start a writer thread that appends to `sink`, prefixing every line
    /// with `pid=<pid> `, with room for `capacity` queued lines.
    pub fn spawn(
        sink: impl Write + Send + 'static,
        pid: u32,
        capacity: usize,
    ) -> io::Result<(Self, LogGuard)> {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        std::thread::Builder::new()
            .name("ultty-log".into())
            .spawn({
                let dropped = Arc::clone(&dropped);
                move || run(receiver, sink, pid, &dropped)
            })?;
        let writer = Self { sender, dropped };
        Ok((writer.clone(), LogGuard { writer }))
    }

    /// Queue one formatted event; drops and counts it when the queue is
    /// full or the writer thread is gone.
    fn send(&self, line: Vec<u8>) {
        if line.is_empty() {
            return;
        }
        match self.sender.try_send(Message::Line(line)) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Lines dropped since the writer thread last reported them.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Wait up to `timeout` for every line queued so far to be written.
    /// Returns whether the writer thread confirmed it.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (done, wait) = mpsc::sync_channel(1);
        self.sender.send(Message::Flush(done)).is_ok() && wait.recv_timeout(timeout).is_ok()
    }
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        self.writer.flush(FLUSH_TIMEOUT);
    }
}

/// One event's bytes, sent when the formatter drops the writer.
pub struct EventWriter<'a> {
    writer: &'a BackgroundWriter,
    buffer: Vec<u8>,
}

impl Write for EventWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for EventWriter<'_> {
    fn drop(&mut self) {
        self.writer.send(std::mem::take(&mut self.buffer));
    }
}

impl<'a> MakeWriter<'a> for BackgroundWriter {
    type Writer = EventWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        EventWriter {
            writer: self,
            buffer: Vec::new(),
        }
    }
}

fn run(receiver: Receiver<Message>, mut sink: impl Write, pid: u32, dropped: &AtomicU64) {
    let prefix = format!("pid={pid} ");
    let mut failed = false;
    let mut batch = Vec::new();
    while let Ok(first) = receiver.recv() {
        let mut flushes = Vec::new();
        let mut next = Some(first);
        while let Some(message) = next {
            match message {
                Message::Line(line) => push_prefixed(&mut batch, &prefix, &line),
                Message::Flush(done) => flushes.push(done),
            }
            next = receiver.try_recv().ok();
        }
        let lost = dropped.swap(0, Ordering::Relaxed);
        if lost > 0 {
            let note = format!(
                "WARN ultty::logging: dropped {lost} log lines while the log writer was behind\n"
            );
            push_prefixed(&mut batch, &prefix, note.as_bytes());
        }
        if !batch.is_empty() {
            let written = sink.write_all(&batch).and_then(|()| sink.flush());
            if let Err(error) = written {
                if !failed {
                    failed = true;
                    tracing::warn!(%error, "could not write the log file; later errors are not reported");
                }
            }
            batch.clear();
        }
        for done in flushes {
            let _ = done.send(());
        }
    }
}

/// Append `text` to `out` with `prefix` at the start of each line, ending
/// with a newline.
fn push_prefixed(out: &mut Vec<u8>, prefix: &str, text: &[u8]) {
    for line in text.split_inclusive(|&byte| byte == b'\n') {
        out.extend_from_slice(prefix.as_bytes());
        out.extend_from_slice(line);
    }
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Mutex};

    /// A sink tests can read, and hold shut until `gate` opens.
    #[derive(Clone)]
    struct SharedSink {
        bytes: Arc<Mutex<Vec<u8>>>,
        gate: Option<Arc<Barrier>>,
    }

    impl Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(gate) = self.gate.take() {
                gate.wait();
            }
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn text(sink: &SharedSink) -> String {
        String::from_utf8(sink.bytes.lock().unwrap().clone()).unwrap()
    }

    fn log(writer: &BackgroundWriter, line: &str) {
        writer.make_writer().write_all(line.as_bytes()).unwrap();
    }

    #[test]
    fn test_background_writer_prefixes_every_line_with_the_pid() {
        let sink = SharedSink {
            bytes: Arc::default(),
            gate: None,
        };
        let (writer, _guard) = BackgroundWriter::spawn(sink.clone(), 42, 16).unwrap();
        log(&writer, "one\n");
        log(&writer, "two\nthree\n");
        assert!(writer.flush(Duration::from_secs(5)));
        assert_eq!(text(&sink), "pid=42 one\npid=42 two\npid=42 three\n");
    }

    #[test]
    fn test_background_writer_drops_when_full_and_reports_the_count() {
        let gate = Arc::new(Barrier::new(2));
        let sink = SharedSink {
            bytes: Arc::default(),
            gate: Some(Arc::clone(&gate)),
        };
        let (writer, _guard) = BackgroundWriter::spawn(sink.clone(), 7, 4).unwrap();
        log(&writer, "first\n");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while writer
            .sender
            .try_send(Message::Line(b"probe\n".to_vec()))
            .is_ok()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the queue never filled"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let before = writer.dropped();
        for _ in 0..10 {
            log(&writer, "lost\n");
        }
        assert_eq!(
            writer.dropped() - before,
            10,
            "a full queue drops, never blocks"
        );

        gate.wait();
        assert!(writer.flush(Duration::from_secs(5)));
        log(&writer, "after\n");
        assert!(writer.flush(Duration::from_secs(5)));
        let written = text(&sink);
        assert!(!written.contains("lost"), "{written}");
        assert!(
            written.contains("pid=7 WARN ultty::logging: dropped 10 log lines"),
            "{written}"
        );
        assert!(written.ends_with("pid=7 after\n"), "{written}");
        assert_eq!(writer.dropped(), 0);
    }
}
