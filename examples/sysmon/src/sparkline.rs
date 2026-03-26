/// Render a sparkline from a history of values (0-100).
/// Uses ASCII characters for GPU-renderer compatibility.
pub fn sparkline(history: &[f32], width: usize) -> String {
    const CHARS: [char; 8] = [' ', '.', '_', '-', '=', '~', '^', '*'];
    let start = if history.len() > width {
        history.len() - width
    } else {
        0
    };
    let mut result = String::with_capacity(width);
    for _ in 0..(width.saturating_sub(history.len())) {
        result.push(' ');
    }
    for &v in &history[start..] {
        let idx = ((v / 100.0) * 7.0).clamp(0.0, 7.0) as usize;
        result.push(CHARS[idx]);
    }
    result
}

/// Render a horizontal bar with filled/empty blocks.
/// Uses ASCII `#` and `-` for terminal compatibility.
pub fn bar(percent: f32, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "#".repeat(filled), "-".repeat(empty))
}

/// Format bytes to human-readable string.
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

/// Format uptime seconds to human-readable string.
pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}
