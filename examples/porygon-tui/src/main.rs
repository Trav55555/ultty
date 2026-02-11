use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::widgets::{RichText, Stack};

#[derive(Debug, Clone)]
struct ChatMessage {
    role: String,
    content: String,
    timestamp: Instant,
}

#[derive(Debug, Clone)]
struct AgentStatus {
    cost_usd: f64,
    budget_usd: f64,
    pressure: String,
    model: String,
    context_quality: String,
    step: String,
    agent_name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ApprovalRequest {
    id: String,
    tool_name: String,
    params: serde_json::Value,
    risk_score: f64,
    reasoning: String,
    blast_radius: String,
    reversible: bool,
}

impl Default for AgentStatus {
    fn default() -> Self {
        Self {
            cost_usd: 0.0,
            budget_usd: 5.0,
            pressure: "normal".into(),
            model: "claude-sonnet-4".into(),
            context_quality: "peak".into(),
            step: "idle".into(),
            agent_name: "agent".into(),
        }
    }
}

struct PorygonTui {
    messages: Vec<ChatMessage>,
    input: String,
    status: AgentStatus,
    daemon_url: String,
    connected: bool,
    error: Option<String>,
    spinner_frame: usize,
    waiting_response: bool,
    pending_approval: Option<ApprovalRequest>,
    show_approval_modal: bool,
}

impl PorygonTui {
    fn new() -> Self {
        let daemon_url = std::env::var("PORYGON_URL")
            .unwrap_or_else(|_| "http://localhost:18789".to_string());

        Self {
            messages: vec![ChatMessage {
                role: "system".into(),
                content: format!("Porygon TUI - daemon: {daemon_url}"),
                timestamp: Instant::now(),
            }],
            input: String::new(),
            status: AgentStatus::default(),
            daemon_url,
            connected: false,
            error: None,
            spinner_frame: 0,
            waiting_response: false,
            pending_approval: None,
            show_approval_modal: false,
        }
    }
}

#[derive(Debug)]
enum Msg {
    AppendChar(char),
    DeleteChar,
    SendMessage,
    PollConnection,
    PollStatus,
    DaemonResponse(String),
    DaemonError(String),
    ConnectionChecked(bool),
    StatusUpdated(AgentStatus),
    SpinnerTick,
    PollApprovals,
    ApprovalReceived(ApprovalRequest),
    ApprovalDecision(bool),
    ApprovalSubmitted,
    NoApprovalsPending,
}

impl Application for PorygonTui {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::AppendChar(c) => {
                self.input.push(c);
                Cmd::none()
            }
            Msg::DeleteChar => {
                self.input.pop();
                Cmd::none()
            }
            Msg::SendMessage => {
                if self.input.is_empty() || self.waiting_response {
                    return Cmd::none();
                }

                let user_text = self.input.clone();
                self.input.clear();
                self.messages.push(ChatMessage {
                    role: "user".into(),
                    content: user_text.clone(),
                    timestamp: Instant::now(),
                });

                self.waiting_response = true;
                self.error = None;
                self.status.step = "awaiting daemon response".into();

                let url = format!("{}/api/ask", self.daemon_url);
                Cmd::spawn(async move {
                    match send_to_daemon(&url, &user_text).await {
                        Ok(response) => Msg::DaemonResponse(response),
                        Err(err) => Msg::DaemonError(err),
                    }
                })
            }
            Msg::PollConnection => {
                let url = self.daemon_url.clone();
                Cmd::spawn(async move { Msg::ConnectionChecked(check_connection(&url).await) })
            }
            Msg::PollStatus => {
                let url = self.daemon_url.clone();
                Cmd::spawn(async move {
                    match fetch_status(&url).await {
                        Ok(status) => Msg::StatusUpdated(status),
                        Err(_) => Msg::StatusUpdated(AgentStatus::default()),
                    }
                })
            }
            Msg::PollApprovals => {
                let url = format!("{}/api/approvals/pending", self.daemon_url);
                Cmd::spawn(async move {
                    match fetch_pending_approval(&url).await {
                        Ok(Some(request)) => Msg::ApprovalReceived(request),
                        Ok(None) => Msg::NoApprovalsPending,
                        Err(_) => Msg::NoApprovalsPending,
                    }
                })
            }
            Msg::DaemonResponse(response) => {
                self.waiting_response = false;
                self.status.step = "idle".into();
                self.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: response,
                    timestamp: Instant::now(),
                });
                Cmd::none()
            }
            Msg::DaemonError(err) => {
                self.waiting_response = false;
                self.status.step = "idle".into();
                self.error = Some(err.clone());
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("Error: {err}"),
                    timestamp: Instant::now(),
                });
                Cmd::none()
            }
            Msg::ConnectionChecked(ok) => {
                self.connected = ok;
                if !ok {
                    self.error = Some("Cannot reach daemon".into());
                }
                Cmd::none()
            }
            Msg::StatusUpdated(status) => {
                self.status.cost_usd = status.cost_usd;
                self.status.budget_usd = status.budget_usd;
                self.status.pressure = status.pressure;
                self.status.model = status.model;
                self.status.context_quality = status.context_quality;
                self.status.agent_name = status.agent_name;
                if !self.waiting_response {
                    self.status.step = status.step;
                }
                Cmd::none()
            }
            Msg::SpinnerTick => {
                self.spinner_frame = (self.spinner_frame + 1) % 10;
                Cmd::none()
            }
            Msg::ApprovalReceived(request) => {
                self.pending_approval = Some(request);
                self.show_approval_modal = true;
                Cmd::none()
            }
            Msg::ApprovalDecision(approved) => {
                self.show_approval_modal = false;
                if let Some(request) = self.pending_approval.take() {
                    let url = format!("{}/api/approvals/{}/decide", self.daemon_url, request.id);
                    Cmd::spawn(async move {
                        let _ = submit_approval(&url, approved).await;
                        Msg::ApprovalSubmitted
                    })
                } else {
                    Cmd::none()
                }
            }
            Msg::ApprovalSubmitted => Cmd::none(),
            Msg::NoApprovalsPending => Cmd::none(),
        }
    }

    fn subscriptions(&self) -> Sub<Msg> {
        let mut subs = vec![
            Sub::every(Duration::from_secs(3), || Msg::PollConnection),
            Sub::every(Duration::from_secs(2), || Msg::PollStatus),
            Sub::every(Duration::from_millis(500), || Msg::PollApprovals),
        ];
        if self.waiting_response {
            subs.push(Sub::every(Duration::from_millis(80), || Msg::SpinnerTick));
        }
        Sub::batch(subs)
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        if self.show_approval_modal {
            return match event {
                Event::Key(ke) => match ke.key {
                    Key::Char('y') | Key::Enter => Some(Msg::ApprovalDecision(true)),
                    Key::Char('n') | Key::Escape => Some(Msg::ApprovalDecision(false)),
                    _ => None,
                },
                _ => None,
            };
        }

        match event {
            Event::Key(ke) => match ke.key {
                Key::Enter => Some(Msg::SendMessage),
                Key::Backspace => Some(Msg::DeleteChar),
                Key::Char(c) => Some(Msg::AppendChar(c)),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        let main_content = Column::new()
            .child(self.header_view())
            .child(Divider::horizontal())
            .child(ScrollView::new().child(self.messages_view()))
            .child(Divider::horizontal())
            .child(self.input_view())
            .child(Divider::horizontal())
            .child(self.status_bar_view());

        if self.show_approval_modal {
            if let Some(req) = &self.pending_approval {
                return Stack::new().child(main_content).child(
                    Modal::new()
                        .title("Tool Approval Required")
                        .body(self.approval_modal_body(req))
                        .buttons(vec!["Approve (y/Enter)", "Reject (n/Esc)"])
                        .visible(true),
                );
            }
        }

        Stack::new()
            .child(main_content)
            .child(Modal::new().visible(false))
    }
}

impl PorygonTui {
    fn approval_modal_body(&self, req: &ApprovalRequest) -> String {
        let params_display = serde_json::to_string_pretty(&req.params)
            .unwrap_or_else(|_| format!("{:?}", req.params));
        let risk = format!("{:.0}%", req.risk_score * 100.0);
        let reversible = if req.reversible { "yes" } else { "no" };

        format!(
            "Tool: {}\nRisk: {}\nBlast radius: {}\nReversible: {}\n\nReasoning:\n{}\n\nParameters:\n{}",
            req.tool_name, risk, req.blast_radius, reversible, req.reasoning, params_display
        )
    }

    fn header_view(&self) -> Row {
        let (conn_icon, conn_color) = if self.connected {
            ("●", Color::GREEN)
        } else {
            ("○", Color::RED)
        };

        Row::new().child(
            RichText::new()
                .span(format!(" {} ", conn_icon), Style::new().fg(conn_color))
                .span("Porygon TUI", Style::new().bold().fg(Color::CYAN))
                .span(format!("  {}", self.daemon_url), Style { dim: true, ..Style::new() }),
        )
    }

    fn messages_view(&self) -> Column {
        let mut col = Column::new().gap(0.5).padding(1.0);
        for msg in &self.messages {
            let (prefix, color) = match msg.role.as_str() {
                "user" => ("You", Color::GREEN),
                "assistant" => ("Porygon", Color::CYAN),
                "system" => ("System", Color::YELLOW),
                _ => ("?", Color::WHITE),
            };
            let age = msg.timestamp.elapsed().as_secs();
            col = col.child(
                RichText::new()
                    .span(
                        format!("[{age:>3}s] {prefix}: "),
                        Style::new().fg(color).bold(),
                    )
                    .span(msg.content.clone(), Style::default()),
            );
        }

        if self.waiting_response {
            col = col.child(
                Row::new()
                    .gap(1.0)
                    .child(Spinner::new().frame(self.spinner_frame))
                    .child(Text::new("Thinking...")),
            );
        }

        col
    }

    fn input_view(&self) -> Row {
        Row::new().child(Text::new(format!("> {}|", self.input)))
    }

    fn status_bar_view(&self) -> Row {
        let (pressure_icon, pressure_color) = match self.status.pressure.as_str() {
            "normal" => ("●", Color::GREEN),
            "elevated" => ("▲", Color::YELLOW),
            "critical" => ("◆", Color::RED),
            _ => ("?", Color::WHITE),
        };

        let (ctx_icon, ctx_color) = match self.status.context_quality.as_str() {
            "peak" => ("◆", Color::GREEN),
            "good" => ("◆", Color::CYAN),
            "degrading" => ("◆", Color::YELLOW),
            "poor" => ("◆", Color::RED),
            _ => ("◆", Color::WHITE),
        };

        let ratio = if self.status.budget_usd > 0.0 {
            self.status.cost_usd / self.status.budget_usd
        } else {
            0.0
        };
        let bar_width = 10usize;
        let filled = (ratio.clamp(0.0, 1.0) * bar_width as f64).round() as usize;
        let budget_bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(bar_width - filled));

        let budget_color = if ratio > 0.8 {
            Color::RED
        } else if ratio > 0.5 {
            Color::YELLOW
        } else {
            Color::GREEN
        };

        let error = self.error.as_deref().unwrap_or("ok");

        Row::new().child(
            RichText::new()
                .span(
                    format!(" ${:.2}/${:.2} ", self.status.cost_usd, self.status.budget_usd),
                    Style::default(),
                )
                .span(budget_bar, Style::new().fg(budget_color))
                .span(format!(" {} ", pressure_icon), Style::new().fg(pressure_color).bold())
                .span(format!("{} ", self.status.model), Style { dim: true, ..Style::new() })
                .span(format!("{} ", self.status.agent_name), Style::new().fg(Color::CYAN))
                .span(format!("ctx:{} ", ctx_icon), Style::new().fg(ctx_color))
                .span(format!("{} ", self.status.step), Style::default())
                .span(format!("{} ", error), Style::new().fg(Color::YELLOW))
                .span("Enter:send Ctrl+C:quit", Style { dim: true, ..Style::new() }),
        )
    }
}

#[derive(Serialize)]
struct AskRequest {
    prompt: String,
}

#[derive(Deserialize)]
struct AskResponse {
    response: Option<String>,
    content: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct StatusResponse {
    cost_usd: Option<f64>,
    budget_usd: Option<f64>,
    pressure: Option<String>,
    model: Option<String>,
    context_quality: Option<String>,
    step: Option<String>,
    agent_name: Option<String>,
}

async fn send_to_daemon(url: &str, message: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .json(&AskRequest {
            prompt: message.to_string(),
        })
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }

    let body: AskResponse = response.json().await.map_err(|err| err.to_string())?;
    Ok(body
        .response
        .or(body.content)
        .or(body.message)
        .unwrap_or_else(|| "(empty response)".to_string()))
}

async fn check_connection(daemon_url: &str) -> bool {
    reqwest::Client::new()
        .get(daemon_url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok()
}

async fn fetch_status(daemon_url: &str) -> anyhow::Result<AgentStatus> {
    let url = format!("{daemon_url}/api/status");
    let body: StatusResponse = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(2))
        .send()
        .await?
        .json()
        .await?;

    let mut status = AgentStatus::default();
    status.cost_usd = body.cost_usd.unwrap_or(status.cost_usd);
    status.budget_usd = body.budget_usd.unwrap_or(status.budget_usd);
    status.pressure = body.pressure.unwrap_or(status.pressure);
    status.model = body.model.unwrap_or(status.model);
    status.context_quality = body.context_quality.unwrap_or(status.context_quality);
    status.step = body.step.unwrap_or(status.step);
    status.agent_name = body.agent_name.unwrap_or(status.agent_name);
    Ok(status)
}

async fn fetch_pending_approval(url: &str) -> anyhow::Result<Option<ApprovalRequest>> {
    let response = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(1))
        .send()
        .await?;

    if response.status() == reqwest::StatusCode::NO_CONTENT
        || response.status() == reqwest::StatusCode::NOT_FOUND
    {
        return Ok(None);
    }

    let request: ApprovalRequest = response.json().await?;
    Ok(Some(request))
}

async fn submit_approval(url: &str, approved: bool) -> anyhow::Result<()> {
    reqwest::Client::new()
        .post(url)
        .json(&serde_json::json!({ "approved": approved }))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    PorygonTui::new().run()
}
