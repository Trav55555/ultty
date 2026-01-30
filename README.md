# Ultimate TUI Terminal

A high-performance TUI framework experimentation sandbox, inspired by the Claude Code architecture analysis.

## Architecture
- **Language:** Rust
- **Layout:** Taffy (Flexbox)
- **Rendering:** Ratatui (Diffing) + Crossterm (Backend)
- **Loop:** Game-style loop (Update -> Layout -> Render -> Diff)

## Roadmap
- [x] Initial Scaffolding
- [ ] Implement "Transient Alt Mode" (Buffer switching)
- [ ] Connect Taffy layout tree to Ratatui widgets
- [ ] Benchmark vs React/Ink

## Usage
Run the demo:
```bash
cargo run
```
