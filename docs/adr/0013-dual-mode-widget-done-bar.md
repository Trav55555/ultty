# ADR 0013: Dual-Mode Widget Done Bar

An Ulttui widget is done only when its core behavior works in both ANSI mode and Ultty-native mode, with keyboard-first interaction, resize resilience, realistic-content visual correctness, and focused coverage or snapshots for each mode. Native-only visual demos and ANSI-only fallback behavior are not enough for a first-class widget.

Amended 2026-11-10 by ADR 0021 §4: a widget also declares its role and label for assistive technology (`View::semantics`, with its state where it has one: checked, expanded, value, position), covered by a unit test and by `snapshot_widget_semantics` in `crates/ulttui/tests/snapshots.rs`. ANSI mode has nothing more to do.
