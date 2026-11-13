# Native transcript selection verification

The tested code is commit `5f2ab1b759a07b813544f02f52f75aaf2106da7d`. Merge commit `a770081f33e0bf4a325948a6ef5847a543d45d98` incorporates Ultty main and has the identical source tree (`git diff 5f2ab1b a770081` is empty). The verification binaries were built from the tested code:

| Binary | SHA-256 |
| --- | --- |
| `ultty` | `9504d7ba4ada284220f1769264a95c12d59efff83e42caeea8e1de681468f7f0` |
| `ulttyctl` | `fac58a722f3528a6d31ad8f3da9b0fab05409fa838df3827e85ce133a3980e75` |
| `porygon-tui` | `3ed6a82a1b1bafc26500d6f1f235c19b97cc47f651cf53803a1bd7c76761634f` |

Native drag selection copies rendered transcript text, including code whitespace and real line breaks. Soft wraps do not add line breaks. A completed selection retains its captured text through scrolling and resizing; its highlight disappears if its document revision no longer matches. If an active drag loses required rows or its revision changes, selection cancels visibly and does not copy partial text or fall through to the terminal grid. Selection is bounded to 1 MiB and 65,536 captured rows. Ctrl+Shift+C copies the native selection; Porygon's Ctrl+Y copies the latest assistant reply as raw Markdown. ANSI users use their terminal's own selection and copy controls.

Ctrl+click activates an HTTP(S) destination on release only when the pointer has not dragged. Markdown labels retain their targets through wrapping and style changes. Visible raw URLs require a known end before activation; a URL cut off at the visible tail is not opened. Oversized or excessive link metadata is omitted and its text remains selectable. Link opening uses the terminal's existing URL-opening policy.

## Checks

- Workspace tests, Porygon's 106 tests, workspace and app clippy with `-D warnings`, and both formatting checks passed on the tested code. Focused regressions cover metadata updates, missing rows and revisions, UTF-8 and code whitespace, graphemes spanning style boundaries, and incomplete URL tails.
- Independent X11/native verification passed all 12 core selection cases, 3 painted Unicode cases, 4 split-pane cases, 7 semantic-link cases, and the grid control. The painted cases selected the actual painted `A` following a wide character, a combining sequence, and a ZWJ emoji. The incumbent failed the same painted-position control.
- Independent compatibility checks passed at ANSI widths 80 and 160 (6 cases each) and in native mode (1 case). The native fixture uses the compact authenticated status route.
- Independent source review checked pane/session ownership, bounded metadata and capture, style-split grapheme wrapping and rendering, revision handling, URL boundaries, and link target validation. Independent visual review found no blocking clipping or split-pane highlight spill; exact Unicode selection is established by clipboard and painted-position probes.

The frozen binary, logs, screenshots and machine-readable receipts are retained in the local `ux-native-selection-verification` artifact directory. The native profile's primary receipt is `1794605218521738296`; painted Unicode, split and link receipts are `1794605237442734947`, `1794605243736899995` and `1794605249506320068`. A test fixture initially retained the preceding selection's highlight; after clearing that state, the same frozen binary passed the cancellation check with the highlight gone and clipboard unchanged. An earlier sandboxed workspace run failed socket and PTY fixtures with `EPERM`; the successful workspace run used local fixture access.
