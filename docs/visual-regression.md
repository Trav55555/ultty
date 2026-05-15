# Visual Regression

Ultty's canonical visual regression path is renderer capture through
`ulttyctl screenshot`, not compositor screenshots.

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

For pointer and resize behavior around the same chrome surfaces, use
[`chrome-pointer-audit.md`](chrome-pointer-audit.md).
