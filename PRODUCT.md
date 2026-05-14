# Product

## Register

product

## Users

Ultty serves terminal power users and developers building or using native TUI applications. They work in dense command-line environments, often across shells, editors, agents, dashboards, and long-running development workflows. They need the terminal to stay fast, readable, inspectable, and keyboard-first while preserving compatibility with existing Unix terminal habits.

## Product Purpose

Ultty is a GPU-accelerated terminal emulator and native TUI framework that replaces ANSI-only rendering bottlenecks with a semantic binary protocol, terminal-side layout, retained scene state, and atomic frames. Success means terminal-native workflows feel responsive under real load, avoid flicker and tearing, remain compatible with existing terminal software, and expose enough runtime state for developers to trust and debug native TUI apps.

## Brand Personality

Serious systems tool with an experimental workstation edge. The product should feel technically grounded, precise, durable, and built for people who care about latency, protocol boundaries, and terminal ergonomics. It can have personality, but that personality should come from clarity, tactility, and operational confidence rather than decorative spectacle.

## Anti-references

Ultty should not look like a generic dark SaaS dashboard, a cyberpunk neon shell, retro terminal cosplay, a VS Code clone, or a Linear-style minimalist web app transplanted into a terminal. Avoid glossy AI-tool tropes, metric-card dashboards, decorative gradients, excessive glow, and UI chrome that competes with the working terminal surface.

## Design Principles

1. Preserve the working surface: terminal content remains primary; chrome and overlays support orientation without stealing attention.
2. Make runtime state inspectable: native sessions, dirty state, socket paths, layout timing, errors, focus, and protocol capabilities should be visible when needed and quiet otherwise.
3. Favor operational density over decoration: every visible element should help users navigate, debug, compare, or act.
4. Respect terminal muscle memory: keyboard-first workflows, predictable focus, clear close paths, and Unix conventions matter more than novelty.
5. Show capability through behavior: responsiveness, stable layout, clean wrapping, and low idle work are more persuasive than flashy demos.

## Accessibility & Inclusion

Target WCAG AA for visible UI surfaces, with reduced-motion-safe behavior and color-blind-safe status communication. Do not rely on color alone for critical state. Keep overlays keyboard accessible, readable at common terminal font sizes, and resilient to long text, narrow windows, and high-density work sessions.
