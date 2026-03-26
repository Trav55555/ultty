#!/usr/bin/env python3
"""MCP server for Ultty terminal emulator.

Exposes ulttyctl commands as MCP tools over stdio JSON-RPC.
Requires ulttyctl binary on PATH or at ../target/release/ulttyctl.
"""

import json
import os
import subprocess
import sys
import base64
import tempfile
from pathlib import Path

# Find ulttyctl binary
ULTTYCTL = None
for candidate in [
    os.environ.get("ULTTYCTL_PATH"),
    str(Path(__file__).parent.parent / "target" / "release" / "ulttyctl"),
    str(Path(__file__).parent.parent / "target" / "debug" / "ulttyctl"),
    "ulttyctl",
]:
    if candidate and (Path(candidate).is_file() or candidate == "ulttyctl"):
        ULTTYCTL = candidate
        break

if not ULTTYCTL:
    print("Error: ulttyctl not found", file=sys.stderr)
    sys.exit(1)


def run_ulttyctl(*args, timeout=10):
    """Run ulttyctl with args and return (stdout, stderr, returncode)."""
    try:
        result = subprocess.run(
            [ULTTYCTL] + list(args),
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        return result.stdout.strip(), result.stderr.strip(), result.returncode
    except subprocess.TimeoutExpired:
        return "", "timeout", 1
    except FileNotFoundError:
        return "", f"ulttyctl not found at {ULTTYCTL}", 1


# ── MCP Tool Definitions ──────────────────────────────────────────────

TOOLS = [
    {
        "name": "ultty_screenshot",
        "description": "Take a screenshot of the Ultty terminal window. Returns the image as base64-encoded PNG.",
        "inputSchema": {
            "type": "object",
            "properties": {},
        },
    },
    {
        "name": "ultty_send_text",
        "description": "Send text to the terminal as if typed by the user.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to send to the terminal",
                }
            },
            "required": ["text"],
        },
    },
    {
        "name": "ultty_send_key",
        "description": "Send a key press to the terminal. Keys: enter, tab, escape, backspace, delete, up, down, left, right, home, end, pageup, pagedown, space, f1-f12, or single characters (a, b, 1, etc). Modifiers: ctrl, shift, alt.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Key name (e.g., 'enter', 'a', 'f1')",
                },
                "modifiers": {
                    "type": "string",
                    "description": "Optional modifiers (e.g., 'ctrl', 'shift', 'ctrl+shift')",
                },
            },
            "required": ["key"],
        },
    },
    {
        "name": "ultty_get_text",
        "description": "Get the visible text content of the terminal.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "scrollback": {
                    "type": "boolean",
                    "description": "Include scrollback buffer (default false)",
                }
            },
        },
    },
    {
        "name": "ultty_info",
        "description": "Get terminal info: size, cursor position, title, etc.",
        "inputSchema": {
            "type": "object",
            "properties": {},
        },
    },
    {
        "name": "ultty_run",
        "description": "Run a command in the current terminal tab.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command to run",
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Command arguments",
                },
            },
            "required": ["command"],
        },
    },
    {
        "name": "ultty_wait_for",
        "description": "Wait for a text pattern to appear in the terminal output.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Text pattern to wait for",
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default 30000)",
                },
            },
            "required": ["pattern"],
        },
    },
    {
        "name": "ultty_new_tab",
        "description": "Create a new terminal tab, optionally running a command.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Optional command to run in the new tab",
                },
            },
        },
    },
    {
        "name": "ultty_resize",
        "description": "Resize the terminal window.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "width": {"type": "integer", "description": "Width in pixels"},
                "height": {"type": "integer", "description": "Height in pixels"},
            },
            "required": ["width", "height"],
        },
    },
    {
        "name": "ultty_search",
        "description": "Search for a pattern in the terminal scrollback.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern",
                },
            },
            "required": ["pattern"],
        },
    },
]


# ── Tool Handlers ─────────────────────────────────────────────────────


def handle_tool(name, arguments):
    """Execute a tool and return the result content."""
    if name == "ultty_screenshot":
        with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as f:
            path = f.name
        stdout, stderr, code = run_ulttyctl("screenshot", path)
        if code != 0:
            return [{"type": "text", "text": f"Error: {stderr or stdout}"}]
        try:
            with open(path, "rb") as f:
                data = base64.standard_b64encode(f.read()).decode()
            return [
                {"type": "image", "data": data, "mimeType": "image/png"},
            ]
        finally:
            os.unlink(path)

    elif name == "ultty_send_text":
        text = arguments.get("text", "")
        stdout, stderr, code = run_ulttyctl("send-text", text)
        return [{"type": "text", "text": stdout if code == 0 else f"Error: {stderr}"}]

    elif name == "ultty_send_key":
        key = arguments.get("key", "")
        mods = arguments.get("modifiers")
        args = ["send-key", key]
        if mods:
            args.append(mods)
        stdout, stderr, code = run_ulttyctl(*args)
        return [{"type": "text", "text": stdout if code == 0 else f"Error: {stderr}"}]

    elif name == "ultty_get_text":
        args = ["text"]
        if arguments.get("scrollback"):
            args.append("--scrollback")
        stdout, stderr, code = run_ulttyctl(*args)
        return [{"type": "text", "text": stdout if code == 0 else f"Error: {stderr}"}]

    elif name == "ultty_info":
        stdout, stderr, code = run_ulttyctl("--json", "info")
        return [{"type": "text", "text": stdout if code == 0 else f"Error: {stderr}"}]

    elif name == "ultty_run":
        cmd = arguments.get("command", "")
        args_list = arguments.get("args", [])
        stdout, stderr, code = run_ulttyctl("run", cmd, *args_list)
        return [{"type": "text", "text": stdout if code == 0 else f"Error: {stderr}"}]

    elif name == "ultty_wait_for":
        pattern = arguments.get("pattern", "")
        timeout = arguments.get("timeout_ms", 30000)
        stdout, stderr, code = run_ulttyctl(
            "wait-for", pattern, "--timeout", str(timeout), timeout=timeout // 1000 + 5
        )
        return [{"type": "text", "text": stdout if code == 0 else f"Error: {stderr}"}]

    elif name == "ultty_new_tab":
        cmd = arguments.get("command")
        args = ["new-tab"]
        if cmd:
            args.append(cmd)
        stdout, stderr, code = run_ulttyctl(*args)
        return [{"type": "text", "text": stdout if code == 0 else f"Error: {stderr}"}]

    elif name == "ultty_resize":
        w = str(arguments.get("width", 800))
        h = str(arguments.get("height", 600))
        stdout, stderr, code = run_ulttyctl("resize", w, h)
        return [{"type": "text", "text": stdout if code == 0 else f"Error: {stderr}"}]

    elif name == "ultty_search":
        pattern = arguments.get("pattern", "")
        stdout, stderr, code = run_ulttyctl("search", pattern)
        return [{"type": "text", "text": stdout if code == 0 else f"Error: {stderr}"}]

    else:
        return [{"type": "text", "text": f"Unknown tool: {name}"}]


# ── MCP Protocol Handler ──────────────────────────────────────────────


def send_response(id, result):
    response = {"jsonrpc": "2.0", "id": id, "result": result}
    msg = json.dumps(response)
    sys.stdout.write(f"Content-Length: {len(msg)}\r\n\r\n{msg}")
    sys.stdout.flush()


def send_error(id, code, message):
    response = {
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    }
    msg = json.dumps(response)
    sys.stdout.write(f"Content-Length: {len(msg)}\r\n\r\n{msg}")
    sys.stdout.flush()


def handle_message(message):
    method = message.get("method")
    id = message.get("id")
    params = message.get("params", {})

    if method == "initialize":
        send_response(
            id,
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "ultty-mcp",
                    "version": "0.1.0",
                },
            },
        )

    elif method == "notifications/initialized":
        pass  # No response needed

    elif method == "tools/list":
        send_response(id, {"tools": TOOLS})

    elif method == "tools/call":
        name = params.get("name", "")
        arguments = params.get("arguments", {})
        try:
            content = handle_tool(name, arguments)
            send_response(id, {"content": content, "isError": False})
        except Exception as e:
            send_response(
                id, {"content": [{"type": "text", "text": str(e)}], "isError": True}
            )

    elif method == "ping":
        send_response(id, {})

    elif id is not None:
        send_error(id, -32601, f"Method not found: {method}")


def main():
    """Run the MCP server on stdio."""
    while True:
        # Read Content-Length header
        line = sys.stdin.readline()
        if not line:
            break
        line = line.strip()
        if not line.startswith("Content-Length:"):
            continue
        length = int(line.split(":")[1].strip())

        # Read blank line
        sys.stdin.readline()

        # Read body
        body = sys.stdin.read(length)
        if not body:
            break

        try:
            message = json.loads(body)
            handle_message(message)
        except json.JSONDecodeError:
            pass


if __name__ == "__main__":
    main()
