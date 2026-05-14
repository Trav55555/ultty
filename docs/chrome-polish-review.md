# Chrome Polish Review - 2026-05-14

## Visual Pass

Reviewed renderer screenshots from:

```bash
scripts/screenshot-smoke.sh /tmp/ultty-screenshot-smoke-issue-60-rerun4
```

Artifacts:

- `/tmp/ultty-screenshot-smoke-issue-60-rerun4/single-tab.png`
- `/tmp/ultty-screenshot-smoke-issue-60-rerun4/multi-tab.png`

## Findings

1. The chrome hierarchy is now correct: the new-tab target and window controls
   stay in the title/control band, and the tab rail appears only for multiple
   tabs.
2. The renderer screenshot path is the right visual regression source. It does
   not include the compositor/window-manager border artifact.
3. The control CLI had a polish mismatch: `ulttyctl tabs` displays tab numbers
   as one-based and help documents `switch-tab <n>` as one-based, but command
   parsing treated explicit tab indices as zero-based. That made user-facing
   tab numbers inconsistent and made visual-smoke cleanup awkward.

## Change

The CLI parser now converts display tab indices to zero-based protocol indices
for `switch-tab <n>` and `close-tab <n>`. Bare `close-tab` still closes the
current tab.

This keeps command behavior aligned with visible tab labels and the help text.
