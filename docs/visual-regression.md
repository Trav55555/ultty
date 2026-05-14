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

The smoke also runs `scripts/analyze-chrome-screenshots.py` to check that:

- single-tab screenshots do not contain a lower tab rail
- multi-tab screenshots do contain a lower tab rail
- screenshot edges do not contain the previous magenta border artifact

The script normalizes the live app to one tab before the first capture, creates
a second tab for the multi-tab capture, and closes the extra tab on exit.
