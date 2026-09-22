> Historical document, archived 2026-09-22. Not maintained.
> See [docs/ISSUE_BACKLOG_30_60_90.md](../ISSUE_BACKLOG_30_60_90.md) for current work.

# Feature Parity Plan: porygon-tui → Definitive AI Agent TUI

**Goal**: Transform porygon-tui from a proof-of-concept into the definitive AI agent terminal experience, surpassing OpenCode, OpenAI Codex CLI, and the existing Python Textual TUI.

**Competitive Advantage**: ultty's native protocol enables GPU-accelerated rendering, retained scene graphs, sub-cell-precision layout, and smooth animations — capabilities no ANSI-based TUI can match.

**Current State**: porygon-tui uses 4 of ~80+ Porygon daemon API endpoints. It has basic chat, tool approval, and status display. The Python TUI already has streaming, slash commands, file trees, and cost dashboards. We need to close the gap and then leap ahead.

---

## 1. Current State Audit

### 1.1 porygon-tui Current Capabilities

**Architecture**:
- 1,818 lines in single `main.rs` file
- 18 `Msg` variants, 10 views/panels
- Elm architecture with ulttui framework
- HTTP daemon integration (PORYGON_URL, defaults to `http://localhost:18789`)

**Features**:
- Splash screen with DockLayout
- Chat with markdown rendering (`MarkdownView`)
- Tool approval modal (risk score, blast radius, reversible flag)
- Sidebar panels:
  - Agent status (cost/budget/model/context quality/step)
  - Weather (braille chart)
  - Moon phase
  - System stats (CPU/RAM via libc)
  - Clock
  - Spinner animation
- Vim-mode text input
- Scroll support (PageUp/Down, mouse wheel)

**API Endpoints Used** (4 total):
- `POST /api/ask` — synchronous chat (blocks until full response)
- `GET /api/status` — agent status polling
- `GET /api/approvals/pending` — approval polling
- `POST /api/approvals/{id}/decide` — approval submission

### 1.2 Porygon AI Daemon — Full API Surface

The daemon exposes **17 API router groups** with **80+ endpoints**. porygon-tui currently uses only 4.

#### Currently Used (4 endpoints)
- ✅ `/api/ask` (POST) — synchronous chat
- ✅ `/api/status` (GET) — agent status polling
- ✅ `/api/approvals/pending` (GET) — approval polling
- ✅ `/api/approvals/{id}/decide` (POST) — approval submission

#### NOT Used (76+ endpoints — massive gap)

**Agent Core** (3 endpoints):
- ❌ `/agent/chat` (POST) — synchronous chat completion (alternative to /api/ask)
- ❌ `/agent/stream` (WebSocket) — streaming chat via WebSocket
- ❌ `/agent/core` (WebSocket) — AgentCore streaming (intent/planning)

**Memory** (3 endpoints):
- ❌ `/memory/search` (POST) — search semantic memory
- ❌ `/memory/remember` (POST) — store a note
- ❌ `/memory/recall` (POST) — recall notes by query

**Project** (3 endpoints):
- ❌ `/project/tree` (GET) — file tree visualization
- ❌ `/project/info` (GET) — project metadata
- ❌ `/project/switch` (POST) — switch project

**Tools** (3 endpoints):
- ❌ `/tools/list` (GET) — tool registry
- ❌ `/tools/{name}` (GET) — tool details
- ❌ `/tools/call` (POST) — manual tool invocation

**Validation** (7 endpoints):
- ❌ `/validation/command` (POST) — validate shell command
- ❌ `/validation/command/blocked` (GET) — check if command blocked
- ❌ `/validation/command/safe` (GET) — check if command safe
- ❌ `/validation/validators` (GET) — list validators
- ❌ `/validation/run` (POST) — run validation pipeline
- ❌ `/validation/challenge` (POST) — run specific validator
- ❌ `/validation/sanitize` (POST) — sanitize prompt

**User/Trainer** (4 endpoints):
- ❌ `/user/trainer` (GET) — trainer profile
- ❌ `/user/trainer/exp` (POST) — add experience
- ❌ `/user/trainer/style` (POST) — add style note
- ❌ `/user/trainer/context` (GET) — get trainer context

**Kanban** (6 endpoints):
- ❌ `/user/kanban` (GET) — get kanban board
- ❌ `/user/kanban/tasks` (GET) — list tasks
- ❌ `/user/kanban/tasks` (POST) — add task
- ❌ `/user/kanban/tasks/{id}` (PUT) — move task
- ❌ `/user/kanban/tasks/{id}` (DELETE) — delete task
- ❌ `/user/kanban/clear-done` (POST) — clear done tasks

**Party** (3 endpoints):
- ❌ `/user/party` (GET) — list agents
- ❌ `/user/party/{name}` (GET) — get agent
- ❌ `/user/party/{name}/status` (PUT) — set agent status

**Session** (10 endpoints):
- ❌ `/session` (GET) — get current session
- ❌ `/session/save` (POST) — save session
- ❌ `/session/resume` (GET) — get resume context
- ❌ `/session/list` (GET) — list all sessions
- ❌ `/session/compact` (POST) — generate compaction
- ❌ `/session/focus` (PUT) — set focus areas
- ❌ `/session/sticky` (POST) — add sticky file
- ❌ `/session/sticky` (DELETE) — remove sticky file
- ❌ `/session` (DELETE) — delete session
- ❌ `/session/increment-turns` (POST) — increment turns

**MCP** (8 endpoints):
- ❌ `/mcp/status` (GET) — get MCP status
- ❌ `/mcp/servers` (GET) — list servers
- ❌ `/mcp/tools` (GET) — list all MCP tools
- ❌ `/mcp/tools/{server}` (GET) — list server tools
- ❌ `/mcp/call` (POST) — call MCP tool
- ❌ `/mcp/connect` (POST) — connect server
- ❌ `/mcp/disconnect/{server}` (POST) — disconnect server
- ❌ `/mcp/reconnect-all` (POST) — reconnect all servers

**Events** (3 endpoints):
- ❌ `/events/stream` (WebSocket) — event stream
- ❌ `/events/subscribers` (GET) — get subscribers
- ❌ `/events/broadcast` (POST) — broadcast event

**Search** (2 endpoints):
- ❌ `/search/web` (POST) — web search
- ❌ `/search/ai` (POST) — AI-powered search

**Models** (4 endpoints):
- ❌ `/models/list` (GET) — list available models
- ❌ `/models/{id}` (GET) — get model info
- ❌ `/models/default` (GET) — get default model
- ❌ `/models/default` (PUT) — set default model

**Tasks** (5 endpoints):
- ❌ `/tasks/submit` (POST) — submit async task
- ❌ `/tasks/{id}` (GET) — get task status/result
- ❌ `/tasks` (GET) — list tasks
- ❌ `/tasks/{id}` (DELETE) — cancel task
- ❌ `/tasks/cleanup` (POST) — delete old tasks

**Cron** (7 endpoints):
- ❌ `/cron` (POST) — create cron job
- ❌ `/cron` (GET) — list cron jobs
- ❌ `/cron/{id}` (GET) — get cron job details
- ❌ `/cron/{id}/enable` (POST) — enable job
- ❌ `/cron/{id}/disable` (POST) — disable job
- ❌ `/cron/{id}/trigger` (POST) — run job now
- ❌ `/cron/{id}` (DELETE) — delete job

**Planner** (8 endpoints):
- ❌ `/planner/create` (POST) — create plan from objective
- ❌ `/planner/current` (GET) — get active plan
- ❌ `/planner/list` (GET) — list saved plans
- ❌ `/planner/{id}` (GET) — get specific plan
- ❌ `/planner/{id}/resume` (POST) — resume a plan
- ❌ `/planner/{id}/abandon` (POST) — abandon a plan
- ❌ `/planner/{id}` (DELETE) — delete a plan
- ❌ `/planner/current/advance` (POST) — advance to next step
- ❌ `/planner/current/complete` (POST) — mark step complete

### 1.3 Python TUI (Textual) — Already Has

The existing Python TUI (2,595 lines across 12 handler modules) already implements:

**Core Features**:
- ✅ Streaming via WebSocket (`tui_stream.py`, 555 lines)
- ✅ Tool call widgets with approval flows
- ✅ Thought bubbles, phase indicators, token counters
- ✅ File tree panel
- ✅ Cost dashboard
- ✅ Autocomplete + slash command registry (`tui_command_registry.py`)
- ✅ Auth flows
- ✅ Remote streaming (`tui_remote_stream.py`)

**Handler Modules** (12 total):
- `tui_stream.py` (555 lines) — WebSocket streaming
- `tui_command_registry.py` — slash commands
- `tui_remote_stream.py` — remote streaming
- 9 additional handlers for various features

---

## 2. Competitive Analysis

### 2.1 OpenCode (Go, Bubbletea) — Now Archived, Continued as "Crush"

**Key Features**:
- Multi-pane layout with resizable splits
- Real-time streaming chat (token-by-token rendering)
- Tool execution display with live output
- Session management (save/load/list)
- Cost tracking with budget alerts
- File diff viewer with syntax highlighting
- Inline code execution with sandboxing
- Keyboard-driven navigation (vim-like)
- Theme system
- LSP integration for code intelligence
- Git integration (diff, blame, status)

**Strengths**:
- Mature streaming implementation
- Rich developer tooling (LSP, git)
- Polished UX with vim-style navigation

**Weaknesses**:
- ANSI-based (grid-locked layout, no GPU acceleration)
- Archived (development moved to Crush)

### 2.2 OpenAI Codex CLI (Rust, Ratatui)

**Key Features**:
- Sandbox execution (container-based, Docker/Firecracker)
- Autonomy levels (suggest → auto-edit → full-auto)
- Streaming output with incremental rendering
- Approval flows with risk assessment
- Multi-file diff display
- Conversation history with branching
- Model selection
- Cost tracking
- Minimal, focused UI (chat + output, no sidebar bloat)

**Strengths**:
- Strong security model (sandboxing)
- Granular autonomy control
- Clean, minimal UX

**Weaknesses**:
- ANSI-based (grid-locked layout)
- Limited to OpenAI models

---

## 3. Feature Matrix

| Feature | porygon-tui | Python TUI | OpenCode | Codex CLI | Priority |
|---------|-------------|------------|----------|-----------|----------|
| **Core Chat** |
| Streaming responses | ❌ | ✅ | ✅ | ✅ | 🔴 P0 |
| Markdown rendering | ✅ | ✅ | ✅ | ✅ | — |
| Code blocks | 🔶 | ✅ | ✅ | ✅ | 🟡 P1 |
| Syntax highlighting | ❌ | ✅ | ✅ | ✅ | 🟡 P1 |
| Token-by-token display | ❌ | ✅ | ✅ | ✅ | 🔴 P0 |
| Abort/cancel mid-stream | ❌ | ✅ | ✅ | ✅ | 🔴 P0 |
| **Agent Control** |
| Tool approval | ✅ | ✅ | ✅ | ✅ | — |
| Risk assessment display | ✅ | ✅ | ✅ | ✅ | — |
| Autonomy levels | ❌ | ❌ | ❌ | ✅ | 🟢 P2 |
| Cost tracking | 🔶 | ✅ | ✅ | ✅ | 🟡 P1 |
| Budget alerts | ❌ | ✅ | ✅ | ❌ | 🟢 P2 |
| Phase indicators | 🔶 | ✅ | ✅ | ❌ | 🟡 P1 |
| **Context** |
| File tree | ❌ | ✅ | ✅ | ❌ | 🟡 P1 |
| Project switching | ❌ | ❌ | ❌ | ❌ | 🟢 P2 |
| Session save/load | ❌ | ✅ | ✅ | ❌ | 🔴 P0 |
| Session list | ❌ | ✅ | ✅ | ❌ | 🔴 P0 |
| Memory search | ❌ | ✅ | ❌ | ❌ | 🔴 P0 |
| Memory store | ❌ | ✅ | ❌ | ❌ | 🔴 P0 |
| **Developer Tools** |
| Diff viewer | ❌ | ❌ | ✅ | ✅ | 🟡 P1 |
| Code execution display | ❌ | ✅ | ✅ | ✅ | 🟡 P1 |
| LSP integration | ❌ | ❌ | ✅ | ❌ | 🟢 P3 |
| Git integration | ❌ | ❌ | ✅ | ❌ | 🟢 P3 |
| Sandbox execution | ❌ | ❌ | ❌ | ✅ | 🟢 P3 |
| **Planning** |
| Plan creation | ❌ | ❌ | ❌ | ❌ | 🟢 P2 |
| Step tracking | ❌ | ❌ | ❌ | ❌ | 🟢 P2 |
| Kanban board | ❌ | ❌ | ❌ | ❌ | 🟢 P2 |
| **Infrastructure** |
| WebSocket streaming | ❌ | ✅ | ✅ | ✅ | 🔴 P0 |
| Event bus subscription | ❌ | ❌ | ❌ | ❌ | 🟢 P2 |
| MCP server management | ❌ | ❌ | ❌ | ❌ | 🟡 P1 |
| Model selection | ❌ | ✅ | ✅ | ✅ | 🟢 P2 |
| **UX** |
| Keyboard shortcuts | 🔶 | ✅ | ✅ | ✅ | 🟡 P1 |
| Vim mode | 🔶 | ❌ | ✅ | ❌ | 🟡 P1 |
| Search | ❌ | ✅ | ✅ | ❌ | 🟢 P2 |
| Themes | ❌ | ✅ | ✅ | ❌ | 🟢 P2 |
| Responsive layout | 🔶 | ✅ | ✅ | ✅ | 🟡 P1 |
| Slash commands | ❌ | ✅ | ❌ | ❌ | 🔴 P0 |
| Command palette | ❌ | ❌ | ❌ | ❌ | 🟢 P2 |

**Legend**: ✅ Has it | 🔶 Partial | ❌ Missing

---

## 4. Framework-Level Gaps (ulttui)

These are **blocking issues** in the ulttui framework that prevent porygon-tui from implementing critical features. These must be fixed before the corresponding app features can be built.

| Gap | Impact | Effort | Details |
|-----|--------|--------|---------|
| No streaming text output | 🔴 CRITICAL | High | Token-by-token rendering impossible. Need `AppMessage::AppendContent` or incremental text updates in protocol. Currently, apps can only replace entire content regions. |
| No incremental list updates | 🔴 HIGH | Medium | Full re-render on every new message. Need protocol support for appending to existing content (e.g., `AppendListItem`). Long conversations cause performance degradation. |
| No custom subscription sources | 🟡 MEDIUM | Medium | Can't subscribe to WebSocket streams, file watchers, or external event sources. `Sub` only supports timers (`Sub::every`). Need `Sub::stream` or `Sub::custom`. |
| No diff rendering widget | 🟡 MEDIUM | Medium | No way to show file diffs with +/- coloring, line numbers, side-by-side or unified view. Critical for code review workflows. |
| Modal body is text-only | 🟡 LOW | Small | `Modal::body()` takes `String`, not `View`. Can't show rich approval details (code preview, file paths, structured data). Need `Modal::body_view()`. |
| No searchable Select widget | 🟡 LOW | Small | Can't filter long lists (tools, models, sessions). Need `Select::searchable()` or `Select::filterable()`. |
| No virtual scrolling / lazy loading | 🟡 LOW | Medium | Long conversations (100+ messages) will get slow. Need `List::virtual_scroll()` or `List::lazy()` to render only visible items. |
| No tree widget | 🟡 MEDIUM | Medium | File tree display needs a collapsible tree view with keyboard navigation (expand/collapse, arrow keys). Need `Tree` widget. |
| No animation primitives | 🟢 LOW | Medium | Can't animate transitions, loading states, or smooth scrolling. Need `Animation` API or `Transition` support. |
| No image widget | 🟢 LOW | Small | Can't display tool output screenshots, diagrams, or charts. Need `Image` widget with native protocol support. |

---

## 5. Implementation Phases

### Phase 1: Streaming Foundation (P0 — Highest Impact)

**Why first**: The single biggest UX gap. Every competitor streams. porygon-tui waits for full response (often 10-30 seconds). This is the most user-visible deficiency.

#### Framework Work (2-3 weeks)

**New Protocol Messages**:
- `AppendContent(region_id, content)` — append to existing content without replacing
- `StreamStart(region_id)` — mark region as streaming (show typing indicator)
- `StreamEnd(region_id)` — mark region as complete

**New Framework APIs**:
- `Cmd::websocket(url, on_message)` — connect to WebSocket, emit messages as they arrive
- `Sub::stream(url, on_message)` — subscribe to WebSocket stream
- `MarkdownView::streaming(bool)` — enable streaming mode (show cursor, auto-scroll)
- `List::append(item)` — append item without full re-render

**Terminal Work**:
- Handle `AppendContent` in `NativeSession`
- Support incremental text updates in scene graph
- Optimize rendering for append-only updates (don't recompute full layout)

#### App Work (1 week)

**Replace `/api/ask` with `/agent/stream`**:
- Connect to WebSocket on app start
- Send chat messages via WebSocket
- Receive tokens as they arrive
- Append tokens to chat view incrementally
- Show typing indicator during streaming
- Support abort/cancel mid-stream (`/agent/abort`)

**UX Improvements**:
- Auto-scroll to bottom as tokens arrive
- Show "Agent is typing..." indicator
- Display token count in real-time
- Smooth cursor animation at end of stream

**Estimated Effort**: 2-3 weeks (framework) + 1 week (app) = **3-4 weeks total**

---

### Phase 2: Session & Memory (P0 — Core Workflow)

**Why second**: Users need to save/resume work. Memory makes the agent smarter over time. This is table-stakes functionality that competitors already have.

#### Framework Work (None)

No framework changes needed. All work is app-level.

#### App Work (1-2 weeks)

**Session Management** (`/session/*` endpoints):
- Session list view (show all sessions with metadata)
- Session save (auto-save on exit, manual save with `/save`)
- Session load (resume from list or `/resume <id>`)
- Session fork (branch conversation)
- Session export (save to file)

**Memory System** (`/memory/*` endpoints):
- Memory search (semantic search across past conversations)
- Memory store (explicitly save important context)
- Memory recall (retrieve relevant memories for current task)
- Memory browser (view all stored memories)

**Slash Command System**:
- Command registry (map `/command` to handler)
- Input autocomplete (show available commands as user types)
- Command palette (Ctrl+P to search commands)
- Built-in commands:
  - `/save [name]` — save current session
  - `/resume [id]` — resume session
  - `/sessions` — list all sessions
  - `/remember <text>` — store memory
  - `/recall <query>` — search memories
  - `/forget <id>` — delete memory
  - `/help` — show command list

**UX Improvements**:
- Session metadata display (date, message count, cost)
- Session search (filter by keyword, date range)
- Memory relevance scoring (show why memory was recalled)

**Estimated Effort**: **1-2 weeks**

---

### Phase 3: Tool Visibility (P1 — Agent Transparency)

**Why third**: Users need to see what the agent is doing, not just the final answer. Tool calls are the agent's "hands" — they should be visible and inspectable.

#### Framework Work (1 week)

**Rich Modal Body**:
- Change `Modal::body(String)` to `Modal::body_view(impl View)`
- Support arbitrary view hierarchy in modal body
- Add `Modal::scrollable(bool)` for long content

**Collapsible/Expandable Sections**:
- New `Collapsible` widget
- `Collapsible::new(header, content, expanded)`
- Keyboard navigation (Enter to toggle, arrow keys to navigate)

#### App Work (1 week)

**Tool Execution Display**:
- Show tool calls as they happen (not just in approval modal)
- Display tool name, arguments, and result
- Collapsible tool call history
- Color-code by status (pending, approved, rejected, completed, failed)

**Tool Registry Browser** (`/tools/*` endpoints):
- List all available tools
- Show tool details (name, description, parameters, examples)
- Manual tool invocation (`/tools/call`)
- Tool usage statistics (how often each tool is used)

**Enhanced Approval Modal**:
- Rich content display (code preview, file paths, structured data)
- Syntax highlighting for code
- File diff preview for file modifications
- Collapsible sections for long content
- Keyboard shortcuts (y/n for approve/reject, j/k to scroll)

**MCP Server Status Panel** (`/mcp/*` endpoints):
- List all MCP servers
- Show server status (running, stopped, error)
- Start/stop servers
- View server logs
- Server health indicators

**Estimated Effort**: 1 week (framework) + 1 week (app) = **2 weeks total**

---

### Phase 4: Project Context (P1 — Developer Productivity)

**Why fourth**: Developers need to see and navigate their project. File trees and diffs are essential for understanding what the agent is doing.

#### Framework Work (2 weeks)

**Tree Widget**:
- New `Tree` widget with collapsible nodes
- `Tree::new(root)` with `TreeNode` hierarchy
- Keyboard navigation (arrow keys, Enter to expand/collapse)
- Lazy loading (load children on expand)
- Search/filter support
- Icon support (file type icons)

**Diff Widget**:
- New `Diff` widget for side-by-side or unified diffs
- `Diff::new(old, new, mode)` where mode is `Unified` or `SideBySide`
- Syntax highlighting
- Line numbers
- +/- coloring
- Scroll synchronization (side-by-side mode)
- Jump to next/prev change

#### App Work (1 week)

**File Tree Panel** (`/project/tree` endpoint):
- Display project file tree
- Expand/collapse directories
- File type icons
- Highlight modified files
- Click to open file (future: integrate with editor)

**Project Info Display** (`/project/info` endpoint):
- Show project name, path, language, framework
- Display project statistics (file count, line count, size)
- Show git status (branch, uncommitted changes)

**Project Switching** (`/project/switch` endpoint):
- List recent projects
- Switch to different project
- Auto-save current session on switch

**File Diff Viewer**:
- Show diffs for tool outputs (file modifications)
- Side-by-side or unified view
- Syntax highlighting
- Jump to next/prev change
- Approve/reject changes

**Estimated Effort**: 2 weeks (framework) + 1 week (app) = **3 weeks total**

---

### Phase 5: Planning & Tasks (P2 — Workflow Management)

**Why fifth**: Planning and task management are power-user features. They're important for complex workflows but not critical for basic usage.

#### Framework Work (None)

No framework changes needed. All work is app-level.

#### App Work (1-2 weeks)

**Plan Creation and Tracking** (`/planner/*` endpoints):
- Create plan from goal
- View plan steps
- Execute plan
- Track plan progress
- Pause/resume plan execution
- Modify plan steps

**Kanban Board View** (`/user/kanban/*` endpoints):
- Display kanban board with columns (To Do, In Progress, Done)
- Drag-and-drop tasks between columns
- Create/edit/delete tasks
- Task metadata (priority, assignee, due date)
- Filter/search tasks

**Async Task Queue Display** (`/tasks/*` endpoints):
- List all async tasks
- Show task status (pending, running, completed, failed)
- Cancel running tasks
- View task results
- Cleanup completed tasks

**Cron Job Management** (`/cron/*` endpoints):
- List all cron jobs
- Create/edit/delete cron jobs
- Enable/disable cron jobs
- Run cron job manually
- View cron job history

**Estimated Effort**: **1-2 weeks**

---

### Phase 6: Advanced UX (P2 — Polish)

**Why sixth**: These are nice-to-have features that improve the overall experience but aren't critical for core functionality.

#### Framework Work (1-2 weeks)

**Virtual Scrolling**:
- `List::virtual_scroll(bool)` — render only visible items
- Optimize for long lists (1000+ items)
- Smooth scrolling with momentum

**Searchable Select Widget**:
- `Select::searchable(bool)` — enable search/filter
- Fuzzy matching
- Keyboard navigation (type to filter, arrow keys to select)

**Animation Primitives**:
- `Animation` API for smooth transitions
- `Transition::fade()`, `Transition::slide()`, `Transition::scale()`
- Easing functions (linear, ease-in, ease-out, ease-in-out)
- Animation callbacks (on_start, on_end)

#### App Work (1 week)

**Model Selection** (`/models/*` endpoints):
- List all available models
- Show model details (name, provider, context window, cost)
- Set default model
- Switch model mid-conversation

**Web Search Integration** (`/search/*` endpoints):
- Web search command (`/search <query>`)
- AI-powered search (`/ai-search <query>`)
- Display search results
- Click to open in browser

**Trainer Profile Display** (`/user/trainer` endpoint):
- Show trainer profile (experience, style, context)
- Update trainer settings
- View training history

**Agent Party Management** (`/user/party/*` endpoints):
- List agents in party
- Add/remove agents
- View agent details
- Switch active agent

**Validation Pipeline UI** (`/validation/*` endpoints):
- Run security scan
- Run quality analysis
- Run compliance check
- View validation reports
- Display validation results with severity levels

**Event Stream Subscription** (`/events/stream` WebSocket):
- Subscribe to real-time event bus
- Display events as they happen
- Filter events by type
- Event history

**Estimated Effort**: 1-2 weeks (framework) + 1 week (app) = **2-3 weeks total**

---

### Phase 7: Competitive Advantages (P3 — Differentiation)

**Why last**: These are features that only ultty can do. They're the long-term differentiators that make porygon-tui the definitive AI agent TUI.

#### Unique Capabilities (ultty native protocol only)

**GPU-Accelerated Diff Rendering**:
- Smooth scrolling through large diffs (1000+ lines)
- Sub-cell-precision alignment (not grid-locked)
- Syntax highlighting with GPU shaders
- Instant rendering (no ANSI parsing overhead)

**Native Image Display**:
- Tool output screenshots
- Diagrams and charts
- Inline images in chat
- Image preview in file tree
- GPU-accelerated image rendering

**Sub-Cell-Precision Layout**:
- Pixel-perfect alignment (not constrained to character grid)
- Smooth animations (fade, slide, scale)
- Responsive layout with smooth resizing
- No layout jank

**Smooth Animations**:
- Loading indicators with smooth rotation
- Fade transitions between views
- Slide-in/slide-out panels
- Smooth scrolling with momentum
- Typing indicator with smooth cursor animation

**Split Pane with Native Protocol Regions**:
- Each pane is an independent native protocol region
- No interference between panes
- Smooth resizing with live preview
- Drag-and-drop between panes

**Retained Scene Graph**:
- Zero-cost idle (no re-render when nothing changes)
- Incremental updates (only changed regions re-render)
- Smooth 60fps animations
- Low memory footprint

**Estimated Effort**: **Ongoing** (incremental improvements over time)

---

## 6. Protocol Extensions Needed

| Extension | Purpose | Phase | Details |
|-----------|---------|-------|---------|
| `AppendContent` | Incremental text updates for streaming | 1 | Append to existing content without replacing. Enables token-by-token rendering. |
| `StreamStart` | Mark region as streaming | 1 | Show typing indicator, auto-scroll, prepare for incremental updates. |
| `StreamEnd` | Mark region as complete | 1 | Hide typing indicator, finalize content, stop auto-scroll. |
| `TreeContent` | Collapsible tree widget data | 4 | Hierarchical data structure with expand/collapse state. |
| `DiffContent` | Unified/side-by-side diff data | 4 | Old/new content with line numbers, +/- markers, syntax highlighting. |
| `RichModal` | Modal with View body instead of String | 3 | Support arbitrary view hierarchy in modal body. |
| `ImageContent` | Native image display | 7 | Image data (PNG, JPEG, SVG) with GPU rendering. |
| `AnimationContent` | Animation primitives | 6 | Transition effects (fade, slide, scale) with easing functions. |

---

## 7. Summary Table

| Phase | Description | Framework Work | App Work | Effort | Impact |
|-------|-------------|----------------|----------|--------|--------|
| 1 | Streaming Foundation | High (protocol + widgets) | High (WebSocket integration) | 3-4 wk | 🔴 Critical |
| 2 | Session & Memory | None | Medium (endpoints + slash commands) | 1-2 wk | 🔴 Critical |
| 3 | Tool Visibility | Medium (rich modal + collapsible) | Medium (tool display + MCP) | 2 wk | 🟡 High |
| 4 | Project Context | High (tree + diff widgets) | Medium (file tree + diffs) | 3 wk | 🟡 High |
| 5 | Planning & Tasks | None | Medium (planner + kanban + cron) | 1-2 wk | 🟢 Medium |
| 6 | Advanced UX | Medium (virtual scroll + search + animation) | Medium (models + search + validation) | 2-3 wk | 🟢 Medium |
| 7 | Differentiation | Low (leverage existing protocol) | Medium (GPU features) | Ongoing | 🟢 Medium |

**Total Estimated Effort**: 12-16 weeks (3-4 months)

**Critical Path**: Phase 1 (Streaming) → Phase 2 (Session & Memory) → Phase 3 (Tool Visibility) → Phase 4 (Project Context)

**Parallelization Opportunities**:
- Phase 5 (Planning & Tasks) can start after Phase 2 (no framework dependencies)
- Phase 6 (Advanced UX) can be worked on incrementally alongside other phases
- Phase 7 (Differentiation) is ongoing and can be worked on opportunistically

---

## 8. Success Metrics

### 8.1 Feature Parity Metrics

| Metric | Current | Target | Timeline |
|--------|---------|--------|----------|
| API endpoints used | 4 / 80+ | 60+ / 80+ | 3 months |
| Streaming latency | N/A (no streaming) | < 50ms | Phase 1 |
| Session management | ❌ | ✅ | Phase 2 |
| Tool visibility | 🔶 (approval only) | ✅ (full display) | Phase 3 |
| File tree | ❌ | ✅ | Phase 4 |
| Diff viewer | ❌ | ✅ | Phase 4 |
| Planning | ❌ | ✅ | Phase 5 |

### 8.2 Competitive Advantage Metrics

| Metric | ANSI TUIs | porygon-tui Target | Advantage |
|--------|-----------|-------------------|-----------|
| Frame render time | 5-10ms | < 1ms | 5-10x faster |
| Streaming latency | 50-100ms | < 50ms | 2x faster |
| Diff scroll smoothness | Janky (grid-locked) | Smooth (GPU) | Qualitative |
| Image display | ❌ (ASCII art only) | ✅ (native) | Unique |
| Animation smoothness | Janky (ANSI redraw) | Smooth (GPU) | Qualitative |
| Idle CPU usage | 1-5% (constant redraw) | 0% (retained scene) | 100% reduction |

### 8.3 User Experience Metrics

| Metric | Current | Target | Timeline |
|--------|---------|--------|----------|
| Time to first token | N/A (no streaming) | < 200ms | Phase 1 |
| Session save/load time | N/A | < 500ms | Phase 2 |
| File tree load time | N/A | < 1s | Phase 4 |
| Diff render time (1000 lines) | N/A | < 100ms | Phase 4 |
| Keyboard shortcut coverage | 🔶 (basic) | ✅ (comprehensive) | Phase 6 |

---

## 9. Risk Assessment

### 9.1 Technical Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| WebSocket streaming complexity | Medium | High | Start with simple implementation, iterate. Use existing Rust WebSocket libraries (tokio-tungstenite). |
| Framework API design mistakes | Medium | High | Prototype in examples first. Get feedback before committing to API. |
| Protocol extension compatibility | Low | Medium | Version protocol messages. Maintain backward compatibility. |
| Performance degradation (long conversations) | Medium | Medium | Implement virtual scrolling early. Profile and optimize. |
| Scope creep | High | Medium | Stick to phases. Don't add features outside the plan. |

### 9.2 Resource Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Underestimated effort | Medium | Medium | Add 20% buffer to estimates. Re-evaluate after Phase 1. |
| Framework work blocks app work | High | High | Prioritize framework work. Parallelize where possible. |
| Daemon API changes | Low | Medium | Version API endpoints. Maintain compatibility with older daemon versions. |

### 9.3 User Experience Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Streaming feels laggy | Low | High | Optimize rendering pipeline. Target < 50ms latency. |
| Too many features overwhelm users | Medium | Medium | Progressive disclosure. Hide advanced features behind keyboard shortcuts. |
| Keyboard shortcuts conflict | Medium | Low | Document all shortcuts. Make them configurable. |

---

## 10. Next Steps

### 10.1 Immediate Actions (Week 1)

1. **Prototype WebSocket streaming** in a standalone example
2. **Design `AppendContent` protocol message** and get feedback
3. **Spike on `Sub::stream` API** for WebSocket subscriptions
4. **Create Phase 1 task breakdown** with detailed subtasks

### 10.2 Short-Term Goals (Month 1)

1. **Complete Phase 1** (Streaming Foundation)
2. **Start Phase 2** (Session & Memory)
3. **Validate streaming performance** (< 50ms latency)
4. **Get user feedback** on streaming UX

### 10.3 Medium-Term Goals (Month 2-3)

1. **Complete Phase 2-4** (Session, Tools, Project Context)
2. **Start Phase 5-6** (Planning, Advanced UX)
3. **Conduct competitive analysis** (compare against OpenCode, Codex CLI)
4. **Refine roadmap** based on user feedback

### 10.4 Long-Term Goals (Month 4+)

1. **Complete Phase 5-6** (Planning, Advanced UX)
2. **Begin Phase 7** (Competitive Advantages)
3. **Public beta release**
4. **Gather user feedback** and iterate

---

## 11. Conclusion

porygon-tui has the foundation to become the definitive AI agent TUI. The ulttui framework provides a solid base, and ultty's native protocol offers capabilities no ANSI-based TUI can match.

**The gap is real**: We're using 4 of 80+ daemon endpoints. The Python TUI already has streaming, slash commands, and file trees. OpenCode and Codex CLI set the bar for agent transparency and developer tooling.

**The path is clear**: Seven phases, 12-16 weeks, with a focus on streaming first (the biggest UX gap), then session management, tool visibility, and project context.

**The advantage is ours**: GPU rendering, retained scene graphs, sub-cell layout, and smooth animations. Once we close the feature gap, we'll have a TUI that no competitor can match.

**The goal is achievable**: This is not a rewrite. It's a systematic expansion of an already-working system. Each phase is independently valuable. Each phase builds on the last.

**The time is now**: This project will be the main terminal experience going forward. It should be the peak of AI agent TUI usability and capability. Let's make it happen.
