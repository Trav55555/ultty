# Ultimate TUI Terminal: Architecture & Design Spec

**Objective:** Build a high-performance, developer-friendly TUI framework that eliminates "TUI flicker" and overcomes the limitations of standard terminal buffers, inspired by the Claude Code vs. OpenCode architectural debates.

## 1. The Core Problem: Bandwidth vs. Latency
As highlighted in the Theo/Claude Code breakdown:
- **Web Approach (React/Ink):** Renders a virtual DOM, diffs it, and sends patches.
- **The Bottleneck:** "Append-only" terminals require excessive ANSI escape codes to modify past content (cursor jumps, clears).
- **Result:** A 16ms frame budget (60fps) is consumed by 11ms of layout/diffing, leaving <5ms for I/O. Large updates (thousands of chars) saturate the TTY baud rate/bandwidth, causing flicker.

## 2. Proposed Architecture: "The Hybrid Engine"

### A. The Stack
- **Language:** **Rust**. (Non-negotiable for performance control).
- **Layout Engine:** **Taffy**. (Rust-native Flexbox implementation, successor to Yoga).
- **Backend:** **Crossterm** or **Termion** (for raw TTY handling).
- **Rendering Strategy:** **Double Buffering** with "Smart Diffing".

### B. The Render Loop (The "Zero-Flicker" Strategy)
Instead of immediate mode rendering (writing directly to stdout), we implement a Game Loop architecture:

1.  **State Update:** Application state changes.
2.  **Layout Pass (Taffy):** Calculate `x, y, w, h` for all nodes. (Fast, sub-1ms in Rust).
3.  **Virtual Buffer Draw:** Render components to an in-memory `Buffer` (2D grid of `Cell` structs: char + style).
4.  **Diffing Pass:**
    - Compare `CurrentBuffer` vs. `PreviousBuffer`.
    - Generate the *minimal* set of ANSI instructions required to transform the screen.
    - *Critical Optimization:* Use "atomic" screen updates where possible (synchronous output) to prevent tearing.

### C. Solving the "Buffer Mode" Dilemma

**The Mode:** **"Transient Alt Mode"**

Most TUIs force you to choose:
1.  **Standard Buffer:** Good history, but fragile layout.
2.  **Alt Mode:** Perfect layout, but practically no history (disappears on exit).

**Our Solution:**
- The application runs in **Alt Mode** while active (giving full screen control).
- We maintain an internal **"Virtual Scrollback"** buffer.
- On exit, we **flush** the meaningful history to the user's standard terminal buffer.
- This gives the *feeling* of a standard CLI tool (history persists) with the *performance* of an Alt Mode app (vim/htop).

## 3. Component System (The "React-like" DX)
Developers love React's component model. We will replicate the *DX* without the *Runtime Overhead*.

```rust
// Conceptual API
fn render(app: &App) -> Element {
    Column::new() // Flexbox Column
        .padding(1)
        .border(Borders::ALL)
        .child(
            Text::new("Status: Running")
                .style(Style::default().fg(Color::Green))
        )
        .child(
            ProgressBar::new(app.progress)
        )
}
```

- **Declarative:** UI is a function of State.
- **Immediate Mode:** No "Virtual DOM" object overhead; we construct the layout tree every frame (Rust is fast enough for this).

## 4. Key Innovations to Research
- **Resize Handling:** Intercept `SIGWINCH`. Pause rendering. Query terminal size. Re-layout. Resume.
- **Input Handling:** Asynchronous event loop (tokio) to handle keypresses without blocking the render loop.
- **"Diff-aware" Rendering:** If only one character changes, send 5 bytes (jump + char), not a full redraw.

## 5. Implementation Roadmap
1.  **`src/layout.rs`**: Integrate Taffy for Flexbox layout.
2.  **`src/buffer.rs`**: Implement the Double Buffer system.
3.  **`src/terminal.rs`**: Raw TTY wrapper.
4.  **`examples/demo.rs`**: A stress test (resizing, fast updates).
