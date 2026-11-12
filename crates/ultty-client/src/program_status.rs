//! OSC 7501 revision 0.4 reporting without reading terminal input.
//!
//! Reports are optional terminal hints. A successful write does not mean
//! the terminal supports them or that the reported operation succeeded.

/// State to report to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Waiting for the next instruction.
    Idle,
    /// Running, optionally with a percentage.
    Working,
    /// Completed, with a result ready to inspect.
    Done,
    /// Waiting for user input or approval.
    Blocked,
    /// Failed and stopped.
    Error,
    /// Remove the addressed record and all its descendants.
    Clear,
}

/// What a blocked application says it needs. This never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Approval for an operation.
    Permission,
    /// An answer to a question.
    Question,
    /// Authentication or credentials.
    Auth,
}

/// A report validated in full before any bytes are written.
///
/// ## Example
///
/// ```
/// use ultty_client::program_status::{Report, State};
/// let report = Report { msg: Some("Building".into()), ..Report::new(State::Working) };
/// assert!(report.encode().is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Required state; `Clear` removes a subtree instead of storing state.
    pub state: State,
    /// Hierarchical ID; `None` addresses the root.
    pub id: Option<String>,
    /// Reason for a blocked report; ignored for other states.
    pub kind: Option<Kind>,
    /// Percentage, 0 through 100, for working or blocked reports.
    pub progress: Option<u8>,
    /// Stable ASCII application ID, at most 32 bytes.
    pub app: Option<String>,
    /// Plain single-line label, at most 192 UTF-8 bytes.
    pub title: Option<String>,
    /// Plain single-line explanation, at most 2048 UTF-8 bytes.
    pub msg: Option<String>,
}

/// Invalid report data; no part of such a report is written.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StatusError {
    /// A hierarchical ID violates length, segment or depth limits.
    #[error("invalid program status id")]
    InvalidId,
    /// An application ID is not a 1-32 byte ASCII identifier.
    #[error("invalid program status app")]
    InvalidApp,
    /// Percentage exceeds 100.
    #[error("program status progress exceeds 100")]
    InvalidProgress,
    /// A text value contains a control character or exceeds its byte cap.
    #[error("invalid program status {0}")]
    InvalidText(&'static str),
    /// Complete encoded report exceeds the wire cap.
    #[error("program status sequence exceeds 4096 bytes")]
    TooLong,
}

impl Report {
    /// Start a root report with the supplied state and no optional fields.
    pub fn new(state: State) -> Self {
        Self {
            state,
            id: None,
            kind: None,
            progress: None,
            app: None,
            title: None,
            msg: None,
        }
    }

    /// Remove a record and its subtree; `None` removes all records.
    pub fn clear(id: Option<String>) -> Self {
        Self {
            id,
            ..Self::new(State::Clear)
        }
    }

    /// Encode one complete ST-terminated report after bounded validation.
    pub fn encode(&self) -> Result<Vec<u8>, StatusError> {
        if self.id.as_ref().is_some_and(|id| {
            id.len() > 128 || id.split('/').count() > 8 || !id.split('/').all(valid_segment)
        }) {
            return Err(StatusError::InvalidId);
        }
        if self.app.as_ref().is_some_and(|app| !valid_segment(app)) {
            return Err(StatusError::InvalidApp);
        }
        if self.progress.is_some_and(|progress| progress > 100) {
            return Err(StatusError::InvalidProgress);
        }
        for (name, value, limit) in [("title", &self.title, 192), ("msg", &self.msg, 2048)] {
            if value
                .as_ref()
                .is_some_and(|value| value.len() > limit || value.chars().any(char::is_control))
            {
                return Err(StatusError::InvalidText(name));
            }
        }
        let state = match self.state {
            State::Idle => "idle",
            State::Working => "working",
            State::Done => "done",
            State::Blocked => "blocked",
            State::Error => "error",
            State::Clear => "clear",
        };
        let mut wire = format!("\x1b]7501;state={state}");
        if let Some(id) = &self.id {
            wire.push_str(&format!(":id={id}"));
        }
        if self.state == State::Blocked {
            if let Some(kind) = self.kind {
                let kind = match kind {
                    Kind::Permission => "permission",
                    Kind::Question => "question",
                    Kind::Auth => "auth",
                };
                wire.push_str(&format!(":kind={kind}"));
            }
        }
        if matches!(self.state, State::Working | State::Blocked) {
            if let Some(progress) = self.progress {
                wire.push_str(&format!(":progress={progress}"));
            }
        }
        if let Some(app) = &self.app {
            wire.push_str(&format!(":app={app}"));
        }
        if let Some(title) = &self.title {
            wire.push_str(&format!(":title={}", encode_text(title.as_bytes())));
        }
        if let Some(msg) = &self.msg {
            wire.push_str(&format!(":msg={}", encode_text(msg.as_bytes())));
        }
        wire.push_str("\x1b\\");
        if wire.len() > 4096 {
            return Err(StatusError::TooLong);
        }
        Ok(wire.into_bytes())
    }
}

fn valid_segment(value: &str) -> bool {
    (1..=32).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
}

fn encode_text(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        encoded.push(ALPHABET[(a >> 2) as usize] as char);
        encoded.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_encoder_vectors_and_clear() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("✓", "4pyT"),
        ] {
            assert_eq!(encode_text(plain.as_bytes()), encoded);
        }
        let report = Report {
            id: Some("build/test".into()),
            kind: Some(Kind::Permission),
            progress: Some(50),
            app: Some("cargo".into()),
            msg: Some("Continue?".into()),
            ..Report::new(State::Blocked)
        };
        assert_eq!(report.encode().unwrap(), b"\x1b]7501;state=blocked:id=build/test:kind=permission:progress=50:app=cargo:msg=Q29udGludWU/\x1b\\");
        assert_eq!(
            Report::clear(None).encode().unwrap(),
            b"\x1b]7501;state=clear\x1b\\"
        );
        assert_eq!(
            Report::clear(Some("build".into())).encode().unwrap(),
            b"\x1b]7501;state=clear:id=build\x1b\\"
        );
    }

    #[test]
    fn test_status_encoder_bounds_and_control_rejection() {
        let mut report = Report {
            title: Some("a".repeat(192)),
            msg: Some("a".repeat(2048)),
            ..Report::new(State::Working)
        };
        assert!(report.encode().is_ok());
        report.msg = Some("a".repeat(2049));
        assert_eq!(report.encode(), Err(StatusError::InvalidText("msg")));
        report.msg = None;
        report.title = Some("a".repeat(193));
        assert_eq!(report.encode(), Err(StatusError::InvalidText("title")));
        for text in ["a\nb", "\t", "\0", "\u{7f}", "\u{80}", "\u{9f}"] {
            report.title = Some(text.into());
            assert_eq!(report.encode(), Err(StatusError::InvalidText("title")));
        }
        report.title = None;
        for id in ["", "/root", "a//b", "a/b/c/d/e/f/g/h/i", "bad:id"] {
            report.id = Some(id.into());
            assert_eq!(report.encode(), Err(StatusError::InvalidId));
        }
    }
}
