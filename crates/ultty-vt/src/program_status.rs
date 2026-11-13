//! Bounded OSC 7501 program records, independent of screen and presentation.
//!
//! Implements revision 0.4, checked 2026-11-12 at
//! <https://www.superlogical.com/rex/docs/build/program-status>.

use serde::{Deserialize, Serialize};

/// Maximum complete sequence length, including OSC and its terminator.
pub const MAX_SEQUENCE_BYTES: usize = 4096;
/// Maximum retained records in one terminal.
pub const MAX_RECORDS: usize = 256;
/// Static feature reply; never includes application-controlled text.
pub const FEATURE_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

/// A program's reported state. Reports do not grant authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramState {
    /// Waiting for the next instruction.
    Idle,
    /// Running; cleared at prompt or process exit.
    Working,
    /// Completed; survives prompt and process exit.
    Done,
    /// Waiting for user input; cleared at prompt or process exit.
    Blocked,
    /// Failed; survives prompt and process exit.
    Error,
}

/// What a blocked program says it needs, without authorizing any action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockedKind {
    /// Approval for an operation.
    Permission,
    /// An answer to a question.
    Question,
    /// Authentication or credentials.
    Auth,
}

/// One retained report. An empty ID identifies the root record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramStatusRecord {
    /// Hierarchical ID, or empty for the root.
    pub id: String,
    /// Application-reported state.
    pub state: ProgramState,
    /// Why a blocked program needs input.
    pub kind: Option<BlockedKind>,
    /// Percentage for working or blocked records; absent is indeterminate.
    pub progress: Option<u8>,
    /// Explicit application ID; inheritance is resolved by the store.
    pub app: Option<String>,
    /// Single-line decoded label.
    pub title: Option<String>,
    /// Single-line decoded explanation, treated as plain text.
    pub msg: Option<String>,
    /// Monotonic update order within the store, used for eviction.
    pub updated: u64,
}

/// A validated mutation emitted by the VT parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgramStatusReport {
    /// Completely replace one record.
    Set(ProgramStatusRecord),
    /// Remove the ID and descendants; empty means all records.
    Clear(String),
}

/// The lifecycle operation responsible for the latest generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusChangeReason {
    /// A report replaced a record, possibly evicting the oldest.
    #[default]
    Report,
    /// A report cleared a subtree.
    Clear,
    /// A shell prompt removed transient records.
    Prompt,
    /// The attached process exited after final output was applied.
    ProcessExit,
    /// RIS cleared records.
    Reset,
}

/// Authoritative current records. Notifications may be coalesced or lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramStatusSnapshot {
    /// Changes whenever records or their update order change.
    pub generation: u64,
    /// Cause of the most recent change, rather than a complete delta log.
    pub reason: StatusChangeReason,
    /// Records in least-to-most recently updated order.
    pub records: Vec<ProgramStatusRecord>,
}

/// Per-terminal bounded records with prompt/reset/exit lifecycle.
#[derive(Debug, Default)]
pub struct ProgramStatusStore {
    records: Vec<ProgramStatusRecord>,
    generation: u64,
    reason: StatusChangeReason,
}

impl ProgramStatusStore {
    /// Current mutation generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Cause of the most recent mutation.
    pub fn reason(&self) -> StatusChangeReason {
        self.reason
    }

    /// Records in least-to-most recently updated order.
    pub fn records(&self) -> &[ProgramStatusRecord] {
        &self.records
    }

    /// Clone the bounded current state for a consumer resynchronization.
    pub fn snapshot(&self) -> ProgramStatusSnapshot {
        ProgramStatusSnapshot {
            generation: self.generation,
            reason: self.reason,
            records: self.records.clone(),
        }
    }

    /// Apply one validated report. Returns whether the generation changed.
    pub fn apply(&mut self, report: ProgramStatusReport) -> bool {
        match report {
            ProgramStatusReport::Clear(id) => {
                if !id.is_empty() && !valid_id(id.as_bytes()) {
                    return false;
                }
                self.remove_where(StatusChangeReason::Clear, |record| {
                    id.is_empty()
                        || record.id == id
                        || record
                            .id
                            .strip_prefix(&id)
                            .is_some_and(|tail| tail.starts_with('/'))
                })
            }
            ProgramStatusReport::Set(mut record) => {
                // Structured embedders can call apply without the OSC parser.
                // Keep the same storage bounds at this public boundary.
                if (!record.id.is_empty() && !valid_id(record.id.as_bytes()))
                    || record
                        .app
                        .as_ref()
                        .is_some_and(|app| !valid_segment(app.as_bytes()))
                    || record.progress.is_some_and(|progress| progress > 100)
                    || [&record.title, &record.msg]
                        .into_iter()
                        .zip([192, 2048])
                        .any(|(text, limit)| {
                            text.as_ref().is_some_and(|text| {
                                text.len() > limit || text.chars().any(char::is_control)
                            })
                        })
                {
                    return false;
                }
                if record.state != ProgramState::Blocked {
                    record.kind = None;
                }
                if !matches!(record.state, ProgramState::Working | ProgramState::Blocked) {
                    record.progress = None;
                }
                if let Some(index) = self.records.iter().position(|old| old.id == record.id) {
                    self.records.remove(index);
                } else if self.records.len() == MAX_RECORDS {
                    self.records.remove(0);
                }
                self.changed(StatusChangeReason::Report);
                record.updated = self.generation;
                self.records.push(record);
                true
            }
        }
    }

    /// Remove working/blocked records when a new shell prompt begins.
    pub fn on_prompt(&mut self) -> bool {
        self.remove_transient(StatusChangeReason::Prompt)
    }

    /// Remove working/blocked records after the attached process's output.
    pub fn on_process_exit(&mut self) -> bool {
        self.remove_transient(StatusChangeReason::ProcessExit)
    }

    /// A full reset clears every record; a soft reset must not call this.
    pub fn on_ris(&mut self) -> bool {
        self.remove_where(StatusChangeReason::Reset, |_| true)
    }

    /// Explicit app ID or the nearest existing ancestor's app ID.
    pub fn effective_app(&self, id: &str) -> Option<&str> {
        let mut ancestor = id;
        loop {
            if let Some(app) = self.records.iter().find_map(|record| {
                (record.id == ancestor)
                    .then_some(record.app.as_deref())
                    .flatten()
            }) {
                return Some(app);
            }
            if ancestor.is_empty() {
                return None;
            }
            ancestor = ancestor.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
    }

    fn changed(&mut self, reason: StatusChangeReason) {
        self.generation = self.generation.wrapping_add(1);
        self.reason = reason;
    }

    fn remove_transient(&mut self, reason: StatusChangeReason) -> bool {
        self.remove_where(reason, |record| {
            matches!(record.state, ProgramState::Working | ProgramState::Blocked)
        })
    }

    fn remove_where(
        &mut self,
        reason: StatusChangeReason,
        mut remove: impl FnMut(&ProgramStatusRecord) -> bool,
    ) -> bool {
        let before = self.records.len();
        self.records.retain(|record| !remove(record));
        let changed = self.records.len() != before;
        if changed {
            self.changed(reason);
        }
        changed
    }
}

/// Parsed OSC body, before applying it to a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParsedStatus {
    Query,
    Report(ProgramStatusReport),
}

/// Parse the bytes following `7501;`, checking every pair before mutation.
pub(crate) fn parse(body: &[u8]) -> Option<ParsedStatus> {
    if body.trim_ascii() == b"?" {
        return Some(ParsedStatus::Query);
    }
    let mut state = None;
    let mut id = None;
    let mut kind = None;
    let mut progress = None;
    let mut app = None;
    let mut title = None;
    let mut msg = None;
    for pair in body.split(|byte| *byte == b':') {
        let Some(equal) = pair.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = pair[..equal].trim_ascii();
        let value = pair[equal + 1..].trim_ascii();
        if key.len() > 16 {
            return None;
        }
        // Check sizes even when a later duplicate would replace this pair.
        match key {
            b"msg" if value.len() > 2732 => return None,
            b"title" if value.len() > 256 => return None,
            b"app" if value.len() > 32 => return None,
            b"id" if value.len() > 128 => return None,
            _ => {}
        }
        if key == b"id" && !valid_id(value) {
            return None;
        }
        if key.is_empty()
            || !key.iter().all(u8::is_ascii_lowercase)
            || !value
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.,+/=-".contains(byte))
        {
            continue;
        }
        match key {
            b"state" => state = Some(value),
            b"id" => {
                if !valid_id(value) {
                    return None;
                }
                id = Some(value);
            }
            b"kind" => kind = Some(value),
            b"progress" => progress = Some(value),
            b"app" => app = Some(value),
            b"title" => title = Some(decode_text(value, 192)?),
            b"msg" => msg = Some(decode_text(value, 2048)?),
            _ => {}
        }
    }
    let id = String::from_utf8(id.unwrap_or_default().to_vec()).ok()?;
    let state = match state? {
        b"clear" => return Some(ParsedStatus::Report(ProgramStatusReport::Clear(id))),
        b"idle" => ProgramState::Idle,
        b"working" => ProgramState::Working,
        b"done" => ProgramState::Done,
        b"blocked" => ProgramState::Blocked,
        b"error" => ProgramState::Error,
        _ => return None,
    };
    let kind = if state == ProgramState::Blocked {
        match kind {
            Some(b"permission") => Some(BlockedKind::Permission),
            Some(b"question") => Some(BlockedKind::Question),
            Some(b"auth") => Some(BlockedKind::Auth),
            _ => None,
        }
    } else {
        None
    };
    let progress = if matches!(state, ProgramState::Working | ProgramState::Blocked) {
        progress.and_then(|value| {
            if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return None;
            }
            let percentage = std::str::from_utf8(value).ok()?.parse::<u8>().ok()?;
            (percentage <= 100).then_some(percentage)
        })
    } else {
        None
    };
    let app = app.filter(|value| valid_segment(value)).map(|value| {
        // The segment grammar is ASCII.
        String::from_utf8_lossy(value).into_owned()
    });
    Some(ParsedStatus::Report(ProgramStatusReport::Set(
        ProgramStatusRecord {
            id,
            state,
            kind,
            progress,
            app,
            title,
            msg,
            updated: 0,
        },
    )))
}

fn valid_segment(value: &[u8]) -> bool {
    (1..=32).contains(&value.len())
        && value
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(byte))
}

fn valid_id(value: &[u8]) -> bool {
    value.len() <= 128
        && value.split(|byte| *byte == b'/').count() <= 8
        && value.split(|byte| *byte == b'/').all(valid_segment)
}

fn decode_text(encoded: &[u8], limit: usize) -> Option<String> {
    // Standard base64, optional complete padding, canonical unused bits.
    // No filtering: bad alphabet/padding discards the entire report.
    let unpadded = encoded
        .iter()
        .position(|byte| *byte == b'=')
        .unwrap_or(encoded.len());
    let padding = encoded.len() - unpadded;
    let tail = unpadded % 4;
    if tail == 1
        || padding > 2
        || !encoded[unpadded..].iter().all(|byte| *byte == b'=')
        || (padding != 0 && (!encoded.len().is_multiple_of(4) || padding != (4 - tail) % 4))
    {
        return None;
    }
    let mut decoded = Vec::with_capacity(unpadded * 3 / 4);
    let mut bits = 0u32;
    let mut count = 0;
    for byte in &encoded[..unpadded] {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            decoded.push((bits >> count) as u8);
        }
    }
    if (count > 0 && bits & ((1 << count) - 1) != 0) || decoded.len() > limit {
        return None;
    }
    let text = String::from_utf8(decoded).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::{Action, AnsiOutput, AnsiParser};
    use crate::grid::{Dimensions, Terminal, TerminalHandler};

    fn mutation(body: &str) -> ProgramStatusReport {
        match parse(body.as_bytes()).unwrap() {
            ParsedStatus::Report(report) => report,
            ParsedStatus::Query => panic!("expected report"),
        }
    }

    fn feed(terminal: &mut Terminal, bytes: &[u8]) {
        let mut parser = AnsiParser::new();
        let mut handler = TerminalHandler::new(terminal);
        parser.feed(bytes, |action| handler.handle(action));
    }

    #[test]
    fn test_status_replacement_clear_and_inheritance() {
        let mut store = ProgramStatusStore::default();
        store.apply(mutation("state=idle:app=cargo:msg=cm9vdA=="));
        store.apply(mutation("state=working:id=build/test:progress=25"));
        store.apply(mutation("state=blocked:id=build:app=rustc:kind=permission"));
        store.apply(mutation("state=error:id=builder"));
        assert_eq!(store.effective_app("build/test"), Some("rustc"));
        store.apply(mutation("state=done:id=build"));
        assert_eq!(store.effective_app("build/test"), Some("cargo"));
        assert!(store
            .records()
            .iter()
            .find(|record| record.id == "build")
            .unwrap()
            .app
            .is_none());
        store.apply(mutation("state=clear:id=build"));
        assert_eq!(store.records().len(), 2);
        assert!(store.records().iter().any(|record| record.id == "builder"));
        let generation = store.generation();
        assert!(!store.apply(mutation("state=clear:id=build")));
        assert_eq!(store.generation(), generation);
        store.apply(mutation("state=clear"));
        assert!(store.records().is_empty());
        assert_eq!(store.reason(), StatusChangeReason::Clear);
    }

    #[test]
    fn test_status_capacity_evicts_least_recently_updated() {
        let mut store = ProgramStatusStore::default();
        for index in 0..MAX_RECORDS {
            store.apply(mutation(&format!("state=working:id=job{index}")));
        }
        store.apply(mutation("state=done:id=job0"));
        store.apply(mutation("state=error:id=new"));
        assert_eq!(store.records().len(), MAX_RECORDS);
        assert!(!store.records().iter().any(|record| record.id == "job1"));
        assert!(store.records().iter().any(|record| record.id == "job0"));
        assert_eq!(store.records().last().unwrap().updated, store.generation());
    }

    #[test]
    fn test_status_lifecycle_is_terminal_owned() {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        for (index, state) in ["idle", "working", "blocked", "done", "error"]
            .iter()
            .enumerate()
        {
            feed(
                &mut terminal,
                format!("\x1b]7501;state={state}:id=s{index}\x07").as_bytes(),
            );
        }
        feed(&mut terminal, b"\x1b[?1049h\x1b[!p\x1b[?1049l");
        assert_eq!(terminal.program_status().records().len(), 5);
        feed(&mut terminal, b"\x1b]133;A\x07");
        assert_eq!(terminal.program_status().records().len(), 3);
        assert_eq!(
            terminal.program_status().reason(),
            StatusChangeReason::Prompt
        );
        feed(&mut terminal, b"\x1b]7501;state=blocked:id=again\x07");
        terminal.process_exited();
        assert_eq!(terminal.program_status().records().len(), 3);
        assert_eq!(
            terminal.program_status().reason(),
            StatusChangeReason::ProcessExit
        );
        feed(&mut terminal, b"\x1bc");
        assert!(terminal.program_status().records().is_empty());
        assert_eq!(
            terminal.program_status().reason(),
            StatusChangeReason::Reset
        );
    }

    #[test]
    fn test_status_validates_every_duplicate_before_mutation() {
        for invalid in [
            "state=done:msg=Zh==:msg=b2s=",
            "state=done:title=/w==:title=b2s=",
            "state=done:msg=Cg==:msg=b2s=",
            "state=done:msg=woU=:msg=b2s=",
            "state=done:id=/bad:id=good",
        ] {
            assert_eq!(parse(invalid.as_bytes()), None, "{invalid}");
        }
        for (key, length) in [("msg", 2733), ("title", 257), ("app", 33), ("id", 129)] {
            assert_eq!(
                parse(format!("state=done:{key}={}:{}=ok", "A".repeat(length), key).as_bytes()),
                None
            );
        }
        assert_eq!(parse(b"state=done:abcdefghijklmnopq=a"), None);
    }

    #[test]
    fn test_status_parser_ignores_unknown_and_malformed_pairs() {
        let report = mutation("bad: =no:unknown=ok:x=bad!:state=working:progress=101:app=bad/name:state=blocked:kind=new:msg=b2s");
        let ProgramStatusReport::Set(record) = report else {
            panic!("set")
        };
        assert_eq!(record.state, ProgramState::Blocked);
        assert_eq!(record.msg.as_deref(), Some("ok"));
        assert_eq!(
            (record.kind, record.progress, record.app),
            (None, None, None)
        );
        for invalid in [
            "id=x",
            "state=unknown",
            "state=done:id=",
            "state=done:id=a//b",
            "state=done:id=a/b/c/d/e/f/g/h/i",
        ] {
            assert!(parse(invalid.as_bytes()).is_none(), "{invalid}");
        }
    }

    #[test]
    fn test_status_strict_base64_text_vectors() {
        for (wire, expected) in [
            ("", ""),
            ("Zg", "f"),
            ("Zg==", "f"),
            ("Zm8", "fo"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("4pyT", "✓"),
        ] {
            assert_eq!(
                decode_text(wire.as_bytes(), 2048).as_deref(),
                Some(expected)
            );
        }
        for invalid in [
            "A", "Zg=", "Zg===", "Zm8==", "Zg==AA", "Zh==", "Zm9=", "_w==", "Z g==", "/w==",
            "AA==", "Hw==", "fw==", "woA=", "wp8=",
        ] {
            assert_eq!(decode_text(invalid.as_bytes(), 2048), None, "{invalid}");
        }
        assert_eq!(decode_text(b"Zm9v", 2), None);
    }

    #[test]
    fn test_status_decoded_text_caps_and_structured_store_boundary() {
        let msg = format!("{}YWE=", "YWFh".repeat(682));
        let report = mutation(&format!("state=done:msg={msg}:title={}", "YWFh".repeat(64)));
        let ProgramStatusReport::Set(mut record) = report else {
            panic!("set")
        };
        assert_eq!(record.msg.as_ref().unwrap().len(), 2048);
        assert_eq!(record.title.as_ref().unwrap().len(), 192);
        assert!(parse(format!("state=done:msg={}", "YWFh".repeat(683)).as_bytes()).is_none());
        let mut store = ProgramStatusStore::default();
        record.msg = Some("a".repeat(2049));
        assert!(!store.apply(ProgramStatusReport::Set(record)));
        assert_eq!(store.generation(), 0);
        assert!(store.records().is_empty());
    }

    #[test]
    fn test_status_parser_every_split_and_feed_path() {
        let wire =
            b"\x1b]7501;state=blocked:id=build/test:kind=question:progress=50:msg=SGVsbG8=\x1b\\";
        let mut expected = Vec::new();
        AnsiParser::new().feed(wire, |action| expected.push(action));
        assert_eq!(expected.len(), 1);
        for boundary in 0..=wire.len() {
            for spans in [false, true] {
                let mut parser = AnsiParser::new();
                let mut actions = Vec::new();
                for chunk in [&wire[..boundary], &wire[boundary..]] {
                    if spans {
                        parser.feed_with_ascii_spans(chunk, |output| match output {
                            AnsiOutput::Action(action) => actions.push(action),
                            AnsiOutput::PrintAscii(_) => panic!("status printed"),
                        });
                    } else {
                        parser.feed(chunk, |action| actions.push(action));
                    }
                }
                assert_eq!(actions, expected, "boundary {boundary}, spans {spans}");
            }
        }
    }

    #[test]
    fn test_status_wire_budget_and_oversize_recovery() {
        for terminator in [b"\x07".as_slice(), b"\x1b\\".as_slice()] {
            for extra in [0, 1, 70000] {
                let mut wire = b"\x1b]7501;state=done:x=".to_vec();
                wire.resize(MAX_SEQUENCE_BYTES - terminator.len() + extra, b'a');
                wire.extend_from_slice(terminator);
                wire.extend_from_slice(b"X");
                let mut actions = Vec::new();
                AnsiParser::new().feed(&wire, |action| actions.push(action));
                if extra == 0 {
                    assert!(matches!(
                        actions.as_slice(),
                        [Action::ProgramStatus(_), Action::Print('X')]
                    ));
                } else {
                    assert_eq!(actions, [Action::Print('X')]);
                }
            }
        }
    }

    #[test]
    fn test_status_query_is_static_and_ordered_before_da() {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        feed(&mut terminal, b"\x1b]7501;?\x07\x1b[c");
        let replies = terminal.take_responses();
        assert_eq!(replies.first().map(Vec::as_slice), Some(FEATURE_REPLY));
        assert!(replies.len() > 1);
        assert!(terminal.program_status().records().is_empty());
    }
}
