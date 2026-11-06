# How agent tools sandbox the commands they run

Research notes for the [built-in sandboxing](../ISSUE_BACKLOG_30_60_90.md#explore-built-in-sandboxing)
candidate issue. Gathered 2026-11-06 from vendor docs, engineering posts,
source, and advisories. Most vendor pages are undated; treat them as current
on that day. Unconfirmed points are marked.

## Cursor

Source: [Agent sandboxing](https://cursor.com/en-US/blog/agent-sandboxing), 2026-02-18.

- **Mechanism.** On macOS, Seatbelt (`sandbox-exec`) with profiles generated at runtime. On Linux, Landlock for the filesystem plus seccomp for syscalls. On Windows, the Linux sandbox inside WSL2.
- **Alternatives considered.**
  - App Sandbox was rejected because every binary would need signing.
  - Containers were rejected because they run only Linux binaries.
  - VMs were rejected for their startup time and memory cost.
- **Default denies.**
  - Writes are denied to editor and agent config (`.vscode`, `.cursor`, `.code-workspace`, `.cursorignore`), `.git/config` and `.git/hooks`.
  - On Linux, files listed in `.cursorignore` are replaced with unreadable copies.
  - Network access usually needs approval; the post does not spell out the exact policy.
- **Lesson.** Agents retried blocked commands until the tool result named the constraint that blocked them. Sandboxed agents stop about 40% less often than unsandboxed ones.

## OpenAI Codex CLI

Sources:
- [Sandboxing](https://learn.chatgpt.com/docs/sandboxing)
- [Approvals and security](https://learn.chatgpt.com/docs/agent-approvals-security)
- [Linux sandbox README](https://github.com/openai/codex/blob/main/codex-rs/linux-sandbox/README.md)

Details:
- **Modes.** `read-only`, `workspace-write` (the default), and `danger-full-access`. Approval is `on-request` or `never`.
- **macOS.** Seatbelt, with one profile per mode.
- **Linux and WSL2.**
  - bubblewrap mounts `/` read-only and binds the writable roots read-write.
  - `.git`, including the target of a `gitdir:` file, and `.codex` are re-mounted read-only.
  - It always unshares the user namespace, and unshares PIDs by default.
  - When the network is restricted, it unshares the network namespace or bridges TCP to a proxy over a Unix socket.
  - seccomp sets `no_new_privs`, blocks network syscalls, and blocks `io_uring` in every mode.
  - Landlock is now legacy, because it cannot isolate Unix sockets.
- **Windows.** A native sandbox.
- **Network.** Off by default; `network_access = true` enables it. An optional proxy limits traffic to a domain allowlist.
- **Exceptions.** `writable_roots`, plus allow, prompt or forbid rules on command prefixes. `.git`, `.codex`, `.agents` and `.aws` stay protected.
- **Limits.**
  - It often fails inside Docker, which blocks user namespaces; the docs suggest using the container as the boundary instead.
  - The proxy does not cover web search, MCP or the model's own traffic.
  - CVE-2025-59532: a working directory chosen by the model could become the writable root. Fixed in 0.39.0.

## Google Antigravity

Source: [Terminal Sandbox](https://antigravity.google/docs/sandbox/).

- **Mechanism.** Linux namespaces on Linux, and Seatbelt on macOS. The CLI uses AppContainer on Windows 11 24H2 and later. No VM and no Docker.
- **Defaults.** Project folders are read-write and system directories are readable. `~/.ssh` and `.env` are blocked, and anything not mounted is invisible. No network except allowlisted domains.
- **When it applies.** On in the Default and Auto presets, off in Request Review and Turbo. Escalating a command to run on the host needs approval, and `command(...)` allow and deny rules also apply.
- **Limits.**
  - If the host blocks unprivileged user namespaces, the sandbox silently does nothing.
  - Pillar Security reached code execution through a native tool that runs outside the sandbox ([report](https://cyberscoop.com/google-antigravity-pillar-security-agent-sandbox-escape-remote-code-execution/)). Patched 2026-02-28.
- **Undocumented.** How browser actions are isolated.

## Apple

- **Seatbelt / `sandbox-exec`.**
  - Profiles are written in SBPL, a Scheme-like language. `sandbox_init` has been deprecated since 10.8, and SBPL is undocumented for third parties ([Apple DTS](https://developer.apple.com/forums/thread/777509)). Large profiles have a reported crash (FB16964888).
  - Despite that, Chrome, Codex, Cursor and Claude Code all depend on it. Apple can change it without notice.
- **App Sandbox.** Entitlement-based and needs a signed bundle, so it cannot confine arbitrary command-line binaries. Children inherit the sandbox (unverified here).
- **Containerization framework and the `container` CLI** (WWDC 2025; [container](https://github.com/apple/container), [containerization](https://github.com/apple/containerization)).
  - Each Linux container runs in its own lightweight VM on Virtualization.framework.
  - Needs Apple silicon and macOS 26.
  - Runs Linux binaries only, so it cannot sandbox the user's macOS tools.

## Anthropic Claude Code

Sources:
- [Sandboxing](https://code.claude.com/docs/en/sandboxing)
- [Beyond permission prompts](https://claude.com/resources/articles/beyond-permission-prompts-making-claude-code-more-secure-and-autonomous)

Details:
- **Mechanism.** The open-source `@anthropic-ai/sandbox-runtime`.
  - macOS uses Seatbelt.
  - Linux and WSL2 use bubblewrap with a network namespace; `socat` relays traffic to a proxy on the host.
  - An optional seccomp filter blocks Unix sockets.
- **Defaults.**
  - Off by default.
  - When on, writes are limited to the working directory, `$TMPDIR` and added directories.
  - Reads cover most of the disk, `~/.ssh` included, unless `denyRead` is set.
  - Protected paths: settings, hooks, `.git/hooks`, `.git/config`, shell rc files.
- **Network.** All traffic goes through a proxy that checks hostnames against an allowlist, which starts empty.
  - Cloud metadata and link-local addresses are refused.
  - Non-TCP traffic, and tools that ignore proxy variables, cannot connect.
- **Limits.**
  - If the sandbox is unavailable, commands run unsandboxed unless `failIfUnavailable` is set.
  - With defaults, it filters on hostname only and does not inspect TLS.
  - Broad `excludedCommands` let the agent write a file and then run it outside the sandbox.

## Options for Ultty

- **Linux: bubblewrap plus seccomp around the tab's child,** as Codex and Claude Code do. This needs unprivileged user namespaces, which Ubuntu 24.04 restricts through AppArmor and which containers often block. Landlock plus seccomp applied in-process between `fork` and `exec` needs no extra binary, but cannot isolate Unix sockets or the network by itself.
- **macOS: Seatbelt with a generated SBPL profile.** This is the only practical way to confine native tools. It is deprecated and undocumented, so it needs a canary test on every macOS release. Apple `container` VMs suit Linux workloads only.
- **Per tab.** Put the sandbox between the PTY child and the shell (`ultty` → sandbox → `$SHELL`). Every descendant then inherits it, and `setsid`, the controlling tty and job control keep working.
- **PID namespace trade-off.** It stops the shell signalling host processes. But the shell can no longer see them, and features that read `/proc/<pid>/cwd` (cwd tracking, `ulttyctl info`) need care.
- **Network.** Ultty would need its own proxy and allowlist. Non-TCP traffic and tools that ignore proxy variables will fail; document that.
- **Fail closed.** Show a visible "sandbox unavailable" state rather than silently running unsandboxed.
- **Protect Ultty itself.** This is the risk specific to Ultty. Deny the sandboxed process the tab's native-protocol socket, the control socket (`ulttyctl`), Ultty's config and shell rc files. Otherwise a sandboxed agent can drive the terminal or persist outside the sandbox.
- **Open risks.**
  - SBPL can break between macOS releases.
  - Unix sockets and D-Bus can leak access.
  - `/tmp` is shared.
  - Credentials are readable by default; deny `~/.ssh` and similar explicitly.
  - Users can grant broad exceptions.
