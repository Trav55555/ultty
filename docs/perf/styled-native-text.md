# Styled Native Text Allocation Note

This note tracks the #42 styled/native text rendering cleanup.

## Scope

The hot path is `TextRenderer::build_styled_text_vertices`, which is used by
native markdown, code blocks, tables, widgets, and rich text. Before this
change, every small styled span created:

- a fresh `cosmic_text::Buffer`
- a fresh `Vec<Vertex>`
- a fresh `Vec<u32>`
- a fresh byte-offset vector
- a linear `.position()` scan over byte offsets for each shaped glyph

That was especially wasteful for markdown and code rendering, where a single
line can become many small spans.

## Change

`TextRenderer::append_styled_text_vertices` now appends directly into
caller-owned vertex and index buffers while reusing renderer-owned shaping and
byte-offset scratch buffers.

Markdown inline spans, markdown tables, blockquote/list prefixes, code spans,
and markdown rules use the append path directly. Clipped rich text still needs a
temporary vertex/index buffer before clipping, but it still benefits from the
reused shaping buffer and binary-search byte-offset lookup.

## Allocation Delta

For append-capable styled spans:

| Per-span work | Before | After |
|---------------|--------|-------|
| `cosmic_text::Buffer` allocation | new buffer | reused renderer buffer |
| Byte-offset map | new vector | reused renderer vector |
| Output vertices | temporary vector then copied | appended in caller buffer |
| Output indices | temporary vector then remapped | appended in caller buffer |
| Glyph offset lookup | linear scan per glyph | binary search over reused map |

This removes the repeated small output allocations for markdown/code/table spans
and changes glyph offset lookup from repeated linear scans to logarithmic lookup
without rebuilding mapping storage.

## Verification

```bash
cargo test -p ultty renderer::text
cargo test -p ultty native_render
cargo clippy -p ultty --all-targets -- -D warnings
```
