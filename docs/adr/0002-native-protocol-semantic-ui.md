# ADR 0002: Native Protocol Carries Semantic Terminal UI

Ultty's native protocol is a semantic terminal UI boundary, not a generic remote GUI or arbitrary drawing protocol. Native applications should express retained regions, terminal-side layout, structured content, focus, events, and incremental updates so the Ultty terminal can preserve intent, optimize rendering, and expose runtime state; raw drawing APIs should be rejected unless they support that semantic model.

**Amended by [ADR 0018](0018-in-band-native-transport.md) (2026-11-05):** "structured content" means primitives the terminal draws (text, styled spans, images), not formats it interprets. The terminal no longer parses Markdown or highlights Code; apps style text themselves.
