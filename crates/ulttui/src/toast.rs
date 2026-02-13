use std::time::Duration;

/// Severity level for a toast notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastSeverity {
    Info,
    Success,
    Warning,
    Error,
}

/// A toast notification message.
#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub severity: ToastSeverity,
    pub duration: Duration,
}

/// Framework-managed collection of active toasts.
#[derive(Debug, Clone, Default)]
pub struct ToastOverlay {
    toasts: Vec<Toast>,
}

impl ToastOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(mut self, toast: Toast) -> Self {
        self.toasts.push(toast);
        self
    }

    pub fn from_toasts(toasts: Vec<Toast>) -> Self {
        Self { toasts }
    }

    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }
}

impl Toast {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity: ToastSeverity::Info,
            duration: Duration::from_secs(3),
        }
    }

    pub fn success(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity: ToastSeverity::Success,
            duration: Duration::from_secs(3),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity: ToastSeverity::Warning,
            duration: Duration::from_secs(5),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity: ToastSeverity::Error,
            duration: Duration::from_secs(5),
        }
    }

    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{Toast, ToastOverlay, ToastSeverity};
    use std::time::Duration;

    #[test]
    fn toast_info_defaults() {
        let toast = Toast::info("hello");
        assert_eq!(toast.message, "hello");
        assert_eq!(toast.severity, ToastSeverity::Info);
        assert_eq!(toast.duration, Duration::from_secs(3));
    }

    #[test]
    fn toast_success_defaults() {
        let toast = Toast::success("done");
        assert_eq!(toast.message, "done");
        assert_eq!(toast.severity, ToastSeverity::Success);
        assert_eq!(toast.duration, Duration::from_secs(3));
    }

    #[test]
    fn toast_warning_defaults() {
        let toast = Toast::warning("warn");
        assert_eq!(toast.severity, ToastSeverity::Warning);
        assert_eq!(toast.duration, Duration::from_secs(5));
    }

    #[test]
    fn toast_error_defaults() {
        let toast = Toast::error("boom");
        assert_eq!(toast.severity, ToastSeverity::Error);
        assert_eq!(toast.duration, Duration::from_secs(5));
    }

    #[test]
    fn toast_custom_duration() {
        let toast = Toast::info("custom").duration(Duration::from_secs(10));
        assert_eq!(toast.duration, Duration::from_secs(10));
    }

    #[test]
    fn toast_overlay_collects_toasts() {
        let overlay = ToastOverlay::new()
            .push(Toast::info("one"))
            .push(Toast::error("two"));
        assert_eq!(overlay.toasts().len(), 2);
        assert!(!overlay.is_empty());
    }
}
