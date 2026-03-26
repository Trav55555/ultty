/// Render a multi-row ASCII chart from a history of values (0-100).
/// Each column is one time point, each row is a threshold level.
/// Returns multiple lines to be rendered as separate Text widgets.
pub fn chart(history: &[f32], width: usize, height: usize) -> Vec<String> {
    let start = if history.len() > width {
        history.len() - width
    } else {
        0
    };
    let values = &history[start..];
    let pad = width.saturating_sub(values.len());

    let mut rows = Vec::with_capacity(height);
    for row in 0..height {
        // Top row = highest threshold, bottom row = lowest
        let threshold = 100.0 * (height - row) as f32 / height as f32;
        let half_threshold = threshold - 100.0 / (2.0 * height as f32);

        let mut line = String::with_capacity(width + 6);

        // Y-axis label
        if row == 0 {
            line.push_str(&format!("{:>3}%|", threshold as u32));
        } else if row == height - 1 {
            line.push_str("  0%|");
        } else if row == height / 2 {
            line.push_str(&format!("{:>3}%|", (threshold) as u32));
        } else {
            line.push_str("    |");
        }

        // Padding for missing history
        for _ in 0..pad {
            line.push(' ');
        }

        // Data columns
        for &v in values {
            if v >= threshold {
                line.push('#');
            } else if v >= half_threshold {
                line.push(':');
            } else {
                line.push(' ');
            }
        }

        rows.push(line);
    }

    rows
}

/// Render a horizontal bar with filled/empty blocks.
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
