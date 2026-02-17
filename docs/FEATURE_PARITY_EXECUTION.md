# Feature Parity Execution Plan

**Source**: Architectural review comparing porygon-tui against Porygon AI daemon, OpenCode, and Codex CLI.

**Principle**: Each chunk is ≤1 day, independently mergeable, and leaves tests green. Phases build on each other but chunks within a phase are independent where possible.

**Companion documents**:
- `docs/FEATURE_PARITY_PLAN.md` — Architectural overview and rationale
- `docs/TECH_DEBT_PLAN.md` — Code cleanup (separate from feature work)

---

## Phase 1: Streaming Foundation (P0)

The single biggest UX gap. Every competitor streams. porygon-tui waits for full response (often 10-30 seconds). This is the most user-visible deficiency.

### 1.1 Protocol: Add `AppendContent` message

**File(s)**: `crates/ultty-protocol/src/messages.rs`, `crates/ultty-protocol/src/codec.rs`

**Problem/Goal**: The protocol has no way to append to existing content. `SetContent` replaces entirely. Streaming requires incremental updates.

**Work**:
1. Add `AppendContent { id: RegionId, content: Content }` variant to `AppMessage` enum in `messages.rs`
2. Assign tag `0x0015` (next available after `SetRegionMetadata` at `0x0014`)
3. Add encode/decode in `codec.rs` — follows same pattern as `SetContent` (tag + region_id + content payload)
4. Add roundtrip test in `codec.rs` tests: encode `AppendContent`, decode, verify equality

**Done when**: `cargo test -p ultty-protocol` passes. `AppMessage::AppendContent` encodes and decodes correctly.

---

### 1.2 Protocol: Add `StreamStart` / `StreamEnd` control messages

**File(s)**: `crates/ultty-protocol/src/messages.rs`, `crates/ultty-protocol/src/codec.rs`

**Problem/Goal**: Terminal needs to know when a region is actively streaming (to show typing indicator, auto-scroll).

**Work**:
1. Add `StreamStart { id: RegionId }` and `StreamEnd { id: RegionId }` to `AppMessage` enum
2. Assign tags `0x0016` and `0x0017`
3. Encode/decode: just tag + region_id (4 bytes payload each)
4. Add roundtrip tests for both messages

**Done when**: Both messages encode/decode. `cargo test -p ultty-protocol` passes.

---

### 1.3 Terminal: Handle `AppendContent` in `NativeSession`

**File(s)**: `crates/ultty/src/protocol/session.rs`, `crates/ultty/src/scene/region.rs`

**Problem/Goal**: `NativeSession::handle_message` doesn't handle `AppendContent`. The scene tree needs an append operation.

**Work**:
1. In `session.rs`, add match arm for `AppMessage::AppendContent { id, content }` in `handle_message()`
2. For `Content::Text(text)`: append to existing text content. If region currently has `Content::Text(existing)`, set content to `Content::Text(existing + new)`. If region has `Content::None`, set content to new.
3. For `Content::RichText(rt)`: append spans to existing RichText spans vec.
4. Mark `render_dirty = true` (not `constraints_dirty` — position doesn't change, only content grows)
5. Add test: create region, SetContent with "Hello ", AppendContent with "World" → region content is "Hello World"

**Done when**: `cargo test -p ultty` passes. Append test verifies content accumulation.

---

### 1.4 Terminal: Handle `StreamStart`/`StreamEnd` state

**File(s)**: `crates/ultty/src/protocol/session.rs`

**Problem/Goal**: Terminal needs to track which regions are streaming. This enables future UX (typing indicator, auto-scroll hint).

**Work**:
1. Add `streaming_regions: HashSet<RegionId>` field to `NativeSession`
2. On `StreamStart { id }`: insert into set. On `StreamEnd { id }`: remove from set.
3. Add `pub fn is_streaming(&self, id: RegionId) -> bool` accessor
4. Add test: send StreamStart, verify `is_streaming()` returns true, send StreamEnd, verify false

**Done when**: `cargo test -p ultty` passes. `is_streaming()` returns correct state after start/end.

---

### 1.5 Framework: Add `Cmd::Stream` for WebSocket connections

**File(s)**: `crates/ulttui/src/cmd.rs`, `crates/ulttui/src/app.rs`, `crates/ulttui/Cargo.toml`

**Problem/Goal**: `Cmd` only has `Spawn` (one-shot async). We need a way to open a long-lived WebSocket connection that emits messages over time.

**Work**:
1. Add `tokio-tungstenite` and `futures-util` dependencies to `ulttui/Cargo.toml`
2. Add new variant to `Cmd<M>`:
   ```rust
   Stream {
       id: String,
       url: String,
       on_message: Box<dyn Fn(String) -> M + Send + 'static>,
       on_error: Box<dyn Fn(String) -> M + Send + 'static>,
       on_close: Box<dyn Fn() -> M + Send + 'static>,
   }
   ```
3. Add `Cmd::stream(id, url, on_message, on_error, on_close)` constructor
4. In `app.rs` event loop, when `Cmd::Stream` is returned: spawn a tokio task that connects via `tokio-tungstenite`, reads messages in a loop, and sends them back via a channel to the main event loop
5. Store active stream handles in a `HashMap<String, JoinHandle>` on the runner
6. Add `Cmd::CloseStream { id: String }` variant to close a stream by ID

**Done when**: `cargo check -p ulttui` compiles. Unit test verifies Cmd::Stream variant exists.

---

### 1.6 Framework: Add `Sub::Stream` for persistent WebSocket subscriptions

**File(s)**: `crates/ulttui/src/cmd.rs`, `crates/ulttui/src/app.rs`

**Problem/Goal**: `Sub` only supports timer intervals. Apps need to subscribe to external event sources (WebSocket, SSE).

**Work**:
1. Add variant to `Sub<M>`:
   ```rust
   Stream {
       id: String,
       url: String,
       on_message: Box<dyn Fn(String) -> M + Send + 'static>,
   }
   ```
2. Add `Sub::stream(id, url, on_message)` constructor
3. In `app.rs` subscription reconciliation: compare current stream subs with previous. Open new connections, close removed ones.
4. Stream messages arrive as events that get dispatched through update()
5. Add test: create Sub::Stream, verify it matches correctly in reconciliation

**Done when**: `cargo check -p ulttui` compiles. Basic test: Sub::Stream variant matches correctly.

---

### 1.7 Framework: Use `AppendContent` in native backend

**File(s)**: `crates/ulttui/src/backend/native.rs`

**Problem/Goal**: The native backend currently does full-frame replacement every render cycle. For streaming, it should be able to send `AppendContent` for incremental updates instead of rebuilding the entire view tree.

**Work**:
1. Add `pub fn append_content(&mut self, region_id: RegionId, content: Content) -> anyhow::Result<()>` method to `NativeBackend`
2. This sends `AppMessage::AppendContent { id: region_id, content }` over the socket
3. No frame wrapping needed — AppendContent should be immediate (outside BeginFrame/EndFrame)
4. This is a building block — the actual streaming widget will call this in Phase 1.9

**Done when**: Method exists and compiles. `cargo check -p ulttui` passes.

---

### 1.8 Framework: Add `MarkdownView::streaming()` mode

**File(s)**: `crates/ulttui/src/widgets/markdown.rs`

**Problem/Goal**: MarkdownView parses the full markdown string on every render. For streaming, we need a mode that accepts incremental text and re-parses efficiently.

**Work**:
1. Add `streaming: bool` field to `MarkdownView` (default false)
2. Add `.streaming(bool) -> Self` builder method
3. When `streaming == true`:
   - Append a blinking cursor (▊) to the rendered output
   - Set `overflow_y: Hidden` (auto-scroll handled by framework)
4. When streaming mode is toggled off: remove cursor, finalize content
5. This is cosmetic for now — actual incremental parsing is a later optimization

**Done when**: `cargo test -p ulttui` passes. `MarkdownView::new("test").streaming(true)` compiles and shows cursor.

---

### 1.9 App: Add WebSocket client for `/agent/stream`

**File(s)**: `examples/porygon-tui/src/main.rs`, `examples/porygon-tui/Cargo.toml`

**Problem/Goal**: porygon-tui uses `POST /api/ask` (sync, waits for full response). Need to use `/agent/stream` WebSocket for streaming.

**Work**:
1. Add `tokio-tungstenite` + `futures-util` to porygon-tui Cargo.toml
2. Add new `Msg` variants: `StreamToken(String)`, `StreamComplete`, `StreamError(String)`, `AbortStream`
3. Replace the `Cmd::spawn(send_to_daemon(...))` in `Msg::SendMessage` with `Cmd::stream(...)` that connects to `ws://{daemon_url}/agent/stream`
4. Each WebSocket message from daemon is a JSON with `{ "type": "token", "content": "..." }` or `{ "type": "done" }`
5. `Msg::StreamToken`: append token to last assistant message content
6. `Msg::StreamComplete`: finalize message, mark `waiting_response = false`
7. Keep the old HTTP fallback for daemons that don't support WebSocket (try WS first, fall back to POST)

**Done when**: `cargo check -p porygon-tui` passes. When connected to a daemon with WebSocket support, tokens stream in real-time.

---

### 1.10 App: Streaming UX (typing indicator, auto-scroll, abort)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: During streaming, the UX should show partial response growing, auto-scroll to bottom, and allow Ctrl+C or Escape to abort.

**Work**:
1. During streaming: show a growing MarkdownView in the last message card (re-render incrementally)
2. Auto-scroll: after each `StreamToken`, call `self.auto_scroll_chat()` if near bottom
3. Abort: on `Escape` or `Ctrl+C` during `waiting_response`, send `Cmd::CloseStream("agent")` and `Msg::AbortStream`
4. Show token count in status bar during streaming: `"streaming: {n} tokens"`
5. Update hex_gauge spinner to show during streaming

**Done when**: Streaming messages appear token-by-token. Escape aborts. Token count shows in status bar.

---

## Phase 2: Session & Memory (P0)

Users need to save/resume work. Memory makes the agent smarter over time. This is table-stakes functionality that competitors already have.

### 2.1 App: Slash command parser

**File(s)**: `examples/porygon-tui/src/main.rs` (new: `examples/porygon-tui/src/commands.rs`)

**Problem/Goal**: No slash command system exists. User input starting with `/` should be intercepted as commands.

**Work**:
1. Create `commands.rs` with a `CommandRegistry` struct
2. `CommandRegistry` holds a `HashMap<String, CommandHandler>` where `CommandHandler` is `Box<dyn Fn(&str) -> Option<Msg>>`
3. Register built-in commands: `/help`, `/save`, `/resume`, `/sessions`, `/remember`, `/recall`, `/model`, `/models`
4. In `Msg::SendMessage`: check if input starts with `/`. If so, parse command name + args, look up in registry, execute handler.
5. `/help` returns a system message listing all commands

**Done when**: Typing `/help` shows command list instead of sending to daemon. `cargo check -p porygon-tui` passes.

---

### 2.2 App: Session list and resume (`/sessions`, `/resume`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No session management. Need to list, save, and resume conversations.

**Work**:
1. Add `Msg` variants: `SessionsFetched(Vec<SessionInfo>)`, `SessionResumed(Vec<ChatMessage>)`, `SessionSaved`, `SessionError(String)`
2. Add `SessionInfo` struct: `{ id: String, name: String, date: String, message_count: usize }`
3. `/sessions` command: `GET /session/list` → display as system message (formatted table)
4. `/resume <id>` command: `GET /session/{id}` → load messages into `self.messages`
5. `/save [name]` command: `POST /session/save` with current messages serialized

**Done when**: `/sessions` lists sessions from daemon. `/resume <id>` loads a previous conversation. `/save` persists current conversation.

---

### 2.3 App: Memory commands (`/remember`, `/recall`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No memory system integration. Porygon daemon has semantic memory at `/memory/*`.

**Work**:
1. Add `Msg` variants: `MemoryStored`, `MemoryRecalled(Vec<MemoryResult>)`, `MemoryError(String)`
2. Add `MemoryResult` struct: `{ content: String, score: f64, timestamp: String }`
3. `/remember <text>` command: `POST /memory/remember` with body `{ "content": "<text>" }`
4. `/recall <query>` command: `POST /memory/recall` with body `{ "query": "<query>" }` → display results as system message
5. `/search-memory <query>` command: `POST /memory/search` → display results

**Done when**: `/remember test note` stores a memory. `/recall test` retrieves it. Works when daemon is connected.

---

### 2.4 App: Auto-save session on exit

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: Conversations are lost when the app exits.

**Work**:
1. Add `session_id: Option<String>` field to `PorygonTui` (set when resuming or after first save)
2. On clean exit (Ctrl+C → confirmation → exit): fire `POST /session/save` with current messages
3. If session_id is Some, update existing session. If None, create new session.
4. Show "Session saved" toast on success

**Done when**: Exiting the app saves the session. Re-launching and `/resume` brings back the conversation.

---

### 2.5 App: Input autocomplete for slash commands

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: Users don't know what commands are available. Show suggestions as they type `/`.

**Work**:
1. When input starts with `/`, filter command registry for matches
2. Show matching commands as a small overlay below the input (use RichText with dim styling)
3. Tab to select suggestion, Enter to execute
4. Implementation: add `suggestions: Vec<String>` field to PorygonTui, update on every input change
5. Render suggestions as a Column below input (max 5 items, scrollable)

**Done when**: Typing `/s` shows `/save`, `/sessions`, `/search-memory`. Tab completes.

---

## Phase 3: Tool Visibility (P1)

Users need to see what the agent is doing, not just the final answer. Tool calls are the agent's "hands" — they should be visible and inspectable.

### 3.1 Framework: Rich modal body (View instead of String)

**File(s)**: `crates/ulttui/src/widgets/modal.rs`

**Problem/Goal**: `Modal::body()` takes `String`. Tool approval needs rich content (code blocks, colored text, structured data).

**Work**:
1. Add `body_view: Option<Box<dyn View>>` field to `Modal`
2. Add `.body_view(view: impl View + 'static) -> Self` builder method
3. In native rendering (`render_child_native`): if `body_view` is Some, render it instead of the text body
4. In ANSI rendering (`render_ansi`): if `body_view` is Some, render it into the modal body area
5. Keep existing `.body(String)` working — it creates a Text widget internally
6. Update snapshot tests

**Done when**: `Modal::new().body_view(Column::new().child(Text::new("rich")))` renders correctly. All existing modal tests pass.

---

### 3.2 Framework: Collapsible widget

**File(s)**: new `crates/ulttui/src/widgets/collapsible.rs`, `crates/ulttui/src/widgets/mod.rs`

**Problem/Goal**: No way to show expandable/collapsible sections. Needed for tool call details, long outputs.

**Work**:
1. Create `Collapsible` widget: `Collapsible::new(header: impl View, body: impl View)`
2. Add `.expanded(bool) -> Self` builder (default false)
3. Add `.id(id) -> Self` for focus management
4. Render: when collapsed, show header + `▸` chevron. When expanded, show header + `▾` chevron + body.
5. Native rendering: two regions (header + body). Body region has `display: None` when collapsed.
6. ANSI rendering: conditionally render body area
7. Add to `mod.rs` exports
8. Add snapshot tests for collapsed and expanded states

**Done when**: `Collapsible::new(Text::new("Header"), Text::new("Body")).expanded(false)` renders header only. `.expanded(true)` renders both. Tests pass.

---

### 3.3 App: Tool execution display in chat

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: Tool calls are invisible — only the approval modal shows. Users need to see tool execution as it happens.

**Work**:
1. Add `ToolCall` struct: `{ name: String, args: String, status: ToolStatus, result: Option<String> }`
2. Add `ToolStatus` enum: `Pending`, `Approved`, `Rejected`, `Running`, `Completed`, `Failed`
3. Add `tool_calls: Vec<ToolCall>` field to `PorygonTui`
4. When approval received: add ToolCall with status Pending
5. When approval decided: update status to Approved/Rejected
6. Render tool calls in messages_view as Collapsible sections with:
   - Header: `🔧 {tool_name} [{status}]` with status-colored text
   - Body: args + result (when completed)
7. Poll tool results from daemon status updates

**Done when**: Tool calls appear inline in the chat as collapsible cards with status indicators.

---

### 3.4 App: Enhanced approval modal with rich content

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: Approval modal body is plain text. Need rich content (syntax highlighted code, colored risk, structured params).

**Work**:
1. Replace `self.approval_modal_body(req)` (returns String) with a View-based approach
2. Use `Modal::body_view(...)` from 3.1
3. Build a Column with:
   - Tool name in bold
   - Risk score as colored badge (green < 0.3, yellow < 0.7, red ≥ 0.7)
   - Blast radius + reversible as RichText
   - Reasoning in dim text
   - Parameters as MarkdownView (code block)
4. Add keyboard shortcuts display: `y/Enter: Approve | n/Esc: Reject`

**Done when**: Approval modal shows structured, colored content instead of plain text.

---

### 3.5 App: Tool registry browser (`/tools`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No visibility into available tools. `/tools` command should show the tool registry.

**Work**:
1. Add `/tools` slash command
2. `GET /tools/list` → display as system message with formatted table
3. Add `/tool <name>` command: `GET /tools/{name}` → show tool details (description, parameters, examples)
4. Add `Msg::ToolsFetched(Vec<ToolInfo>)` variant

**Done when**: `/tools` lists all available tools from daemon. `/tool <name>` shows details.

---

### 3.6 App: MCP server status panel

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No visibility into MCP servers. Need a panel or command to show MCP status.

**Work**:
1. Add `/mcp` slash command: `GET /mcp/status` → display server list with connection status
2. Add `/mcp-tools` command: `GET /mcp/tools` → list all available MCP tools
3. Add `Msg::McpStatusFetched(McpStatus)` variant
4. Optionally: add MCP status indicator to sidebar (connected server count)

**Done when**: `/mcp` shows server status. `/mcp-tools` lists available tools.

---

## Phase 4: Project Context (P1)

Developers need to see and navigate their project. File trees and diffs are essential for understanding what the agent is doing.

### 4.1 Framework: Tree widget

**File(s)**: new `crates/ulttui/src/widgets/tree.rs`, `crates/ulttui/src/widgets/mod.rs`

**Problem/Goal**: No tree widget for file tree display.

**Work**:
1. Create `TreeNode` struct: `{ label: String, children: Vec<TreeNode>, expanded: bool, icon: Option<String> }`
2. Create `Tree` widget: `Tree::new(root: TreeNode)`
3. Add `.id(id) -> Self` for focus management
4. Render: indented list with `▸`/`▾` for expand/collapse, `├──` tree guides
5. Native rendering: flatten tree into Column of RichText lines with indentation
6. ANSI rendering: same flattening into ratatui Paragraph lines
7. Keyboard: `Enter` to toggle expand/collapse, `j`/`k` or arrow keys to navigate
8. Add `selected_path: Vec<usize>` for tracking cursor position
9. Add to `mod.rs` exports
10. Add snapshot tests

**Done when**: `Tree::new(root)` renders with expand/collapse. Keyboard navigation works. Tests pass.

---

### 4.2 Framework: Diff widget

**File(s)**: new `crates/ulttui/src/widgets/diff.rs`, `crates/ulttui/src/widgets/mod.rs`

**Problem/Goal**: No way to render file diffs with +/- coloring.

**Work**:
1. Create `DiffLine` struct: `{ kind: DiffKind, content: String, old_line: Option<usize>, new_line: Option<usize> }`
2. Create `DiffKind` enum: `Context`, `Added`, `Removed`, `Header`
3. Create `DiffView` widget: `DiffView::new(lines: Vec<DiffLine>)`
4. Render: colored lines (green for added, red for removed, dim for context, bold for headers)
5. Line numbers in gutter
6. Native rendering: Column of RichText lines
7. ANSI rendering: ratatui styled lines
8. Add `.mode(DiffMode)` builder: `Unified` (default), `SideBySide` (future)
9. Add to `mod.rs` exports
10. Add snapshot tests

**Done when**: `DiffView::new(lines)` renders colored diff. Tests pass.

---

### 4.3 App: File tree panel (`/tree`, `/scan`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No project file tree visibility.

**Work**:
1. Add `/tree` slash command: `GET /project/tree` → parse response into `TreeNode` hierarchy
2. Display file tree as a system message using `Tree` widget (from 4.1) rendered as formatted text
3. Add `/scan` as alias for `/tree`
4. Add `Msg::FileTreeFetched(TreeNode)` variant
5. Optionally: add file tree as a sidebar panel (toggled with `/tree`)

**Done when**: `/tree` shows project file tree from daemon. Directories are expandable.

---

### 4.4 App: Project info display (`/project`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No project metadata display.

**Work**:
1. Add `/project` slash command: `GET /project/info` → display project metadata
2. Show: project name, path, language, framework, file count
3. Add `/switch <path>` command: `POST /project/switch` → switch project
4. Add `Msg::ProjectInfoFetched(ProjectInfo)` variant

**Done when**: `/project` shows project info. `/switch <path>` changes project context.

---

### 4.5 App: File diff viewer for tool outputs

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: When tools modify files, users can't see what changed.

**Work**:
1. When a tool call result contains a diff (detected by `---`/`+++` or `@@` markers), render it using `DiffView` from 4.2
2. Parse unified diff format into `Vec<DiffLine>`
3. Show diff in a Collapsible section within the tool call display (from 3.3)
4. Add diff parsing utility function: `fn parse_unified_diff(text: &str) -> Vec<DiffLine>`

**Done when**: Tool outputs containing diffs render with colored +/- lines instead of plain text.

---

## Phase 5: Planning & Tasks (P2)

Planning and task management are power-user features. They're important for complex workflows but not critical for basic usage.

### 5.1 App: Plan creation and tracking (`/plan`, `/next`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No planning integration.

**Work**:
1. Add `/plan <goal>` command: `POST /planner/create` with `{ "objective": "<goal>" }` → display plan steps
2. Add `/current` command: `GET /planner/current` → show active plan with step status
3. Add `/next` command: `POST /planner/current/advance` → advance to next step
4. Add `Msg::PlanCreated(Plan)`, `Msg::PlanFetched(Plan)`, `Msg::PlanAdvanced` variants
5. Add `Plan` struct: `{ id: String, objective: String, steps: Vec<PlanStep> }`
6. Add `PlanStep` struct: `{ description: String, status: String }`
7. Display plan as numbered list with status indicators (✅/🔄/⏳)

**Done when**: `/plan <goal>` creates a plan. `/current` shows it. `/next` advances.

---

### 5.2 App: Kanban board (`/kanban`, `/add`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No task management.

**Work**:
1. Add `/kanban` command: `GET /user/kanban` → display board as formatted text (To Do | In Progress | Done columns)
2. Add `/add <task>` command: `POST /user/kanban/tasks` with `{ "title": "<task>" }`
3. Add `/done <id>` command: `PUT /user/kanban/tasks/{id}` to move task to Done
4. Add `Msg::KanbanFetched(Board)`, `Msg::TaskAdded`, `Msg::TaskMoved` variants

**Done when**: `/kanban` shows board. `/add` creates tasks. `/done` completes them.

---

### 5.3 App: Async task queue display (`/tasks`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No async task visibility.

**Work**:
1. Add `/tasks` command: `GET /tasks` → list tasks with status
2. Add `/task <id>` command: `GET /tasks/{id}` → show task details and result
3. Add `/cancel-task <id>` command: `DELETE /tasks/{id}` → cancel running task
4. Add `Msg::TasksFetched(Vec<TaskInfo>)` variant

**Done when**: `/tasks` lists async tasks. `/cancel-task` cancels them.

---

### 5.4 App: Cron job management (`/cron`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No cron job visibility.

**Work**:
1. Add `/cron` command: `GET /cron` → list cron jobs with status (enabled/disabled, next run)
2. Add `/cron-run <id>` command: `POST /cron/{id}/trigger` → run job now
3. Add `/cron-toggle <id>` command: toggle enable/disable
4. Add `Msg::CronFetched(Vec<CronJob>)` variant

**Done when**: `/cron` lists jobs. `/cron-run` triggers manual execution.

---

## Phase 6: Advanced UX (P2)

Nice-to-have features that improve the overall experience but aren't critical for core functionality.

### 6.1 App: Model selection (`/models`, `/model`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: Can't see or change the AI model.

**Work**:
1. Add `/models` command: `GET /models/list` → display available models with details
2. Add `/model <id>` command: `PUT /models/default` → set default model
3. Show current model in sidebar (already shown, but add ability to change)
4. Add `Msg::ModelsFetched(Vec<ModelInfo>)`, `Msg::ModelChanged` variants

**Done when**: `/models` lists models. `/model <id>` changes the active model.

---

### 6.2 App: Web search integration (`/search`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No web search capability.

**Work**:
1. Add `/search <query>` command: `POST /search/web` → display search results
2. Add `/ai-search <query>` command: `POST /search/ai` → display AI-summarized results
3. Results displayed as system messages with title, URL, and snippet
4. Add `Msg::SearchResults(Vec<SearchResult>)` variant

**Done when**: `/search <query>` returns web results. `/ai-search` returns summarized results.

---

### 6.3 App: Trainer profile and event stream

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No trainer profile visibility, no real-time event subscription.

**Work**:
1. Add `/profile` command: `GET /user/trainer` → display trainer profile (XP, style, context)
2. Add `/party` command: `GET /user/party` → list agents
3. Optionally: subscribe to `/events/stream` WebSocket for real-time notifications (using Sub::Stream from 1.6)
4. Show event notifications as toasts

**Done when**: `/profile` shows trainer info. `/party` lists agents.

---

### 6.4 App: Validation pipeline (`/validate`, `/gym`)

**File(s)**: `examples/porygon-tui/src/main.rs`

**Problem/Goal**: No validation UI.

**Work**:
1. Add `/validate <command>` command: `POST /validation/command` → show if command is safe/blocked
2. Add `/gym` command: `POST /validation/run` → run full validation pipeline, display results
3. Results shown with severity-colored indicators

**Done when**: `/validate ls -rf /` shows risk assessment. `/gym` runs pipeline.

---

### 6.5 Framework: Virtual scrolling for long conversations

**File(s)**: `crates/ulttui/src/widgets/scroll.rs`, `crates/ulttui/src/widgets/list.rs`

**Problem/Goal**: Long conversations (100+ messages) cause performance degradation because all message views are rendered every frame.

**Work**:
1. Add `virtual_scroll: bool` field to ScrollView (default false)
2. When enabled: only render children that are within the visible viewport (±1 page buffer)
3. Use `content_height` and `scroll_y` to calculate visible range
4. For native rendering: only create regions for visible children
5. For ANSI rendering: only render visible items in the ratatui frame

**Done when**: ScrollView with 1000 children and `virtual_scroll(true)` only renders ~50 visible items. Performance test shows constant render time regardless of list length.

---

## Phase 7: Competitive Advantages (P3)

Features that only ultty can do. These are the long-term differentiators that make porygon-tui the definitive AI agent TUI.

### 7.1 Terminal: Streaming cursor animation

**File(s)**: `crates/ultty/src/renderer/native_render.rs`

**Problem/Goal**: No visual feedback during streaming in the terminal renderer.

**Work**:
1. Check `NativeSession::is_streaming(region_id)` during render
2. For streaming regions: append a blinking block cursor (▊) after the last text character
3. Use a timer-based animation (toggle visibility every 500ms)

**Done when**: Streaming regions show a blinking cursor in the GPU renderer.

---

### 7.2 Terminal: Native image widget rendering

**File(s)**: `crates/ultty/src/renderer/native_render.rs`

**Problem/Goal**: Image widget exists in protocol but GPU rendering may not display tool output screenshots.

**Work**:
1. Verify `WidgetContent::Image` renders correctly in the GPU renderer
2. If not: add GPU texture upload + quad rendering for image content
3. Support PNG and JPEG decode (via `image` crate, likely already a dependency)
4. Scale to fit region bounds while maintaining aspect ratio

**Done when**: `Image::new(rgba_data, width, height)` renders correctly in ultty's GPU renderer.

---

### 7.3 App: porygon-tui module split

**File(s)**: `examples/porygon-tui/src/main.rs` → split into modules

**Problem/Goal**: main.rs is now approaching 2000+ lines with all the new features. Needs modularization.

**Work**:
1. Create `examples/porygon-tui/src/commands.rs` — slash command registry + handlers
2. Create `examples/porygon-tui/src/daemon.rs` — all HTTP/WebSocket client functions
3. Create `examples/porygon-tui/src/sidebar.rs` — sidebar_view, weather_view, moon_view, cassette_view
4. Create `examples/porygon-tui/src/views.rs` — messages_view, input_view, header_view, status_bar_view
5. Create `examples/porygon-tui/src/types.rs` — ChatMessage, AgentStatus, WeatherData, ApprovalRequest, etc.
6. Keep `main.rs` as Application impl + update + on_event (~300 lines)

**Done when**: `main.rs` is under 400 lines. `cargo run -p porygon-tui` still compiles and runs.

---

## Summary Table

| Phase | Chunks | Est. Hours | Risk if Skipped | Dependencies |
|-------|--------|-----------|-----------------|--------------|
| 1. Streaming Foundation | 10 | 30-40 | Dead-on-arrival vs competitors | None |
| 2. Session & Memory | 5 | 10-15 | Users lose all conversation history | Phase 1 (streaming) |
| 3. Tool Visibility | 6 | 12-15 | Agent actions are opaque | Phase 1 (streaming) |
| 4. Project Context | 5 | 15-20 | No code navigation or diff viewing | Phase 3 (collapsible) |
| 5. Planning & Tasks | 4 | 8-10 | Power-user features missing | Phase 2 (slash commands) |
| 6. Advanced UX | 5 | 10-15 | Missing model selection, search | Phase 2 (slash commands) |
| 7. Competitive Advantages | 3 | 8-10 | Lose ultty's unique value prop | Phase 1 (streaming) |
| **Total** | **38** | **~93-125** | | |

**Critical Path**: Phase 1 → Phase 2 → Phase 3 → Phase 4

**Parallelization Opportunities**:
- Phase 5 (Planning & Tasks) can start after Phase 2 (no framework dependencies)
- Phase 6 (Advanced UX) can be worked on incrementally alongside other phases
- Phase 7 (Competitive Advantages) is ongoing and can be worked on opportunistically

**Estimated Total Time**: 12-16 weeks (3-4 months)
