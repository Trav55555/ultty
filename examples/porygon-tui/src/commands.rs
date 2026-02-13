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
    /// Fetch the list of saved sessions from the daemon.
    FetchSessions,
    /// Resume a saved session by ID.
    ResumeSession { id: String },
    /// Save the current session with an optional name.
    SaveSession { name: Option<String> },
    /// Clear chat history (local only).
    ClearChat,
    /// Store a memory via the daemon.
    StoreMemory { text: String },
    /// Search memories via the daemon.
    RecallMemory { query: String },
    /// Fetch tool registry from daemon.
    FetchTools,
    /// Fetch details for a specific tool.
    FetchToolDetails { name: String },
    /// Fetch MCP server status.
    FetchMcpStatus,
    /// Fetch MCP tools list.
    FetchMcpTools,
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
                    description: "Save current session",
                    usage: "/save [name]",
                },
                Command {
                    name: "sessions",
                    description: "List saved sessions",
                    usage: "/sessions",
                },
                Command {
                    name: "resume",
                    description: "Resume a saved session",
                    usage: "/resume <id>",
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
            "resume" => {
                if args.is_empty() {
                    Some(CommandAction::SystemMessage(
                        "Usage: /resume <id>".to_string(),
                    ))
                } else {
                    Some(CommandAction::ResumeSession { id: args.join(" ") })
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
            "model" => Some(CommandAction::SystemMessage(
                "/model is not yet implemented.".to_string(),
            )),
            "models" => Some(CommandAction::SystemMessage(
                "/models is not yet implemented.".to_string(),
            )),
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
            _ => Some(CommandAction::SystemMessage(format!(
                "Unknown command: /{command_name}. Type /help for help."
            ))),
        }
    }

    /// Get command completions for a partial input.
    ///
    /// Returns all commands whose names start with the given partial string.
    #[allow(dead_code)]
    pub fn completions(&self, partial: &str) -> Vec<&Command> {
        let partial = partial.trim_start_matches('/');
        self.commands
            .iter()
            .filter(|cmd| cmd.name.starts_with(partial))
            .collect()
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

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}
