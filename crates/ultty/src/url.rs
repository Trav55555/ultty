//! URL detection and click-to-open functionality.
//!
//! This module provides URL detection in terminal output and the ability
//! to open URLs in the default browser.

use regex::Regex;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

/// URL regex pattern.
///
/// Matches HTTP and HTTPS URLs, being careful to avoid trailing punctuation.
static URL_REGEX: OnceLock<Regex> = OnceLock::new();

fn url_regex() -> &'static Regex {
    URL_REGEX.get_or_init(|| {
        Regex::new(r#"https?://[^\s<>"'`\{\}\|\\\^\[\]]+[^\s<>"'`\{\}\|\\\^\[\]\.,;:!?\)\]]"#)
            .expect("Invalid URL regex")
    })
}

/// Find an HTTP(S) URL containing the given UTF-8 byte position in text.
pub fn url_at_text(text: &str, byte_offset: usize) -> Option<String> {
    url_regex()
        .find_iter(text)
        .find(|item| {
            item.start() <= byte_offset
                && byte_offset < item.end()
                && text[item.end()..]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_whitespace() || ch == ')')
        })
        .map(|item| item.as_str().to_owned())
}

/// A detected URL with its position in the terminal grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedUrl {
    /// The URL text
    pub url: String,
    /// Start column (0-indexed)
    pub start_col: u16,
    /// End column (exclusive, 0-indexed)
    pub end_col: u16,
    /// Row (0-indexed)
    pub row: u16,
}

impl DetectedUrl {
    /// Check if a position is within this URL.
    pub fn contains(&self, col: u16, row: u16) -> bool {
        row == self.row && col >= self.start_col && col < self.end_col
    }
}

/// URL detector for terminal content.
#[derive(Debug, Default)]
pub struct UrlDetector {
    /// Cached detected URLs
    urls: Vec<DetectedUrl>,
    /// Whether detection is enabled
    enabled: bool,
    /// Per-row content hashes for dirty tracking (row index -> hash)
    row_hashes: Vec<u64>,
}

impl UrlDetector {
    /// Create a new URL detector.
    pub fn new(enabled: bool) -> Self {
        Self {
            urls: Vec::new(),
            enabled,
            row_hashes: Vec::new(),
        }
    }

    /// Check if URL detection is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enable or disable URL detection.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.urls.clear();
            self.row_hashes.clear();
        }
    }

    /// Detect URLs in the visible terminal grid.
    ///
    /// Uses per-row content hashing to skip unchanged rows. Only rows whose
    /// content hash differs from the previous call are re-scanned.
    pub fn detect<F>(&mut self, rows: u16, get_row_text: F)
    where
        F: Fn(u16) -> String,
    {
        if !self.enabled {
            self.urls.clear();
            self.row_hashes.clear();
            return;
        }

        let row_count = rows as usize;

        // Resize hash cache if row count changed — full rescan needed
        if self.row_hashes.len() != row_count {
            self.row_hashes.resize(row_count, 0);
            // Force full rescan by zeroing all hashes
            self.row_hashes.fill(0);
            self.urls.clear();
        }

        for row_idx in 0..rows {
            let text = get_row_text(row_idx);
            let hash = Self::hash_str(&text);

            if hash == self.row_hashes[row_idx as usize] && self.row_hashes[row_idx as usize] != 0 {
                // Row unchanged, keep existing URLs for this row
                continue;
            }

            // Row changed: remove old URLs for this row and re-detect
            self.row_hashes[row_idx as usize] = hash;
            self.urls.retain(|u| u.row != row_idx);
            self.detect_in_line(&text, row_idx);
        }
    }

    /// Hash a string for dirty-row comparison.
    fn hash_str(s: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }

    /// Detect URLs in a single line.
    fn detect_in_line(&mut self, text: &str, row: u16) {
        for mat in url_regex().find_iter(text) {
            // Convert byte offsets to character offsets for column positions
            let start_col = text[..mat.start()].chars().count() as u16;
            let end_col = text[..mat.end()].chars().count() as u16;

            self.urls.push(DetectedUrl {
                url: mat.as_str().to_string(),
                start_col,
                end_col,
                row,
            });
        }
    }

    /// Get all detected URLs.
    pub fn urls(&self) -> &[DetectedUrl] {
        &self.urls
    }

    /// Detect OSC 8 hyperlinks from terminal cells.
    ///
    /// This scans visible cells for hyperlink IDs and adds them to the detected URLs.
    /// The `get_hyperlink` function maps cell hyperlink IDs to their URIs.
    pub fn detect_hyperlinks<F, G>(
        &mut self,
        rows: u16,
        cols: u16,
        get_cell_hyperlink: F,
        get_hyperlink_uri: G,
    ) where
        F: Fn(u16, u16) -> u32,       // (row, col) -> hyperlink_id
        G: Fn(u32) -> Option<String>, // hyperlink_id -> uri
    {
        if !self.enabled {
            return;
        }

        for row_idx in 0..rows {
            let mut col = 0u16;
            while col < cols {
                let hyperlink_id = get_cell_hyperlink(row_idx, col);
                if hyperlink_id != 0 {
                    // Found a hyperlink, scan for its extent
                    let start_col = col;
                    while col < cols && get_cell_hyperlink(row_idx, col) == hyperlink_id {
                        col += 1;
                    }
                    let end_col = col;

                    // Get the URI for this hyperlink
                    if let Some(uri) = get_hyperlink_uri(hyperlink_id) {
                        self.urls.push(DetectedUrl {
                            url: uri,
                            start_col,
                            end_col,
                            row: row_idx,
                        });
                    }
                } else {
                    col += 1;
                }
            }
        }
    }

    /// Find a URL at the given position.
    pub fn url_at(&self, col: u16, row: u16) -> Option<&DetectedUrl> {
        self.urls.iter().find(|u| u.contains(col, row))
    }

    /// Check if there's a URL at the given position.
    pub fn has_url_at(&self, col: u16, row: u16) -> bool {
        self.url_at(col, row).is_some()
    }

    /// Open a URL with the desktop's handler for its scheme.
    ///
    /// Only the schemes in [`OPENABLE_SCHEMES`] are opened. Links come from
    /// terminal output (OSC 8 lets any program label a link with text that
    /// differs from its target), so a `file:` link to a `.desktop` file or
    /// a link to some other registered scheme handler would let a `cat`ed
    /// file or a remote host start local programs. `file:` is refused for
    /// the same reason, even for paths on this host.
    pub fn open_url(url: &str) -> Result<(), UrlError> {
        if !is_openable(url) {
            return Err(UrlError::SchemeNotAllowed(url.to_string()));
        }
        open::that(url).map_err(|e| UrlError::OpenFailed(e.to_string()))
    }
}

/// URI schemes that Ctrl+click opens.
pub const OPENABLE_SCHEMES: &[&str] = &["http", "https", "mailto", "ftp"];

/// Whether `url` has one of the [`OPENABLE_SCHEMES`] (case-insensitive).
fn is_openable(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    !rest.is_empty()
        && !url.chars().any(char::is_control)
        && OPENABLE_SCHEMES
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
}

/// Error opening a URL.
#[derive(Debug)]
pub enum UrlError {
    /// Failed to open the URL
    OpenFailed(String),
    /// The URL's scheme is not one Ultty opens
    SchemeNotAllowed(String),
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlError::OpenFailed(e) => write!(f, "Failed to open URL: {}", e),
            UrlError::SchemeNotAllowed(url) => write!(
                f,
                "Refused to open {url:?}: only {} links are opened",
                OPENABLE_SCHEMES.join(", ")
            ),
        }
    }
}

impl std::error::Error for UrlError {}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // Helper: run the detector on a single line and return found URLs
    // ---------------------------------------------------------------
    fn detect_line(text: &str) -> Vec<DetectedUrl> {
        let mut d = UrlDetector::new(true);
        let owned = text.to_string();
        d.detect(1, |_| owned.clone());
        d.urls.clone()
    }

    fn urls_from(text: &str) -> Vec<String> {
        detect_line(text).into_iter().map(|u| u.url).collect()
    }

    #[test]
    fn web_and_mail_links_are_openable() {
        for url in [
            "https://docs.rs/ultty",
            "http://example.com/a?b=c",
            "HTTPS://EXAMPLE.COM",
            "mailto:someone@example.com",
            "ftp://ftp.example.com/pub",
        ] {
            assert!(is_openable(url), "{url}");
        }
    }

    #[test]
    fn other_schemes_are_refused() {
        for url in [
            "file:///home/user/evil.desktop",
            "file://localhost/etc/passwd",
            "smb://host/share",
            "ssh://host",
            "javascript:alert(1)",
            "vscode://file/x",
            "/usr/bin/xterm",
            "-x",
            "https:",
            "https://example.com/\nnext",
            "",
        ] {
            assert!(!is_openable(url), "{url}");
        }
    }

    #[test]
    fn open_url_refuses_a_file_link_before_launching_anything() {
        assert!(matches!(
            UrlDetector::open_url("file:///tmp/x.desktop"),
            Err(UrlError::SchemeNotAllowed(_))
        ));
    }

    // ==================== Basic protocol matching ====================

    #[test]
    fn detect_http_url() {
        let mut detector = UrlDetector::new(true);
        detector.detect(1, |_| "Visit http://example.com for more info".to_string());
        assert_eq!(detector.urls.len(), 1);
        assert_eq!(detector.urls[0].url, "http://example.com");
    }

    #[test]
    fn detect_https_url() {
        let mut detector = UrlDetector::new(true);
        detector.detect(1, |_| {
            "Check https://github.com/user/repo for code".to_string()
        });
        assert_eq!(detector.urls.len(), 1);
        assert_eq!(detector.urls[0].url, "https://github.com/user/repo");
    }

    #[test]
    fn http_vs_https() {
        assert_eq!(urls_from("http://a.com"), vec!["http://a.com"]);
        assert_eq!(urls_from("https://a.com"), vec!["https://a.com"]);
    }

    // ==================== URLs with paths, query strings, fragments ==

    #[test]
    fn detect_url_with_path_and_query() {
        let found = urls_from("Link: https://example.com/path/to/page?foo=bar&baz=qux");
        assert_eq!(
            found,
            vec!["https://example.com/path/to/page?foo=bar&baz=qux"]
        );
    }

    #[test]
    fn url_with_fragment() {
        let found = urls_from("https://example.com/page#section-2");
        assert_eq!(found, vec!["https://example.com/page#section-2"]);
    }

    #[test]
    fn url_with_port() {
        let found = urls_from("http://localhost:8080/path");
        assert_eq!(found, vec!["http://localhost:8080/path"]);
    }

    #[test]
    fn url_with_auth() {
        let found = urls_from("https://user:pass@example.com/foo");
        assert_eq!(found, vec!["https://user:pass@example.com/foo"]);
    }

    #[test]
    fn url_with_encoded_chars() {
        let found = urls_from("https://example.com/path%20with%20spaces");
        assert_eq!(found, vec!["https://example.com/path%20with%20spaces"]);
    }

    #[test]
    fn url_with_tilde() {
        let found = urls_from("http://example.com/~user/page");
        assert_eq!(found, vec!["http://example.com/~user/page"]);
    }

    #[test]
    fn url_with_dash_and_underscore() {
        let found = urls_from("https://my-site.example.com/some_page");
        assert_eq!(found, vec!["https://my-site.example.com/some_page"]);
    }

    // ==================== Multiple URLs ==============================

    #[test]
    fn detect_multiple_urls() {
        let found = urls_from("First: http://a.com Second: https://b.com");
        assert_eq!(found, vec!["http://a.com", "https://b.com"]);
    }

    #[test]
    fn detect_multiple_urls_back_to_back() {
        let found = urls_from("http://a.com http://b.com http://c.com");
        assert_eq!(found.len(), 3);
        assert_eq!(found[0], "http://a.com");
        assert_eq!(found[1], "http://b.com");
        assert_eq!(found[2], "http://c.com");
    }

    // ==================== Multi-row detection ========================

    #[test]
    fn detect_url_across_rows() {
        let mut detector = UrlDetector::new(true);
        detector.detect(3, |row| match row {
            0 => "Row 0 has http://first.com here".to_string(),
            1 => "Row 1 has nothing".to_string(),
            2 => "Row 2 has https://second.com here".to_string(),
            _ => String::new(),
        });
        assert_eq!(detector.urls.len(), 2);
        assert_eq!(detector.urls[0].row, 0);
        assert_eq!(detector.urls[1].row, 2);
    }

    #[test]
    fn row_indices_are_correct() {
        let mut detector = UrlDetector::new(true);
        detector.detect(5, |row| {
            if row == 3 {
                "http://example.com".to_string()
            } else {
                "nothing here".to_string()
            }
        });
        assert_eq!(detector.urls.len(), 1);
        assert_eq!(detector.urls[0].row, 3);
    }

    // ==================== Column positions ===========================

    #[test]
    fn url_at_position() {
        let mut detector = UrlDetector::new(true);
        detector.detect(1, |_| "Visit https://example.com now".to_string());
        // "Visit " is 6 chars, URL starts at col 6
        assert!(detector.has_url_at(6, 0));
        assert!(detector.has_url_at(10, 0));
        assert!(!detector.has_url_at(5, 0)); // Before URL
        assert!(!detector.has_url_at(0, 1)); // Wrong row
    }

    #[test]
    fn column_positions_basic() {
        let urls = detect_line("AB https://x.co CD");
        assert_eq!(urls.len(), 1);
        // "AB " = 3 chars -> start_col = 3
        assert_eq!(urls[0].start_col, 3);
        // "https://x.co" = 12 chars -> end_col = 3 + 12 = 15
        assert_eq!(urls[0].end_col, 15);
    }

    #[test]
    fn url_at_beginning_of_line() {
        let urls = detect_line("https://example.com rest");
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].start_col, 0);
    }

    #[test]
    fn url_at_end_of_line() {
        let urls = detect_line("go to https://example.com");
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://example.com");
    }

    #[test]
    fn url_at_returns_correct_url() {
        let mut detector = UrlDetector::new(true);
        detector.detect(1, |_| "http://a.co http://b.co".to_string());
        // "http://a.co" is at cols 0..11, " http://b.co" starts at 12
        let first = detector.url_at(0, 0);
        assert!(first.is_some());
        assert_eq!(first.unwrap().url, "http://a.co");

        let second = detector.url_at(12, 0);
        assert!(second.is_some());
        assert_eq!(second.unwrap().url, "http://b.co");
    }

    // ==================== Unicode / multibyte =======================

    #[test]
    fn unicode_prefix_column_offset() {
        // Each CJK character is one char but occupies 2 cells in terminal.
        // However our detector counts *chars*, not cells, so columns are
        // char-based.
        let urls = detect_line("\u{4e16}\u{754c} https://example.com");
        assert_eq!(urls.len(), 1);
        // Two CJK chars + space = 3 chars -> start_col = 3
        assert_eq!(urls[0].start_col, 3);
        assert_eq!(urls[0].url, "https://example.com");
    }

    // ==================== Trailing punctuation stripping =============

    #[test]
    fn url_excludes_trailing_period() {
        let found = urls_from("See http://example.com.");
        assert_eq!(found, vec!["http://example.com"]);
    }

    #[test]
    fn url_excludes_trailing_comma() {
        let found = urls_from("Visit http://example.com, then");
        assert_eq!(found, vec!["http://example.com"]);
    }

    #[test]
    fn url_excludes_trailing_semicolon() {
        let found = urls_from("url: http://example.com;");
        assert_eq!(found, vec!["http://example.com"]);
    }

    #[test]
    fn url_excludes_trailing_colon() {
        let found = urls_from("url: http://example.com:");
        assert_eq!(found, vec!["http://example.com"]);
    }

    #[test]
    fn url_excludes_trailing_exclamation() {
        let found = urls_from("Wow http://example.com!");
        assert_eq!(found, vec!["http://example.com"]);
    }

    #[test]
    fn url_excludes_trailing_question_mark() {
        // A bare trailing '?' is stripped even though '?' is valid in URLs
        let found = urls_from("Is it http://example.com?");
        assert_eq!(found, vec!["http://example.com"]);
    }

    #[test]
    fn url_excludes_trailing_closing_paren() {
        let found = urls_from("(http://example.com)");
        assert_eq!(found, vec!["http://example.com"]);
    }

    #[test]
    fn url_excludes_trailing_closing_bracket() {
        let found = urls_from("[http://example.com]");
        assert_eq!(found, vec!["http://example.com"]);
    }

    #[test]
    fn url_excludes_multiple_trailing_punct() {
        // Trailing ")." -> the regex strips both because ')' and '.' are
        // both in the excluded-final-char class.
        let found = urls_from("(see http://example.com).");
        assert_eq!(found.len(), 1);
        // The URL should not include ')' or '.'
        assert!(!found[0].ends_with('.'));
        assert!(!found[0].ends_with(')'));
    }

    #[test]
    fn url_with_internal_parens_and_path() {
        // Wikipedia-style URLs with parens in path are common.
        // The regex keeps internal parens but strips a trailing one if
        // followed by nothing or punctuation.
        let found = urls_from("https://en.wikipedia.org/wiki/Rust_(programming_language)");
        assert_eq!(found.len(), 1);
        // The trailing ')' is stripped by the regex
        assert!(found[0].starts_with("https://en.wikipedia.org/wiki/Rust_(programming_language"));
    }

    #[test]
    fn url_keeps_query_params_with_question_mark() {
        // '?' followed by more chars is kept
        let found = urls_from("https://example.com/search?q=hello");
        assert_eq!(found, vec!["https://example.com/search?q=hello"]);
    }

    // ==================== Excluded characters ========================

    #[test]
    fn url_stops_at_angle_brackets() {
        let found = urls_from("<https://example.com>");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], "https://example.com");
    }

    #[test]
    fn url_stops_at_double_quote() {
        let found = urls_from("href=\"https://example.com\"");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], "https://example.com");
    }

    #[test]
    fn url_stops_at_single_quote() {
        let found = urls_from("url='https://example.com'");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], "https://example.com");
    }

    #[test]
    fn url_stops_at_backtick() {
        let found = urls_from("`https://example.com`");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], "https://example.com");
    }

    #[test]
    fn url_stops_at_whitespace() {
        let found = urls_from("https://example.com more text");
        assert_eq!(found, vec!["https://example.com"]);
    }

    #[test]
    fn url_stops_at_curly_braces() {
        let found = urls_from("{https://example.com}");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], "https://example.com");
    }

    #[test]
    fn url_stops_at_pipe() {
        let found = urls_from("https://example.com|rest");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], "https://example.com");
    }

    // ==================== Non-URLs that should NOT match =============

    #[test]
    fn no_match_ftp() {
        assert!(urls_from("ftp://files.example.com").is_empty());
    }

    #[test]
    fn no_match_bare_domain() {
        assert!(urls_from("example.com").is_empty());
    }

    #[test]
    fn no_match_www_without_scheme() {
        assert!(urls_from("www.example.com").is_empty());
    }

    #[test]
    fn no_match_plain_text() {
        assert!(urls_from("hello world no urls here").is_empty());
    }

    #[test]
    fn no_match_email() {
        assert!(urls_from("user@example.com").is_empty());
    }

    #[test]
    fn no_match_file_path() {
        assert!(urls_from("/usr/local/bin/program").is_empty());
    }

    #[test]
    fn no_match_empty_line() {
        assert!(urls_from("").is_empty());
    }

    #[test]
    fn no_match_only_scheme() {
        // "http://" alone needs at least 2 chars after "//" (one body + one final)
        assert!(urls_from("http://").is_empty());
    }

    // ==================== DetectedUrl::contains ======================

    #[test]
    fn detected_url_contains() {
        let url = DetectedUrl {
            url: "http://test.com".to_string(),
            start_col: 5,
            end_col: 20,
            row: 3,
        };
        assert!(url.contains(5, 3)); // start inclusive
        assert!(url.contains(19, 3)); // end-1 inclusive
        assert!(!url.contains(4, 3)); // before start
        assert!(!url.contains(20, 3)); // at end (exclusive)
        assert!(!url.contains(10, 2)); // wrong row
        assert!(!url.contains(10, 4)); // wrong row
    }

    #[test]
    fn detected_url_contains_single_width() {
        // Edge case: URL occupying a single column
        let url = DetectedUrl {
            url: "http://x".to_string(),
            start_col: 0,
            end_col: 1,
            row: 0,
        };
        assert!(url.contains(0, 0));
        assert!(!url.contains(1, 0));
    }

    // ==================== Enabled / disabled toggle ==================

    #[test]
    fn disabled_detector() {
        let mut detector = UrlDetector::new(false);
        detector.detect(1, |_| "http://example.com".to_string());
        assert!(detector.urls.is_empty());
    }

    #[test]
    fn toggle_enabled() {
        let mut detector = UrlDetector::new(true);
        assert!(detector.is_enabled());

        detector.detect(1, |_| "http://a.com".to_string());
        assert_eq!(detector.urls.len(), 1);

        detector.set_enabled(false);
        assert!(!detector.is_enabled());
        // Disabling clears cached URLs
        assert!(detector.urls.is_empty());
    }

    #[test]
    fn re_enable_detection() {
        let mut detector = UrlDetector::new(false);
        detector.detect(1, |_| "http://a.com".to_string());
        assert!(detector.urls.is_empty());

        detector.set_enabled(true);
        detector.detect(1, |_| "http://a.com".to_string());
        assert_eq!(detector.urls.len(), 1);
    }

    #[test]
    fn detect_clears_previous_results() {
        let mut detector = UrlDetector::new(true);
        detector.detect(1, |_| "http://first.com".to_string());
        assert_eq!(detector.urls.len(), 1);

        detector.detect(1, |_| "http://second.com".to_string());
        assert_eq!(detector.urls.len(), 1);
        assert_eq!(detector.urls[0].url, "http://second.com");
    }

    // ==================== Default trait ==============================

    #[test]
    fn default_detector_is_disabled() {
        let detector = UrlDetector::default();
        assert!(!detector.is_enabled());
        assert!(detector.urls().is_empty());
    }

    // ==================== OSC 8 hyperlink detection ==================

    #[test]
    fn detect_hyperlinks_basic() {
        let mut detector = UrlDetector::new(true);
        // Simulate a 1x10 grid where cols 2..5 have hyperlink id 1
        detector.detect_hyperlinks(
            1,
            10,
            |_row, col| if (2..5).contains(&col) { 1 } else { 0 },
            |id| {
                if id == 1 {
                    Some("https://example.com".to_string())
                } else {
                    None
                }
            },
        );
        assert_eq!(detector.urls.len(), 1);
        assert_eq!(detector.urls[0].url, "https://example.com");
        assert_eq!(detector.urls[0].start_col, 2);
        assert_eq!(detector.urls[0].end_col, 5);
        assert_eq!(detector.urls[0].row, 0);
    }

    #[test]
    fn detect_hyperlinks_multiple_on_same_row() {
        let mut detector = UrlDetector::new(true);
        // id=1 at cols 0..3, id=2 at cols 5..8
        detector.detect_hyperlinks(
            1,
            10,
            |_row, col| {
                if col < 3 {
                    1
                } else if (5..8).contains(&col) {
                    2
                } else {
                    0
                }
            },
            |id| match id {
                1 => Some("https://a.com".to_string()),
                2 => Some("https://b.com".to_string()),
                _ => None,
            },
        );
        assert_eq!(detector.urls.len(), 2);
        assert_eq!(detector.urls[0].url, "https://a.com");
        assert_eq!(detector.urls[1].url, "https://b.com");
    }

    #[test]
    fn detect_hyperlinks_across_rows() {
        let mut detector = UrlDetector::new(true);
        detector.detect_hyperlinks(
            3,
            5,
            |row, col| {
                if row == 0 && col < 3 {
                    1
                } else if row == 2 && (1..4).contains(&col) {
                    2
                } else {
                    0
                }
            },
            |id| match id {
                1 => Some("https://row0.com".to_string()),
                2 => Some("https://row2.com".to_string()),
                _ => None,
            },
        );
        assert_eq!(detector.urls.len(), 2);
        assert_eq!(detector.urls[0].row, 0);
        assert_eq!(detector.urls[1].row, 2);
    }

    #[test]
    fn detect_hyperlinks_disabled() {
        let mut detector = UrlDetector::new(false);
        detector.detect_hyperlinks(1, 5, |_, _| 1, |_| Some("https://x.com".to_string()));
        assert!(detector.urls.is_empty());
    }

    #[test]
    fn detect_hyperlinks_unknown_id() {
        let mut detector = UrlDetector::new(true);
        // hyperlink id 99 has no URI mapping
        detector.detect_hyperlinks(1, 5, |_, col| if col < 3 { 99 } else { 0 }, |_| None);
        assert!(detector.urls.is_empty());
    }

    // ==================== UrlError display ===========================

    #[test]
    fn url_error_display() {
        let err = UrlError::OpenFailed("something went wrong".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("Failed to open URL"));
        assert!(msg.contains("something went wrong"));
    }

    #[test]
    fn url_error_is_error_trait() {
        let err = UrlError::OpenFailed("test".to_string());
        // Confirm it implements std::error::Error (this compiles = passes)
        let _: &dyn std::error::Error = &err;
    }

    // ==================== Real-world URL patterns ====================

    #[test]
    fn github_url_with_path() {
        let found = urls_from("https://github.com/user/repo/blob/main/src/lib.rs");
        assert_eq!(
            found,
            vec!["https://github.com/user/repo/blob/main/src/lib.rs"]
        );
    }

    #[test]
    fn url_with_complex_query() {
        let found = urls_from("https://www.google.com/search?q=rust+language&hl=en&num=10");
        assert_eq!(
            found,
            vec!["https://www.google.com/search?q=rust+language&hl=en&num=10"]
        );
    }

    #[test]
    fn url_with_fragment_and_query() {
        let found = urls_from("https://docs.rs/regex/latest/regex/?search=find#method.find");
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("#method.find"));
    }

    #[test]
    fn localhost_url() {
        let found = urls_from("http://127.0.0.1:3000/api/v1/data");
        assert_eq!(found, vec!["http://127.0.0.1:3000/api/v1/data"]);
    }

    #[test]
    fn url_surrounded_by_markdown_parens() {
        // Markdown: [link](https://example.com)
        let found = urls_from("[link](https://example.com)");
        assert_eq!(found.len(), 1);
        // The trailing ')' is stripped
        assert_eq!(found[0], "https://example.com");
    }

    #[test]
    fn url_in_angle_bracket_markdown() {
        let found = urls_from("See <https://example.com> for details");
        assert_eq!(found, vec!["https://example.com"]);
    }

    #[test]
    fn url_with_hash_fragment_only() {
        let found = urls_from("https://example.com#top");
        assert_eq!(found, vec!["https://example.com#top"]);
    }
}
