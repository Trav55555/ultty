# Chrome Pointer Audit

Use `scripts/chrome-pointer-audit.sh` for the live chrome interaction pass after
tab, title-band, resize, or screenshot-path changes.

```bash
cargo run -p ultty --bin ultty
scripts/chrome-pointer-audit.sh /tmp/ultty-chrome-pointer-audit
```

The audit produces `/tmp/ultty-chrome-pointer-audit/audit.md` plus renderer
screenshots and a command log.

## Product Regression Evidence

The script verifies the parts Ultty owns directly:

- control socket reachability
- one-tab normalization
- renderer screenshot smoke for single-tab, two-tab, and dense-tab states
- visible one-based tab command behavior for new, switch, and close
- command-driven resize followed by renderer screenshot capture

These checks are release evidence because they use Ultty's renderer and control
protocol.

## Compositor Debugging Evidence

Desktop pointer operations such as title-band drag and edge resize cross into
the window manager. The audit records `xdotool` availability and active-window
geometry when possible, but those checks are debugging evidence, not canonical
product regression evidence.

Manual checks for a focused Ultty window:

- title/control band empty space drags the window
- new-tab and window control targets do not start drag
- each window edge and corner enters resize behavior
- tab rail left click switches tabs
- tab rail middle click closes the targeted tab
- tab close target closes only the targeted tab
- dense-tab hover states do not shift tab geometry

Use `ulttyctl screenshot` artifacts for product review and compositor tools only
when debugging desktop/window-manager behavior.
