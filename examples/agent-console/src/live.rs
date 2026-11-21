//! Translates versioned CLI streams into the console's event schema.

mod codex;

use std::io::{BufRead, Lines};
use std::time::Instant;
use std::vec;

use anyhow::{bail, Context};

use agent_console::event::{Event, EventKind};

use self::codex::Codex;

const CODEX_VERSION: &str = "0.171.0";

pub(super) struct Args {
    adapter: Codex,
    prompt: Option<String>,
}

pub(super) fn parse_args(mut args: impl Iterator<Item = String>) -> anyhow::Result<Args> {
    let provider = args.next().context(super::USAGE)?;
    if provider != "codex" {
        bail!("unknown live adapter {provider:?}");
    }
    if args.next().as_deref() != Some("--cli-version") {
        bail!(super::USAGE);
    }
    let version = args.next().context(super::USAGE)?;
    if version != CODEX_VERSION && version != format!("codex-cli {CODEX_VERSION}") {
        bail!("Codex CLI {version:?} is unsupported; expected {CODEX_VERSION}");
    }
    let prompt = match args.next().as_deref() {
        Some("--prompt") => Some(args.next().context(super::USAGE)?),
        None => None,
        _ => bail!(super::USAGE),
    };
    if args.next().is_some() {
        bail!(super::USAGE);
    }
    Ok(Args {
        adapter: Codex::default(),
        prompt,
    })
}

pub(super) struct Reader<R> {
    lines: std::iter::Enumerate<Lines<R>>,
    pending: vec::IntoIter<EventKind>,
    adapter: Codex,
    start: Instant,
    t: u64,
}

impl<R: BufRead> Reader<R> {
    pub(super) fn new(input: R, args: Args) -> Self {
        let pending = args
            .prompt
            .map(|text| EventKind::Prompt { text })
            .into_iter()
            .collect::<Vec<_>>()
            .into_iter();
        Self {
            lines: input.lines().enumerate(),
            pending,
            adapter: args.adapter,
            start: Instant::now(),
            t: 0,
        }
    }

    pub(super) fn finish(self) -> anyhow::Result<()> {
        self.adapter.finish()
    }

    fn fill(&mut self, index: usize, line: std::io::Result<String>) -> anyhow::Result<()> {
        let line = line.with_context(|| format!("live input line {}: read failed", index + 1))?;
        if !line.trim().is_empty() {
            self.pending = self
                .adapter
                .translate(&line, self.t)
                .with_context(|| format!("live input line {}", index + 1))?
                .into_iter();
        }
        Ok(())
    }
}

impl<R: BufRead> Iterator for Reader<R> {
    type Item = anyhow::Result<Event>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(kind) = self.pending.next() {
                return Some(Ok(Event::new(self.t, kind)));
            }
            let (index, line) = self.lines.next()?;
            self.t = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
            if let Err(error) = self.fill(index, line) {
                return Some(Err(error));
            }
        }
    }
}
