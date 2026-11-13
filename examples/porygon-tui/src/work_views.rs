use ulttui::prelude::*;
use unicode_width::UnicodeWidthChar;

use crate::work::{attention, category, AttentionItem, Page};
use crate::PorygonTui;

pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let mut row = String::new();
        let mut used = 0;
        for ch in line.chars() {
            let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + cells > width && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(ch);
            used += cells;
        }
        rows.push(row);
    }
    rows
}

fn rows(app: &PorygonTui) -> (Vec<String>, usize) {
    let state = &app.work;
    let width = usize::from(app.terminal_width.saturating_sub(2).max(1));
    let mut rows = Vec::new();
    let mut selected_row = 0;
    if state.loading {
        rows.extend(wrap("Loading...", width));
    }
    if let Some(error) = &state.error {
        rows.extend(wrap(&format!("Error: {error}"), width));
    }
    match state.page {
        Page::Work => {
            if state.runs.is_empty() && !state.loading && state.error.is_none() {
                rows.push("No runs returned.".into());
            }
            for (i, run) in state.runs.iter().enumerate() {
                if i == state.selected {
                    selected_row = rows.len();
                }
                rows.extend(wrap(
                    &format!(
                        "{} [{} / {}] {} | {}",
                        if i == state.selected { ">" } else { " " },
                        category(&run.status),
                        run.status,
                        run.objective,
                        run.id
                    ),
                    width,
                ));
            }
        }
        Page::Attention => {
            let stream = app
                .pending_approval
                .as_ref()
                .filter(|_| app.approval_is_stream);
            let items = attention(state, stream.map(|r| r.id.as_str()));
            if items.is_empty() && !state.loading && state.error.is_none() {
                rows.push("No pending approvals or waiting runs returned.".into());
            }
            for (i, item) in items.iter().enumerate() {
                if i == state.selected {
                    selected_row = rows.len();
                }
                let text = match item {
                    AttentionItem::Stream => stream
                        .map(|r| format!("Approval: {} | {}", r.reasoning, r.id))
                        .unwrap_or_default(),
                    AttentionItem::Approval(index) => {
                        let a = &state.approvals[*index];
                        format!("Approval: {} | {}", a.objective, a.id)
                    }
                    AttentionItem::Run(index) => {
                        let r = &state.runs[*index];
                        format!("{}: {} | {}", r.status, r.objective, r.id)
                    }
                };
                rows.extend(wrap(
                    &format!("{} {text}", if i == state.selected { ">" } else { " " }),
                    width,
                ));
            }
            rows.extend(wrap("Questions: open the waiting run to continue; no separate question route is available.", width));
        }
        Page::Detail => {
            if let Some(run) = state
                .runs
                .iter()
                .find(|r| Some(&r.id) == state.detail_id.as_ref())
            {
                for line in [
                    format!("Objective: {}", run.objective),
                    format!("Run: {} | {}", run.id, run.status),
                    format!("Result: {}", run.result.as_deref().unwrap_or("unknown")),
                    format!("Error: {}", run.error.as_deref().unwrap_or("none reported")),
                    "Accepted: unknown | Merged: unknown".into(),
                ] {
                    rows.extend(wrap(&line, width));
                }
                if state.reports.is_empty() && !state.loading && state.error.is_none() {
                    rows.push("Verification: no reports returned".into());
                }
                for report in &state.reports {
                    rows.extend(wrap(
                        &format!(
                            "Verification {} | passed={} | {}",
                            report.report_type, report.passed, report.summary
                        ),
                        width,
                    ));
                    let revision = report
                        .artifacts
                        .get("revision")
                        .or_else(|| report.artifacts.get("commit_sha"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    rows.extend(wrap(&format!("Reported revision: {revision}"), width));
                    rows.extend(wrap(
                        &format!(
                            "Checks: {}",
                            serde_json::to_string_pretty(&report.checks).unwrap_or_default()
                        ),
                        width,
                    ));
                    rows.extend(wrap(
                        &format!(
                            "Artifacts: {}",
                            serde_json::to_string_pretty(&report.artifacts).unwrap_or_default()
                        ),
                        width,
                    ));
                }
                if state.reports.is_empty() {
                    rows.push("Reported revision: unknown".into());
                }
            }
        }
        Page::Closed => {}
    }
    (rows, selected_row)
}

pub(crate) fn max_scroll(app: &PorygonTui) -> usize {
    rows(app)
        .0
        .len()
        .saturating_sub(usize::from(app.terminal_height.saturating_sub(3).max(1)))
}

pub(crate) fn view(app: &PorygonTui) -> Column {
    let state = &app.work;
    let available = usize::from(app.terminal_height.saturating_sub(3).max(1));
    let title = match state.page {
        Page::Work => "Work | Enter review | r refresh | Esc",
        Page::Attention => "Attention | Enter inspect | r refresh | Esc",
        Page::Detail => "Review | a attach | r refresh | Esc",
        Page::Closed => "",
    };
    let (rows, selected_row) = rows(app);
    let start = if state.page == Page::Detail {
        state
            .detail_scroll
            .min(rows.len().saturating_sub(available))
    } else {
        selected_row.saturating_sub(available.saturating_sub(1))
    };
    let mut column = Column::new()
        .width(Dimension::Percent(100.0))
        .height(Dimension::Percent(100.0))
        .padding(1.0)
        .child(Text::new(title));
    for row in rows.into_iter().skip(start).take(available) {
        column = column.child(Text::new(row));
    }
    column
}
