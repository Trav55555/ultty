use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: Option<String>,
    pub date: Option<String>,
    pub message_count: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryResult {
    pub content: String,
    pub score: Option<f64>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub status: Option<String>,
    pub tools_count: Option<usize>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileTreeEntry {
    pub name: String,
    #[serde(default)]
    pub is_dir: bool,
    #[serde(default)]
    pub children: Vec<FileTreeEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default)]
    pub file_count: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanStep {
    pub description: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Plan {
    #[allow(dead_code)]
    #[serde(default)]
    pub id: String,
    pub objective: String,
    #[serde(default)]
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KanbanTask {
    pub id: String,
    pub title: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KanbanBoard {
    #[serde(default)]
    pub todo: Vec<KanbanTask>,
    #[serde(default)]
    pub in_progress: Vec<KanbanTask>,
    #[serde(default)]
    pub done: Vec<KanbanTask>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AsyncTaskInfo {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub result: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CronJob {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub next_run: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub context_window: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub snippet: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrainerProfile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub xp: u64,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValidationResult {
    #[serde(default)]
    pub safe: bool,
    #[serde(default)]
    pub risk_level: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub details: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AgentStatus {
    pub cost_usd: f64,
    pub budget_usd: f64,
    pub pressure: String,
    pub model: String,
    pub context_quality: String,
    pub step: String,
    pub agent_name: String,
    pub cpu_usage: f64,
    pub ram_usage: f64,
}

#[derive(Debug, Clone)]
pub struct WeatherData {
    pub temp_c: i32,
    pub description: String,
    pub emoji: String,
    pub city: String,
    pub forecast: Vec<(String, i32, i32)>,
    pub hourly_temps: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub risk_score: f64,
    pub reasoning: String,
    pub blast_radius: String,
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ToolStatus {
    Pending,
    Approved,
    Rejected,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub args: String,
    pub status: ToolStatus,
    pub result: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StreamPerfSummary {
    pub ttft_ms: u64,
    pub total_ms: u64,
    pub token_count: usize,
    pub append_sample_count: usize,
    pub append_p95_ms: Option<u64>,
    pub append_avg_ms: Option<u64>,
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
            cpu_usage: 0.0,
            ram_usage: 0.0,
        }
    }
}
