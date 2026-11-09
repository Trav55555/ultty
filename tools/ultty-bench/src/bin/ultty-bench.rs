//! Headless benchmark driver.
//!
//! ultty-bench vt [options]       parse+grid throughput through the production path
//! ultty-bench corpus --out DIR   write the corpora as files for the PTY drivers
//!
//! With the `proto` feature (ADR 0022 page-storage prototype):
//! ultty-bench vt --suite proto   reference (`vt`), variant A and variant C;
//!                                `proto-a` and `proto-c` run one variant
//! ultty-bench proto-check        A and C against the reference, row by row
//! ultty-bench proto-flood        PTY wall-clock A/B/C (see `proto::flood`)
//!
//! `vt` options:
//!   --corpus NAMES      comma list, or `all` (default: all)
//!   --corpus-file PATH  add an external file (refused if it has a bare LF)
//!   --size-mib N        corpus size (default 16)
//!   --geometry LIST     e.g. 120x40,80x24 (default 120x40)
//!   --screen LIST       main,alt (default main)
//!   --suite LIST        vt,parse (default vt); proto,proto-a,proto-c
//!                       with the `proto` feature
//!   --runs N            blocks; every case runs once per block, in a
//!                       shuffled order, so drift hits all cases alike (default 10)
//!   --warmup N          untimed passes per case before block 0 (default 1)
//!   --block-start N     first block number (lets a driver interleave arms)
//!   --arm LABEL         arm label for A/B analysis (default A)
//!   --out PATH          append JSON lines here (default: stdout)
//!
//! Every sample is one JSON object; see docs/perf/README.md for the schema.
//! The console summary (median, IQR, min, n) goes to stderr.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use serde_json::{json, Value};
use ultty_bench::corpus::{self, Corpus};
use ultty_bench::{env, stats};
use ultty_vt::ansi::{AnsiOutput, AnsiParser};
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

/// Bytes handed to the parser per call: what one PTY read delivers.
const CHUNK: usize = 64 * 1024;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("vt") => cmd_vt(&args[1..]),
        Some("corpus") => cmd_corpus(&args[1..]),
        #[cfg(feature = "proto")]
        Some("proto-check") => cmd_proto_check(&args[1..]),
        #[cfg(feature = "proto")]
        Some("proto-flood") => cmd_proto_flood(&args[1..]),
        _ => Err(
            "usage: ultty-bench <vt|corpus> [options] (see the source header for options)".into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("ultty-bench: {message}");
            ExitCode::FAILURE
        }
    }
}

/// `--flag value` pairs; every flag takes exactly one value.
fn parse_flags(args: &[String], known: &[&str]) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let name = flag
            .strip_prefix("--")
            .filter(|n| known.contains(n))
            .ok_or_else(|| format!("unknown argument {flag:?}"))?;
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        flags
            .entry(name.to_string())
            .or_default()
            .push(value.clone());
    }
    Ok(flags)
}

fn one<'a>(flags: &'a BTreeMap<String, Vec<String>>, name: &str, default: &'a str) -> &'a str {
    flags
        .get(name)
        .and_then(|v| v.last())
        .map_or(default, String::as_str)
}

fn number<T: std::str::FromStr>(
    flags: &BTreeMap<String, Vec<String>>,
    name: &str,
    default: &str,
) -> Result<T, String> {
    one(flags, name, default)
        .parse()
        .map_err(|_| format!("--{name} must be a number"))
}

fn list<'a>(
    flags: &'a BTreeMap<String, Vec<String>>,
    name: &str,
    default: &'a str,
) -> Vec<&'a str> {
    one(flags, name, default)
        .split(',')
        .filter(|s| !s.is_empty())
        .collect()
}

fn selected_corpora(names: &[&str]) -> Result<Vec<Corpus>, String> {
    if names == ["all"] {
        return Ok(Corpus::ALL.to_vec());
    }
    names
        .iter()
        .map(|n| Corpus::from_name(n).ok_or_else(|| format!("unknown corpus {n:?}")))
        .collect()
}

struct Input {
    name: String,
    bytes: Vec<u8>,
    sha256: String,
}

fn load_inputs(flags: &BTreeMap<String, Vec<String>>) -> Result<Vec<Input>, String> {
    let size_mib: usize = number(flags, "size-mib", "16")?;
    let mut inputs = Vec::new();
    let external = flags.get("corpus-file").cloned().unwrap_or_default();
    // External files alone mean "just these", unless --corpus asks for more.
    let default_corpus = if external.is_empty() { "all" } else { "" };
    for corpus in selected_corpora(&list(flags, "corpus", default_corpus))? {
        let g = corpus::generate(corpus, size_mib * 1024 * 1024);
        inputs.push(Input {
            name: corpus.name().to_string(),
            bytes: g.bytes,
            sha256: g.sha256,
        });
    }
    for path in external {
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?;
        if let Some(offset) = corpus::first_bare_lf(&bytes) {
            return Err(format!(
                "{path} has a bare LF at byte {offset}; convert it to CRLF (or replay it \
                 through a PTY with ONLCR) so the result reflects real terminal output"
            ));
        }
        let name = Path::new(&path)
            .file_stem()
            .map_or_else(|| path.clone(), |s| s.to_string_lossy().into_owned());
        let sha256 = corpus::sha256_hex(&bytes);
        inputs.push(Input {
            name,
            bytes,
            sha256,
        });
    }
    if inputs.is_empty() {
        return Err("no corpus selected".into());
    }
    Ok(inputs)
}

fn parse_geometry(s: &str) -> Result<(u16, u16), String> {
    let (c, r) = s
        .split_once('x')
        .ok_or_else(|| format!("geometry {s:?} is not COLSxROWS"))?;
    let cols = c.parse().map_err(|_| format!("bad cols in {s:?}"))?;
    let rows = r.parse().map_err(|_| format!("bad rows in {s:?}"))?;
    Ok((cols, rows))
}

/// One parse+grid pass over `data`. Kept out of line so instruction-count
/// tools can attribute (and toggle collection on) exactly this work.
#[inline(never)]
fn run_vt_pass(data: &[u8], cols: u16, rows: u16, alt: bool) -> f64 {
    let mut terminal = Terminal::new(Dimensions::new(cols, rows));
    let mut parser = AnsiParser::new();
    if alt {
        terminal.enter_alternate_screen_clear();
    }
    let start = Instant::now();
    for chunk in data.chunks(CHUNK) {
        let mut handler = TerminalHandler::new(&mut terminal);
        parser.feed_with_ascii_spans(chunk, |out| match out {
            AnsiOutput::Action(action) => handler.handle(action),
            AnsiOutput::PrintAscii(bytes) => handler.handle_ascii_span(bytes),
        });
    }
    let secs = start.elapsed().as_secs_f64();
    black_box(terminal.grid().cursor().position);
    secs
}

/// [`run_vt_pass`] for prototype variant A (ADR 0022).
#[cfg(feature = "proto")]
#[inline(never)]
fn run_proto_a_pass(data: &[u8], cols: u16, rows: u16) -> f64 {
    let mut terminal = ultty_vt::grid::proto::TerminalA::new(cols, rows);
    let mut parser = AnsiParser::new();
    let start = Instant::now();
    for chunk in data.chunks(CHUNK) {
        terminal.feed(&mut parser, chunk);
    }
    let secs = start.elapsed().as_secs_f64();
    black_box(terminal.cursor());
    black_box(terminal.bookkeeping_digest());
    secs
}

/// [`run_vt_pass`] for prototype variant C (ADR 0022).
#[cfg(feature = "proto")]
#[inline(never)]
fn run_proto_c_pass(data: &[u8], cols: u16, rows: u16) -> f64 {
    let mut terminal = ultty_vt::grid::proto::TerminalC::new(cols, rows);
    let mut parser = AnsiParser::new();
    let start = Instant::now();
    for chunk in data.chunks(CHUNK) {
        terminal.feed(&mut parser, chunk);
    }
    let secs = start.elapsed().as_secs_f64();
    black_box(terminal.cursor());
    black_box(terminal.bookkeeping_digest());
    secs
}

/// Parser only, into a sink the optimiser cannot see through.
#[inline(never)]
fn run_parse_pass(data: &[u8]) -> f64 {
    let mut parser = AnsiParser::new();
    let mut outputs = 0usize;
    let start = Instant::now();
    for chunk in data.chunks(CHUNK) {
        parser.feed_with_ascii_spans(chunk, |out| {
            black_box(&out);
            outputs += 1;
        });
    }
    let secs = start.elapsed().as_secs_f64();
    black_box(outputs);
    secs
}

#[derive(Clone)]
struct Case {
    suite: &'static str,
    input: usize,
    cols: u16,
    rows: u16,
    alt: bool,
}

impl Case {
    fn id(&self, inputs: &[Input]) -> String {
        let name = &inputs[self.input].name;
        match self.suite {
            "tp.parse" => name.clone(),
            _ => format!(
                "{name}/{}x{}/{}",
                self.cols,
                self.rows,
                if self.alt { "alt" } else { "main" }
            ),
        }
    }

    fn run(&self, inputs: &[Input]) -> f64 {
        let data = &inputs[self.input].bytes;
        match self.suite {
            "tp.parse" => run_parse_pass(data),
            #[cfg(feature = "proto")]
            "tp.proto-a" => run_proto_a_pass(data, self.cols, self.rows),
            #[cfg(feature = "proto")]
            "tp.proto-c" => run_proto_c_pass(data, self.cols, self.rows),
            _ => run_vt_pass(data, self.cols, self.rows, self.alt),
        }
    }
}

/// Deterministic Fisher-Yates so a block's order is reproducible.
fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    for i in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

fn cmd_vt(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(
        args,
        &[
            "corpus",
            "corpus-file",
            "size-mib",
            "geometry",
            "screen",
            "suite",
            "runs",
            "warmup",
            "block-start",
            "arm",
            "out",
        ],
    )?;
    let runs: u64 = number(&flags, "runs", "10")?;
    let warmup: u64 = number(&flags, "warmup", "1")?;
    let block_start: u64 = number(&flags, "block-start", "0")?;
    let arm = one(&flags, "arm", "A").to_string();
    let geometries = list(&flags, "geometry", "120x40")
        .into_iter()
        .map(parse_geometry)
        .collect::<Result<Vec<_>, _>>()?;
    let screens = list(&flags, "screen", "main");
    for screen in &screens {
        if !matches!(*screen, "main" | "alt") {
            return Err(format!("unknown screen {screen:?}"));
        }
    }
    let mut suites = list(&flags, "suite", "vt");
    if let Some(at) = suites.iter().position(|s| *s == "proto") {
        suites.splice(at..=at, ["vt", "proto-a", "proto-c"]);
    }

    let inputs = load_inputs(&flags)?;
    let mut cases = Vec::new();
    for (input, _) in inputs.iter().enumerate() {
        for suite in &suites {
            match *suite {
                "parse" => cases.push(Case {
                    suite: "tp.parse",
                    input,
                    cols: 0,
                    rows: 0,
                    alt: false,
                }),
                #[cfg(feature = "proto")]
                "proto-a" | "proto-c" => {
                    if screens != ["main"] {
                        return Err("the prototype has only a main screen".into());
                    }
                    for &(cols, rows) in &geometries {
                        cases.push(Case {
                            suite: if *suite == "proto-a" {
                                "tp.proto-a"
                            } else {
                                "tp.proto-c"
                            },
                            input,
                            cols,
                            rows,
                            alt: false,
                        });
                    }
                }
                "vt" => {
                    for &(cols, rows) in &geometries {
                        for screen in &screens {
                            cases.push(Case {
                                suite: "tp.vt",
                                input,
                                cols,
                                rows,
                                alt: *screen == "alt",
                            });
                        }
                    }
                }
                other => return Err(format!("unknown suite {other:?}")),
            }
        }
    }

    let mut out: Box<dyn Write> = match flags.get("out").and_then(|v| v.last()) {
        Some(path) => Box::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(PathBuf::from(path))
                .map_err(|e| format!("open {path}: {e}"))?,
        ),
        None => Box::new(std::io::stdout().lock()),
    };

    let mut env = env::snapshot();
    for _ in 0..warmup {
        for case in &cases {
            case.run(&inputs);
        }
    }

    let mut samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut lines = Vec::new();
    for block in block_start..block_start + runs {
        let mut order = cases.clone();
        shuffle(&mut order, block + 1);
        for case in &order {
            let secs = case.run(&inputs);
            let input = &inputs[case.input];
            let mib_s = input.bytes.len() as f64 / (1024.0 * 1024.0) / secs;
            let id = case.id(&inputs);
            samples
                .entry(format!("{} {id}", case.suite))
                .or_default()
                .push(mib_s);
            lines.push(json!({
                "suite": case.suite,
                "claim": if case.suite == "tp.parse" { "micro" } else { "headless-e2e" },
                "case": id,
                "arm": arm,
                "block": block,
                "value": mib_s,
                "unit": "MiB/s",
                "secs": secs,
                "bytes": input.bytes.len(),
                "corpus": input.name,
                "corpus_sha256": input.sha256,
                "chunk": CHUNK,
            }));
        }
    }
    env["load_end"] = json!(env::load_average());
    for mut line in lines {
        line["rev"] = env["rev"].clone();
        line["dirty"] = env["dirty"].clone();
        line["env"] = env.clone();
        writeln!(out, "{line}").map_err(|e| format!("write results: {e}"))?;
    }
    out.flush().map_err(|e| format!("write results: {e}"))?;

    report(&samples, &env);
    Ok(())
}

fn report(samples: &BTreeMap<String, Vec<f64>>, env: &Value) {
    eprintln!(
        "load {} -> {}, pin {}, governor {} / epp {}",
        env["load_start"], env["load_end"], env["pin"], env["governor"], env["epp"]
    );
    eprintln!(
        "{:<48} {:>9} {:>9} {:>9} {:>4}",
        "case (MiB/s)", "median", "IQR", "min", "n"
    );
    for (case, values) in samples {
        if let Some(s) = stats::summarize(values) {
            eprintln!(
                "{case:<48} {:>9.1} {:>9.1} {:>9.1} {:>4}",
                s.median,
                s.q3 - s.q1,
                s.min,
                s.n
            );
        }
    }
}

fn cmd_corpus(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args, &["out", "size-mib", "corpus"])?;
    let dir = PathBuf::from(
        flags
            .get("out")
            .and_then(|v| v.last())
            .ok_or("corpus needs --out DIR")?,
    );
    let size_mib: usize = number(&flags, "size-mib", "16")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let mut manifest = Vec::new();
    for corpus in selected_corpora(&list(&flags, "corpus", "all"))? {
        let g = corpus::generate(corpus, size_mib * 1024 * 1024);
        let path = dir.join(format!("{}.txt", corpus.name()));
        std::fs::write(&path, &g.bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
        eprintln!("{} {} bytes {}", g.sha256, g.bytes.len(), path.display());
        manifest.push(json!({
            "corpus": corpus.name(),
            "file": path.file_name().map(|n| n.to_string_lossy().into_owned()),
            "bytes": g.bytes.len(),
            "sha256": g.sha256,
            "seed": corpus::SEED,
            "authored_for": format!("{}x{}", corpus::AUTHORED_COLS, corpus::AUTHORED_ROWS),
        }));
    }
    let manifest_path = dir.join("manifest.json");
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(&manifest_path, text + "\n")
        .map_err(|e| format!("write {}: {e}", manifest_path.display()))?;
    Ok(())
}

/// `proto-check`: variants A and C against the reference, every history
/// and screen row cell for cell, so the prototype numbers compare
/// equivalent work.
#[cfg(feature = "proto")]
fn cmd_proto_check(args: &[String]) -> Result<(), String> {
    use ultty_bench::proto::{self, Arm};
    let flags = parse_flags(args, &["corpus", "corpus-file", "size-mib", "geometry"])?;
    let geometries = list(&flags, "geometry", "120x40,300x75")
        .into_iter()
        .map(parse_geometry)
        .collect::<Result<Vec<_>, _>>()?;
    let inputs = load_inputs(&flags)?;
    let mut failed = 0;
    for input in &inputs {
        for &(cols, rows) in &geometries {
            for arm in [Arm::A, Arm::C] {
                match proto::check(arm, cols, rows, &input.bytes) {
                    Ok(lines) => eprintln!(
                        "ok   {}/{cols}x{rows} {}: {lines} rows identical",
                        input.name,
                        arm.name()
                    ),
                    Err(message) => {
                        failed += 1;
                        eprintln!(
                            "FAIL {}/{cols}x{rows} {}: {message}",
                            input.name,
                            arm.name()
                        );
                    }
                }
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} case(s) differ from the reference"));
    }
    Ok(())
}

/// `proto-flood`: the PTY wall-clock harness, every arm once per round per
/// geometry, the arm order reversed on odd rounds.
///
/// Options: --corpus NAMES (default ascii), --size-mib N (default 32),
/// --geometry LIST (default 120x40,300x75), --rounds N (default 6),
/// --arms LIST (default ref,A,C), --dir DIR (corpus files; default the
/// system temp dir), --out PATH (JSON lines; default stdout).
#[cfg(feature = "proto")]
fn cmd_proto_flood(args: &[String]) -> Result<(), String> {
    use ultty_bench::proto::{self, Arm};
    let flags = parse_flags(
        args,
        &[
            "corpus", "size-mib", "geometry", "rounds", "arms", "dir", "out",
        ],
    )?;
    let size_mib: usize = number(&flags, "size-mib", "32")?;
    let rounds: usize = number(&flags, "rounds", "6")?;
    let geometries = list(&flags, "geometry", "120x40,300x75")
        .into_iter()
        .map(parse_geometry)
        .collect::<Result<Vec<_>, _>>()?;
    let arms = list(&flags, "arms", "ref,A,C")
        .into_iter()
        .map(|name| Arm::from_name(name).ok_or_else(|| format!("unknown arm {name:?}")))
        .collect::<Result<Vec<_>, _>>()?;
    let dir = flags
        .get("dir")
        .and_then(|v| v.last())
        .map_or_else(std::env::temp_dir, PathBuf::from);
    let mut out: Box<dyn Write> = match flags.get("out").and_then(|v| v.last()) {
        Some(path) => Box::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(PathBuf::from(path))
                .map_err(|e| format!("open {path}: {e}"))?,
        ),
        None => Box::new(std::io::stdout().lock()),
    };
    let mut env = env::snapshot();
    let mut samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for corpus in selected_corpora(&list(&flags, "corpus", "ascii"))? {
        let generated = corpus::generate(corpus, size_mib * 1024 * 1024);
        let path = dir.join(format!("ultty-proto-{}.txt", corpus.name()));
        std::fs::write(&path, &generated.bytes)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        for round in 0..rounds {
            for &(cols, rows) in &geometries {
                let mut order = arms.clone();
                if round % 2 == 1 {
                    order.reverse();
                }
                for &arm in &order {
                    let run = proto::flood(arm, &path, cols, rows)
                        .map_err(|e| format!("flood {}: {e}", arm.name()))?;
                    let mib_s = generated.bytes.len() as f64 / (1024.0 * 1024.0) / run.secs;
                    let case = format!("{}/{cols}x{rows}", corpus.name());
                    samples
                        .entry(format!("{case} {}", arm.name()))
                        .or_default()
                        .push(mib_s);
                    let line = json!({
                        "suite": "tp.proto-flood",
                        "case": case,
                        "arm": arm.name(),
                        "round": round,
                        "value": mib_s,
                        "unit": "MiB/s",
                        "secs": run.secs,
                        "reads": run.reads,
                        "frames": run.frames,
                        "bytes": generated.bytes.len(),
                        "corpus_sha256": generated.sha256,
                        "load": env::load_average(),
                    });
                    writeln!(out, "{line}").map_err(|e| format!("write results: {e}"))?;
                }
            }
        }
        std::fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
    }
    out.flush().map_err(|e| format!("write results: {e}"))?;
    env["load_end"] = json!(env::load_average());
    eprintln!(
        "load {} -> {}, pin {}, governor {} / epp {}",
        env["load_start"], env["load_end"], env["pin"], env["governor"], env["epp"]
    );
    eprintln!(
        "{:<28} {:>9} {:>9} {:>9} {:>9} {:>8} {:>4}",
        "case (MiB/s)", "median", "min", "max", "IQR", "IQR/med", "n"
    );
    for (case, values) in &samples {
        if let Some(s) = stats::summarize(values) {
            eprintln!(
                "{case:<28} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>7.1}% {:>4}",
                s.median,
                s.min,
                s.max,
                s.q3 - s.q1,
                (s.q3 - s.q1) / s.median * 100.0,
                s.n
            );
        }
    }
    Ok(())
}
