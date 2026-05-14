# ADR 0006: Framework-Owned Markdown Streaming

Ulttui owns markdown parsing and block-level diffing for append-oriented markdown streams, while Ultty owns generic semantic append/update mechanics, retained region state, scroll anchoring, focus, and efficient rendering. The terminal should not become a markdown policy engine; it should provide protocol primitives that let frameworks update semantic content incrementally.
