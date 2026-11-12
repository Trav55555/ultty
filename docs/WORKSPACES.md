# Launch profiles and saved workspaces

`ultty --profile NAME` starts a configured profile instead of restoring the last window. An unknown name is an error. A profile can launch ordinary OpenSSH commands, including aliases which resolve over a user-managed tailnet. SSH authentication and host-key prompts remain in the pane.

```toml
[profiles.development]
working_dir = "/home/me/project"

[[profiles.development.tabs]]
title = "Development"
command = "ssh"
args = ["-tt", "devbox"]

[[profiles.development.tabs.panes]]
command = "cargo"
args = ["watch", "-x", "test"]
split = "vertical"
```

Every configured pane splits the preceding pane in configuration order. `horizontal` means side by side and is the default; `vertical` means stacked. A pane's working directory overrides the tab's, which overrides the profile's. These are local launch directories, including when the command is SSH. Arguments retain their boundaries. All split directions are validated before any process is started.

`ultty --workspace NAME` opens a saved layout in a new window with fresh shells. It validates the complete layout before starting them. The window saves changes back to that named workspace when it exits. Workspace names accept up to 64 ASCII letters, digits, dots, dashes and underscores; `last` is reserved for automatic restoration.

A saved layout contains directories, titles, focus and split weights. It does not resume running processes or execute previously used command text. Explicit `ultty -e COMMAND ARGS...` still bypasses restoration and does not overwrite saved layouts. Profiles and workspaces cannot be combined with `-e` in one invocation.

Pane zoom changes the visible rectangle without rewriting the split tree. Hidden panes keep running at their last dimensions. Keyboard focus changes while zoomed show the newly focused pane; unzoom restores the stored split geometry. Swapping panes within a tab preserves process and pane identities. Moving live panes between tabs is not supported by this operation.
