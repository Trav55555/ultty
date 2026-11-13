//! Explicit copy actions preserve source text rather than rendered wrapping.

use ulttui::prelude::Cmd;
use ulttui::toast::Toast;

use crate::{Msg, PorygonTui};

impl PorygonTui {
    pub(crate) fn copy_latest_reply(&self) -> Cmd<Msg> {
        let Some(reply) = self
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "assistant" && !message.content.is_empty())
        else {
            return Cmd::ShowToast(Toast::warning("No reply to copy yet."));
        };
        copy(reply.content.clone())
    }

    pub(crate) fn copy_review(&self) -> Cmd<Msg> {
        if self.work.loading || self.work.error.is_some() {
            return Cmd::ShowToast(Toast::warning(
                "Refresh this review before copying its evidence.",
            ));
        }
        let Some(run) = self
            .work
            .runs
            .iter()
            .find(|run| Some(&run.id) == self.work.detail_id.as_ref())
        else {
            return Cmd::none();
        };
        let reports: Vec<_> = self
            .work
            .reports
            .iter()
            .map(|report| {
                serde_json::json!({
                    "run_id": report.run_id, "report_type": report.report_type,
                    "passed": report.passed, "summary": report.summary,
                    "checks": report.checks, "artifacts": report.artifacts,
                })
            })
            .collect();
        match serde_json::to_string_pretty(&serde_json::json!({
            "run_id": run.id, "status": run.status, "result": run.result,
            "error": run.error, "verification_reports": reports,
        })) {
            Ok(text) => copy(text),
            Err(error) => Cmd::ShowToast(Toast::error(format!(
                "Could not prepare review for copying: {error}"
            ))),
        }
    }
}

fn copy(text: String) -> Cmd<Msg> {
    Cmd::copy_to_clipboard_with_result(text, |result| {
        Msg::ClipboardWritten(result.map_err(|error| error.to_string()))
    })
}
