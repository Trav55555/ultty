# Visual Regression

Ultty's canonical visual regression path is renderer capture through
`ulttyctl screenshot`, not compositor screenshots.

## Latest Verified Baseline

Verified on 2026-05-15 at commit `b50bdef`.

Commands:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/terminal-surface-smoke.sh /tmp/ultty-terminal-surface-smoke
scripts/native-richtext-smoke.sh /tmp/ultty-native-richtext-smoke
scripts/append-content-smoke.sh /tmp/ultty-append-content-smoke
scripts/screenshot-smoke.sh /tmp/ultty-screenshot-smoke
```

Result:

- `cargo test --workspace` passed.
- `cargo clippy --workspace --all-targets -- -D warnings` passed.
- Terminal surface smoke passed and produced prompt, scrollback, cursor, and
  resize artifacts.
- Native rich text smoke passed and produced `hello-native.png`.
- AppendContent smoke passed and produced before/after append screenshots.
- Chrome screenshot smoke passed for single-tab, multi-tab, and dense-tabs
  screenshots.

Manual screenshot review did not find a new chrome or renderer defect. The
top control band, lower tab rail, tab-rail hidden state, dense-tab state, and
old magenta-border regression all matched the expected visual model.

## Chrome Smoke

Start Ultty, then run:

```bash
scripts/screenshot-smoke.sh /tmp/ultty-screenshot-smoke
```

The smoke captures:

| Artifact | Purpose |
|----------|---------|
| `single-tab.png` | Verifies the title/control band is visible and the lower tab rail is hidden. |
| `multi-tab.png` | Verifies the title/control band and lower tab rail are both visible. |
| `dense-tabs.png` | Verifies the lower tab rail remains distinct when many tabs are present. |

The smoke also runs `scripts/analyze-chrome-screenshots.py` to check that:

- single-tab screenshots do not contain a lower tab rail
- multi-tab screenshots do contain a lower tab rail
- dense-tab screenshots keep the lower tab rail distinct from the title band
- screenshot edges do not contain the previous magenta border artifact

The script normalizes the live app to one tab before the first capture, creates
a second tab for the multi-tab capture, creates additional tabs for the dense
capture, and closes the extra tabs on exit.

## Terminal Surface Smoke

Start Ultty, then run:

```bash
scripts/terminal-surface-smoke.sh /tmp/ultty-terminal-surface-smoke
```

Use this smoke when a change affects the terminal content area rather than only
the window chrome. It captures:

| Artifact | Purpose |
|----------|---------|
| `prompt.png` | Verifies the empty prompt and terminal background render. |
| `scrollback.png` | Verifies scrollback content and the terminal scrollbar render together. |
| `cursor-underline.png` | Verifies DECSCUSR underline cursor rendering. |
| `cursor-bar.png` | Verifies DECSCUSR bar cursor rendering. |
| `selection.txt` | Records the control protocol selection query result for the run. |
| `resize-evidence.txt` | Records control and compositor state around resize setup. |
| `resize-900x640.png` | Verifies terminal content rendering after a control-protocol resize. |

Selection screenshots still require pointer-driven selection setup; the control
protocol can query the current selection but does not synthesize mouse drags.

The resize command uses winit's advisory `request_inner_size` path. Some
desktop sessions or window managers may acknowledge the request without
delivering a matching `WindowEvent::Resized` to Ultty. When that happens,
renderer screenshots continue to use the last configured surface size. Treat
`resize-evidence.txt` as the source of truth for whether the resize actually
affected the live renderer in that run.

The smoke runs `scripts/analyze-terminal-surface-screenshots.py` to check that
the captured terminal surface is nonblank, does not contain the old magenta
edge artifact, and that the scrollback capture includes visible terminal
content plus right-edge scrollbar activity.

## Native Rich Text Smoke

Start Ultty, then run:

```bash
scripts/native-richtext-smoke.sh /tmp/ultty-native-richtext-smoke
```

Use this smoke when native protocol text, markdown, code, or widget rendering
changes. It opens the `hello-native` example in a separate tab, waits for the
native UI to report that it rendered, and captures:

| Artifact | Purpose |
|----------|---------|
| `hello-native.png` | Verifies native Text, Markdown, Code, Progress, and Spinner content through the real protocol path. |

This complements renderer/unit coverage for wrapping and clipping. If the
screenshot reveals a visual defect, reduce it to a focused renderer or widget
test before changing broad native rendering behavior.

The smoke runs `scripts/analyze-native-richtext-screenshot.py` to check that
the native screenshot is nonblank, has distinct native content bands, avoids
the old magenta edge artifact, and keeps the body background dark enough for
readable markdown/code text. It is a structural check, not OCR; keep manual
review for detailed copy or syntax-highlighting claims.

For pointer and resize behavior around the same chrome surfaces, use
[`chrome-pointer-audit.md`](chrome-pointer-audit.md).

## AppendContent Smoke

Start Ultty, then run:

```bash
scripts/append-content-smoke.sh /tmp/ultty-append-content-smoke
```

Use this smoke when changing `AppendContent`, native protocol session handling,
or append-oriented streaming paths. It opens a small native protocol app in a
separate tab, captures a screenshot before the append, waits for the app to
send `AppendContent`, captures another screenshot, and validates that the
renderer output changed below the title band.

Artifacts:

| Artifact | Purpose |
|----------|---------|
| `before-append.png` | Captures the initial native text before append. |
| `after-append.png` | Captures the native text after `AppendContent` arrives. |

The smoke runs `scripts/analyze-append-content-screenshots.py` to check that
the two screenshots are valid PNGs, share dimensions, avoid the old magenta
edge artifact, and differ in the rendered content area.
