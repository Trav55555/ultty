# Track C4: tmux Control-Mode (`-CC`) Client Scope and Risk Assessment

Status: **Scoped (no build yet)** — 2026-11-23. Plan item C4 of
[docs/agent-tui-plan.md](../agent-tui-plan.md).

## Question

Can Ultty's in-band native protocol (`ESC _ U ... ESC \`, [ADR 0018](../adr/0018-in-band-native-transport.md))
and inline surfaces ([ADR 0020](../adr/0020-inline-surfaces.md)) work inside `tmux`, and what is the scope
and risk of making Ultty a `tmux -CC` control-mode client?

## Empirical Verification (`tmux 3.6`, 2026-11-23)

Tested directly against a local isolated `tmux 3.6` server (`tmux -L <sock> -f /dev/null`)
attached with `tmux -CC attach-session`:

1. **Control-mode detection:** `tmux list-clients -F '#{client_name} control=#{client_control_mode}'`
   reported `control=1` for the attached `-CC` client.
2. **Pane-to-client APC delivery (`%output`):** A shell running inside pane `%0` printed a bare
   APC (`printf '\033_UTEST\033\\'`). Without `allow-passthrough`, the control client received
   the raw octal-escaped bytes attributed to `%0`:
   ```text
   %output %0 \033_UTEST\033\134$ 
   ```
3. **`allow-set-title` interaction:** Because `tmux`'s own VT parser also consumes the bare APC,
   with the default `allow-set-title on`, `#{pane_title}` changed to `'UTEST'`. After running
   `set-option -g allow-set-title off`, a second bare APC (`UTEST2`) still arrived verbatim in
   `%output %0 \033_UTEST2\033\134` while `#{pane_title}` stayed unchanged.
4. **Client-to-pane raw reply (`send-keys -H`):** Sending `send-keys -t %0 -H 1b 5f 55 4f 4b 1b 5c`
   from the control client delivered the raw 7-byte sequence `ESC _ U O K ESC \` into pane `%0`'s
   PTY input without key-name translation.

### Why Tern's plain-tmux trick does not transfer directly to Ultty

Tern documents setting a custom tmux session option (`@tern-tsp`) and disabling `allow-set-title`
so TSP programs inside plain tmux can speak APC. Plain `tmux` (without `-CC`) never forwards bare
APC strings to the outer terminal (`input_exit_apc` in tmux 3.6/3.8 only updates `pane_title`),
and DCS `tmux;` passthrough requires `allow-passthrough on`, provides no pane geometry or pane
identity, breaks under floating panes (tmux 3.8), and routes replies only to the active pane.
For Ultty to render per-pane native scenes or inline surfaces inside tmux, **control mode (`-CC`)
is the only technically sound path**: it gives Ultty per-pane byte streams (`%output %<id>`),
exact pane geometry (`%layout-change`), and per-pane raw replies (`send-keys -t %<id> -H`).

## Proposed Architecture (Future Project Scope)

If built as a standalone project, Ultty's tmux control-mode client would consist of five layers:

1. **Control-mode protocol parser (`crates/ultty/src/tmux/`):**
   - Detect DCS `1000p` (`\x1bP1000p`) entry and `%exit` teardown on a local or SSH PTY.
   - Parse `%begin`/`%end`/`%error` command blocks, `%output`/`%extended-output` octal escapes,
     `%layout-change`, `%window-add`/`%window-close`/`%window-renamed`, `%window-pane-changed`,
     `%session-changed`, and flow-control notifications (`%pause`/`%continue`).
   - Support tmux 3.8's legacy layout strings (control-mode clients still receive the classic
     checksum + geometry syntax) with strict bounds checks on pane counts and dimensions.
2. **Window / pane mapping onto Ultty tabs and splits:**
   - Map each tmux window (`@<id>`) to a managed Ultty tab and each tmux pane (`%<id>`) to a
     split leaf backed by an independent `ultty-vt` `Terminal` instance fed from `%output %<id>`.
   - Translate Ultty UI actions (new tab, split, close pane, resize, focus) into tmux commands
     (`new-window`, `split-window`, `kill-pane`, `refresh-client -C`, `select-pane`).
   - Enforce tmux's constraint that managed tabs cannot mix local non-tmux splits with tmux panes.
3. **Bidirectional native protocol routing per pane:**
   - Run Ultty's existing in-band APC demuxer on each pane's `%output %<id>` stream, binding any
     native session or inline surface to that specific split's `TerminalId`.
   - Route terminal-to-app replies (`ServerHello`, `SessionReady`, `LayoutComputed`, input events)
     back to `%<id>` via `send-keys -t %<id> -H` in bounded chunks.
   - Automatically set `allow-set-title off` (and advertise a custom tmux session option such as
     `@ultty-control=1` via `update-environment` or session options) so `ultty-client::detect`
     can opt in to probing inside a control-mode-attached session while keeping the probe disabled
     under plain tmux.
4. **Flow control and resize synchronization:**
   - Negotiate `refresh-client -f pause-after=<N>` so a flooding pane triggers `%pause`/`%continue`
     instead of stalling the control channel or growing buffers without bound.
   - Respect tmux's "smallest attached client" window-sizing rule and render unused margin cells
     cleanly when a smaller non-control client is also attached.
5. **Security hardening and fuzzing:**
   - Gate DCS `1000p` auto-entry behind an explicit config opt-in or interactive confirmation
     (never silently enter control mode on untrusted remote output, preventing spoofed control
     streams like Ghostty's `GHSA-hcr9-7hcr-mjm8`).
   - Fuzz the `%layout-change` and `%output` parsers with strict caps on pane count, zero/huge
     pane dimensions (preventing `GHSA-hp9c-qp4q-cpw3`-class panics), and pending command queues.

## Risks and Open Design Questions

| Risk | Severity | Details |
|---|---|---|
| **Multi-client rendering divergence** | High | If a user attaches both Ultty (`-CC`) and a plain terminal (`tmux attach`) to the same tmux session, an app speaking Ultty's in-band native protocol would leave the plain terminal seeing only fallback/blank output or raw APC side effects. Mitigation: disable `@ultty-control` whenever a non-control client is attached (`#{session_attached}` > control clients). |
| **Attack surface from untrusted output** | High | Any program can emit `\x1bP1000p` (even when tmux is not installed). Must require user opt-in or an explicit command (`ultty tmux-attach` / command palette action) rather than blind VT auto-triggering. |
| **Pane-mode input semantics** | Medium | `send-keys -H` delivers raw bytes to a pane in normal mode, but behavior while a pane is in tmux copy-mode (`%pane-mode-changed`) or alternate screen must be guarded so binary protocol frames are never interpreted as tmux copy-mode bindings. |
| **Maintenance cost** | High | Only iTerm2 has shipped stable tmux `-CC` integration in 10+ years; Ghostty has worked on it across multiple releases without wiring the GUI, and Windows Terminal's PR remains stalled on pane-architecture mismatches. |
| **Duplicate persistence layer** | Medium | Ultty already ships its own native session host (`ultty-session`, [SESSION_ATTACHMENT.md](../SESSION_ATTACHMENT.md)) with full pane identity, native protocol support, and zero tmux layout mismatch. |

## Recommendation

**Defer implementation as a separate, user-driven project.** Ultty already solves session
persistence natively via `ultty-session`, and `ultty-client` already falls back cleanly to ANSI
inside plain `tmux` without renaming panes (Plan item E2). Revisit `tmux -CC` only if sustained
user demand shows `tmux` remote workflows (e.g. corporate bastion hosts where `ultty-session`
cannot be installed) outweigh the multi-release engineering and security maintenance cost.
