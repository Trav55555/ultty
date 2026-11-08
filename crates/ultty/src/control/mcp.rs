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
    ControlData, ControlRequest, ControlResponse, PaneInfo, WaitCondition,
};

use super::server::{parse_key, parse_modifiers};

/// MCP protocol versions this server speaks, newest first. Their tool
/// calls are the same; a client asking for another gets the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Default `ultty_wait` timeout.
const DEFAULT_WAIT_MS: u64 = 30_000;

const INSTRUCTIONS: &str = "Drive the Ultty terminal. Every pane has a stable id \
from ultty_list_panes; tools that act on a pane take `pane` and use the focused \
pane without it. Run a command with ultty_run_command, then ultty_wait with \
until=command_finished (needs shell integration) or until=quiet, then \
ultty_read_text.";

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
            "tools/list" => json!({ "tools": tools() }),
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
            "ultty_run_command" => self.request(ControlRequest::RunCommand {
                command: required_str(args, "command")?.to_string(),
                args: Vec::new(),
                target: pane,
            })?,
            "ultty_wait" => self.wait(args, pane)?,
            "ultty_search" => self.request(ControlRequest::Search {
                pattern: required_str(args, "pattern")?.to_string(),
                regex: optional_bool(args, "regex")?,
                case_sensitive: optional_bool(args, "case_sensitive")?,
                include_scrollback: true,
                target: pane,
            })?,
            "ultty_new_tab" => return self.new_tab(args),
            "ultty_screenshot" => return self.screenshot(),
            _ => return Err(format!("unknown tool: {name}")),
        };
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
        let before: Vec<u64> = self.panes()?.iter().map(|pane| pane.id).collect();
        let command_args = match args.get("args") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or("args must be a list of strings")?,
            Some(_) => return Err("args must be a list of strings".to_string()),
        };
        self.request(ControlRequest::NewTab {
            title: optional_str(args, "title")?.map(str::to_string),
            command: optional_str(args, "command")?.map(str::to_string),
            args: command_args,
            working_dir: optional_str(args, "cwd")?.map(str::to_string),
        })?;
        let opened: Vec<PaneInfo> = self
            .panes()?
            .into_iter()
            .filter(|pane| !before.contains(&pane.id))
            .collect();
        Ok(vec![json_text(&opened)])
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
            "ultty_run_command",
            "Type a command line into a pane's shell and press Enter. Returns at once; \
             use ultty_wait to know when it is done.",
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
        tool(
            "ultty_new_tab",
            "Open a tab, optionally running a command, and return its new pane.",
            json!({
                "command": { "type": "string" },
                "args": { "type": "array", "items": { "type": "string" } },
                "cwd": { "type": "string" },
                "title": { "type": "string" },
            }),
            &[],
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

    fn pane(id: u64) -> PaneInfo {
        PaneInfo {
            id,
            tab_index: 0,
            active: true,
            focused: true,
            title: String::new(),
            foreground: None,
            pid: None,
            cwd: None,
            cols: 80,
            rows: 24,
        }
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
            |_| ControlResponse::Ok,
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "ultty");
        let tools = lines[1]["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == "ultty_wait"));
        assert!(sent.is_empty());
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
        let mut listed = 0;
        let (lines, _) = exchange(
            &[call(1, "ultty_new_tab", json!({ "command": "htop" }))],
            |request| match request {
                ControlRequest::ListPanes => {
                    listed += 1;
                    let panes = if listed == 1 {
                        vec![pane(0)]
                    } else {
                        vec![pane(0), pane(5)]
                    };
                    ControlResponse::Data(ControlData::Panes { panes })
                }
                _ => ControlResponse::Ok,
            },
        );
        let opened: Vec<PaneInfo> =
            serde_json::from_str(lines[0]["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(opened, vec![pane(5)]);
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
