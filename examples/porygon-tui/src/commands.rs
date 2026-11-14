//! Slash command system for porygon-tui.
//!
//! Provides a command registry that parses and executes slash commands,
//! returning actions that the main app can handle.

use serde_json::Value;

/// Represents an action to be taken in response to a command.
#[derive(Debug, Clone)]
pub enum CommandAction {
    /// Display text as a system chat message.
    SystemMessage(String),
    /// Send a request to the daemon (for FP 2.2+).
    #[allow(dead_code)]
    DaemonRequest {
        endpoint: String,
        method: String,
        body: Option<Value>,
    },
    /// Display a toast notification.
    #[allow(dead_code)]
    ShowToast(String),
    /// Combine multiple actions.
    #[allow(dead_code)]
    Batch(Vec<CommandAction>),
    /// Open the canonical conversation picker.
    FetchSessions,
    /// Open a canonical conversation by ID without submitting.
    ResumeSession { id: String },
    /// Save the current session with an optional name.
    SaveSession { name: Option<String> },
    /// Fork a session (or current session if omitted).
    ForkSession { id: Option<String> },
    /// Export a session by ID.
    ExportSession { id: String },
    /// Clear chat history (local only).
    ClearChat,
    /// Store a memory via the daemon.
    StoreMemory { text: String },
    /// Search memories via the daemon.
    RecallMemory { query: String },
    /// Browse recent memories via the daemon.
    FetchMemoryBrowse,
    /// Fetch tool registry from daemon.
    FetchTools,
    /// Fetch details for a specific tool.
    FetchToolDetails { name: String },
    /// Fetch MCP server status.
    FetchMcpStatus,
    /// Fetch MCP tools list.
    FetchMcpTools,
    /// Fetch the project file tree from daemon, optionally scoped to a subpath.
    FetchFileTree { path: Option<String> },
    /// Fetch project metadata from daemon.
    FetchProjectInfo,
    /// Switch to a different project directory.
    SwitchProject { path: String },
    /// Create a new plan with an objective.
    CreatePlan { objective: String },
    /// Fetch the current active plan.
    FetchCurrentPlan,
    /// Advance to the next step of the current plan.
    AdvancePlan,
    /// Fetch the kanban board.
    FetchKanban,
    /// Add a task to the kanban board.
    AddKanbanTask { title: String },
    /// Mark a kanban task as done.
    CompleteKanbanTask { id: String },
    /// Fetch the async task queue.
    FetchAsyncTasks,
    /// Fetch details for a specific async task.
    FetchAsyncTaskDetail { id: String },
    /// Cancel an async task.
    CancelAsyncTask { id: String },
    /// Fetch cron jobs list.
    FetchCronJobs,
    /// Trigger a cron job immediately.
    TriggerCronJob { id: String },
    /// Toggle a cron job enabled/disabled.
    ToggleCronJob { id: String },
    /// Fetch available models list.
    FetchModels,
    /// Set the active model.
    SetModel { id: String },
    /// Perform a web search.
    WebSearch { query: String },
    /// Perform an AI-summarized search.
    AiSearch { query: String },
    /// Fetch trainer profile.
    FetchProfile,
    /// Fetch agent party list.
    FetchParty,
    /// Validate a command for safety.
    ValidateCommand { command: String },
    /// Run the full validation pipeline.
    RunGym,
    /// Show rolling streaming performance metrics.
    ShowPerfMetrics,
    /// Benchmark app-level virtual scrolling windows.
    BenchmarkVirtualScroll,
    /// Benchmark diff parsing/render prep for large diffs.
    BenchmarkDiffWidget,
    /// Trigger immediate runtime reconnect checks.
    ReconnectRuntime,
    /// Open the pending decision view.
    ShowApprovals,
    /// Inspect an existing durable run without submitting a turn.
    AttachRun { id: String },
}

/// Metadata for a registered command.
#[derive(Debug, Clone)]
pub struct Command {
    /// Command name (without the `/` prefix).
    pub name: &'static str,
    /// Short description of what the command does.
    pub description: &'static str,
    /// Usage string showing how to invoke the command.
    pub usage: &'static str,
}

/// Registry of available slash commands.
pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    /// Create a new command registry with all built-in commands.
    pub fn new() -> Self {
        Self {
            commands: vec![
                Command {
                    name: "help",
                    description: "Show available commands",
                    usage: "/help",
                },
                Command {
                    name: "save",
                    description: "Legacy transcript save",
                    usage: "/save [name]",
                },
                Command {
                    name: "sessions",
                    description: "Open conversation picker",
                    usage: "/sessions",
                },
                Command {
                    name: "approvals",
                    description: "Open pending decisions",
                    usage: "/approvals",
                },
                Command {
                    name: "attach",
                    description: "Inspect a durable run without resubmitting",
                    usage: "/attach <run-id>",
                },
                Command {
                    name: "resume",
                    description: "Open a conversation without submitting",
                    usage: "/resume <id>",
                },
                Command {
                    name: "fork",
                    description: "Legacy transcript fork",
                    usage: "/fork [id]",
                },
                Command {
                    name: "export",
                    description: "Legacy transcript export",
                    usage: "/export <id>",
                },
                Command {
                    name: "remember",
                    description: "Store a memory",
                    usage: "/remember <text>",
                },
                Command {
                    name: "recall",
                    description: "Search memories",
                    usage: "/recall <query>",
                },
                Command {
                    name: "memory",
                    description: "Browse recent memories",
                    usage: "/memory",
                },
                Command {
                    name: "model",
                    description: "Switch model",
                    usage: "/model <name>",
                },
                Command {
                    name: "models",
                    description: "List available models",
                    usage: "/models",
                },
                Command {
                    name: "search",
                    description: "Web search",
                    usage: "/search <query>",
                },
                Command {
                    name: "ai-search",
                    description: "AI-summarized search",
                    usage: "/ai-search <query>",
                },
                Command {
                    name: "profile",
                    description: "Trainer profile",
                    usage: "/profile",
                },
                Command {
                    name: "party",
                    description: "List agents",
                    usage: "/party",
                },
                Command {
                    name: "validate",
                    description: "Validate command safety",
                    usage: "/validate <command>",
                },
                Command {
                    name: "gym",
                    description: "Run validation pipeline",
                    usage: "/gym",
                },
                Command {
                    name: "perf",
                    description: "Show stream perf metrics",
                    usage: "/perf",
                },
                Command {
                    name: "bench-scroll",
                    description: "Benchmark virtual scroll",
                    usage: "/bench-scroll",
                },
                Command {
                    name: "bench-diff",
                    description: "Benchmark diff parsing",
                    usage: "/bench-diff",
                },
                Command {
                    name: "reconnect",
                    description: "Reconnect runtime checks",
                    usage: "/reconnect",
                },
                Command {
                    name: "clear",
                    description: "Clear chat history",
                    usage: "/clear",
                },
                Command {
                    name: "tools",
                    description: "List available tools",
                    usage: "/tools",
                },
                Command {
                    name: "tool",
                    description: "Show tool details",
                    usage: "/tool <name>",
                },
                Command {
                    name: "mcp",
                    description: "MCP server status",
                    usage: "/mcp",
                },
                Command {
                    name: "mcp-tools",
                    description: "List MCP tools",
                    usage: "/mcp-tools",
                },
                Command {
                    name: "tree",
                    description: "Show project file tree",
                    usage: "/tree [path]",
                },
                Command {
                    name: "scan",
                    description: "Scan project file tree",
                    usage: "/scan [path]",
                },
                Command {
                    name: "expand",
                    description: "Expand/load tree path",
                    usage: "/expand <path>",
                },
                Command {
                    name: "project",
                    description: "Show project info",
                    usage: "/project",
                },
                Command {
                    name: "switch",
                    description: "Switch project directory",
                    usage: "/switch <path>",
                },
                Command {
                    name: "plan",
                    description: "Create a plan",
                    usage: "/plan <goal>",
                },
                Command {
                    name: "current",
                    description: "Show current plan",
                    usage: "/current",
                },
                Command {
                    name: "next",
                    description: "Advance plan step",
                    usage: "/next",
                },
                Command {
                    name: "kanban",
                    description: "Show kanban board",
                    usage: "/kanban",
                },
                Command {
                    name: "add",
                    description: "Add kanban task",
                    usage: "/add <task>",
                },
                Command {
                    name: "done",
                    description: "Complete kanban task",
                    usage: "/done <id>",
                },
                Command {
                    name: "tasks",
                    description: "List async tasks",
                    usage: "/tasks",
                },
                Command {
                    name: "task",
                    description: "Show task details",
                    usage: "/task <id>",
                },
                Command {
                    name: "cancel-task",
                    description: "Cancel async task",
                    usage: "/cancel-task <id>",
                },
                Command {
                    name: "cron",
                    description: "List cron jobs",
                    usage: "/cron",
                },
                Command {
                    name: "cron-run",
                    description: "Trigger cron job",
                    usage: "/cron-run <id>",
                },
                Command {
                    name: "cron-toggle",
                    description: "Toggle cron job",
                    usage: "/cron-toggle <id>",
                },
            ],
        }
    }

    /// Parse a user input string and return a command action if it's a valid command.
    ///
    /// If the input starts with `/`, it's treated as a command. If the command
    /// is not found, returns a SystemMessage with an error. Otherwise, returns None.
    pub fn parse(&self, input: &str) -> Option<CommandAction> {
        if !input.starts_with('/') {
            return None;
        }

        let trimmed = input.trim_start_matches('/');
        let parts: Vec<&str> = trimmed.split_whitespace().collect();

        if parts.is_empty() {
            return Some(CommandAction::SystemMessage(
                "Empty command. Type /help for help.".to_string(),
            ));
        }

        let command_name = parts[0];
        let args = &parts[1..];

        match command_name {
            "help" => Some(self.handle_help()),
            "save" => {
                let name = if args.is_empty() {
                    None
                } else {
                    Some(args.join(" "))
                };
                Some(CommandAction::SaveSession { name })
            }
            "sessions" => Some(CommandAction::FetchSessions),
            "approvals" => Some(CommandAction::ShowApprovals),
            "attach" if args.len() == 1 => Some(CommandAction::AttachRun {
                id: args[0].to_string(),
            }),
            "attach" => Some(CommandAction::SystemMessage(
                "Usage: /attach <run-id>".into(),
            )),
            "resume" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /resume <id>".to_string(),
                    ))
                } else {
                    Some(CommandAction::ResumeSession { id: args.join(" ") })
                }
            }
            "fork" => {
                let id = if args.is_empty() {
                    None
                } else {
                    Some(args.join(" "))
                };
                Some(CommandAction::ForkSession { id })
            }
            "export" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /export <id>".to_string(),
                    ))
                } else {
                    Some(CommandAction::ExportSession { id: args.join(" ") })
                }
            }
            "remember" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /remember <text>".to_string(),
                    ))
                } else {
                    Some(CommandAction::StoreMemory {
                        text: args.join(" "),
                    })
                }
            }
            "recall" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /recall <query>".to_string(),
                    ))
                } else {
                    Some(CommandAction::RecallMemory {
                        query: args.join(" "),
                    })
                }
            }
            "memory" => Some(CommandAction::FetchMemoryBrowse),
            "models" => Some(CommandAction::FetchModels),
            "model" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /model <id>".to_string(),
                    ))
                } else {
                    Some(CommandAction::SetModel { id: args.join(" ") })
                }
            }
            "search" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /search <query>".to_string(),
                    ))
                } else {
                    Some(CommandAction::WebSearch {
                        query: args.join(" "),
                    })
                }
            }
            "ai-search" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /ai-search <query>".to_string(),
                    ))
                } else {
                    Some(CommandAction::AiSearch {
                        query: args.join(" "),
                    })
                }
            }
            "profile" => Some(CommandAction::FetchProfile),
            "party" => Some(CommandAction::FetchParty),
            "validate" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /validate <command>".to_string(),
                    ))
                } else {
                    Some(CommandAction::ValidateCommand {
                        command: args.join(" "),
                    })
                }
            }
            "gym" => Some(CommandAction::RunGym),
            "perf" => Some(CommandAction::ShowPerfMetrics),
            "bench-scroll" => Some(CommandAction::BenchmarkVirtualScroll),
            "bench-diff" => Some(CommandAction::BenchmarkDiffWidget),
            "reconnect" => Some(CommandAction::ReconnectRuntime),
            "clear" => Some(CommandAction::ClearChat),
            "tools" => Some(CommandAction::FetchTools),
            "tool" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /tool <name>".to_string(),
                    ))
                } else {
                    Some(CommandAction::FetchToolDetails {
                        name: args.join(" "),
                    })
                }
            }
            "mcp" => Some(CommandAction::FetchMcpStatus),
            "mcp-tools" => Some(CommandAction::FetchMcpTools),
            "tree" | "scan" => {
                let path = if args.is_empty() {
                    None
                } else {
                    Some(args.join(" "))
                };
                Some(CommandAction::FetchFileTree { path })
            }
            "expand" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /expand <path>".to_string(),
                    ))
                } else {
                    Some(CommandAction::FetchFileTree {
                        path: Some(args.join(" ")),
                    })
                }
            }
            "project" => Some(CommandAction::FetchProjectInfo),
            "switch" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /switch <path>".to_string(),
                    ))
                } else {
                    Some(CommandAction::SwitchProject {
                        path: args.join(" "),
                    })
                }
            }
            "plan" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /plan <goal>".to_string(),
                    ))
                } else {
                    Some(CommandAction::CreatePlan {
                        objective: args.join(" "),
                    })
                }
            }
            "current" => Some(CommandAction::FetchCurrentPlan),
            "next" => Some(CommandAction::AdvancePlan),
            "kanban" => Some(CommandAction::FetchKanban),
            "add" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /add <task>".to_string(),
                    ))
                } else {
                    Some(CommandAction::AddKanbanTask {
                        title: args.join(" "),
                    })
                }
            }
            "done" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /done <id>".to_string(),
                    ))
                } else {
                    Some(CommandAction::CompleteKanbanTask { id: args.join(" ") })
                }
            }
            "tasks" => Some(CommandAction::FetchAsyncTasks),
            "task" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /task <id>".to_string(),
                    ))
                } else {
                    Some(CommandAction::FetchAsyncTaskDetail { id: args.join(" ") })
                }
            }
            "cancel-task" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /cancel-task <id>".to_string(),
                    ))
                } else {
                    Some(CommandAction::CancelAsyncTask { id: args.join(" ") })
                }
            }
            "cron" => Some(CommandAction::FetchCronJobs),
            "cron-run" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /cron-run <id>".to_string(),
                    ))
                } else {
                    Some(CommandAction::TriggerCronJob { id: args.join(" ") })
                }
            }
            "cron-toggle" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /cron-toggle <id>".to_string(),
                    ))
                } else {
                    Some(CommandAction::ToggleCronJob { id: args.join(" ") })
                }
            }
            _ => Some(CommandAction::SystemMessage(format!(
                "Unknown command: /{command_name}. Type /help for help."
            ))),
        }
    }

    /// Rank commands by case-insensitive name, alias, or description match.
    pub fn completions(&self, partial: &str) -> Vec<&Command> {
        let query = partial.trim_start_matches('/').trim().to_lowercase();
        let mut matches: Vec<_> = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(index, cmd)| {
                let alias = match cmd.name {
                    "sessions" => "conversations history resume",
                    "approvals" => "attention decisions permission",
                    "models" => "model provider",
                    "project" => "workspace directory cwd",
                    _ => "",
                };
                let score = [cmd.name, alias, cmd.description]
                    .iter()
                    .enumerate()
                    .filter_map(|(field, text)| {
                        fuzzy_score(&text.to_lowercase(), &query).map(|score| score + field * 100)
                    })
                    .min()?;
                Some((score, index, cmd))
            })
            .collect();
        matches.sort_by_key(|(score, index, _)| (*score, *index));
        matches.into_iter().map(|(_, _, cmd)| cmd).collect()
    }

    /// Handle the /help command.
    fn handle_help(&self) -> CommandAction {
        let mut help_text = String::from("**Available Commands:**\n\n");

        for cmd in &self.commands {
            help_text.push_str(&format!(
                "• `{}` — {}\n  Usage: `{}`\n\n",
                cmd.name, cmd.description, cmd.usage
            ));
        }

        help_text.push_str("Type a command to execute it.");

        CommandAction::SystemMessage(help_text)
    }
}

fn fuzzy_score(text: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    if text == query {
        return Some(0);
    }
    if text.starts_with(query) {
        return Some(1);
    }
    if text.contains(query) {
        return Some(10);
    }
    let mut chars = query.chars();
    let mut next = chars.next()?;
    let mut first = None;
    for (index, ch) in text.chars().enumerate() {
        if ch == next {
            first.get_or_insert(index);
            if let Some(ch) = chars.next() {
                next = ch;
            } else {
                return Some(20 + index.saturating_sub(first.unwrap_or(0)));
            }
        }
    }
    None
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandAction, CommandRegistry};

    #[test]
    fn command_search_ranks_names_and_supports_case_and_aliases() {
        let registry = CommandRegistry::new();
        assert_eq!(registry.completions("MoD")[0].name, "model");
        assert!(registry
            .completions("mdl")
            .iter()
            .any(|c| c.name == "model"));
        assert!(registry
            .completions("permission")
            .iter()
            .any(|c| c.name == "approvals"));
        assert!(registry.completions("zzzxq").is_empty());
    }

    #[test]
    fn parse_perf_command_returns_show_perf_action() {
        let registry = CommandRegistry::new();
        assert!(matches!(
            registry.parse("/perf"),
            Some(CommandAction::ShowPerfMetrics)
        ));
    }

    #[test]
    fn parse_tree_with_path_returns_scoped_fetch() {
        let registry = CommandRegistry::new();
        assert!(matches!(
            registry.parse("/tree crates/ulttui/src"),
            Some(CommandAction::FetchFileTree {
                path: Some(path)
            }) if path == "crates/ulttui/src"
        ));
    }

    #[test]
    fn parse_export_requires_id() {
        let registry = CommandRegistry::new();
        assert!(matches!(
            registry.parse("/export abc123"),
            Some(CommandAction::ExportSession { id }) if id == "abc123"
        ));
    }
}
