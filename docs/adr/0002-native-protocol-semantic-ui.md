# ADR 0002: Native Protocol Carries Semantic Terminal UI

Ultty's native protocol is a semantic terminal UI boundary, not a generic remote GUI or arbitrary drawing protocol. Native applications should express retained regions, terminal-side layout, structured content, focus, events, and incremental updates so the Ultty terminal can preserve intent, optimize rendering, and expose runtime state; raw drawing APIs should be rejected unless they support that semantic model.
