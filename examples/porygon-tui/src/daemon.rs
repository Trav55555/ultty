use std::time::{Duration, Instant};

use serde::Deserialize;
use ulttui::prelude::*;
use ulttui::toast::Toast;
use ulttui::widgets::{parse_unified_diff, DiffKind};

use crate::commands::CommandAction;
use crate::types::*;
use crate::views::{format_clock, format_file_tree, format_timestamp};
use crate::{Msg, PorygonTui};

pub async fn fetch_status(
    client: &reqwest::Client,
    daemon_url: &str,
) -> anyhow::Result<AgentStatus> {
    let url = format!("{}/status/tui", daemon_url.trim_end_matches('/'));
    let body: serde_json::Value = client
        .get(url)
        .timeout(Duration::from_secs(2))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    anyhow::ensure!(body.get("model").is_some(), "Invalid daemon TUI status");
    let (cpu, ram) = get_system_stats();
    Ok(AgentStatus {
        model: body["model"]["name"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        context_quality: "unknown".into(),
        pressure: "unknown".into(),
        cpu_usage: unit_fraction(cpu),
        ram_usage: unit_fraction(ram),
        ..AgentStatus::default()
    })
}

/// Clamp a usage value into `0.0..=1.0`, treating NaN as zero.
pub(crate) fn unit_fraction(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn get_system_stats() -> (f64, f64) {
    let mut cpu = 0.0;
    let mut ram = 0.0;

    #[cfg(target_os = "linux")]
    {
        // CPU usage from loadavg (1 min)
        let mut load = [0.0; 1];
        // SAFETY: `load` is a one-element array and we ask for at most one
        // sample, so getloadavg never writes past the buffer.
        if unsafe { libc::getloadavg(load.as_mut_ptr(), 1) } != -1 {
            // Normalize by number of cores
            // SAFETY: sysconf only reads its integer argument and has no
            // memory-safety preconditions.
            let cores = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) } as f64;
            cpu = (load[0] / cores).clamp(0.0, 1.0);
        }

        // RAM usage from /proc/meminfo
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            let mut total = 0.0;
            let mut free = 0.0;
            let mut available = 0.0;
            for line in meminfo.lines() {
                if line.starts_with("MemTotal:") {
                    total = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                } else if line.starts_with("MemFree:") {
                    free = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                } else if line.starts_with("MemAvailable:") {
                    available = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                }
            }
            if total > 0.0 {
                let used = if available > 0.0 {
                    total - available
                } else {
                    total - free
                };
                ram = (used / total).clamp(0.0, 1.0);
            }
        }
    }

    (cpu, ram)
}

pub async fn fetch_weather(client: &reqwest::Client) -> anyhow::Result<WeatherData> {
    let loc: serde_json::Value = client
        .get("https://ipapi.co/json/")
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .json()
        .await
        .unwrap_or(serde_json::json!({
            "latitude": 43.65,
            "longitude": -79.38,
            "city": "Toronto"
        }));
    let lat = loc["latitude"].as_f64().unwrap_or(43.65);
    let lon = loc["longitude"].as_f64().unwrap_or(-79.38);
    let city = loc["city"].as_str().unwrap_or("").to_string();

    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&current=temperature_2m,weathercode&daily=temperature_2m_max,temperature_2m_min&hourly=temperature_2m&timezone=auto"
    );
    let data: serde_json::Value = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .json()
        .await?;

    let current = &data["current"];
    let temp_c = current["temperature_2m"].as_f64().unwrap_or(0.0) as i32;
    let code = current["weathercode"].as_i64().unwrap_or(0) as i32;
    let (emoji, description) = wmo_description(code);

    let daily = &data["daily"];
    let dates = daily["time"].as_array();
    let highs = daily["temperature_2m_max"].as_array();
    let lows = daily["temperature_2m_min"].as_array();

    let mut forecast = Vec::new();
    if let (Some(d), Some(h), Some(l)) = (dates, highs, lows) {
        for (i, date_val) in d.iter().enumerate().take(5.min(d.len())) {
            let date = date_val.as_str().unwrap_or("").to_string();
            let date_short = if date.len() >= 5 {
                date[date.len() - 5..].to_string()
            } else {
                date
            };
            let high = h.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
            let low = l.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
            forecast.push((date_short, high, low));
        }
    }

    // Parse hourly temperature data
    let hourly_temps: Vec<f64> = data["hourly"]["temperature_2m"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_default();

    Ok(WeatherData {
        temp_c,
        description,
        emoji,
        city,
        forecast,
        hourly_temps,
    })
}

fn wmo_description(code: i32) -> (String, String) {
    match code {
        0 => ("☀".into(), "Clear".into()),
        1 => ("🌤".into(), "Mostly Clear".into()),
        2 => ("⛅".into(), "Partly Cloudy".into()),
        3 => ("☁".into(), "Overcast".into()),
        45 | 48 => ("🌫".into(), "Fog".into()),
        51..=55 => ("🌦".into(), "Drizzle".into()),
        56 | 57 => ("🌧".into(), "Freezing Drizzle".into()),
        61..=65 => ("🌧".into(), "Rain".into()),
        66 | 67 => ("🌧".into(), "Freezing Rain".into()),
        71..=77 => ("❄".into(), "Snow".into()),
        80..=82 => ("🌦".into(), "Showers".into()),
        85 | 86 => ("🌨".into(), "Snow Showers".into()),
        95..=99 => ("⛈".into(), "Thunderstorm".into()),
        _ => ("🌡".into(), "Unknown".into()),
    }
}

pub async fn fetch_sessions(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Vec<SessionInfo>, String> {
    let url = format!("{daemon_url}/session/list");
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
struct SessionResumeResponse {
    messages: Vec<SessionMessage>,
}

#[derive(Debug, Deserialize)]
struct SessionMessage {
    role: String,
    content: String,
    timestamp: Option<String>,
}

pub async fn fetch_session(
    client: &reqwest::Client,
    daemon_url: &str,
    id: &str,
) -> Result<Vec<ChatMessage>, String> {
    let url = format!("{daemon_url}/session/{id}");
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    let body: SessionResumeResponse = response.json().await.map_err(|e| e.to_string())?;
    Ok(body
        .messages
        .into_iter()
        .map(|msg| ChatMessage {
            role: msg.role,
            content: msg.content,
            timestamp: msg.timestamp.unwrap_or_else(format_timestamp),
        })
        .collect())
}

pub async fn save_session(
    client: &reqwest::Client,
    daemon_url: &str,
    session_id: Option<&str>,
    name: Option<&str>,
    messages: &[ChatMessage],
) -> Result<(), String> {
    let url = format!("{daemon_url}/session/save");
    let payload = serde_json::json!({
        "name": name,
        "session_id": session_id,
        "messages": messages,
    });
    let response = client
        .post(&url)
        .json(&payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    Ok(())
}

pub async fn fork_session(
    client: &reqwest::Client,
    daemon_url: &str,
    id: Option<&str>,
) -> Result<String, String> {
    let url = format!("{daemon_url}/session/fork");
    let payload = serde_json::json!({ "id": id });
    let response = client
        .post(&url)
        .json(&payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }

    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    let new_id = body
        .get("id")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("session_id").and_then(|v| v.as_str()))
        .or_else(|| body.get("fork_id").and_then(|v| v.as_str()))
        .unwrap_or("unknown");
    Ok(new_id.to_string())
}

pub async fn export_session(
    client: &reqwest::Client,
    daemon_url: &str,
    id: &str,
) -> Result<String, String> {
    let primary = format!("{daemon_url}/session/export/{id}");
    let fallback = format!("{daemon_url}/session/{id}/export");

    for url in [&primary, &fallback] {
        let response = client
            .get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status().is_success() {
            return response.text().await.map_err(|e| e.to_string());
        }
    }

    Err("failed to export session".to_string())
}

pub async fn store_memory(
    client: &reqwest::Client,
    daemon_url: &str,
    text: &str,
) -> Result<(), String> {
    let url = format!("{daemon_url}/memory/remember");
    let payload = serde_json::json!({ "content": text });
    let response = client
        .post(&url)
        .json(&payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    Ok(())
}

pub async fn recall_memory(
    client: &reqwest::Client,
    daemon_url: &str,
    query: &str,
) -> Result<Vec<MemoryResult>, String> {
    let url = format!("{daemon_url}/memory/recall");
    let payload = serde_json::json!({ "query": query });
    let response = client
        .post(&url)
        .json(&payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn browse_memory(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Vec<MemoryResult>, String> {
    let browse_url = format!("{daemon_url}/memory/browse");
    let browse_response = client
        .get(&browse_url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if browse_response.status().is_success() {
        return browse_response.json().await.map_err(|e| e.to_string());
    }

    recall_memory(client, daemon_url, "").await
}

pub async fn fetch_models(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Vec<ModelInfo>, String> {
    let url = format!("{daemon_url}/models/list");
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn set_model(client: &reqwest::Client, daemon_url: &str, id: &str) -> Result<(), String> {
    let url = format!("{daemon_url}/models/default");
    let payload = serde_json::json!({ "model_id": id });
    let response = client
        .put(&url)
        .json(&payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    Ok(())
}

pub async fn web_search(
    client: &reqwest::Client,
    daemon_url: &str,
    query: &str,
) -> Result<Vec<SearchResult>, String> {
    let url = format!("{daemon_url}/search/web");
    let payload = serde_json::json!({ "query": query });
    let response = client
        .post(&url)
        .json(&payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn ai_search(
    client: &reqwest::Client,
    daemon_url: &str,
    query: &str,
) -> Result<Vec<SearchResult>, String> {
    let url = format!("{daemon_url}/search/ai");
    let payload = serde_json::json!({ "query": query });
    let response = client
        .post(&url)
        .json(&payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn fetch_profile(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<TrainerProfile, String> {
    let url = format!("{daemon_url}/user/trainer");
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn fetch_party(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Vec<AgentInfo>, String> {
    let url = format!("{daemon_url}/user/party");
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn validate_command(
    client: &reqwest::Client,
    daemon_url: &str,
    command: &str,
) -> Result<ValidationResult, String> {
    let url = format!("{daemon_url}/validation/command");
    let payload = serde_json::json!({ "command": command });
    let response = client
        .post(&url)
        .json(&payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn run_gym(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<ValidationResult, String> {
    let url = format!("{daemon_url}/validation/run");
    let response = client
        .post(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn fetch_tools(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Vec<ToolInfo>, String> {
    let url = format!("{daemon_url}/tools/list");
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn fetch_file_tree(
    client: &reqwest::Client,
    daemon_url: &str,
    path: Option<&str>,
) -> Result<FileTreeEntry, String> {
    let request = client.get(format!("{daemon_url}/project/tree"));
    let request = if let Some(path) = path {
        request.query(&[("path", path)])
    } else {
        request
    };
    let resp = request.send().await.map_err(|e| e.to_string())?;
    resp.json::<FileTreeEntry>()
        .await
        .map_err(|e| e.to_string())
}

pub async fn fetch_project_info(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<ProjectInfo, String> {
    let resp = client
        .get(format!("{daemon_url}/project/info"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json::<ProjectInfo>().await.map_err(|e| e.to_string())
}

pub async fn switch_project(
    client: &reqwest::Client,
    daemon_url: &str,
    path: &str,
) -> Result<(), String> {
    let resp = client
        .post(format!("{daemon_url}/project/switch"))
        .json(&serde_json::json!({ "path": path }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Failed to switch project: {}", resp.status()))
    }
}

pub async fn create_plan(
    client: &reqwest::Client,
    daemon_url: &str,
    objective: &str,
) -> Result<Plan, String> {
    let resp = client
        .post(format!("{daemon_url}/planner/create"))
        .json(&serde_json::json!({ "objective": objective }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json::<Plan>().await.map_err(|e| e.to_string())
}

pub async fn fetch_current_plan(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Plan, String> {
    let resp = client
        .get(format!("{daemon_url}/planner/current"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json::<Plan>().await.map_err(|e| e.to_string())
}

pub async fn advance_plan(client: &reqwest::Client, daemon_url: &str) -> Result<(), String> {
    let resp = client
        .post(format!("{daemon_url}/planner/current/advance"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Failed: {}", resp.status()))
    }
}

pub async fn fetch_kanban(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<KanbanBoard, String> {
    let resp = client
        .get(format!("{daemon_url}/user/kanban"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json::<KanbanBoard>().await.map_err(|e| e.to_string())
}

pub async fn add_kanban_task(
    client: &reqwest::Client,
    daemon_url: &str,
    title: &str,
) -> Result<(), String> {
    let resp = client
        .post(format!("{daemon_url}/user/kanban/tasks"))
        .json(&serde_json::json!({ "title": title }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Failed: {}", resp.status()))
    }
}

pub async fn complete_kanban_task(
    client: &reqwest::Client,
    daemon_url: &str,
    id: &str,
) -> Result<(), String> {
    let resp = client
        .put(format!("{daemon_url}/user/kanban/tasks/{id}"))
        .json(&serde_json::json!({ "status": "done" }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Failed: {}", resp.status()))
    }
}

pub async fn fetch_async_tasks(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Vec<AsyncTaskInfo>, String> {
    let resp = client
        .get(format!("{daemon_url}/tasks"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn fetch_async_task_detail(
    client: &reqwest::Client,
    daemon_url: &str,
    id: &str,
) -> Result<AsyncTaskInfo, String> {
    let resp = client
        .get(format!("{daemon_url}/tasks/{id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn cancel_async_task(
    client: &reqwest::Client,
    daemon_url: &str,
    id: &str,
) -> Result<(), String> {
    let resp = client
        .delete(format!("{daemon_url}/tasks/{id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Failed: {}", resp.status()))
    }
}

pub async fn fetch_cron_jobs(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Vec<CronJob>, String> {
    let resp = client
        .get(format!("{daemon_url}/cron"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn trigger_cron_job(
    client: &reqwest::Client,
    daemon_url: &str,
    id: &str,
) -> Result<(), String> {
    let resp = client
        .post(format!("{daemon_url}/cron/{id}/trigger"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Failed: {}", resp.status()))
    }
}

pub async fn toggle_cron_job(
    client: &reqwest::Client,
    daemon_url: &str,
    id: &str,
) -> Result<(), String> {
    let resp = client
        .post(format!("{daemon_url}/cron/{id}/toggle"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Failed: {}", resp.status()))
    }
}

pub async fn fetch_tool_details(
    client: &reqwest::Client,
    daemon_url: &str,
    name: &str,
) -> Result<ToolInfo, String> {
    let url = format!("{daemon_url}/tools/{name}");
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn fetch_mcp_status(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Vec<McpServerInfo>, String> {
    let url = format!("{daemon_url}/mcp/status");
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn fetch_mcp_tools(
    client: &reqwest::Client,
    daemon_url: &str,
) -> Result<Vec<ToolInfo>, String> {
    let url = format!("{daemon_url}/mcp/tools");
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

impl PorygonTui {
    const PERF_HISTORY_LIMIT: usize = 512;

    fn push_metric(history: &mut Vec<u64>, value: u64) {
        history.push(value);
        if history.len() > Self::PERF_HISTORY_LIMIT {
            let overflow = history.len() - Self::PERF_HISTORY_LIMIT;
            history.drain(0..overflow);
        }
    }

    fn extend_metrics(history: &mut Vec<u64>, values: &[u64]) {
        history.extend_from_slice(values);
        if history.len() > Self::PERF_HISTORY_LIMIT {
            let overflow = history.len() - Self::PERF_HISTORY_LIMIT;
            history.drain(0..overflow);
        }
    }

    fn percentile(samples: &[u64], q: f64) -> Option<u64> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
        Some(sorted[idx])
    }

    fn stream_perf_report(&self) -> String {
        let ttft_p95 = Self::percentile(&self.stream_ttft_history_ms, 0.95)
            .map(|v| format!("{v}ms"))
            .unwrap_or_else(|| "n/a".to_string());
        let ttft_avg = if self.stream_ttft_history_ms.is_empty() {
            "n/a".to_string()
        } else {
            let total: u64 = self.stream_ttft_history_ms.iter().sum();
            format!("{}ms", total / self.stream_ttft_history_ms.len() as u64)
        };

        let append_p95 = Self::percentile(&self.append_latency_history_ms, 0.95)
            .map(|v| format!("{v}ms"))
            .unwrap_or_else(|| "n/a".to_string());
        let append_avg = if self.append_latency_history_ms.is_empty() {
            "n/a".to_string()
        } else {
            let total: u64 = self.append_latency_history_ms.iter().sum();
            format!("{}ms", total / self.append_latency_history_ms.len() as u64)
        };

        let cancel_p95 = Self::percentile(&self.cancel_visual_stop_history_ms, 0.95)
            .map(|v| format!("{v}ms"))
            .unwrap_or_else(|| "n/a".to_string());
        let cancel_avg = if self.cancel_visual_stop_history_ms.is_empty() {
            "n/a".to_string()
        } else {
            let total: u64 = self.cancel_visual_stop_history_ms.iter().sum();
            format!(
                "{}ms",
                total / self.cancel_visual_stop_history_ms.len() as u64
            )
        };

        let last = self
            .last_stream_perf
            .as_ref()
            .map(|perf| {
                format!(
                    "last stream: ttft={}ms total={}ms tokens={} append_p95={} append_avg={} samples={}",
                    perf.ttft_ms,
                    perf.total_ms,
                    perf.token_count,
                    perf.append_p95_ms
                        .map(|v| format!("{v}ms"))
                        .unwrap_or_else(|| "n/a".to_string()),
                    perf.append_avg_ms
                        .map(|v| format!("{v}ms"))
                        .unwrap_or_else(|| "n/a".to_string()),
                    perf.append_sample_count
                )
            })
            .unwrap_or_else(|| "last stream: n/a".to_string());

        format!(
            "**Streaming Perf Metrics**\n\n\
             - stream samples: {}\n\
             - ttft p95: {} (avg {})\n\
             - append latency samples: {}\n\
             - append latency p95: {} (avg {})\n\
             - cancel→visual-stop samples: {}\n\
             - cancel→visual-stop p95: {} (avg {})\n\
             - {}\n\n\
             Targets: ttft p95 < 200ms, append p95 < 50ms, cancel p95 < 150ms.",
            self.stream_ttft_history_ms.len(),
            ttft_p95,
            ttft_avg,
            self.append_latency_history_ms.len(),
            append_p95,
            append_avg,
            self.cancel_visual_stop_history_ms.len(),
            cancel_p95,
            cancel_avg,
            last
        )
    }

    fn benchmark_virtual_scroll(&self) -> String {
        let counts = [1_000usize, 5_000usize, 10_000usize];
        let viewport = (self.terminal_height as f32 - 7.0).max(1.0);
        let mut lines = vec!["**Virtual Scroll Benchmark**".to_string()];

        for &count in &counts {
            let mut scroll = ScrollView::new().virtual_scroll(true).item_height(4.0);
            scroll.set_content_height(count as f32 * 4.0 + 4.0);
            scroll.set_viewport_height(viewport);

            let start = Instant::now();
            let max_y = (count as f32 * 4.0 - viewport).max(0.0);
            let mut rendered = 0usize;

            for step in 0..400 {
                let ratio = step as f32 / 399.0;
                scroll.scroll_to(max_y * ratio);
                if let Some((start_idx, end_idx)) = scroll.visible_child_range(count) {
                    rendered += end_idx.saturating_sub(start_idx);
                } else {
                    rendered += count;
                }
            }

            let elapsed = start.elapsed().as_millis() as u64;
            let avg_window = rendered as f64 / 400.0;
            lines.push(format!(
                "- n={count}: {elapsed}ms over 400 window computations (avg visible {:.1} items)",
                avg_window
            ));
        }

        lines.push(
            "Target check: virtual window computation should stay low-latency under 10k items."
                .to_string(),
        );
        lines.join("\n")
    }

    fn benchmark_diff_widget(&self) -> String {
        let mut diff = String::from("--- a/file.txt\n+++ b/file.txt\n@@ -1,1000 +1,1000 @@\n");
        for i in 0..1_000 {
            if i % 2 == 0 {
                diff.push_str(&format!("-old line {i}\n+new line {i}\n"));
            } else {
                diff.push_str(&format!(" context line {i}\n"));
            }
        }

        let parse_start = Instant::now();
        let lines = parse_unified_diff(&diff);
        let parse_ms = parse_start.elapsed().as_millis() as u64;

        let style_start = Instant::now();
        let mut added = 0usize;
        let mut removed = 0usize;
        for line in &lines {
            if line.kind == DiffKind::Added {
                added += 1;
            } else if line.kind == DiffKind::Removed {
                removed += 1;
            }
        }
        let style_ms = style_start.elapsed().as_millis() as u64;

        format!(
            "**Diff Benchmark (1000 lines)**\n\n- parse: {}ms\n- classify/style prep: {}ms\n- total lines: {}\n- added: {}\n- removed: {}\n\nTarget check: native render path should remain under ~100ms for this scale.",
            parse_ms,
            style_ms,
            lines.len(),
            added,
            removed
        )
    }

    pub(crate) fn begin_stream_metrics(&mut self) {
        let now = Instant::now();
        self.stream_started_at = Some(now);
        self.first_token_at = None;
        self.last_token_at = None;
        self.append_latencies_ms.clear();
        self.last_stream_perf = None;
    }

    fn note_stream_token_metric(&mut self) {
        let now = Instant::now();
        if self.first_token_at.is_none() {
            self.first_token_at = Some(now);
        }
        if let Some(previous) = self.last_token_at {
            let delta = now.duration_since(previous).as_millis() as u64;
            self.append_latencies_ms.push(delta);
        }
        self.last_token_at = Some(now);
    }

    pub(crate) fn finish_stream_metrics(&mut self) -> Option<StreamPerfSummary> {
        let started = self.stream_started_at.take()?;
        let finished = Instant::now();
        let total_ms = finished.duration_since(started).as_millis() as u64;
        let ttft_ms = self
            .first_token_at
            .map(|first| first.duration_since(started).as_millis() as u64)
            .unwrap_or(total_ms);

        let mut append_latencies = std::mem::take(&mut self.append_latencies_ms);
        append_latencies.sort_unstable();
        let append_sample_count = append_latencies.len();
        let append_p95_ms = if append_latencies.is_empty() {
            None
        } else {
            let idx = ((append_latencies.len() - 1) as f64 * 0.95).round() as usize;
            Some(append_latencies[idx])
        };
        let append_avg_ms = if append_latencies.is_empty() {
            None
        } else {
            let total: u64 = append_latencies.iter().sum();
            Some(total / append_latencies.len() as u64)
        };

        let summary = StreamPerfSummary {
            ttft_ms,
            total_ms,
            token_count: self.stream_token_count,
            append_sample_count,
            append_p95_ms,
            append_avg_ms,
        };

        Self::push_metric(&mut self.stream_ttft_history_ms, ttft_ms);
        Self::extend_metrics(&mut self.append_latency_history_ms, &append_latencies);

        self.first_token_at = None;
        self.last_token_at = None;
        self.last_stream_perf = Some(summary.clone());

        Some(summary)
    }

    pub(crate) fn handle_update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Init => {
                if self.started {
                    Cmd::none()
                } else {
                    self.started = true;
                    let mut cmds = vec![Cmd::message(Msg::PollConnection)];
                    if self.weather_enabled {
                        cmds.push(Cmd::message(Msg::PollWeather));
                    }
                    Cmd::batch(cmds)
                }
            }
            Msg::SendMessage => {
                let text = self.input.current_value().to_string();
                if text.trim().is_empty() {
                    return Cmd::none();
                }
                if text.starts_with('/') {
                    if let Some(action) = self.command_registry.parse(&text) {
                        self.input.submit_value();
                        self.suggestions.clear();
                        return self.handle_command_action(action);
                    }
                    return Cmd::ShowToast(Toast::warning("Unknown command; draft preserved."));
                }
                self.start_native_turn()
            }
            Msg::PollConnection => {
                let Some(connection) = self.connection.clone() else {
                    return Cmd::none();
                };
                Cmd::spawn(async move { Msg::ConnectionChecked(connection.check().await) })
            }
            Msg::PollStatus => {
                if !self.connected {
                    return Cmd::none();
                }
                let url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_status(&client, &url).await {
                        Ok(status) => Msg::StatusUpdated(status),
                        Err(_) => {
                            let (cpu, ram) = get_system_stats();
                            Msg::StatusUpdated(AgentStatus {
                                cpu_usage: cpu,
                                ram_usage: ram,
                                ..AgentStatus::default()
                            })
                        }
                    }
                })
            }
            Msg::PollApprovals => {
                if !self.connected || self.waiting_response || self.pending_approval.is_some() {
                    return Cmd::none();
                }
                let Some(connection) = self.connection.clone() else {
                    return Cmd::none();
                };
                Cmd::spawn(async move {
                    match crate::approval_client::fetch_pending(
                        &connection.client,
                        &connection.base_url,
                        20,
                    )
                    .await
                    {
                        Ok(list) => match list.items.into_iter().next() {
                            Some(item) => Msg::ApprovalReceived(
                                ApprovalRequest {
                                    id: item.id,
                                    tool_name: item
                                        .context
                                        .tool_name
                                        .clone()
                                        .unwrap_or(item.source),
                                    params: serde_json::to_value(&item.context).unwrap_or_default(),
                                    risk_score: -1.0,
                                    reasoning: item.objective,
                                    blast_radius: "unknown".into(),
                                    reversible: None,
                                },
                                false,
                            ),
                            None => Msg::NoApprovalsPending,
                        },
                        Err(error) => Msg::ApprovalLoadFailed(error.to_string()),
                    }
                })
            }
            Msg::StreamToken(token) => {
                self.note_stream_token_metric();
                self.streaming_content.push_str(&token);
                self.stream_token_count += 1;

                if let Some(last) = self
                    .current_response
                    .and_then(|index| self.messages.get_mut(index))
                {
                    if last.role == "assistant" {
                        last.content.clone_from(&self.streaming_content);
                    }
                }

                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::NativeEvent(id, epoch, event) => {
                if epoch != self.run_epoch {
                    return Cmd::none();
                }
                self.native_event(id, event)
            }
            Msg::InspectRun => self.inspect_run(false),
            Msg::RunObserved(id, epoch, result) => {
                if self.waiting_response || epoch != self.run_epoch {
                    Cmd::none()
                } else {
                    self.observe_run(id, result)
                }
            }
            Msg::StopObserved(id, epoch, result) => {
                if epoch != self.run_epoch {
                    return Cmd::none();
                }
                self.observe_run(id, result)
            }
            Msg::AbortStream => self.stop_native_run(),
            Msg::ConnectionChecked(result) => {
                let was_connected = self.connected;
                self.connected = result.is_ok();
                if let Err(error) = result {
                    self.error = Some(error);
                }
                if self.connected && !was_connected {
                    if self.current_run.is_some() {
                        return Cmd::message(Msg::InspectRun);
                    }
                    self.status.step = "connected".into();
                    Cmd::ShowToast(Toast::success("Authenticated daemon connection"))
                } else {
                    Cmd::none()
                }
            }
            Msg::StatusUpdated(status) => {
                self.status.cost_usd = status.cost_usd;
                self.status.budget_usd = status.budget_usd;
                self.status.pressure = status.pressure;
                self.status.model = status.model;
                self.status.context_quality = status.context_quality;
                self.status.agent_name = status.agent_name;
                self.status.cpu_usage = status.cpu_usage;
                self.status.ram_usage = status.ram_usage;
                if !self.waiting_response && self.current_run.is_none() {
                    self.status.step = status.step;
                }
                Cmd::none()
            }
            Msg::SpinnerTick => {
                self.spinner_frame = (self.spinner_frame + 1) % 10;
                Cmd::none()
            }
            Msg::ClockTick => {
                self.clock_text = format_clock();
                let (cpu, ram) = get_system_stats();
                self.status.cpu_usage = cpu;
                self.status.ram_usage = ram;
                Cmd::none()
            }
            // Weather is opt-in (PORYGON_WEATHER=1): it sends the user's IP to
            // a third-party geolocation service.
            Msg::PollWeather if !self.weather_enabled => Cmd::none(),
            Msg::PollWeather => {
                let client = reqwest::Client::new();
                Cmd::spawn(async move {
                    match fetch_weather(&client).await {
                        Ok(data) => Msg::WeatherFetched(data),
                        Err(_) => Msg::WeatherFailed,
                    }
                })
            }
            Msg::WeatherFetched(data) => {
                self.weather = Some(data);
                Cmd::none()
            }
            Msg::WeatherFailed => Cmd::none(),
            Msg::ApprovalReceived(request, is_stream) => {
                if !is_stream && self.waiting_response {
                    return Cmd::none();
                }
                if self.pending_approval.is_some() {
                    if is_stream {
                        self.queued_stream_approvals.push_back(request);
                    }
                    return Cmd::none();
                }
                self.approval_is_stream = is_stream;
                self.pending_approval = Some(request);
                self.show_approval_modal = true;
                Cmd::none()
            }
            Msg::ApprovalDecision(approved) => self.decide_approval(approved),
            Msg::ApprovalSubmitted(id, result) => {
                if self.pending_approval.as_ref().is_none_or(|r| r.id != id) {
                    return Cmd::none();
                }
                match result {
                    Ok(()) => {
                        self.approval_in_flight = false;
                        self.pending_approval = None;
                        self.show_approval_modal = false;
                        self.approval_is_stream = false;
                        Cmd::ShowToast(Toast::success(
                            "Daemon confirmed the decision; execution may still be pending.",
                        ))
                    }
                    Err(error) => {
                        self.approval_in_flight = true;
                        self.error = Some(error.clone());
                        Cmd::ShowToast(Toast::error(format!(
                            "Decision unconfirmed. R refreshes; Escape closes. {error}"
                        )))
                    }
                }
            }
            Msg::DismissApproval => {
                self.show_approval_modal = false;
                Cmd::none()
            }
            Msg::RefreshApproval => {
                let (Some(request), Some(connection)) =
                    (self.pending_approval.as_ref(), self.connection.clone())
                else {
                    return Cmd::none();
                };
                if self.approval_is_stream {
                    return Cmd::message(Msg::InspectRun);
                }
                let id = request.id.clone();
                Cmd::spawn(async move {
                    let result = crate::approval_client::fetch_pending(
                        &connection.client,
                        &connection.base_url,
                        200,
                    )
                    .await
                    .map(|queue| {
                        queue.items.iter().any(|item| item.id == id)
                            || queue.count > queue.items.len() as u64
                    })
                    .map_err(|e| e.to_string());
                    Msg::ApprovalReconciled(id, result)
                })
            }
            Msg::ApprovalReconciled(id, result) => {
                if self.pending_approval.as_ref().is_none_or(|r| r.id != id) {
                    return Cmd::none();
                }
                match result {
                    Ok(false) => {
                        self.pending_approval = None;
                        self.show_approval_modal = false;
                        self.approval_in_flight = false;
                        Cmd::ShowToast(Toast::warning(
                            "No longer pending. Inspect the run for its outcome.",
                        ))
                    }
                    Ok(true) => Cmd::ShowToast(Toast::warning(
                        "Still pending; an unconfirmed decision cannot be retried safely.",
                    )),
                    Err(error) => Cmd::ShowToast(Toast::error(error)),
                }
            }
            Msg::ApprovalLoadFailed(error) => {
                self.error = Some(error);
                Cmd::none()
            }
            Msg::NoApprovalsPending => Cmd::none(),
            Msg::SessionsFetched(sessions) => {
                let mut table = String::from("**Saved Sessions:**\n\n");
                if sessions.is_empty() {
                    table.push_str("No saved sessions found.");
                } else {
                    table.push_str("| ID | Name | Date | Messages |\n");
                    table.push_str("|---|---|---|---|\n");
                    for session in sessions {
                        let name = session.name.unwrap_or_else(|| "Unnamed".to_string());
                        let date = session.date.unwrap_or_else(|| "Unknown".to_string());
                        let count = session.message_count.unwrap_or(0);
                        table.push_str(&format!(
                            "| {} | {} | {} | {} |\n",
                            session.id, name, date, count
                        ));
                    }
                    table.push_str("\nUse `/resume <id>` to load a session.");
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: table,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::SessionResumed(messages) => {
                if self.run_unresolved || self.pending_approval.is_some() {
                    return Cmd::ShowToast(Toast::warning(
                        "Resolve the current run before loading another session.",
                    ));
                }
                self.current_response = None;
                self.messages = messages;
                self.messages.insert(
                    0,
                    ChatMessage {
                        role: "system".into(),
                        content: "Session resumed.".into(),
                        timestamp: format_timestamp(),
                    },
                );
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Session resumed"))
            }
            Msg::SessionSaved => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Session saved successfully.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Session saved"))
            }
            Msg::SessionForked(new_id) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("Session forked successfully. New ID: `{new_id}`"),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Session forked"))
            }
            Msg::SessionExported(content) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("**Session Export**\n\n```json\n{content}\n```"),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Session exported"))
            }
            Msg::SessionError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("Session error: {err}"),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::error(format!("Session error: {err}")))
            }
            Msg::MemoryStored => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Memory stored successfully.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Memory stored"))
            }
            Msg::MemoryRecalled(results) => {
                if results.is_empty() {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "No matching memories found.".into(),
                        timestamp: format_timestamp(),
                    });
                } else {
                    let mut formatted = String::from("**Memory Results:**\n\n");
                    for result in results {
                        formatted.push_str(&format!("• {}", result.content));
                        if let Some(score) = result.score {
                            formatted.push_str(&format!(" (score: {:.2})", score));
                        }
                        if let Some(timestamp) = result.timestamp {
                            formatted.push_str(&format!(" — {}", timestamp));
                        }
                        formatted.push_str("\n\n");
                    }
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: formatted,
                        timestamp: format_timestamp(),
                    });
                }
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::MemoryBrowsed(results) => {
                let mut content = String::from("**Memory Browser**\n\n");
                if results.is_empty() {
                    content.push_str("No memory records found.");
                } else {
                    for result in results {
                        let score = result
                            .score
                            .map(|v| format!("{v:.2}"))
                            .unwrap_or_else(|| "n/a".to_string());
                        let ts = result.timestamp.unwrap_or_else(|| "unknown".to_string());
                        content.push_str(&format!(
                            "• {}  \n  score: {}  \n  ts: {}\n\n",
                            result.content, score, ts
                        ));
                    }
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::MemoryError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("Memory error: {err}"),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::error(format!("Memory error: {err}")))
            }
            Msg::ToolsFetched(tools) => {
                let mut table = String::from("**Available Tools:**\n\n");
                if tools.is_empty() {
                    table.push_str("No tools available.");
                } else {
                    table.push_str("| Name | Description |\n");
                    table.push_str("|---|---|\n");
                    for tool in tools {
                        let desc = tool.description.unwrap_or_else(|| "N/A".to_string());
                        table.push_str(&format!("| {} | {} |\n", tool.name, desc));
                    }
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: table,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::ToolDetailsFetched(tool) => {
                let params_str = tool
                    .parameters
                    .as_ref()
                    .and_then(|p| serde_json::to_string_pretty(p).ok())
                    .unwrap_or_else(|| "N/A".to_string());
                let desc = tool
                    .description
                    .unwrap_or_else(|| "No description".to_string());
                let content = format!(
                    "**Tool: {}**\n\n{}\n\n**Parameters:**\n```json\n{}\n```",
                    tool.name, desc, params_str
                );
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::McpStatusFetched(servers) => {
                self.mcp_server_count = servers.len();
                self.mcp_healthy_count = servers
                    .iter()
                    .filter(|server| {
                        server
                            .status
                            .as_deref()
                            .map(|status| {
                                matches!(
                                    status.to_ascii_lowercase().as_str(),
                                    "ok" | "healthy" | "connected" | "online"
                                )
                            })
                            .unwrap_or(false)
                    })
                    .count();
                self.mcp_last_error = None;
                let mut table = String::from("**MCP Server Status:**\n\n");
                if servers.is_empty() {
                    table.push_str("No MCP servers configured.");
                } else {
                    table.push_str("| Server | Status | Tools | Last Error |\n");
                    table.push_str("|---|---|---|---|\n");
                    for server in &servers {
                        let status = server.status.as_deref().unwrap_or("unknown").to_string();
                        let tools_count = server.tools_count.unwrap_or(0);
                        let last_error = server.last_error.as_deref().unwrap_or("-");
                        table.push_str(&format!(
                            "| {} | {} | {} | {} |\n",
                            server.name, status, tools_count, last_error
                        ));
                    }
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: table,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::McpToolsFetched(tools) => {
                let mut table = String::from("**MCP Tools:**\n\n");
                if tools.is_empty() {
                    table.push_str("No MCP tools available.");
                } else {
                    table.push_str("| Name | Description |\n");
                    table.push_str("|---|---|\n");
                    for tool in tools {
                        let desc = tool.description.unwrap_or_else(|| "N/A".to_string());
                        table.push_str(&format!("| {} | {} |\n", tool.name, desc));
                    }
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: table,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::McpError(err) => {
                self.mcp_last_error = Some(err.clone());
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("MCP error: {err}"),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::warning(format!("MCP error: {err}")))
            }
            Msg::ToolsError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("Tools error: {err}"),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::error(format!("Tools error: {err}")))
            }
            Msg::FileTreeFetched { path, tree } => {
                let mut formatted = if let Some(path) = path {
                    format!("**Project File Tree: `{path}`**\n\n")
                } else {
                    String::from("**Project File Tree:**\n\n")
                };
                formatted.push_str(&format_file_tree(&tree, "", true));
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: formatted,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::FileTreeError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("File tree error: {err}"),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::error(format!("File tree error: {err}")))
            }
            Msg::ProjectInfoFetched(info) => {
                let language = info.language.unwrap_or_else(|| "Unknown".to_string());
                let framework = info.framework.unwrap_or_else(|| "Unknown".to_string());
                let file_count = info
                    .file_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "Unknown".to_string());
                let content = format!(
                    "**Project:** {}\n**Path:** `{}`\n**Language:** {}\n**Framework:** {}\n**Files:** {}",
                    info.name, info.path, language, framework, file_count
                );
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::ProjectSwitched => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Project switched successfully.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Project switched successfully."))
            }
            Msg::ProjectError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("Project error: {err}"),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::error(format!("Project error: {err}")))
            }
            Msg::PlanCreated(plan) | Msg::PlanFetched(plan) => {
                let mut content = format!("**Plan: {}**\n\n", plan.objective);
                for step in plan.steps {
                    let status_icon = match step.status.as_str() {
                        "done" | "completed" => "✅",
                        "in_progress" | "running" => "🔄",
                        _ => "⏳",
                    };
                    content.push_str(&format!("{status_icon} {}\n", step.description));
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::PlanAdvanced => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Plan advanced to next step.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Plan advanced to next step."))
            }
            Msg::PlanError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: err,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::KanbanFetched(board) => {
                let mut content = format!("**Kanban Board**\n\n**To Do** ({})\n", board.todo.len());
                for task in board.todo {
                    content.push_str(&format!("• [{}] {}\n", task.id, task.title));
                }

                content.push_str(&format!(
                    "\n**In Progress** ({})\n",
                    board.in_progress.len()
                ));
                for task in board.in_progress {
                    content.push_str(&format!("• [{}] {}\n", task.id, task.title));
                }

                content.push_str(&format!("\n**Done** ({})\n", board.done.len()));
                for task in board.done {
                    content.push_str(&format!("• [{}] {}\n", task.id, task.title));
                }

                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::KanbanTaskAdded => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Task added to board.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Task added to board."))
            }
            Msg::KanbanTaskMoved => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Task moved to Done.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Task moved to Done."))
            }
            Msg::KanbanError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: err,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::AsyncTasksFetched(tasks) => {
                let mut content = format!("**Async Tasks** ({})\n\n", tasks.len());
                for task in tasks {
                    let name = task.name.unwrap_or_else(|| "Unnamed".to_string());
                    content.push_str(&format!("• [{}] {} — {}\n", task.id, name, task.status));
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::AsyncTaskDetailFetched(task) => {
                let mut content = format!(
                    "**Async Task**\n\n**ID:** {}\n**Name:** {}\n**Status:** {}",
                    task.id,
                    task.name.unwrap_or_else(|| "Unnamed".to_string()),
                    task.status
                );
                if let Some(result) = task.result {
                    content.push_str(&format!("\n**Result:**\n{result}"));
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::AsyncTaskCancelled => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Task cancelled.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Task cancelled."))
            }
            Msg::AsyncTaskError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: err,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::CronFetched(jobs) => {
                let mut content = format!("**Cron Jobs** ({})\n\n", jobs.len());
                for job in jobs {
                    let name = job.name.unwrap_or_else(|| "Unnamed".to_string());
                    let schedule = job.schedule.unwrap_or_else(|| "N/A".to_string());
                    let enabled_icon = if job.enabled { "✅" } else { "⏸" };
                    let next_run = job.next_run.unwrap_or_else(|| "N/A".to_string());
                    content.push_str(&format!(
                        "• [{}] {} — {} — {} — Next: {}\n",
                        job.id, name, schedule, enabled_icon, next_run
                    ));
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::CronTriggered => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Cron job triggered.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Cron job triggered."))
            }
            Msg::CronToggled => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Cron job toggled.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Cron job toggled."))
            }
            Msg::CronError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: err,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::ModelsFetched(models) => {
                self.model_last_error = None;
                let mut content = format!("**Available Models** ({})\n\n", models.len());
                for model in models {
                    let name = model.name.unwrap_or_else(|| "Unknown".to_string());
                    let provider = model.provider.unwrap_or_else(|| "Unknown".to_string());
                    let context_window = model
                        .context_window
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "Unknown".to_string());
                    content.push_str(&format!(
                        "• **{}** — {} ({}) — Context: {}\n",
                        model.id, name, provider, context_window
                    ));
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::ModelChanged => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Model updated.".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Model updated."))
            }
            Msg::ModelsError(err) => {
                self.model_last_error = Some(err.clone());
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: err,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::VirtualScrollBenchReady(report) | Msg::DiffBenchReady(report) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: report,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::SearchResultsFetched(results) => {
                let mut content = format!("**Search Results** ({})\n\n", results.len());
                for result in results {
                    content.push_str(&format!(
                        "### {}\n{}\n{}\n\n",
                        result.title, result.url, result.snippet
                    ));
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::SearchError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: err,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::ProfileFetched(profile) => {
                let style = profile.style.unwrap_or_else(|| "N/A".to_string());
                let context = profile.context.unwrap_or_else(|| "N/A".to_string());
                let content = format!(
                    "**Trainer Profile**\n\n**Name:** {}\n**XP:** {}\n**Style:** {}\n**Context:** {}",
                    profile.name, profile.xp, style, context
                );
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::PartyFetched(agents) => {
                let mut content = format!("**Agent Party** ({})\n\n", agents.len());
                for agent in agents {
                    let role = agent.role.unwrap_or_else(|| "Unknown".to_string());
                    let status = agent.status.unwrap_or_else(|| "Unknown".to_string());
                    content.push_str(&format!("• **{}** — {} — {}\n", agent.name, role, status));
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::ProfileError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: err,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::ValidationResultFetched(result) => {
                let safe_icon = if result.safe {
                    "✅ Safe"
                } else {
                    "⛔ Blocked"
                };
                let risk_level = result.risk_level.unwrap_or_else(|| "Unknown".to_string());
                let reason = result.reason.unwrap_or_else(|| "N/A".to_string());
                let mut content = format!(
                    "**Validation: {safe_icon}**\n\n**Risk:** {risk_level}\n**Reason:** {reason}"
                );
                if !result.details.is_empty() {
                    content.push_str("\n\n**Details:**\n");
                    for detail in result.details {
                        content.push_str(&format!("• {detail}\n"));
                    }
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::GymResultFetched(result) => {
                let safe_icon = if result.safe {
                    "✅ Safe"
                } else {
                    "⛔ Blocked"
                };
                let risk_level = result.risk_level.unwrap_or_else(|| "Unknown".to_string());
                let reason = result.reason.unwrap_or_else(|| "N/A".to_string());
                let mut content = format!(
                    "**Gym Results: {safe_icon}**\n\n**Risk:** {risk_level}\n**Reason:** {reason}"
                );
                if !result.details.is_empty() {
                    content.push_str("\n\n**Details:**\n");
                    for detail in result.details {
                        content.push_str(&format!("• {detail}\n"));
                    }
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::ValidationError(err) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: err,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::DraftExitWarning => Cmd::ShowToast(Toast::warning(
                "Unsent draft. Press Ctrl+C again to discard it and quit.",
            )),
            Msg::ExitRequested => {
                if self.connected && self.messages.len() > 1 {
                    let daemon_url = self.daemon_url.clone();
                    let client = self.client.clone();
                    let session_id = self.session_id.clone();
                    let messages = self.messages[1..].to_vec();
                    Cmd::batch(vec![
                        Cmd::spawn(async move {
                            let _ = save_session(
                                &client,
                                &daemon_url,
                                session_id.as_deref(),
                                None,
                                &messages,
                            )
                            .await;
                            Msg::SessionSaved
                        }),
                        Cmd::Quit,
                    ])
                } else {
                    Cmd::Quit
                }
            }
        }
    }

    fn handle_command_action(&mut self, action: CommandAction) -> Cmd<Msg> {
        match action {
            CommandAction::SystemMessage(text) => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: text,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            CommandAction::ShowToast(text) => {
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::info(text))
            }
            CommandAction::DaemonRequest { .. } => {
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Daemon commands not yet wired.".to_string(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            CommandAction::Batch(actions) => {
                let cmds: Vec<Cmd<Msg>> = actions
                    .into_iter()
                    .map(|action| self.handle_command_action(action))
                    .collect();
                Cmd::batch(cmds)
            }
            CommandAction::ClearChat => {
                self.current_response = None;
                self.messages.truncate(1);
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::success("Chat cleared"))
            }
            CommandAction::FetchSessions => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch sessions.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_sessions(&client, &daemon_url).await {
                        Ok(sessions) => Msg::SessionsFetched(sessions),
                        Err(err) => Msg::SessionError(err),
                    }
                })
            }
            CommandAction::ResumeSession { id } => {
                if self.run_unresolved || self.pending_approval.is_some() {
                    return Cmd::ShowToast(Toast::warning(
                        "Resolve the current run before loading another session.",
                    ));
                }
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot resume session.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_session(&client, &daemon_url, &id).await {
                        Ok(messages) => Msg::SessionResumed(messages),
                        Err(err) => Msg::SessionError(err),
                    }
                })
            }
            CommandAction::SaveSession { name } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot save session.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                let session_id = self.session_id.clone();
                let messages = self.messages[1..].to_vec();
                Cmd::spawn(async move {
                    match save_session(
                        &client,
                        &daemon_url,
                        session_id.as_deref(),
                        name.as_deref(),
                        &messages,
                    )
                    .await
                    {
                        Ok(()) => Msg::SessionSaved,
                        Err(err) => Msg::SessionError(err),
                    }
                })
            }
            CommandAction::ForkSession { id } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fork session.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fork_session(&client, &daemon_url, id.as_deref()).await {
                        Ok(new_id) => Msg::SessionForked(new_id),
                        Err(err) => Msg::SessionError(err),
                    }
                })
            }
            CommandAction::ExportSession { id } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot export session.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match export_session(&client, &daemon_url, &id).await {
                        Ok(content) => Msg::SessionExported(content),
                        Err(err) => Msg::SessionError(err),
                    }
                })
            }
            CommandAction::StoreMemory { text } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot store memory.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match store_memory(&client, &daemon_url, &text).await {
                        Ok(()) => Msg::MemoryStored,
                        Err(err) => Msg::MemoryError(err),
                    }
                })
            }
            CommandAction::RecallMemory { query } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot recall memories.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match recall_memory(&client, &daemon_url, &query).await {
                        Ok(results) => Msg::MemoryRecalled(results),
                        Err(err) => Msg::MemoryError(err),
                    }
                })
            }
            CommandAction::FetchMemoryBrowse => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot browse memories.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: "Loading memory browser...".into(),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match browse_memory(&client, &daemon_url).await {
                        Ok(results) => Msg::MemoryBrowsed(results),
                        Err(err) => Msg::MemoryError(err),
                    }
                })
            }
            CommandAction::FetchModels => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch models.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_models(&client, &daemon_url).await {
                        Ok(models) => Msg::ModelsFetched(models),
                        Err(err) => Msg::ModelsError(err),
                    }
                })
            }
            CommandAction::SetModel { id } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot set model.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match set_model(&client, &daemon_url, &id).await {
                        Ok(()) => Msg::ModelChanged,
                        Err(err) => Msg::ModelsError(err),
                    }
                })
            }
            CommandAction::WebSearch { query } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot run search.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match web_search(&client, &daemon_url, &query).await {
                        Ok(results) => Msg::SearchResultsFetched(results),
                        Err(err) => Msg::SearchError(err),
                    }
                })
            }
            CommandAction::AiSearch { query } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot run AI search.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match ai_search(&client, &daemon_url, &query).await {
                        Ok(results) => Msg::SearchResultsFetched(results),
                        Err(err) => Msg::SearchError(err),
                    }
                })
            }
            CommandAction::FetchProfile => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch profile.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_profile(&client, &daemon_url).await {
                        Ok(profile) => Msg::ProfileFetched(profile),
                        Err(err) => Msg::ProfileError(err),
                    }
                })
            }
            CommandAction::FetchParty => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch party.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_party(&client, &daemon_url).await {
                        Ok(agents) => Msg::PartyFetched(agents),
                        Err(err) => Msg::ProfileError(err),
                    }
                })
            }
            CommandAction::ValidateCommand { command } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot validate command.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match validate_command(&client, &daemon_url, &command).await {
                        Ok(result) => Msg::ValidationResultFetched(result),
                        Err(err) => Msg::ValidationError(err),
                    }
                })
            }
            CommandAction::RunGym => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot run gym.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match run_gym(&client, &daemon_url).await {
                        Ok(result) => Msg::GymResultFetched(result),
                        Err(err) => Msg::ValidationError(err),
                    }
                })
            }
            CommandAction::ShowPerfMetrics => {
                let report = self.stream_perf_report();
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: report,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            CommandAction::BenchmarkVirtualScroll => Cmd::message(Msg::VirtualScrollBenchReady(
                self.benchmark_virtual_scroll(),
            )),
            CommandAction::BenchmarkDiffWidget => {
                Cmd::message(Msg::DiffBenchReady(self.benchmark_diff_widget()))
            }
            CommandAction::ReconnectRuntime => Cmd::batch(vec![
                Cmd::message(Msg::PollConnection),
                Cmd::message(Msg::InspectRun),
            ]),
            CommandAction::ShowApprovals => {
                if self.pending_approval.is_some() {
                    self.show_approval_modal = true;
                    Cmd::none()
                } else {
                    Cmd::message(Msg::PollApprovals)
                }
            }
            CommandAction::AttachRun { id } => {
                if self.pending_approval.is_some() || self.approval_in_flight {
                    return Cmd::ShowToast(Toast::warning(
                        "Resolve or refresh the pending approval before selecting another run.",
                    ));
                }
                if self.waiting_response {
                    return Cmd::ShowToast(Toast::warning(
                        "Stop the active stream before selecting another run.",
                    ));
                }
                self.current_run = Some(id);
                self.run_epoch = self.run_epoch.wrapping_add(1);
                self.current_response = None;
                self.run_can_continue = false;
                self.conversation_id = None;
                self.run_unresolved = true;
                self.approval_is_stream = false;
                self.pending_approval = None;
                self.show_approval_modal = false;
                self.inspect_run(false)
            }
            CommandAction::FetchTools => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch tools.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_tools(&client, &daemon_url).await {
                        Ok(tools) => Msg::ToolsFetched(tools),
                        Err(err) => Msg::ToolsError(err),
                    }
                })
            }
            CommandAction::FetchToolDetails { name } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch tool details.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_tool_details(&client, &daemon_url, &name).await {
                        Ok(tool) => Msg::ToolDetailsFetched(tool),
                        Err(err) => Msg::ToolsError(err),
                    }
                })
            }
            CommandAction::FetchMcpStatus => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch MCP status.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_mcp_status(&client, &daemon_url).await {
                        Ok(servers) => Msg::McpStatusFetched(servers),
                        Err(err) => Msg::McpError(err),
                    }
                })
            }
            CommandAction::FetchMcpTools => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch MCP tools.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_mcp_tools(&client, &daemon_url).await {
                        Ok(tools) => Msg::McpToolsFetched(tools),
                        Err(err) => Msg::McpError(err),
                    }
                })
            }
            CommandAction::FetchFileTree { path } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch file tree.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                let requested_path = path.clone();
                Cmd::spawn(async move {
                    match fetch_file_tree(&client, &daemon_url, requested_path.as_deref()).await {
                        Ok(tree) => Msg::FileTreeFetched {
                            path: requested_path,
                            tree,
                        },
                        Err(err) => Msg::FileTreeError(err),
                    }
                })
            }
            CommandAction::FetchProjectInfo => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch project info.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_project_info(&client, &daemon_url).await {
                        Ok(info) => Msg::ProjectInfoFetched(info),
                        Err(err) => Msg::ProjectError(err),
                    }
                })
            }
            CommandAction::SwitchProject { path } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot switch project.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match switch_project(&client, &daemon_url, &path).await {
                        Ok(()) => Msg::ProjectSwitched,
                        Err(err) => Msg::ProjectError(err),
                    }
                })
            }
            CommandAction::CreatePlan { objective } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot create plan.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match create_plan(&client, &daemon_url, &objective).await {
                        Ok(plan) => Msg::PlanCreated(plan),
                        Err(err) => Msg::PlanError(err),
                    }
                })
            }
            CommandAction::FetchCurrentPlan => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch current plan.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_current_plan(&client, &daemon_url).await {
                        Ok(plan) => Msg::PlanFetched(plan),
                        Err(err) => Msg::PlanError(err),
                    }
                })
            }
            CommandAction::AdvancePlan => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot advance plan.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match advance_plan(&client, &daemon_url).await {
                        Ok(()) => Msg::PlanAdvanced,
                        Err(err) => Msg::PlanError(err),
                    }
                })
            }
            CommandAction::FetchKanban => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch kanban board.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_kanban(&client, &daemon_url).await {
                        Ok(board) => Msg::KanbanFetched(board),
                        Err(err) => Msg::KanbanError(err),
                    }
                })
            }
            CommandAction::AddKanbanTask { title } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot add kanban task.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match add_kanban_task(&client, &daemon_url, &title).await {
                        Ok(()) => Msg::KanbanTaskAdded,
                        Err(err) => Msg::KanbanError(err),
                    }
                })
            }
            CommandAction::CompleteKanbanTask { id } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot complete kanban task.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match complete_kanban_task(&client, &daemon_url, &id).await {
                        Ok(()) => Msg::KanbanTaskMoved,
                        Err(err) => Msg::KanbanError(err),
                    }
                })
            }
            CommandAction::FetchAsyncTasks => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch async tasks.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_async_tasks(&client, &daemon_url).await {
                        Ok(tasks) => Msg::AsyncTasksFetched(tasks),
                        Err(err) => Msg::AsyncTaskError(err),
                    }
                })
            }
            CommandAction::FetchAsyncTaskDetail { id } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch async task detail.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_async_task_detail(&client, &daemon_url, &id).await {
                        Ok(task) => Msg::AsyncTaskDetailFetched(task),
                        Err(err) => Msg::AsyncTaskError(err),
                    }
                })
            }
            CommandAction::CancelAsyncTask { id } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot cancel async task.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match cancel_async_task(&client, &daemon_url, &id).await {
                        Ok(()) => Msg::AsyncTaskCancelled,
                        Err(err) => Msg::AsyncTaskError(err),
                    }
                })
            }
            CommandAction::FetchCronJobs => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot fetch cron jobs.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_cron_jobs(&client, &daemon_url).await {
                        Ok(jobs) => Msg::CronFetched(jobs),
                        Err(err) => Msg::CronError(err),
                    }
                })
            }
            CommandAction::TriggerCronJob { id } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot trigger cron job.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match trigger_cron_job(&client, &daemon_url, &id).await {
                        Ok(()) => Msg::CronTriggered,
                        Err(err) => Msg::CronError(err),
                    }
                })
            }
            CommandAction::ToggleCronJob { id } => {
                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Not connected to daemon. Cannot toggle cron job.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }
                let daemon_url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match toggle_cron_job(&client, &daemon_url, &id).await {
                        Ok(()) => Msg::CronToggled,
                        Err(err) => Msg::CronError(err),
                    }
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PorygonTui;
    use crate::types::ChatMessage;
    use crate::Msg;
    use ulttui::cmd::Cmd;
    use ulttui::event::Event;
    use ulttui::prelude::Application;

    fn extract_ms(line: &str) -> Option<u64> {
        let idx = line.find(": ")?;
        let rest = &line[idx + 2..];
        let ms_idx = rest.find("ms")?;
        rest[..ms_idx].trim().parse::<u64>().ok()
    }

    #[test]
    fn benchmark_virtual_scroll_reports_all_sizes() {
        let app = PorygonTui::new();
        let report = app.benchmark_virtual_scroll();
        println!("{report}");

        assert!(report.contains("n=1000"));
        assert!(report.contains("n=5000"));
        assert!(report.contains("n=10000"));
    }

    #[test]
    fn benchmark_diff_widget_reports_timings() {
        let app = PorygonTui::new();
        let report = app.benchmark_diff_widget();

        let mut parse_ms = None;
        let mut style_ms = None;
        for line in report.lines() {
            if line.starts_with("- parse:") {
                parse_ms = extract_ms(line);
            }
            if line.starts_with("- classify/style prep:") {
                style_ms = extract_ms(line);
            }
        }

        let parse_ms = parse_ms.expect("parse timing missing");
        let style_ms = style_ms.expect("style timing missing");
        println!("diff benchmark parse={}ms style={}ms", parse_ms, style_ms);

        // Keep this conservative for CI stability while still ensuring no major regressions.
        assert!(parse_ms + style_ms <= 200);
    }

    #[test]
    fn stream_token_appends_to_active_assistant_message() {
        let mut app = PorygonTui::new();
        app.waiting_response = true;
        app.messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: String::new(),
            timestamp: "now".to_string(),
        });
        app.current_response = Some(app.messages.len() - 1);

        let cmd = app.update(Msg::StreamToken("hello".to_string()));

        assert!(matches!(cmd, Cmd::None));
        assert_eq!(app.stream_token_count, 1);
        assert_eq!(app.streaming_content, "hello");
        assert_eq!(
            app.messages.last().map(|message| message.content.as_str()),
            Some("hello")
        );
    }

    #[test]
    fn ctrl_c_quit_event_aborts_active_stream() {
        let mut app = PorygonTui::new();
        app.waiting_response = true;

        assert!(matches!(
            app.handle_event(&Event::Quit),
            Some(Msg::AbortStream)
        ));
    }

    #[test]
    fn status_step_text_reports_active_stream_token_count() {
        let mut app = PorygonTui::new();
        app.waiting_response = true;
        app.stream_token_count = 4;

        assert_eq!(app.status_step_text(), "streaming: 4 tokens");
    }

    #[test]
    fn composer_hint_omits_abort_when_idle() {
        let app = PorygonTui::new();

        assert_eq!(
            app.composer_hint(),
            "Disconnected | draft kept here | /reconnect"
        );
    }

    #[test]
    fn composer_hint_shows_abort_when_streaming() {
        let mut app = PorygonTui::new();
        app.waiting_response = true;

        assert_eq!(
            app.composer_hint(),
            "Working | draft kept here | Ctrl+C: stop"
        );
    }

    #[test]
    fn sidebar_hides_on_narrow_terminal_width() {
        let mut app = PorygonTui::new();
        app.terminal_width = 83;

        assert!(!app.should_show_sidebar());
    }

    #[test]
    fn sidebar_shows_when_chat_keeps_readable_width() {
        let mut app = PorygonTui::new();
        app.terminal_width = 100;

        assert!(app.should_show_sidebar());
    }
}
