//! The control plane as a Model Context Protocol server (`ulttyctl mcp`).
//!
//! MCP's stdio transport carries one JSON-RPC message per line. Each tool
//! call becomes one or two control requests, sent through the function the
//! caller supplies, so this module never touches a socket itself and the
//! whole protocol can be tested in memory.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::{json, Map, Value};
use ultty_protocol::control::{
    ControlData, ControlRequest, ControlResponse, PaneInfo, PaneState, PaneStatus, SplitDirection,
    WaitCondition,
};
use ultty_protocol::{OverrideFormat, SemanticActionKind};

use super::server::{parse_key, parse_modifiers};

/// MCP protocol versions this server speaks, newest first. Their tool
/// calls are the same; a client asking for another gets the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Default `ultty_wait` timeout.
const DEFAULT_WAIT_MS: u64 = 30_000;

/// Lines of output `ultty_last_command` returns unless asked for more, so
/// one long build log does not fill the agent's context.
const DEFAULT_TAIL_LINES: u64 = 200;

const INSTRUCTIONS: &str = "Drive the Ultty terminal. Every pane has a stable id \
from ultty_list_panes; tools that act on a pane take `pane` and use the focused \
pane without it. This MCP connection stays pinned to its initial owner epoch and token. \
ultty_run_command types shell text and Enter; its success means input accepted, \
not execution or completion. Use ultty_new_tab or ultty_split with args to launch \
a program with exact argument boundaries; args/cwd require command. Use ultty_wait_job \
with its returned pane ID for direct-child completion. Use ultty_run_and_wait at an \
observed empty OSC133 prompt for fenced shell completion. ultty_wait until=command_finished \
observes the next future completion and must be registered before it happens; \
quiet means only no output. Inspect ultty_last_command with shell integration, \
or ultty_read_text without it.";

/// Sends one control request to Ultty and returns its reply.
pub type Send<'a> = dyn FnMut(ControlRequest) -> Result<ControlResponse, String> + 'a;

/// Serve MCP on `input` and `output` until `input` ends.
///
/// `screenshot_dir` is a private directory Ultty can write a screenshot to
/// for `ultty_screenshot`; without one that tool reports an error.
pub fn serve(
    input: impl BufRead,
    mut output: impl Write,
    send: &mut Send<'_>,
    screenshot_dir: Option<&Path>,
) -> io::Result<()> {
    let mut server = McpServer {
        send,
        screenshot_dir,
        screenshots: 0,
    };
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle_line(&line) {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}

struct McpServer<'s, 'f> {
    send: &'s mut Send<'f>,
    screenshot_dir: Option<&'s Path>,
    screenshots: u64,
}

impl McpServer<'_, '_> {
    /// The reply to one message, or `None` for a notification.
    fn handle_line(&mut self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(e) => return Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return id.map(|id| rpc_error(id, -32600, "not a request"));
        };
        // Notifications (no id) are never answered.
        let id = id?;
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => initialize(&params),
            "ping" => json!({}),
            "tools/list" => match self.available_tools() {
                Ok(tools) => json!({ "tools": tools }),
                Err(message) => return Some(rpc_error(id, -32603, &message)),
            },
            "tools/call" => self.call(&params),
            _ => {
                return Some(rpc_error(
                    id,
                    -32601,
                    &format!("method not found: {method}"),
                ))
            }
        };
        Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    fn call(&mut self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let empty = Map::new();
        let args = params
            .get("arguments")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        match self.run_tool(name, args) {
            Ok(content) => json!({ "content": content, "isError": false }),
            Err(message) => json!({
                "content": [{ "type": "text", "text": message }],
                "isError": true,
            }),
        }
    }

    fn available_tools(&mut self) -> Result<Vec<Value>, String> {
        let ControlResponse::Data(ControlData::Description {
            supported_operations,
            ..
        }) = self.request(ControlRequest::Describe)?
        else {
            return Err("Selected owner did not return control capabilities".into());
        };
        Ok(tools()
            .into_iter()
            .filter(|tool| {
                let operation = match tool["name"].as_str().unwrap_or("") {
                    "ultty_list_panes" => "list_panes",
                    "ultty_info" => "get_info",
                    "ultty_read_text" => "get_text",
                    "ultty_type_text" => "send_text",
                    "ultty_send_key" => "send_key",
                    "ultty_wait" => "wait_for",
                    "ultty_run_and_wait" => "run_command_and_wait",
                    "ultty_split" => "split_pane",
                    "ultty_set_status" => "set_pane_status",
                    name => name.strip_prefix("ultty_").unwrap_or(name),
                };
                supported_operations
                    .iter()
                    .any(|supported| supported == operation)
            })
            .collect())
    }

    fn request(&mut self, request: ControlRequest) -> Result<ControlResponse, String> {
        match (self.send)(request)? {
            ControlResponse::Error { code, message } => Err(format!("error {code}: {message}")),
            response => Ok(response),
        }
    }

    fn panes(&mut self) -> Result<Vec<PaneInfo>, String> {
        match self.request(ControlRequest::ListPanes)? {
            ControlResponse::Data(ControlData::Panes { panes }) => Ok(panes),
            other => Err(unexpected(&other)),
        }
    }

    fn run_tool(&mut self, name: &str, args: &Map<String, Value>) -> Result<Vec<Value>, String> {
        let pane = optional_u64(args, "pane")?;
        let response = match name {
            "ultty_list_panes" => return Ok(vec![json_text(&self.panes()?)]),
            "ultty_workspace_list" => self.request(ControlRequest::WorkspaceList)?,
            "ultty_workspace_save" => self.request(ControlRequest::WorkspaceSave {
                name: required_str(args, "name")?.to_string(),
            })?,
            "ultty_info" => self.request(ControlRequest::GetInfo)?,
            "ultty_read_text" => {
                let lines = optional_u64(args, "scrollback_lines")?;
                self.request(ControlRequest::GetText {
                    include_scrollback: optional_bool(args, "scrollback")? || lines.is_some(),
                    scrollback_lines: lines.map(|lines| lines as usize),
                    target: pane,
                })?
            }
            "ultty_type_text" => self.request(ControlRequest::SendText {
                text: required_str(args, "text")?.to_string(),
                target: pane,
            })?,
            "ultty_send_key" => {
                let name = required_str(args, "key")?;
                let key = parse_key(name).ok_or_else(|| format!("unknown key: {name}"))?;
                let modifiers = parse_modifiers(optional_str(args, "modifiers")?.unwrap_or(""));
                self.request(ControlRequest::SendKey {
                    key,
                    modifiers,
                    target: pane,
                })?
            }
            "ultty_invoke_action" => {
                let widget_id = required_str(args, "widget_id")?;
                if !ultty_protocol::valid_action_widget_id(widget_id) {
                    return Err("invalid widget identity".into());
                }
                let action = match required_str(args, "action")? {
                    "focus" => SemanticActionKind::Focus,
                    "activate" => SemanticActionKind::Activate,
                    _ => return Err("action must be focus or activate".into()),
                };
                self.request(ControlRequest::InvokeAction {
                    target: pane.ok_or("pane is required")?,
                    session: optional_u64(args, "session")?.ok_or("session is required")?,
                    expected_generation: optional_u64(args, "expected_generation")?
                        .ok_or("expected_generation is required")?,
                    region: optional_u64(args, "region")?
                        .and_then(|id| u32::try_from(id).ok())
                        .ok_or("region must be a u32 ID")?,
                    widget_id: widget_id.to_string(),
                    action,
                })?
            }
            "ultty_run_command" => self.request(ControlRequest::RunCommand {
                command: required_str(args, "command")?.to_string(),
                args: Vec::new(),
                target: pane,
            })?,
            "ultty_run_and_wait" => self.request(ControlRequest::RunCommandAndWait {
                command: required_str(args, "command")?.to_string(),
                timeout_ms: optional_u64(args, "timeout_ms")?.unwrap_or(DEFAULT_WAIT_MS),
                target: pane,
            })?,
            "ultty_program_status" => {
                self.request(ControlRequest::ProgramStatus { target: pane })?
            }
            "ultty_focus_pane" => self.request(ControlRequest::FocusPane {
                target: pane.ok_or("missing argument: pane")?,
            })?,
            "ultty_zoom_pane" => self.request(ControlRequest::ZoomPane {
                target: pane.ok_or("missing argument: pane")?,
                zoomed: match args.get("zoomed") {
                    None => true,
                    Some(Value::Bool(value)) => *value,
                    _ => return Err("zoomed must be true or false".to_string()),
                },
            })?,
            "ultty_swap_panes" => self.request(ControlRequest::SwapPanes {
                first: optional_u64(args, "first")?.ok_or("missing argument: first")?,
                second: optional_u64(args, "second")?.ok_or("missing argument: second")?,
            })?,
            "ultty_wait" => self.wait(args, pane)?,
            "ultty_search" => self.request(ControlRequest::Search {
                pattern: required_str(args, "pattern")?.to_string(),
                regex: optional_bool(args, "regex")?,
                case_sensitive: optional_bool(args, "case_sensitive")?,
                include_scrollback: true,
                target: pane,
            })?,
            "ultty_job_status" => self.request(ControlRequest::JobStatus {
                job: optional_u64(args, "job")?.ok_or("job is required")?,
            })?,
            "ultty_wait_job" => self.request(ControlRequest::WaitJob {
                job: optional_u64(args, "job")?.ok_or("job is required")?,
                timeout_ms: optional_u64(args, "timeout_ms")?.unwrap_or(DEFAULT_WAIT_MS),
            })?,
            "ultty_new_tab" => return self.new_tab(args),
            "ultty_split" => return self.split(args, pane),
            "ultty_last_command" => self.request(ControlRequest::LastCommand {
                tail_lines: Some(optional_u64(args, "tail_lines")?.unwrap_or(DEFAULT_TAIL_LINES)),
                target: pane,
            })?,
            "ultty_regions" => self.request(ControlRequest::Regions { target: pane })?,
            "ultty_push_override" => {
                let source = required_str(args, "source")?.to_string();
                let format = match optional_str(args, "format")?.unwrap_or("toml") {
                    "toml" => OverrideFormat::Toml,
                    "luau" => OverrideFormat::Luau,
                    other => return Err(format!("format must be toml or luau, not {other}")),
                };
                self.request(ControlRequest::PushOverride {
                    format,
                    source,
                    target: pane,
                })?
            }
            "ultty_clear_override" => {
                self.request(ControlRequest::ClearOverride { target: pane })?
            }
            "ultty_set_status" => self.request(ControlRequest::SetPaneStatus {
                status: pane_status(args)?,
                target: pane,
            })?,
            "ultty_screenshot" => return self.screenshot(),
            _ => return Err(format!("unknown tool: {name}")),
        };
        if matches!(response, ControlResponse::Data(ControlData::WaitTimeout)) {
            return Err(
                "Timed out waiting for shell completion; the command may still be running"
                    .to_string(),
            );
        }
        Ok(vec![reply_content(response)?])
    }

    fn wait(
        &mut self,
        args: &Map<String, Value>,
        pane: Option<u64>,
    ) -> Result<ControlResponse, String> {
        let until = match optional_str(args, "until")?.unwrap_or("text") {
            "text" => WaitCondition::Text,
            "command_finished" => WaitCondition::CommandFinished,
            "bell" => WaitCondition::Bell,
            "quiet" => WaitCondition::Quiet {
                ms: optional_u64(args, "quiet_ms")?.unwrap_or(500),
            },
            other => {
                return Err(format!(
                    "until must be text, command_finished, bell or quiet, not {other}"
                ))
            }
        };
        let pattern = optional_str(args, "pattern")?.unwrap_or("").to_string();
        if until.is_text() && pattern.is_empty() {
            return Err("until=text needs a pattern".to_string());
        }
        let timeout_ms = optional_u64(args, "timeout_ms")?.unwrap_or(DEFAULT_WAIT_MS);
        match self.request(ControlRequest::WaitFor {
            pattern,
            regex: optional_bool(args, "regex")?,
            timeout_ms,
            until,
            target: pane,
        })? {
            ControlResponse::Data(ControlData::WaitTimeout) => {
                Err(format!("timed out after {timeout_ms} ms"))
            }
            response => Ok(response),
        }
    }

    /// Open a tab and report the id of the pane it made.
    fn new_tab(&mut self, args: &Map<String, Value>) -> Result<Vec<Value>, String> {
        let response = self.request(ControlRequest::NewTab {
            title: optional_str(args, "title")?.map(str::to_string),
            command: optional_str(args, "command")?.map(str::to_string),
            args: command_args(args)?,
            working_dir: optional_str(args, "cwd")?.map(str::to_string),
        })?;
        Ok(vec![reply_content(response)?])
    }

    fn split(
        &mut self,
        args: &Map<String, Value>,
        pane: Option<u64>,
    ) -> Result<Vec<Value>, String> {
        let direction = match optional_str(args, "direction")?.unwrap_or("right") {
            "right" => SplitDirection::Right,
            "down" => SplitDirection::Down,
            _ => return Err("direction must be right or down".to_string()),
        };
        let response = self.request(ControlRequest::SplitPane {
            direction,
            command: optional_str(args, "command")?.map(str::to_string),
            args: command_args(args)?,
            working_dir: optional_str(args, "cwd")?.map(str::to_string),
            target: pane,
        })?;
        Ok(vec![reply_content(response)?])
    }

    fn screenshot(&mut self) -> Result<Vec<Value>, String> {
        let dir = self
            .screenshot_dir
            .ok_or("no private directory to write screenshots to")?;
        self.screenshots += 1;
        let path: PathBuf = dir.join(format!(
            "mcp-screenshot-{}-{}.png",
            std::process::id(),
            self.screenshots
        ));
        let response = self.request(ControlRequest::Screenshot {
            path: Some(path.display().to_string()),
        });
        let bytes = response.and_then(|_| {
            std::fs::read(&path).map_err(|e| format!("reading the screenshot failed: {e}"))
        });
        let _ = std::fs::remove_file(&path);
        Ok(vec![json!({
            "type": "image",
            "mimeType": "image/png",
            "data": base64::engine::general_purpose::STANDARD.encode(bytes?),
        })])
    }
}

/// The status `ultty_set_status` asks for; `state: "clear"` removes it.
fn pane_status(args: &Map<String, Value>) -> Result<Option<PaneStatus>, String> {
    let state = match required_str(args, "state")? {
        "clear" => return Ok(None),
        "idle" => PaneState::Idle,
        "working" => PaneState::Working,
        "waiting" => PaneState::Waiting,
        "done" => PaneState::Done,
        "error" => PaneState::Error,
        other => {
            return Err(format!(
                "state must be idle, working, waiting, done, error or clear, not {other}"
            ))
        }
    };
    let progress = match args.get("progress") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_f64()
                .filter(|n| (0.0..=1.0).contains(n))
                .ok_or("progress must be a number from 0 to 1")? as f32,
        ),
    };
    Ok(Some(PaneStatus {
        state,
        label: optional_str(args, "label")?.unwrap_or("").to_string(),
        progress,
        attention: optional_bool(args, "attention")?,
    }))
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|requested| PROTOCOL_VERSIONS.contains(requested))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "ultty", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

/// A successful reply as tool content: plain text for screen text, JSON for
/// structured data.
fn reply_content(response: ControlResponse) -> Result<Value, String> {
    Ok(match response {
        ControlResponse::Ok => text("ok"),
        ControlResponse::Text { text: body } => text(&body),
        ControlResponse::Data(data) => json_text(&data),
        ControlResponse::Error { code, message } => return Err(format!("error {code}: {message}")),
    })
}

fn unexpected(response: &ControlResponse) -> String {
    format!("unexpected reply from ultty: {response:?}")
}

fn text(body: &str) -> Value {
    json!({ "type": "text", "text": body })
}

fn json_text(value: &impl serde::Serialize) -> Value {
    text(&serde_json::to_string(value).unwrap_or_else(|e| format!("unserializable reply: {e}")))
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn required_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    optional_str(args, key)?.ok_or_else(|| format!("missing argument: {key}"))
}

fn command_args(args: &Map<String, Value>) -> Result<Vec<String>, String> {
    match args.get("args") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "args must be a list of strings".to_string()),
        Some(_) => Err("args must be a list of strings".to_string()),
    }
}

fn optional_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn optional_u64(args: &Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

fn optional_bool(args: &Map<String, Value>, key: &str) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("{key} must be true or false")),
    }
}

fn pane_property() -> Value {
    json!({
        "type": "integer",
        "minimum": 0,
        "description": "Pane id from ultty_list_panes. The focused pane when omitted.",
    })
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "ultty_list_panes",
            "List every pane in every tab: id, tab, focus, title, foreground program, \
             working directory and size.",
            json!({}),
            &[],
        ),
        tool("ultty_workspace_list", "List saved workspace layout recipes.", json!({}), &[]),
        tool("ultty_workspace_save", "Save the current layout and directories as a named workspace recipe. Opening it with ultty --workspace starts fresh shells, not existing processes.",
            json!({ "name": { "type": "string" } }), &["name"]),
        tool(
            "ultty_read_text",
            "Read a pane's screen text, optionally with scrollback.",
            json!({
                "pane": pane_property(),
                "scrollback": { "type": "boolean", "description": "Include all scrollback." },
                "scrollback_lines": {
                    "type": "integer", "minimum": 0,
                    "description": "Include only this many of the most recent scrollback lines.",
                },
            }),
            &[],
        ),
        tool(
            "ultty_type_text",
            "Type text into a pane as if from the keyboard. Does not press Enter.",
            json!({ "pane": pane_property(), "text": { "type": "string" } }),
            &["text"],
        ),
        tool(
            "ultty_send_key",
            "Press a key in a pane: enter, tab, escape, backspace, up, down, left, right, \
             home, end, pageup, pagedown, f1-f12, space or a single character.",
            json!({
                "pane": pane_property(),
                "key": { "type": "string" },
                "modifiers": { "type": "string", "description": "Any of ctrl, alt, shift, meta, e.g. \"ctrl+shift\"." },
            }),
            &["key"],
        ),
        tool(
            "ultty_invoke_action",
            "Focus or activate an explicitly declared native widget. Copy pane/session/shown_generation/region/widget ID from regions. Success means application acknowledgement, not asynchronous task completion. Stale, hidden, ambiguous or unsupported targets fail; reinspect before retrying an unknown outcome.",
            json!({
                "pane": pane_property(),
                "session": {"type": "integer", "minimum": 0},
                "expected_generation": {"type": "integer", "minimum": 0},
                "region": {"type": "integer", "minimum": 0, "maximum": 4294967295u64},
                "widget_id": {"type": "string", "minLength": 1, "maxLength": 256},
                "action": {"type": "string", "enum": ["focus", "activate"]},
            }),
            &["pane", "session", "expected_generation", "region", "widget_id", "action"],
        ),
        tool(
            "ultty_run_command",
            "Type shell text into the foreground PTY and press Enter. The program \
             there must be a shell. Success means input accepted, not completion. \
             Use ultty_new_tab or ultty_split for argv-preserving process launch.",
            json!({ "pane": pane_property(), "command": { "type": "string" } }),
            &["command"],
        ),
        tool(
            "ultty_wait",
            "Wait for something in a pane: text to appear (until=text, the default), the \
             next command to finish with its exit code (until=command_finished; needs \
             shell integration), the bell, or quiet_ms without output (until=quiet). \
             Start it before the thing you wait for can happen.",
            json!({
                "pane": pane_property(),
                "until": { "type": "string", "enum": ["text", "command_finished", "bell", "quiet"] },
                "pattern": { "type": "string", "description": "Text to wait for (until=text)." },
                "regex": { "type": "boolean" },
                "quiet_ms": { "type": "integer", "minimum": 1, "description": "Default 500." },
                "timeout_ms": { "type": "integer", "minimum": 1, "description": "Default 30000." },
            }),
            &[],
        ),
        tool(
            "ultty_run_and_wait",
            "Submit one printable shell line at an observed empty OSC 133 prompt, \
             then wait for that submission's fenced completion. Rejects a busy or \
             edited prompt. Timeout does not cancel the command. Shell marks are \
             observed evidence, not proof of what arbitrary PTY input executed.",
            json!({
                "pane": pane_property(),
                "command": { "type": "string" },
                "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 86400000 },
            }),
            &["command"],
        ),
        tool(
            "ultty_program_status",
            "Read current OSC 7501 program records and their generation. Program \
             status is untrusted reported state; permission-kind status grants no \
             authority. Notifications can coalesce; query the current snapshot.",
            json!({ "pane": pane_property() }),
            &[],
        ),
        tool(
            "ultty_last_command",
            "The last command run in a pane's shell: the command line, its output, its \
             exit code, and whether it is still running. Needs shell integration (OSC \
             133). Pair with ultty_wait until=command_finished.",
            json!({
                "pane": pane_property(),
                "tail_lines": { "type": "integer", "minimum": 1, "description": "Lines of output from the end. Default 200." },
            }),
            &[],
        ),
        tool(
            "ultty_regions",
            "The native (ulttui) apps in a pane's tab: each app's region tree in \
             drawing order, with every region's position and size in cells, scroll, \
             content kind and text, the metadata the app set, and which region has \
             focus. Empty when the tab runs only plain terminal programs; read those \
             with ultty_read_text.",
            json!({ "pane": pane_property() }),
            &[],
        ),
        tool(
            "ultty_push_override",
            "Reshape the native (ulttui) app in a pane without editing its files: push a \
             user override keyed by the widget ids ultty_regions shows. TOML: \
             [widget.<id>] with fg, bg, bold, hidden or text, and [keys] mapping a key \
             to another key or { focus = \"<id>\" }. It applies on the app's next frame, \
             in memory, over the app's override files, and replaces an earlier push; \
             ultty_clear_override takes it away. Fails with the app's parse error when \
             the source is wrong, and when the pane has no native app.",
            json!({
                "pane": pane_property(),
                "source": { "type": "string", "description": "The override file's text." },
                "format": { "type": "string", "enum": ["toml", "luau"], "description": "Default toml." },
            }),
            &["source"],
        ),
        tool(
            "ultty_clear_override",
            "Take back the override pushed into a pane's native app with \
             ultty_push_override; its override files apply as before.",
            json!({ "pane": pane_property() }),
            &[],
        ),
        tool(
            "ultty_search",
            "Find text in a pane's screen and scrollback.",
            json!({
                "pane": pane_property(),
                "pattern": { "type": "string" },
                "regex": { "type": "boolean" },
                "case_sensitive": { "type": "boolean" },
            }),
            &["pattern"],
        ),
        tool("ultty_job_status", "Read current or retained direct-child job state; the spawned pane ID is its job ID.",
            json!({"job": {"type": "integer", "minimum": 0}}), &["job"]),
        tool("ultty_wait_job", "Wait for a direct-child job by identity, including one that already exited. Timeout does not cancel it.",
            json!({"job": {"type": "integer", "minimum": 0}, "timeout_ms": {"type": "integer", "minimum": 1, "maximum": 86400000}}), &["job"]),
        tool(
            "ultty_new_tab",
            "Open a tab, optionally launching a program with exact args, and return \
             its new pane/job ID. Success means no reported setup/exec failure; application readiness and completion are separate.",
            json!({
                "command": { "type": "string" },
                "args": { "type": "array", "items": { "type": "string" } },
                "cwd": { "type": "string" },
                "title": { "type": "string" },
            }),
            &[],
        ),
        tool(
            "ultty_split",
            "Split a pane right or down, optionally launching a program with exact \
             args, and return its new pane/job ID. Success means no reported setup/exec failure; \
             application readiness and completion are separate.",
            json!({
                "pane": pane_property(),
                "direction": { "type": "string", "enum": ["right", "down"] },
                "command": { "type": "string" },
                "args": { "type": "array", "items": { "type": "string" } },
                "cwd": { "type": "string" },
            }),
            &[],
        ),
        tool(
            "ultty_set_status",
            "Show what you are doing in a pane's tab: a colored marker, a short label \
             and optional progress. Set waiting with attention=true when you need the \
             user. state=clear removes it.",
            json!({
                "pane": pane_property(),
                "state": { "type": "string", "enum": ["idle", "working", "waiting", "done", "error", "clear"] },
                "label": { "type": "string", "description": "A few words, e.g. \"running tests\"." },
                "progress": { "type": "number", "minimum": 0, "maximum": 1 },
                "attention": { "type": "boolean", "description": "Mark the window urgent while it is unfocused." },
            }),
            &["state"],
        ),
        tool(
            "ultty_focus_pane",
            "Focus a specific pane and select its tab.",
            json!({ "pane": pane_property() }),
            &["pane"],
        ),
        tool(
            "ultty_zoom_pane",
            "Zoom a pane, or restore its split presentation; preserves the stored layout.",
            json!({ "pane": pane_property(), "zoomed": { "type": "boolean", "default": true } }),
            &["pane"],
        ),
        tool(
            "ultty_swap_panes",
            "Swap two panes within the same tab, preserving pane IDs and processes.",
            json!({ "first": pane_property(), "second": pane_property() }),
            &["first", "second"],
        ),
        tool(
            "ultty_screenshot",
            "Capture the Ultty window as a PNG image.",
            json!({}),
            &[],
        ),
        tool(
            "ultty_info",
            "Window size, cursor, current tab and focus.",
            json!({}),
            &[],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultty_protocol::control::ControlEvent;

    /// Run `messages` through the server against `reply`, returning the
    /// output lines and the requests Ultty would have received.
    fn exchange(
        messages: &[Value],
        mut reply: impl FnMut(&ControlRequest) -> ControlResponse,
    ) -> (Vec<Value>, Vec<ControlRequest>) {
        let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
        let mut output = Vec::new();
        let mut sent = Vec::new();
        let mut send = |request: ControlRequest| {
            let response = reply(&request);
            sent.push(request);
            Ok(response)
        };
        serve(input.as_bytes(), &mut output, &mut send, None).unwrap();
        let lines = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (lines, sent)
    }

    fn call(id: u64, name: &str, arguments: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": name, "arguments": arguments } })
    }

    #[test]
    fn test_invoke_action_tool_requires_exact_identity_and_propagates_refusal() {
        let args = json!({"pane":1,"session":2,"expected_generation":3,"region":4,"widget_id":"inc","action":"activate"});
        let (lines, sent) = exchange(&[call(1, "ultty_invoke_action", args.clone())], |_| {
            ControlResponse::Error {
                code: 15,
                message: "stale frame".into(),
            }
        });
        assert_eq!(
            sent,
            vec![ControlRequest::InvokeAction {
                target: 1,
                session: 2,
                expected_generation: 3,
                region: 4,
                widget_id: "inc".into(),
                action: SemanticActionKind::Activate
            }]
        );
        assert_eq!(lines[0]["result"]["isError"], true);
        for field in [
            "pane",
            "session",
            "expected_generation",
            "region",
            "widget_id",
            "action",
        ] {
            let mut invalid = args.clone();
            invalid.as_object_mut().unwrap().remove(field);
            let (lines, sent) = exchange(&[call(1, "ultty_invoke_action", invalid)], |_| {
                panic!("invalid target reached dispatch")
            });
            assert!(sent.is_empty());
            assert_eq!(lines[0]["result"]["isError"], true);
        }
    }

    #[test]
    fn test_job_tools_preserve_job_identity_and_timeout() {
        let (_, sent) = exchange(
            &[
                call(1, "ultty_job_status", json!({"job": 7})),
                call(2, "ultty_wait_job", json!({"job": 7, "timeout_ms": 50})),
            ],
            |_| ControlResponse::Ok,
        );
        assert_eq!(
            sent,
            vec![
                ControlRequest::JobStatus { job: 7 },
                ControlRequest::WaitJob {
                    job: 7,
                    timeout_ms: 50
                }
            ]
        );
    }

    #[test]
    fn test_split_tool_preserves_argv_and_target() {
        let arguments = vec![
            "space here".to_string(),
            "'quoted'".to_string(),
            "line\nbreak".to_string(),
        ];
        let (lines, sent) = exchange(
            &[call(
                1,
                "ultty_split",
                json!({
                    "pane": 7, "direction": "down", "command": "/usr/bin/printf",
                    "args": arguments, "cwd": "/tmp"
                }),
            )],
            |_| {
                ControlResponse::Data(ControlData::PaneOpened {
                    tab_index: 0,
                    pane: 8,
                    warning: None,
                })
            },
        );
        assert_eq!(
            sent,
            vec![ControlRequest::SplitPane {
                direction: SplitDirection::Down,
                command: Some("/usr/bin/printf".to_string()),
                args: arguments,
                working_dir: Some("/tmp".to_string()),
                target: Some(7),
            }]
        );
        assert_eq!(lines[0]["result"]["isError"], false);
    }

    #[test]
    fn test_split_tool_rejects_invalid_launch_arguments_before_dispatch() {
        let (lines, sent) = exchange(
            &[
                call(1, "ultty_split", json!({"direction": "left"})),
                call(2, "ultty_split", json!({"args": ["valid", 3]})),
            ],
            |_| panic!("invalid tool call reached dispatch"),
        );
        assert!(sent.is_empty());
        assert!(lines.iter().all(|line| line["result"]["isError"] == true));
    }

    #[test]
    fn test_fenced_run_uses_one_request_and_timeout_is_a_tool_error() {
        let (lines, sent) = exchange(
            &[call(
                1,
                "ultty_run_and_wait",
                json!({
                    "pane": 3, "command": "false", "timeout_ms": 50
                }),
            )],
            |_| ControlResponse::Data(ControlData::WaitTimeout),
        );
        assert_eq!(
            sent,
            vec![ControlRequest::RunCommandAndWait {
                command: "false".to_string(),
                timeout_ms: 50,
                target: Some(3)
            }]
        );
        assert_eq!(lines[0]["result"]["isError"], true);
    }

    #[test]
    fn test_workspace_and_pane_tools_dispatch_explicit_operations() {
        let (lines, sent) = exchange(
            &[
                call(1, "ultty_focus_pane", json!({"pane": 7})),
                call(2, "ultty_zoom_pane", json!({"pane": 7, "zoomed": false})),
                call(3, "ultty_swap_panes", json!({"first": 7, "second": 8})),
                call(4, "ultty_workspace_list", json!({})),
                call(5, "ultty_workspace_save", json!({"name": "work"})),
            ],
            |_| ControlResponse::Ok,
        );
        assert_eq!(
            sent,
            vec![
                ControlRequest::FocusPane { target: 7 },
                ControlRequest::ZoomPane {
                    target: 7,
                    zoomed: false
                },
                ControlRequest::SwapPanes {
                    first: 7,
                    second: 8
                },
                ControlRequest::WorkspaceList,
                ControlRequest::WorkspaceSave {
                    name: "work".to_string()
                }
            ]
        );
        assert!(lines.iter().all(|line| line["result"]["isError"] == false));
    }

    #[test]
    fn initialize_answers_one_line_per_message_and_ignores_notifications() {
        let (lines, sent) = exchange(
            &[
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": { "protocolVersion": "2024-11-05" } }),
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            ],
            |_| {
                ControlResponse::Data(ControlData::Description {
                    schema_version: 2,
                    owner: ultty_protocol::control::OwnerRef {
                        pid: 1,
                        epoch: "test".into(),
                    },
                    supported_operations: vec!["wait_for".into(), "get_text".into()],
                })
            },
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "ultty");
        let tools = lines[1]["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == "ultty_wait"));
        assert_eq!(sent, vec![ControlRequest::Describe]);
        assert!(!tools.iter().any(|tool| tool["name"] == "ultty_screenshot"));
    }

    #[test]
    fn an_unknown_protocol_version_gets_the_newest() {
        let (lines, _) = exchange(
            &[json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                      "params": { "protocolVersion": "1999-01-01" } })],
            |_| ControlResponse::Ok,
        );
        assert_eq!(lines[0]["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn test_tools_list_reports_selected_owner_discovery_failure() {
        let (lines, sent) = exchange(
            &[json!({"jsonrpc":"2.0", "id":1, "method":"tools/list"})],
            |_| ControlResponse::Error {
                code: 409,
                message: "owner changed".into(),
            },
        );
        assert_eq!(sent, vec![ControlRequest::Describe]);
        assert_eq!(lines[0]["error"]["code"], -32603);
        assert!(lines[0].get("result").is_none());
    }

    #[test]
    fn pane_tools_target_the_given_pane() {
        let (lines, sent) = exchange(
            &[
                call(
                    1,
                    "ultty_run_command",
                    json!({ "pane": 4, "command": "make test" }),
                ),
                call(
                    2,
                    "ultty_read_text",
                    json!({ "pane": 4, "scrollback_lines": 50 }),
                ),
            ],
            |request| match request {
                ControlRequest::GetText { .. } => ControlResponse::Text {
                    text: "all passed".to_string(),
                },
                _ => ControlResponse::Ok,
            },
        );
        assert_eq!(
            sent,
            vec![
                ControlRequest::RunCommand {
                    command: "make test".to_string(),
                    args: Vec::new(),
                    target: Some(4),
                },
                ControlRequest::GetText {
                    include_scrollback: true,
                    scrollback_lines: Some(50),
                    target: Some(4),
                },
            ]
        );
        assert_eq!(lines[1]["result"]["content"][0]["text"], "all passed");
        assert_eq!(lines[1]["result"]["isError"], false);
    }

    #[test]
    fn wait_for_a_command_reports_its_exit_code() {
        let finished = ControlEvent::CommandFinished {
            pane: 2,
            exit_code: Some(1),
        };
        let (lines, sent) = exchange(
            &[call(
                1,
                "ultty_wait",
                json!({ "pane": 2, "until": "command_finished" }),
            )],
            |_| {
                ControlResponse::Data(ControlData::WaitEvent {
                    event: finished.clone(),
                })
            },
        );
        assert!(matches!(
            sent[0],
            ControlRequest::WaitFor {
                until: WaitCondition::CommandFinished,
                target: Some(2),
                timeout_ms: DEFAULT_WAIT_MS,
                ..
            }
        ));
        let text = lines[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"exit_code\":1"), "{text}");
    }

    #[test]
    fn errors_and_timeouts_are_tool_errors_not_protocol_errors() {
        let (lines, _) = exchange(
            &[
                call(
                    1,
                    "ultty_wait",
                    json!({ "pattern": "done", "timeout_ms": 10 }),
                ),
                call(2, "ultty_read_text", json!({ "pane": 99 })),
                call(3, "ultty_wait", json!({ "until": "text" })),
                call(4, "ultty_send_key", json!({ "key": "nonsense" })),
            ],
            |request| match request {
                ControlRequest::WaitFor { .. } => ControlResponse::Data(ControlData::WaitTimeout),
                _ => ControlResponse::Error {
                    code: 6,
                    message: "No pane 99".to_string(),
                },
            },
        );
        for line in &lines {
            assert_eq!(line["result"]["isError"], true, "{line}");
        }
        assert_eq!(
            lines[0]["result"]["content"][0]["text"],
            "timed out after 10 ms"
        );
        assert_eq!(
            lines[1]["result"]["content"][0]["text"],
            "error 6: No pane 99"
        );
    }

    #[test]
    fn new_tab_returns_the_pane_it_opened() {
        let (lines, sent) = exchange(
            &[call(
                1,
                "ultty_new_tab",
                json!({ "command": "htop", "args": ["-d", "5"] }),
            )],
            |_| {
                ControlResponse::Data(ControlData::TabOpened {
                    tab_index: 1,
                    pane: 5,
                    warning: None,
                })
            },
        );
        assert_eq!(
            sent,
            vec![ControlRequest::NewTab {
                title: None,
                command: Some("htop".to_string()),
                args: vec!["-d".to_string(), "5".to_string()],
                working_dir: None,
            }]
        );
        let reply: Value =
            serde_json::from_str(lines[0]["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(reply["pane"], 5);
    }

    #[test]
    fn set_status_sets_and_clears_a_panes_status() {
        let (lines, sent) = exchange(
            &[
                call(
                    1,
                    "ultty_set_status",
                    json!({ "pane": 3, "state": "working",
                                                   "label": "tests", "progress": 0.5 }),
                ),
                call(2, "ultty_set_status", json!({ "state": "clear" })),
                call(3, "ultty_set_status", json!({ "state": "busy" })),
            ],
            |_| ControlResponse::Ok,
        );
        assert_eq!(
            sent,
            vec![
                ControlRequest::SetPaneStatus {
                    status: Some(PaneStatus {
                        state: PaneState::Working,
                        label: "tests".to_string(),
                        progress: Some(0.5),
                        attention: false,
                    }),
                    target: Some(3),
                },
                ControlRequest::SetPaneStatus {
                    status: None,
                    target: None,
                },
            ]
        );
        assert_eq!(lines[2]["result"]["isError"], true);
    }

    #[test]
    fn last_command_returns_a_bounded_tail_by_default() {
        let (_, sent) = exchange(
            &[
                call(1, "ultty_last_command", json!({ "pane": 2 })),
                call(2, "ultty_last_command", json!({ "tail_lines": 5 })),
            ],
            |_| ControlResponse::Ok,
        );
        assert_eq!(
            sent,
            vec![
                ControlRequest::LastCommand {
                    tail_lines: Some(DEFAULT_TAIL_LINES),
                    target: Some(2),
                },
                ControlRequest::LastCommand {
                    tail_lines: Some(5),
                    target: None,
                },
            ]
        );
    }

    #[test]
    fn malformed_lines_and_unknown_methods_get_json_rpc_errors() {
        let input = "not json\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"resources/list\"}\n";
        let mut output = Vec::new();
        let mut send = |_: ControlRequest| Ok(ControlResponse::Ok);
        serve(input.as_bytes(), &mut output, &mut send, None).unwrap();
        let lines: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines[0]["error"]["code"], -32700);
        assert_eq!(lines[1]["error"]["code"], -32601);
        assert_eq!(lines[1]["id"], 7);
    }
}
