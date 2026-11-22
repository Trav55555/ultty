# `ratatui-ultty`

Bridge crate letting an existing `ratatui` application opt specific screen areas (such as a transcript or log pane) into Ultty native regions while keeping `ratatui` for the rest of the interface and falling back cleanly to ANSI everywhere else.

## Adopting in an Existing `ratatui` App (~20-line diff)

1. Swap `Terminal::new(CrosstermBackend::new(stdout()))` for `UlttyBridge::connect(CrosstermBackend::new(stdout()))`.
2. Swap `crossterm::event::poll/read` for `bridge.poll_event(timeout)`.
3. Replace the log pane's `frame.render_widget(log_paragraph, log_area)` with `frame.native_region(log_area, NativeRegion::log("logs", &log_text).title("Logs"))`.

Inside Ultty, the opted-in area becomes a native scrollable, selectable region backed by incremental `AppendContent` updates. Outside Ultty (or with `UlttyBridge::ansi`), the same call renders a standard `ratatui` `Paragraph` with borders and scroll offset.

## Example

```bash
cargo run -p ratatui-ultty --example log-viewer
```
