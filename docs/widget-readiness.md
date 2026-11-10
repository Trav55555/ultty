# Widget Readiness

This matrix tracks whether each `ulttui` widget has the evidence expected for
dual-mode use. It is a release-readiness index, not an API reference.

Legend:

- `Yes`: implemented and covered by tests or snapshots.
- `Partial`: behavior exists but coverage or one mode is incomplete.
- `N/A`: not meaningful for that widget.
- `Unknown`: needs audit before claiming readiness.

Every built-in widget declares its accessibility role and label (ADR 0013 as
amended by ADR 0021); `snapshot_widget_semantics` records what each one sends.

| Widget | Native mode | ANSI mode | Focus/input | Resize/layout | Snapshot/test evidence | Notes |
|--------|-------------|-----------|-------------|---------------|------------------------|-------|
| Badge | Unknown | Unknown | N/A | Unknown | Unknown | Audit small display widget coverage. |
| BinaryToggle | Unknown | Unknown | Unknown | Unknown | Unknown | Confirm keyboard and native event behavior. |
| Button | Unknown | Unknown | Partial | Unknown | Unknown | Interactive readiness should include focus and click/keyboard paths. |
| Checkbox | Unknown | Unknown | Unknown | Unknown | Unknown | Confirm dual-mode checked/unchecked rendering. |
| Collapsible | Unknown | Unknown | Unknown | Unknown | Unknown | Needs expansion state and nested layout coverage. |
| Column/Row/Stack containers | Partial | Partial | N/A | Partial | Unknown | Known layout pitfalls remain documented in AGENTS.md. |
| Diff | Unknown | Unknown | N/A | Unknown | Unknown | Needs wide-line and color evidence. |
| Divider | Unknown | Unknown | N/A | Unknown | Unknown | Audit ASCII fallback consistency. |
| Dock | Unknown | Unknown | Unknown | Unknown | Unknown | Needs focus and layout evidence. |
| Image | Unknown | Unknown | N/A | Unknown | Unknown | Native image behavior should be separated from ANSI fallback claims. |
| InputHistory | Unknown | Unknown | Unknown | Unknown | Unknown | Needs cursor/history behavior evidence. |
| List | Unknown | Unknown | Unknown | Unknown | Unknown | Needs selection, scrolling, and resize evidence. |
| MarkdownView | Partial | Partial | N/A | Partial | Partial | Streaming/append behavior has live evidence; structural markdown still uses full content updates. |
| Modal | Unknown | Unknown | Unknown | Unknown | Unknown | Needs overlay focus and dismissal evidence. |
| ProgressBar | Unknown | Unknown | N/A | Unknown | Unknown | Audit native/ANSI visual parity. |
| ScrollView | Partial | Partial | Partial | Partial | Unknown | Release-critical for large content and resize. |
| ScrollSearch | Unknown | Unknown | Unknown | Unknown | Unknown | Needs search result and navigation evidence. |
| Select | Unknown | Unknown | Unknown | Unknown | Unknown | Needs keyboard, focus, and popup/list evidence. |
| Spinner | Unknown | Unknown | N/A | Unknown | Unknown | Needs idle-efficiency check for animation. |
| Table | Unknown | Unknown | Unknown | Unknown | Unknown | Needs column sizing and overflow evidence. |
| Tabs | Unknown | Unknown | Unknown | Unknown | Unknown | Needs focus, keyboard, and resize evidence. |
| Text/RichText | Partial | Partial | N/A | Partial | Partial | Native rich text has live smoke evidence. |
| Toggle | Unknown | Unknown | Unknown | Unknown | Unknown | Confirm keyboard and state rendering. |
| Tree | Unknown | Unknown | Unknown | Unknown | Unknown | Needs expansion, selection, and overflow evidence. |
| Typography helpers | Unknown | Unknown | N/A | Unknown | Unknown | Audit fallback rendering claims. |

## Next Audit Pass

For each widget, link the exact unit test, snapshot, or live smoke proving:

1. Native `content()` output.
2. ANSI `render_ansi()` output.
3. Focus or event behavior, when interactive.
4. Resize behavior for constrained and narrow layouts.
5. Any known divergence between native mode and ANSI mode.
