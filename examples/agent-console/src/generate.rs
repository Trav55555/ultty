//! Deterministic generators for the fixtures in `fixtures/`.
//!
//! The committed fixtures are this module's output; `cargo run -p
//! agent-console --bin gen-fixtures` rewrites them, and a test fails if they
//! drift. Every text field is free of trailing whitespace on every line, and
//! the prose, code and tool output include lines longer than 80 columns, CJK,
//! emoji and indented code, because later slices compare what the console
//! shows against these sources.

use crate::event::{Event, EventKind, Stream, ToolStatus};

/// A fixture: its file name in `fixtures/` and the events it holds.
pub struct Fixture {
    /// The file name, such as `short.jsonl`.
    pub name: &'static str,
    /// Builds the fixture's events.
    pub events: fn() -> Vec<Event>,
}

/// Every committed fixture.
pub const FIXTURES: [Fixture; 3] = [
    Fixture {
        name: "short.jsonl",
        events: short,
    },
    Fixture {
        name: "long-tool-output.jsonl",
        events: long_tool_output,
    },
    Fixture {
        name: "two-hours.jsonl",
        events: two_hours,
    },
];

/// `events` as JSON Lines, one event per line, each ending in a newline.
pub fn to_jsonl(events: &[Event]) -> String {
    let mut out = String::new();
    for event in events {
        out.push_str(&serde_json::to_string(event).expect("events serialize"));
        out.push('\n');
    }
    out
}

/// The model name every fixture reports.
pub const MODEL: &str = "example-model-1";

const SHORT_PROMPT: &str =
    "Tabs after wide characters land one column short in the pager. Can you fix expand_tabs?";

const SHORT_OPENING: &str = "I'll start by reading how `expand_tabs` handles tab stops. The function lives in `src/parse.rs`, and the bug report says a tab after a wide character such as 漢 or 🦀 lands one column short, which points at the width calculation rather than at the tab-stop arithmetic itself.

Here is the current loop:

```rust
fn expand_tabs(line: &str) -> String {
    let stop = 4;
    let mut out = String::new();
    for ch in line.chars() {
        if ch == '\\t' {
            let pad = stop - out.len() % stop;
            out.extend(std::iter::repeat(' ').take(pad));
        } else {
            out.push(ch);
        }
    }
    out
}
```

Two problems stand out:

1. `out.len()` counts bytes, so `漢字` counts as six columns instead of four.
2. The stop is hard-coded to 4, while the configuration says 8.
   - `Config::load` reads `tab_width` but never passes it down.

I'll run the parser tests first to see what fails today.";

const SHORT_TEST_OUTPUT: [&str; 10] = [
    "   Compiling pager v0.4.1 (/work/pager)",
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.41s",
    "     Running unittests src/lib.rs (target/debug/deps/pager-5f1c2a9e0b7d4c3a)",
    "running 4 tests",
    "test parse::tests::ascii_tabs ... ok",
    "test parse::tests::tab_after_cjk_漢字 ... FAILED",
    "test parse::tests::tab_after_emoji_🦀 ... FAILED",
    "test parse::tests::configured_width ... FAILED",
    "thread 'parse::tests::tab_after_cjk_漢字' panicked at src/parse.rs:88:9: assertion `left == right` failed: left: \"漢字  x\", right: \"漢字    x\"",
    "test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s",
];

const SHORT_DIFF: &str = "--- a/src/parse.rs
+++ b/src/parse.rs
@@ -1,9 +1,12 @@
-fn expand_tabs(line: &str) -> String {
-    let stop = 4;
+use unicode_width::UnicodeWidthChar;
+
+pub fn expand_tabs(line: &str, stop: usize) -> String {
     let mut out = String::new();
+    let mut column = 0;
     for ch in line.chars() {
         if ch == '\\t' {
-            let pad = stop - out.len() % stop;
+            let pad = stop - column % stop;
             out.extend(std::iter::repeat(' ').take(pad));
+            column += pad;
         } else {
             out.push(ch);
+            column += ch.width().unwrap_or(0);
         }
";

const SHORT_MIDDLE: &str = "Three failures, all from the two problems above. I've changed `expand_tabs` to track the column with `unicode-width` and to take the stop as a parameter:";

const SHORT_CLOSING: &str = "The suite is green ✅. What changed:

- `expand_tabs` measures columns with `unicode-width`, so `日本語のテキスト` and emoji such as 🎉 advance two columns each.
- The tab stop comes from `Config::tab_width` instead of a literal.

The width fix also changes how the pager lays out long lines that mix ASCII and wide characters, so any screenshots or golden files that depend on the old column counts will need regenerating; I found none in this repository.";

/// One realistic short session: a prompt, streamed markdown prose with a
/// code block and lists, one tool call, a diff, an approval, closing prose
/// and status updates.
pub fn short() -> Vec<Event> {
    let mut s = Session::new(0x0053_484f_5254);
    s.prompt(SHORT_PROMPT);
    s.status("thinking");
    s.prose(SHORT_OPENING);
    let call = s.tool_start("bash", "cargo test -p pager parse::");
    s.tool_lines(
        &call,
        SHORT_TEST_OUTPUT.iter().map(|l| (Stream::Stdout, *l)),
        3,
        400,
    );
    s.tool_end(&call, ToolStatus::Error, Some(101), 3_900);
    s.status("editing");
    s.prose(SHORT_MIDDLE);
    s.diff("src/parse.rs", SHORT_DIFF);
    s.approval(
        "cargo test -p pager",
        &["Allow once", "Always allow `cargo test`", "Deny"],
    );
    s.idle(2_500);
    let call = s.tool_start("bash", "cargo test -p pager");
    s.tool_lines(
        &call,
        [
            (Stream::Stdout, "running 4 tests"),
            (
                Stream::Stdout,
                "test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s",
            ),
        ],
        2,
        300,
    );
    s.tool_end(&call, ToolStatus::Ok, Some(0), 2_100);
    s.prose(SHORT_CLOSING);
    s.status("idle");
    s.turn_end();
    s.events
}

/// The number of output lines in [`long_tool_output`]'s tool call.
pub const LONG_OUTPUT_LINES: usize = 20_000;

/// One tool call streaming [`LONG_OUTPUT_LINES`] lines of build and test log,
/// between a prompt and prose.
pub fn long_tool_output() -> Vec<Event> {
    let mut s = Session::new(0x4c4f_4e47);
    s.prompt("Run the whole workspace test suite and tell me what's slow.");
    s.status("thinking");
    s.prose("I'll run the full suite with timing output so the slow tests stand out:\n\n```sh\ncargo test --workspace -- -Z unstable-options --report-time\n```");
    let call = s.tool_start("bash", "cargo test --workspace -- --report-time");
    let mut rng = Rng::new(0x5445_5354);
    let lines: Vec<(Stream, String)> = (0..LONG_OUTPUT_LINES)
        .map(|i| log_line(&mut rng, i))
        .collect();
    s.tool_lines(
        &call,
        lines.iter().map(|(stream, line)| (*stream, line.as_str())),
        50,
        25,
    );
    s.tool_end(&call, ToolStatus::Ok, Some(0), 10_400);
    s.status("summarizing");
    s.prose("Every test passes. The slowest group is `layout::flex::stress_*`, at 1.8–2.4 s each; everything else finishes under 50 ms.\n\n| module | tests | slowest |\n|---|---|---|\n| layout::flex | 640 | 2.4 s |\n| grid::reflow | 1,280 | 41 ms |\n\nThe stress tests rebuild a 10,000-node tree for every case, so sharing one fixture across them would cut most of that time without changing what they check; I can make that change if you want it.");
    s.status("idle");
    s.turn_end();
    s.events
}

/// One line of a long build and test log; some go to stderr, some are
/// longer than 80 columns, some hold CJK or emoji.
fn log_line(rng: &mut Rng, i: usize) -> (Stream, String) {
    const MODULES: [&str; 8] = [
        "layout::flex",
        "grid::reflow",
        "parse::tabs",
        "render::glyphs",
        "unicode::width",
        "scroll::region",
        "protocol::codec",
        "input::keys",
    ];
    const CASES: [&str; 8] = [
        "empty_input",
        "single_row",
        "wrap_at_boundary",
        "wide_cjk_漢字かな",
        "emoji_cluster_👩‍💻",
        "nested_children",
        "resize_shrink",
        "resize_grow",
    ];
    let module = MODULES[rng.below(MODULES.len())];
    let case = CASES[rng.below(CASES.len())];
    match i % 97 {
        0 => (
            Stream::Stderr,
            format!("   Compiling {}-{} v0.{}.{} (/work/crates/{})", module.replace("::", "-"), i / 97, i % 7, i % 13, module.replace("::", "/")),
        ),
        13 => (
            Stream::Stderr,
            format!("warning: unused variable `scratch_{i}` in {module}::{case}: if this is intentional, prefix it with an underscore: `_scratch_{i}` (see `#[warn(unused_variables)]` on by default)"),
        ),
        41 => (
            Stream::Stdout,
            format!("    {module}::{case} produced {} glyph runs over {} rows; 日本語のテキストを含む行は二列で数えます ✅", rng.below(900), rng.below(120)),
        ),
        _ => (
            Stream::Stdout,
            format!("test {module}::tests::{case}_{i} ... ok <{}.{:03}s>", rng.below(3), rng.below(1000)),
        ),
    }
}

/// The number of turns in [`two_hours`].
pub const TWO_HOURS_TURNS: u32 = 200;

/// The recorded length [`two_hours`] aims for, in milliseconds.
pub const TWO_HOURS_MS: u64 = 2 * 60 * 60 * 1000;

const PROMPTS: [&str; 8] = [
    "Why does the pager redraw the whole screen when only the status line changes?",
    "Add a test for resizing while a tool card is still streaming.",
    "The diff view mangles lines with tabs; can you look?",
    "Summarize what changed in src/scroll.rs since yesterday.",
    "Rename `RegionId::raw` to `RegionId::get` everywhere.",
    "Make the status line show tokens per second, please. 速度も表示してください。",
    "Run clippy and fix whatever it reports.",
    "Is there a faster way to measure grapheme width than calling unicode-width per char? 🤔",
];

const PASSAGES: [&str; 6] = [
    "Let me look at how this path is wired. The redraw starts in `Pager::frame`, which compares the previous frame's rows with the new ones and only sends rows that changed, unless the layout generation moved, in which case it sends everything.",
    "Here's the relevant part:\n\n```rust\nimpl Pager {\n    fn frame(&mut self) -> Frame {\n        if self.layout_gen != self.sent_gen {\n            return self.full_frame();\n        }\n        self.diff_rows()\n    }\n}\n```\n\nThe status line bumps `layout_gen` on every update, which is the problem.",
    "A few things to check:\n\n- whether `layout_gen` changes on width-only updates;\n- whether the status line's height can change at all;\n  - it can, when the model name wraps, e.g. `example-model-1 · 漢字のモデル名`;\n- whether tests cover a status update without a layout change.",
    "I've made the change and the targeted tests pass. The fix keeps the status line in its own region, so updating it no longer touches the transcript's layout generation, and the transcript keeps its scroll position while the status ticks 🎯.",
    "One caveat: lines that mix ASCII and wide characters, such as `résumé 日本語 mixed`, are measured per grapheme now, which is slightly slower on very long lines but correct for combining marks and ZWJ sequences like 👨‍👩‍👧.",
    "Nothing else needed changing. If you want, I can also add a benchmark for the width path so a regression shows up in CI rather than in a bug report three weeks from now.",
];

const TOOLS: [(&str, &str); 5] = [
    ("bash", "cargo test -p pager"),
    ("bash", "rg -n layout_gen src/"),
    ("read", "src/scroll.rs"),
    ("bash", "cargo clippy --all-targets"),
    ("edit", "src/pager.rs"),
];

/// About two hours of recorded time: [`TWO_HOURS_TURNS`] turns of prose,
/// tool calls, diffs and the odd approval, from a seeded generator.
pub fn two_hours() -> Vec<Event> {
    let mut s = Session::new(0x3248_4f55_5253);
    let mut rng = Rng::new(0x7477_6f68);
    let turn_ms = TWO_HOURS_MS / u64::from(TWO_HOURS_TURNS);
    for turn in 0..u64::from(TWO_HOURS_TURNS) {
        s.idle_until(turn * turn_ms + rng.below(5_000) as u64);
        s.prompt(PROMPTS[rng.below(PROMPTS.len())]);
        s.status("thinking");
        s.prose(PASSAGES[rng.below(PASSAGES.len())]);
        if rng.below(10) < 7 {
            let (tool, title) = TOOLS[rng.below(TOOLS.len())];
            if rng.below(8) == 0 {
                s.approval(title, &["Allow once", "Deny"]);
                s.idle(1_000 + rng.below(4_000) as u64);
            }
            let call = s.tool_start(tool, title);
            let failed = rng.below(6) == 0;
            let count = 5 + rng.below(60);
            let lines: Vec<(Stream, String)> = (0..count)
                .map(|i| log_line(&mut rng, i + turn as usize * 101))
                .collect();
            s.tool_lines(
                &call,
                lines.iter().map(|(stream, line)| (*stream, line.as_str())),
                1 + rng.below(8),
                40 + rng.below(200) as u64,
            );
            let duration = 300 + rng.below(9_000) as u64;
            if failed {
                s.tool_end(&call, ToolStatus::Error, Some(1), duration);
            } else {
                s.tool_end(&call, ToolStatus::Ok, Some(0), duration);
            }
            s.status("thinking");
        }
        if rng.below(10) < 4 {
            let n = turn + 1;
            s.diff(
                &format!("src/region_{}.rs", turn % 17),
                &format!(
                    "--- a/src/region_{m}.rs\n+++ b/src/region_{m}.rs\n@@ -{n},4 +{n},5 @@\n fn layout(&mut self, width: u16) {{\n-    let cols = self.text.len();\n+    let cols = self.text.width();\n+    let rows = cols.div_ceil(usize::from(width));\n     self.rows = rows;\n     self.dirty = true;\n",
                    m = turn % 17
                ),
            );
        }
        s.prose(PASSAGES[rng.below(PASSAGES.len())]);
        s.status("idle");
        s.turn_end();
    }
    s.events
}

/// Builds a session's events, keeping the clock, ids and token counts.
struct Session {
    events: Vec<Event>,
    t: u64,
    turn: u32,
    calls: u32,
    requests: u32,
    input_tokens: u64,
    output_tokens: u64,
    rng: Rng,
}

impl Session {
    fn new(seed: u64) -> Self {
        Self {
            events: Vec::new(),
            t: 0,
            turn: 0,
            calls: 0,
            requests: 0,
            input_tokens: 0,
            output_tokens: 0,
            rng: Rng::new(seed),
        }
    }

    fn push(&mut self, kind: EventKind) {
        self.events.push(Event::new(self.t, kind));
    }

    fn idle(&mut self, ms: u64) {
        self.t += ms;
    }

    fn idle_until(&mut self, t: u64) {
        self.t = self.t.max(t);
    }

    fn prompt(&mut self, text: &str) {
        self.turn += 1;
        self.input_tokens += 900 + text.len() as u64 / 4;
        self.push(EventKind::Prompt { text: text.into() });
        self.idle(200);
    }

    fn status(&mut self, state: &str) {
        self.push(EventKind::Status {
            model: MODEL.into(),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            state: state.into(),
            elapsed_ms: self.t,
        });
        self.idle(50);
    }

    /// Streams `markdown` as text deltas of a few words each, split only
    /// before a space that follows a non-space, so no delta ends in
    /// whitespace.
    fn prose(&mut self, markdown: &str) {
        let bytes = markdown.as_bytes();
        let splits: Vec<usize> = (1..bytes.len())
            .filter(|&i| bytes[i] == b' ' && !bytes[i - 1].is_ascii_whitespace())
            .collect();
        let mut start = 0;
        let mut next = 0;
        while start < markdown.len() {
            next += 2 + self.rng.below(5);
            let end = splits.get(next).copied().unwrap_or(markdown.len());
            let text = &markdown[start..end];
            self.output_tokens += 1 + text.len() as u64 / 4;
            self.push(EventKind::TextDelta {
                turn: self.turn,
                text: text.into(),
            });
            let pause = 30 + self.rng.below(90) as u64;
            self.idle(pause);
            start = end;
        }
    }

    fn turn_end(&mut self) {
        self.push(EventKind::TurnEnd { turn: self.turn });
        self.idle(100);
    }

    fn tool_start(&mut self, tool: &str, title: &str) -> String {
        self.calls += 1;
        let call = format!("call-{}", self.calls);
        self.push(EventKind::ToolStart {
            call: call.clone(),
            tool: tool.into(),
            title: title.into(),
        });
        self.idle(100);
        call
    }

    /// Streams `lines`, each ending in a newline, in chunks of at most
    /// `per_chunk` lines that never mix streams, `gap_ms` apart.
    fn tool_lines<'a>(
        &mut self,
        call: &str,
        lines: impl IntoIterator<Item = (Stream, &'a str)>,
        per_chunk: usize,
        gap_ms: u64,
    ) {
        let mut chunk: Option<(Stream, String, usize)> = None;
        for (stream, line) in lines {
            match &mut chunk {
                Some((s, text, n)) if *s == stream && *n < per_chunk => {
                    text.push_str(line);
                    text.push('\n');
                    *n += 1;
                }
                _ => {
                    if let Some(done) = chunk.take() {
                        self.tool_chunk(call, done.0, done.1, gap_ms);
                    }
                    chunk = Some((stream, format!("{line}\n"), 1));
                }
            }
        }
        if let Some(done) = chunk {
            self.tool_chunk(call, done.0, done.1, gap_ms);
        }
    }

    fn tool_chunk(&mut self, call: &str, stream: Stream, text: String, gap_ms: u64) {
        self.push(EventKind::ToolOutput {
            call: call.into(),
            stream,
            text,
        });
        self.idle(gap_ms);
    }

    fn tool_end(&mut self, call: &str, status: ToolStatus, exit_code: Option<i32>, ms: u64) {
        self.push(EventKind::ToolEnd {
            call: call.into(),
            status,
            exit_code,
            duration_ms: ms,
        });
        self.idle(150);
    }

    fn diff(&mut self, path: &str, diff: &str) {
        self.push(EventKind::Diff {
            path: path.into(),
            diff: diff.into(),
        });
        self.idle(300);
    }

    fn approval(&mut self, summary: &str, choices: &[&str]) {
        self.requests += 1;
        self.push(EventKind::Approval {
            request: format!("approval-{}", self.requests),
            summary: summary.into(),
            choices: choices.iter().map(|c| (*c).into()).collect(),
        });
        self.idle(100);
    }
}

/// SplitMix64: a small seeded generator, so fixtures are reproducible
/// without a dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}
