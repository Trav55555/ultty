# Markdown Streaming Profiling Note

Ulttui owns markdown parsing and block-level diffing for append-oriented streams. Before the incremental append slice, `MarkdownView::append_chunk` appended to the source string and reparsed the full document on every chunk.

The current fast path avoids the full parser for the common streaming cases that keep the active block shape stable:

- plain paragraph chunks without structural markdown markers extend the last paragraph span in place
- chunks inside an open fenced code block extend the code block text in place until the closing fence arrives
- ambiguous chunks, including split inline styling boundaries and closing code fences, fall back to full parsing so native and ANSI output stay correct

At the protocol boundary, native frame diffing now turns last-span `RichText` growth into `AppendContent`, and Ultty appends that text to the existing rich-text span. This changes steady paragraph streaming from full-region `SetContent` churn to append-only updates while preserving the existing fallback path for structural markdown changes.
