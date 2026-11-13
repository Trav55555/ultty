use crate::work::{category, Page, WorkState};
use ulttui::prelude::*;

pub(crate) fn view(state: &WorkState, height: u16) -> ScrollView {
    let visible = usize::from(height.saturating_sub(5).max(1));
    let start = state.selected.saturating_add(1).saturating_sub(visible);
    let mut column = Column::new().width(Dimension::Percent(100.0)).padding(1.0);
    let title = match state.page {
        Page::Work => "Work | Up/Down select | Enter review | r refresh | Esc back",
        Page::Attention => "Attention | Up/Down select | Enter inspect | r refresh | Esc back",
        Page::Detail => "Run review | Up/Down scroll | Esc work list",
        Page::Closed => "",
    };
    column = column.child(Text::new(title));
    if state.loading {
        column = column.child(Text::new("Loading..."));
    }
    if let Some(error) = &state.error {
        column = column.child(Text::new(format!("Error: {error}")));
    }
    match state.page {
        Page::Work => {
            if state.runs.is_empty() && !state.loading && state.error.is_none() {
                column = column.child(Text::new("No runs returned."));
            }
            for (i, run) in state.runs.iter().enumerate().skip(start).take(visible) {
                column = column.child(Text::new(format!(
                    "{} [{} / {}] {}  {}",
                    if i == state.selected { ">" } else { " " },
                    category(&run.status),
                    run.status,
                    run.id,
                    run.objective
                )));
            }
        }
        Page::Attention => {
            if state.approvals.is_empty() && !state.loading && state.error.is_none() {
                column = column.child(Text::new(
                    "No pending approvals returned. Questions are not available in this daemon.",
                ));
            }
            for (i, item) in state.approvals.iter().enumerate().skip(start).take(visible) {
                column = column.child(Text::new(format!(
                    "{} {} | {} | {}",
                    if i == state.selected { ">" } else { " " },
                    item.id,
                    item.source,
                    item.objective
                )));
            }
        }
        Page::Detail => {
            if let Some(run) = state
                .runs
                .iter()
                .find(|run| Some(&run.id) == state.detail_id.as_ref())
            {
                for line in [
                    format!("Run {} | {}", run.id, run.status),
                    format!("Objective: {}", run.objective),
                    format!("Result: {}", run.result.as_deref().unwrap_or("unknown")),
                    format!("Error: {}", run.error.as_deref().unwrap_or("none reported")),
                    "Revision, changed files and diff: unknown".into(),
                    "Accepted: unknown | Merged: unknown".into(),
                    format!(
                        "To open this run, return to chat and use /attach {}",
                        run.id
                    ),
                ] {
                    column = column.child(Text::new(line));
                }
                if state.reports.is_empty() && !state.loading && state.error.is_none() {
                    column = column.child(Text::new("Verification: no reports returned"));
                }
                for report in &state.reports {
                    column = column.child(Text::new(format!(
                        "Verification {} | passed={} | {}",
                        report.report_type, report.passed, report.summary
                    )));
                    column = column.child(Text::new(format!(
                        "Checks: {}",
                        report
                            .checks
                            .iter()
                            .map(ValueDisplay::display)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                    column = column.child(Text::new(format!("Artifacts: {}", report.artifacts)));
                }
            }
        }
        Page::Closed => {}
    }
    let mut scroll = ScrollView::new().child(column);
    scroll.set_viewport_height(f32::from(height));
    scroll.scroll_to(state.detail_scroll);
    scroll
}
trait ValueDisplay {
    fn display(&self) -> String;
}
impl ValueDisplay for serde_json::Value {
    fn display(&self) -> String {
        self.to_string()
    }
}
