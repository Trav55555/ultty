//! URL detection and click-to-open functionality.
//!
//! This module provides URL detection in terminal output and the ability
//! to open URLs in the default browser.

use regex::Regex;
use std::sync::LazyLock;

/// URL regex pattern.
///
/// Matches HTTP and HTTPS URLs, being careful to avoid trailing punctuation.
static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"https?://[^\s<>"'`\{\}\|\\\^\[\]]+[^\s<>"'`\{\}\|\\\^\[\]\.,;:!?\)\]]"#)
        .expect("Invalid URL regex")
});

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
}

impl UrlDetector {
    /// Create a new URL detector.
    pub fn new(enabled: bool) -> Self {
        Self {
            urls: Vec::new(),
            enabled,
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
        }
    }

    /// Detect URLs in the visible terminal grid.
    ///
    /// Call this when terminal content changes.
    pub fn detect<F>(&mut self, rows: u16, get_row_text: F)
    where
        F: Fn(u16) -> String,
    {
        self.urls.clear();

        if !self.enabled {
            return;
        }

        for row_idx in 0..rows {
            let text = get_row_text(row_idx);
            self.detect_in_line(&text, row_idx);
        }
    }

    /// Detect URLs in a single line.
    fn detect_in_line(&mut self, text: &str, row: u16) {
        for mat in URL_REGEX.find_iter(text) {
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

    /// Find a URL at the given position.
    pub fn url_at(&self, col: u16, row: u16) -> Option<&DetectedUrl> {
        self.urls.iter().find(|u| u.contains(col, row))
    }

    /// Check if there's a URL at the given position.
    pub fn has_url_at(&self, col: u16, row: u16) -> bool {
        self.url_at(col, row).is_some()
    }

    /// Open a URL in the default browser.
    pub fn open_url(url: &str) -> Result<(), UrlError> {
        open::that(url).map_err(|e| UrlError::OpenFailed(e.to_string()))
    }
}

/// Error opening a URL.
#[derive(Debug)]
pub enum UrlError {
    /// Failed to open the URL
    OpenFailed(String),
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlError::OpenFailed(e) => write!(f, "Failed to open URL: {}", e),
        }
    }
}

impl std::error::Error for UrlError {}

#[cfg(test)]
mod tests {
    use super::*;

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
        detector.detect(1, |_| "Check https://github.com/user/repo for code".to_string());

        assert_eq!(detector.urls.len(), 1);
        assert_eq!(detector.urls[0].url, "https://github.com/user/repo");
    }

    #[test]
    fn detect_url_with_path_and_query() {
        let mut detector = UrlDetector::new(true);
        detector.detect(1, |_| "Link: https://example.com/path/to/page?foo=bar&baz=qux".to_string());

        assert_eq!(detector.urls.len(), 1);
        assert_eq!(detector.urls[0].url, "https://example.com/path/to/page?foo=bar&baz=qux");
    }

    #[test]
    fn detect_multiple_urls() {
        let mut detector = UrlDetector::new(true);
        detector.detect(1, |_| "First: http://a.com Second: https://b.com".to_string());

        assert_eq!(detector.urls.len(), 2);
        assert_eq!(detector.urls[0].url, "http://a.com");
        assert_eq!(detector.urls[1].url, "https://b.com");
    }

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
    fn url_at_position() {
        let mut detector = UrlDetector::new(true);
        detector.detect(1, |_| "Visit https://example.com now".to_string());

        // "Visit " is 6 chars, URL starts at col 6
        assert!(detector.has_url_at(6, 0));
        assert!(detector.has_url_at(10, 0));
        assert!(!detector.has_url_at(5, 0));  // Before URL
        assert!(!detector.has_url_at(0, 1));  // Wrong row
    }

    #[test]
    fn url_excludes_trailing_punctuation() {
        let mut detector = UrlDetector::new(true);
        detector.detect(1, |_| "See http://example.com.".to_string());

        assert_eq!(detector.urls.len(), 1);
        assert_eq!(detector.urls[0].url, "http://example.com");
    }

    #[test]
    fn disabled_detector() {
        let mut detector = UrlDetector::new(false);
        detector.detect(1, |_| "http://example.com".to_string());

        assert!(detector.urls.is_empty());
    }

    #[test]
    fn detected_url_contains() {
        let url = DetectedUrl {
            url: "http://test.com".to_string(),
            start_col: 5,
            end_col: 20,
            row: 3,
        };

        assert!(url.contains(5, 3));
        assert!(url.contains(19, 3));
        assert!(!url.contains(4, 3));
        assert!(!url.contains(20, 3));
        assert!(!url.contains(10, 2));
    }
}
