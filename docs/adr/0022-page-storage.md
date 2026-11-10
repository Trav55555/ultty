# ADR 0022: Page-Based Screen and Scrollback Storage

## Status

Accepted 2026-11-09, after the prototype gate in §Prototype passed in round 3 (below). The history of the gate follows. Accepting it adopts round 2's style layout: §1's bits 24..64 become one style key, either an inline style or a 24-bit table id, and the per-page style table interns approximately. The sgr-dense wall-clock cost (3.6–4.4% slower in C on the quiet machine, against +0.5% to +1.4% instructions) is explained in the perf note's "sgr-dense wall clock" section. The implementation should apply SGR directly to the style key rather than going through `Style::of`.

Proposed 2026-11-09. Not accepted until the prototype gate in §Prototype passes; if it fails, this ADR is recorded as Rejected together with the measurements.

**Prototype measured 2026-11-09: not GO** ([2026-11-09-page-storage-prototype.md](../perf/2026-11-09-page-storage-prototype.md)). Variant C meets every GO criterion but one:
- vt/ascii is 18.6–19.4 instructions per byte at 1 and 8 MiB;
- scroll is −35% to −44% and wide-wrap −46% to −56%;
- cjk is −33% to −40%;
- the PTY harness runs ascii +45.5% at 120x40 and +49.9% at 300x75 (paired medians, 120 rounds; per-arm IQR 4–7.6% of the median);
- live heap is about a third of the reference's.

The failure is sgr-dense, at +16% to +23% instructions and −12% to −16% wall clock against a limit of +3%. The cost is interning styles that are mostly new, which A pays as well. "A only" does not qualify: A carries 38–81% of C's instruction cut and 61–71% of its wall-clock gain, below two thirds. The NO-GO kill rule does not fire. Not accepted as written. Accepting it would need either a measured fix for sgr-dense interning or a decision to change that criterion.

**Prototype round 2, 2026-11-09: GO on every criterion but the wall-clock spread** (same note, "Second round"). The prototype's style handling changed in four ways:
- styles with no truecolor, underline color or hyperlink are stored inline in the cell's top 40 bits (a tag bit, fg and bg as kind plus index, and the flags), so they never touch the table;
- the table interns approximately, through a 256-slot cache of recent styles, and a miss appends a new id instead of a hash lookup and insert;
- collection waits for max(4096, 2 × screen cells) new styles;
- the history cell copy is tighter.

Results for C:
- sgr-dense is +0.5% to +1.4% instructions (was +16% to +23%) and level with the reference on the wall clock;
- every other instruction criterion still passes (ascii 18.7–19.5 per byte, scroll −35% to −43%, wide-wrap −46% to −56%, cjk −36% to −43%);
- the PTY harness gives ascii +42.4% and +49.2% (95% CI ±1.5–3%);
- live heap is 1.20 / 1.34 / 9.99 MB.

The clause not met is "spread under 5%": per-arm IQR/median is 8–10% on the shared workstation, pinned or not. "A only" still does not qualify (A carries 55–66% of C's wall-clock gain), and NO-GO does not fire. If accepted, §1 changes: bits 24..64 become one style key, either an inline style or a 24-bit table id, which spends the reserved bits of open question 6, and the "interned per page" table becomes approximate. Accepting the ADR on these numbers, or after a wall-clock rerun on a quiet machine, is the owners' decision.

**Prototype round 3, 2026-11-09: GO** (same note, "Third round: quiet machine"). The round-2 wall clock, rerun unchanged on an idle AWS c7a.2xlarge, gives per-arm IQR/median 0.3–2.6% (C 0.3–0.7%), so the spread clause is met and the gate passes as written. C runs ascii +44.8% and +50.9% (95% CI ±0.2%) and cjk +52% to +54%. "A only" still does not qualify (60% and 49% of C's gain), and NO-GO does not fire. The quiet machine also shows sgr-dense 3.6–4.4% slower in C on the wall clock, where round 2 read it as level; the sgr-dense criterion is measured in instructions (+0.5% to +1.4%), so this does not fail the gate, but it is outside +3% if read as time. Acceptance remains the owners' decision.

## Context

### The gap

On the AWS T4 box (g4dn.xlarge, 4 vCPU, 120x40), live ASCII throughput is 93 MiB/s against Ghostty's 129; unicode-mixed is at parity (24 against 25). [2026-11-08-live-throughput.md](../perf/2026-11-08-live-throughput.md) found that the PTY I/O thread is the bottleneck. It hardly ever waits for data (58 of 20,778 reads returned EAGAIN), every read returns at most 4 KiB, and its CPU went about 75% to parse, grid and scrollback in user space and about 25% to the kernel's PTY reads. Frames held the terminal lock for well under 3% of a flood. Throughput is therefore the I/O thread's CPU per byte.

Three rounds of VT-core work since then ([vt-core-throughput](../perf/2026-11-08-vt-core-throughput.md), [unicode-write-path](../perf/2026-11-08-unicode-write-path.md)) brought `ultty-bench vt` ascii from 68 to 39.0 instructions per byte (`bench/baselines/icount.json`: vt/ascii 40,934,810 for 1 MiB). The largest single ASCII cost before the plain-row shortcut was scrollback encoding: `CompactRow::encode_from` was 39% of ascii, about 1,900 instructions per row. The shortcut made a plain row's encoding a byte copy. What is left is spread across writing 24-byte cells (the vt-core note puts this at about 12 instructions per byte), erasing the recycled row, per-line scroll bookkeeping, and history allocation while it fills.

Ghostty stores the screen and scrollback in fixed-size pages. One page holds rows, 8-byte cells, styles and graphemes, and scrollback is a linked list of pages, so a scroll moves a pointer instead of copying a row.

### Where an ASCII line goes now (derived)

These are not profile numbers. They come from the icount baseline (1 MiB, 120x40) and the line shapes of three corpora in `tools/ultty-bench/src/corpus.rs`:

| corpus | line | instr/byte | instr/row |
|---|---|---:|---:|
| ascii | 84 chars + CRLF | 39.0 | about 3,350 |
| scroll | about 6 digits + CRLF | 174.5 | about 1,400 |
| wide-wrap | 200–4000 chars, soft-wrapped at 120 | 34.6 | about 4,150 per 120-col row |

Fit a per-row cost S, a CR/LF dispatch cost P and a per-character cost c to these. ascii and scroll give c ≈ 25 and S + P ≈ 1,245. wide-wrap, which scrolls by wrapping and has almost no CR/LF, gives S ≈ 1,140 and P ≈ 105. So an ASCII line is about one third per-row overhead (history push, VecDeque moves, `rotate_left` of about 40 `Row` headers of about 64 bytes, erase setup, marking 40 rows dirty, and history allocation while it fills) and about two thirds per-character cost. The per-character 25 breaks down as parse 3.9, writing a 24-byte cell about 12, the plain-row copy and trim about 4, erasing the recycled row's 24-byte cells about 1.5, and per-call bookkeeping. The three corpora are in different history regimes at 1 MiB: wide-wrap and ascii are mostly filling and pay malloc, while scroll is long past full. Treat the fit as ±30%.

### The current model

- **Cell** (`grid/cell.rs`): 24 bytes. A `char`, packed fg, bg and underline colors (`PackedColor`, a u32 each), `CellFlags` (u16, including the structural WIDE_CHAR, WIDE_CHAR_SPACER and HAS_COMBINING flags and the underline shape bits), and a u32 hyperlink id.
- **Row** (`grid/row.rs`): `Vec<Cell>` (one heap allocation per row), a `GraphemeMap` (`Vec<(u16, Vec<char>)>`, one allocation per cluster), and flags in padding: dirty, wrapped, `RowMarks` (ADR 0019), `AnchorTag` (ADR 0020), the `used` mark (cells after it are default; may overstate) and `plain` (every cell ASCII in the default style; may understate). `write_ascii`, `write_char_in_place`, `erase`, `erase_range`, `insert_cells`, `delete_cells` and `resize` keep those marks. `get_mut`, `IndexMut` and `cells_mut` give up on them.
- **Grid** (`grid/mod.rs`): `rows: Vec<Row>` plus per-screen-row `dirty: Vec<bool>` and `row_versions: Vec<u64>`, `content_version`, cursor, attrs template (a `Cell`), scroll region and `LastPrint` (screen coordinates and the cluster `Tail`).
  - `scroll_up` / `scroll_down` call `rows[top..=bottom].rotate_left/right(n)`, erase the n recycled rows (filling up to `used`), and mark the region dirty with one version.
  - `insert_lines` / `delete_lines` clone every moved `Row` (`self.rows[src].clone()`, a heap copy) and allocate a fresh `Row::new` for every blank row.
- **Buffer** (`grid/buffer.rs`): a `Grid` plus `history: VecDeque<CompactRow>`, `max_history` (10,000 by default; 0 for the alternate screen), `scroll_offset`, anchors and `evicted`.
  - `newline` → `scroll_grid_up` → `save_scrolled_rows`, which saves only when the region's top is screen row 0, as in xterm.
  - `save_top_rows` pops the oldest `CompactRow` when history is full, reusing its allocations, then calls `encode_from(row)` and `push_back`, then `grid.scroll_up`.
  - `write_ascii_span` chunks a span at the right margin and saves a row before a pending wrap on the bottom margin.
- **CompactRow** (`grid/history.rs`): the row as UTF-8 (grapheme extras inline after their base), style runs (`Run { len: u16, style: Style }`, about 24 bytes), and per-column marks for wide and grapheme cells (`WIDE_MARK`, extras count). Three paths:
  - the `plain` copy;
  - a one-style, all-ASCII check;
  - the per-cell encoder `encode_cells::<IMPLY>`, which leaves implied flags out of the styles and falls back to explicit flags for rows the writer never leaves.

  `decode` rebuilds a full `Row`. Marks and anchors are readable without decoding.
- **Readers.** `Buffer::visible_row(u16)` and `row_at(usize)` return `Cow<Row>`: borrowed for screen rows, decoded (allocated) for history rows.
  - The renderer (`renderer/text.rs::build_text_vertices_with_offset`, plus `panes.rs`, `mod.rs` and `overlays.rs`) reads `visible_row` for each screen row every frame.
  - `GridTextCache` (`renderer/text_cache.rs`) keeps an owned `Row` copy per cached row and reuses vertices only when cells and graphemes are equal. It deliberately compares content rather than trusting versions, because history scrollback, the alternate screen and shared panes change what a screen row shows without the grid knowing.
  - Selection (`Terminal::selected_text`) works in viewport coordinates through `visible_row(..).copy_text`, and is invalidated through `rows_changed_since`.
  - Search (`app/mod.rs`), `wait` (`control.rs::find_wait_match`), `text --scrollback` (`control.rs::buffer_text`) and `last-command` call `row_at` for every row, which decodes every history row into a `Row` and a `String`.
- **Reflow.** `reflow_all` streams history: decode each `CompactRow` → `ReflowCell`s → `Rewrap::push_row`, which keeps the last screenful as `Row`s and `CompactRow::encode`s older rows. Marks go to the first row of a logical line, and anchors end lines. `resize_height` clones top rows into history or decodes rows pulled back from it.
- **Budgets.** `crates/ultty/benches/memory.rs` gates live heap at measured +5%: empty 80x24 185 KB; empty 300x75 1.229 MB (two screens of 24-byte cells); typical 10k lines 2.915 / 3.461 / 4.358 MB at 80/120/300 columns; the resize case 3.461 MB live and 4.935 MB peak; sgr-dense 10k 18.98 MB. That works out to about 300 bytes per typical history line and about 1.8 KB per sgr-dense line.

## Decision

Store each buffer's screen in one **screen page** and its scrollback in a list of **history pages**, both made of 8-byte packed cells whose styles are interned in a table per page. Rows become slots in a page, not heap objects. Every reader goes through a borrowed `RowView`. The parser's API does not change.

### 1. Packed cells and style tables

```text
PackedCell(u64)
  bits  0..21  codepoint (blank is 0x20, so a default cell is 0x0000_0000_0000_0020)
  bits 21..23  width: 0 narrow, 1 wide, 2 spacer, 3 wide+spacer (never written; kept so
               any Cell round-trips)
  bit  23      grapheme: the page holds extra codepoints for this cell (HAS_COMBINING)
  bits 24..48  style id (24 bits; 0 is the default style)
  bits 48..64  reserved, zero
```

`Style` is today's `history::Style`: fg, bg, underline color (packed u32 each), the `CellFlags` other than the three structural ones, and the hyperlink id, about 20 bytes. Each page owns a `StyleTable { styles: Vec<Style>, index: HashMap<Style, u32> }` (foldhash, as the render caches use), with id 0 always the default.

The public `Cell` stays as the unpacked value type: readers get `Cell` by value, built from the packed cell and the page's table. Packing is lossless for every `Cell`, including the flag combinations that only direct edits create. This keeps the history round-trip property exact.

Why 24 bits and not Ghostty's 16: the screen page's table is collected in place (below), and a 300x220 window holds 66k cells. With 24 bits the live set can never exceed the id space, so there is no overflow path to test.

### 2. The screen page

Each `Buffer` (main and alternate) owns one `ScreenPage`:

```text
ScreenPage {
  cols, rows,
  cells:  Vec<PackedCell>      rows * cols, one allocation, slot s at s*cols..
  slots:  Vec<u16>             screen row r is shown by slot slots[r]
  heads:  Vec<RowHead>         per slot: used u16, uniform style id u32, flags
                               (wrapped, has_graphemes, uniform), RowMarks, AnchorTag
  styles: StyleTable
  graphemes: GraphemeStore     (§4)
}
```

- **Scroll up by n within the region:** copy the top n rows to history if the region top is 0 (§3); then rotate `slots[top..=bottom]` (2 bytes per row, not about 64), clear each recycled slot up to its `used` mark (a u64 fill), and mark the region dirty with one version as today.
- **IL/DL and scroll_down** are rotations of a `slots` sub-slice plus clears. No `Row` clone and no allocation.
- **`uniform`** generalises `plain`: every cell before `used` is ASCII with no structural bits, in the head's style id. It may understate and never overstate. `write_ascii` keeps it when the template's id equals the row's (or the row is empty); anything else clears it. `Row::check_integrity`'s plain check becomes a uniform check.
- **The cursor template** is a `Style` plus a cached id that is invalidated by SGR changes and by collection, so a print resolves its style id once per SGR change, not per cell.
- **Collection:** when the screen table passes max(4096, 2 × live ids after the last collection), mark the ids used by the screen's cells, rebuild the table, and rewrite ids in place. This is O(cells) and rare (tens of µs at 300x75). History pages never collect: they are append-only and freed whole.
- **Dirty and version arrays** stay per screen row, as now, so `rows_changed_since`, selection invalidation and `has_dirty` keep their meaning.

The alternate screen is a `ScreenPage` with no history.

### 3. History pages

```text
History {
  pages: VecDeque<HistoryPage>,   newest last; at most 256 rows each
  front: u16,                     rows of pages[0] already evicted
  free:  Vec<HistoryPage>,        recycled pages, capacity kept
  len, max_history, evicted
}
HistoryPage {
  heads: Vec<HistHead>,     per row: offset u32, kind + len, width, used, wrapped,
                            RowMarks, AnchorTag, style id (uniform rows)
  bytes: Vec<u8>,           row payloads, appended
  styles: StyleTable,
  remap: Vec<u32>, remap_gen,   screen style id -> this page's id, cached
}
```

A row that scrolls off is appended to the newest page in one of two forms:

- **Uniform ASCII:** its bytes, narrowed from the packed cells. Trailing spaces are trimmed when the style is the default, as today; otherwise the row is kept up to `used`. The style is interned once per row.
- **Cells:** a memcpy of the used packed cells, then a style remap per cell (`remap[id]`, interning on a miss), then the row's grapheme extras appended after the cells as (col, count, chars).

No per-row heap allocation happens in either case; pages come from `free` once history is full. Eviction stays per row (`front += 1`; a fully evicted page goes to `free`), so `max_history`, `evicted` and the anchor line numbering are unchanged. `row_at(i)` is O(1): page `(i + front) / 256`. `clear_history` and `set_max_history` drop pages or advance `front`.

This is a deliberate departure from Ghostty. Ghostty's active area is the tail of the page list, so a scroll copies nothing. Here the screen stays in one hot page of fixed size (38 KB at 120x40, against 115 KB of 24-byte cells today), regions, IL and DL never span pages, and history is narrowed as it is written. The copy costs about 1 instruction per byte of input (§Expected gain). Keeping all history at 8 bytes per cell instead would break the memory budgets (§9).

### 4. Graphemes

On both page kinds, the cell's grapheme bit says that extras exist.

- **Screen page:** `GraphemeStore { index: Vec<(u32 cell, u32 start, u8 len)>, chars: Vec<char> }`, sorted by cell index, appended at the end in the common left-to-right case, and compacted when garbage exceeds the live chars. The 16-per-cell cap is unchanged.
- **Write paths** test the bit on the cells they overwrite instead of calling into the map. That removes the per-character `drop_grapheme_entries` call the unicode note found expensive.
- **History rows** carry their extras inline after their cells.
- **`LastPrint` and `Tail`** are unchanged: they are screen coordinates.

### 5. Reading rows: `RowView`, and the renderer and its cache

`Buffer::visible_row` and `row_at` return `RowView<'_>`, a borrow of a screen slot or a history row, with:

- `width`, `cell(col) -> Option<Cell>` (by value), `grapheme_extra(col)`;
- `is_wrapped`, `marks`, `anchor`;
- `text`, `text_trimmed`, `copy_text`;
- `eq_row(&Row)`, and `to_row() -> Row` for an owned copy.

Today's `Row` survives as the owned, unpacked value type. It is used by the renderer cache, reflow input, tests and the integrity proptests. History rows are read in place: a uniform row answers `cell(col)` from its bytes and its one style, so scrolling back, search and `text --scrollback` decode nothing.

The renderer changes `row.get(col)` (`&Cell`) to `row.cell(col)` (`Cell`). `GridTextCache` keeps its owned `Row` per cached row and its content comparison, now done as `RowView::eq_row`: unpack and compare, about 1k instructions per 120-column row, about 40k per frame against a 3.93 M-instruction frame. A later fast path keyed on (page uid, slot, row version) is an open question; the content comparison stays the source of truth for the reasons in `text_cache.rs`'s module docs.

`pty_flood`'s bench thread reads through the same `visible_row`.

### 6. Write paths (the parser does not change)

`AnsiParser::feed_with_ascii_spans`, `AnsiOutput` and `TerminalHandler` are unchanged.

- **ASCII spans:** `Buffer::write_ascii_span` and `Grid::write_ascii_span` keep their chunking and wrap logic. `Row::write_ascii(col, bytes, template)` becomes a page write of `base | b as u64` per byte (`base` = style id << 24), a loop the compiler can vectorise. The wide-orphan check reads the width bits of the two edge cells. `used` and `uniform` are updated per span, not per cell.
- **Characters in place:** `write_char_in_place` writes `base | cp | width bits`, plus the spacer.
- **The slow path:** `write_measured_char_slow` and the cluster code go through a `set_cell(slot, col, Cell)` / `update` API that replaces `get_mut`, `IndexMut` and `cells_mut`. Outside ultty-vt only `crates/ultty/benches/grid.rs` uses those.
- **SGR** changes the template `Style` and drops the cached id.

### 7. Reflow and height resize

- **Width change:** `reflow_all` keeps its streaming shape. It reads source history rows as `RowView`s into `ReflowCell`s, freeing each source page once its last row is consumed. `Rewrap::push_row` hands the last screenful to a new `ScreenPage` and appends older rows to new `History` pages through an owned-`Row` append (styles interned per cell; reflow only, not a hot path). Peak memory stays at about live + one screen + one page, inside the 4.935 MB peak budget. Marks, anchors (`AnchorRuns`, `place_anchors_after_reflow`) and cursor tracking are unchanged.
- **Height-only change:** shrinking appends the top rows to history through the scroll-off copy, with no `Row` clones. Growing reads the newest history rows into fresh slots and pops them. The screen page is rebuilt at the new size, copying slots in screen order.

### 8. Selection, search, `text --scrollback`, marks and anchors

- **Selection** works on the viewport and the version arrays as now; `copy_text` moves to `RowView`.
- **Search, wait-match, `text --scrollback` and `last-command`** use `RowView::text_trimmed` and `text`, with no decode or `Row` allocation per history row, which speeds up search over 10k lines (ADR 0007 names search responsiveness).
- **Marks and anchors** live in the row heads: `marks_at` and `anchor_at` read them without decoding, and `strip_anchor_rows` / `drop_all_anchors` edit them in place.

### 9. Memory accounting (estimates, to be confirmed by the memory bench)

| case | today (measured, budget ÷ 1.05) | proposed (estimate) |
|---|---:|---:|
| empty 300x75 (two screens) | about 1.17 MB | about 0.36 MB (2 × 300·75·8) |
| typical 10k lines, 120x40 | about 3.30 MB (about 300 B/line) | about 1.1–1.3 MB: uniform rows about 84 B text + about 16 B head |
| typical 10k lines, 300x75 | about 4.15 MB | about 1.3–1.5 MB (trimmed rows do not grow with width) |
| sgr-dense 10k lines, 120x40 | about 18.1 MB (about 1.8 KB/line) | about 10.3 MB: 120 × 8 B + head, plus about 13 KB of styles per 256-row page |
| CJK line, 60 wide chars (no budget case) | about 560 B | about 976 B (about 1.7x) |

Every budget case gets smaller. CJK and emoji scrollback get larger. A third row form (UTF-8 plus one style, with a wide bitmap) would fix that if needed (open question). Free pages keep their capacity, as `CompactRow` reuse does today.

## Expected gain

Using the decomposition in §Context:

| part of an ASCII line | today | proposed (estimate) | why |
|---|---:|---:|---|
| per row S (scroll and history) | about 1,140 | about 400–700 | no history malloc, no VecDeque `CompactRow` moves, a 2-byte slot rotate instead of about 2.5 KB of `Row` headers; dirty, anchor and region bookkeeping unchanged |
| CR/LF dispatch P | about 105 | about 105 | parser and handler unchanged |
| per char c | about 25 | about 11–17 | parse 3.9 unchanged; cell write 3–6 (8-byte stores, vectorisable) instead of about 12; clear about 0.5 instead of about 1.5; narrow copy and trim 2.5–3.5 instead of about 4.5 |
| ascii line (84 chars) | about 3,350 | about 1,420–2,230 | |

That is 16.5–26 instructions per byte. Allowing for the fit's error and for costs the fit cannot see, the claim is **vt/ascii −30% to −50% (39 → about 20–27 instructions per byte)**. At 8 MiB, where history is full and nothing allocates, the current baseline is lower (not measured for ascii; scroll suggests a few instructions per byte), so the relative cut there is probably 5–10 points smaller.

Other corpora:

- **scroll:** −35% to −60%, because it is mostly per-row cost.
- **wide-wrap:** in line with ascii.
- **unicode-mixed and cjk:** +5% to +15% throughput. Scrollback encoding was 24% inclusive of unicode-mixed, and the copy plus remap is cheaper than the per-cell encoder, but `write_measured_char` and the cluster checks are untouched.
- **sgr-dense:** about ±5%. Interning once per SGR change adds a hash lookup; a cell row's remap (about 5 instructions per cell) replaces the per-cell encoder (about 31 per cell), but parsing SGR is most of this corpus.

End to end, instructions are not time ([2026-11-06](../perf/2026-11-06-scrollback-cost.md) is the cautionary case). The earlier rounds calibrate the conversion: vt instructions went from 68 to 39 per byte (−43%), and pty_flood ascii from 152 to 199 MiB/s. That fits a fixed kernel share of about 25% of the old time and a time-per-instruction elasticity of about 0.74. The kernel share is now about a third. A −30% to −50% instruction cut then gives **pty_flood ascii +15% to +35%**. If the T4 responds like the workstation, that is **about 105–125 MiB/s against Ghostty's 129: most of the gap, probably not all of it.**

Main sources of uncertainty:
- whether the write loop actually vectorises;
- how much of S is bookkeeping this design keeps;
- the T4's different CPU, its 4 vCPUs shared with the compositor, main thread and feeder, and its memory behaviour.

The prototype exists to replace these estimates with measurements.

Of the gain, the part from packed cells (the per-character write and clear) is separable from the part from pages (S, the narrow copy and the reads without decode). The prototype measures both.

## Alternatives considered

- **Ghostty's model exactly** (active area as the tail of a page list, zero-copy scroll, history at 8 bytes per cell): typical history would be about 6.7 MB trimmed or 9.6 MB untrimmed at 120 columns, and 24 MB at 300, against 3.5 and 4.4 MB budgets. Ultty's per-tab memory (153 MiB for 32 tabs against Ghostty's 760 on the T4) is a stated strength under ADR 0007. Regions, IL and DL spanning two pages need cross-page copies with style remaps. The only thing saved over this proposal is the narrow copy, about 1 instruction per byte. Rejected; the prototype can measure it as an upper bound.
- **A ring buffer of rows** (`VecDeque<Row>`, O(1) scroll): measured on 2026-11-06 with no change. Moving `Row` headers was never the cost, and the cells stay 24 bytes. Rejected.
- **Lazy compaction** (keep recent rows as `Row`s, compact later): every row is still encoded once, so it moves cost without removing it ([vt-core](../perf/2026-11-08-vt-core-throughput.md), "Measured and dropped"). Leaving rows uncompacted means 24-byte-cell history, which is what the 2026-11-06 cache stalls came from. Rejected.
- **Keeping `Vec<Row>` and pooling allocations:** in steady state nothing allocates now (`save_top_rows` reuses the evicted `CompactRow`, and screen rows are recycled by rotation). Pooling only helps while history fills and leaves the per-character cost alone. Rejected as a lever; the page free-list covers it.
- **Packed 8-byte cells inside `Vec<Row>`** (variant A in the prototype), with a grid-level style table and `CompactRow` history built from packed cells: likely most of the per-character gain, and a much smaller change. It keeps the per-row allocations, the `Row` clones in IL/DL, and decoding on read. It is not rejected: if the prototype shows it carries two thirds or more of the gain, it ships instead of pages.
- **Encoding scrollback on another thread:** the total CPU is the same, the T4's 4 vCPUs are already shared by the I/O thread, the main thread, the compositor and the feeder, and the hand-off needs page ownership transfer under the terminal lock. Rejected for now.
- **Making the per-cell encoder faster on 24-byte cells:** a branch-free style comparison was measured and was worse; the plain flag already removed most of it for ASCII. The style-run wins in this proposal come from u32 id comparisons, which need packed cells anyway.

## Prototype and go/no-go (before any refactor)

A throwaway branch adds `ultty-bench --suite proto` with two stripped-down buffers fed by the real `AnsiParser::feed_with_ascii_spans`. They handle PrintAscii, Print through an in-place write, CR, LF, autowrap and SGR (applied to a `Cell` template through the existing attribute code, then interned); everything else is ignored.

- **A:** packed cells in `Vec<Row>`, a grid style table, `CompactRow`-like history built from packed cells.
- **C:** a screen page plus history pages as in §2–§4.

Measurements:

1. **Instructions:** callgrind inside the timed pass, as `icount-gate.sh` does, on ascii, wide-wrap, scroll, sgr-dense and cjk, at 1 and 8 MiB, at 120x40 and 300x75. The current `Buffer` runs through the same harness as the reference.
2. **Wall clock:** a `pty_flood`-style harness on the branch, with `dd` writing 32 MiB into a raw PTY, a reader thread doing 4 KiB reads, parsing and feeding under a mutex, and a bench thread locking once per 16 ms to copy the visible rows. It runs (current `Terminal` + `TerminalHandler`) against A against C, with 6 or more interleaved rounds and the arm order alternating.
3. **Memory:** the three budget workloads (typical at 120 and 300 columns, sgr-dense) fed into C under the memory bench's counting allocator.

**GO for the page design** if all of these hold for C:
- vt/ascii ≤ 27 instructions per byte at both 1 and 8 MiB;
- scroll and wide-wrap −25% or better;
- sgr-dense and cjk no worse than +3%;
- PTY harness ascii median +15% or better at both 120x40 and 300x75, with spread under 5%;
- projected live heap within the current budgets.

**Packed cells only (A):** if A reaches two thirds or more of C's instruction cut and of its wall-clock gain, do migration slices 1–3 and stop.

**NO-GO:** if C's wall-clock gain is under +10%, stop and write the measurements up as a perf note. In every case the numbers go into `docs/perf/` and this ADR's status.

## Migration plan

Every slice is one or more small commits. After each one, all of these pass:
- `cargo test -p ultty-vt` (`conformance.rs`, `integrity.rs` on both feed paths, `selection.rs`, `command_marks.rs`, `inline_anchors.rs`, `scorecard.rs`, the history round-trip proptests);
- `cargo test -p ultty` (`text_cache` and control tests);
- the memory bench;
- `scripts/bench/icount-gate.sh`, with any baseline regeneration in its own commit with a reason;
- the fuzz build.

`pty_flood` is run at slices 3, 4 and 5.

1. **Read seam.** Add `RowView` over the current storage (a wrapper around `&Row` or a decoded `Row`). Port the renderer (`text.rs`, `panes.rs`, `mod.rs`, `overlays.rs`, `text_cache.rs`), `selected_text`, `control.rs`, app search and wait, `input.rs` and `pty_flood` to it, with `cell(col)` by value. No storage change; icount within ±1%.
2. **Differential oracle.** Copy today's `Grid`, `Row`, `Buffer` and `CompactRow` into a `#[cfg(test)] grid::reference` module. Add a proptest that drives the `integrity.rs` stream strategies (both feed paths, plus resizes and scrollback-limit changes) through both implementations, comparing cursor, every visible row, every history row (cells, extras, wrapped, marks, anchors) and `history_len` after every step. It passes trivially at first and is the main safety net for slices 3–5.
3. **Packed cells.** Row storage becomes `Vec<PackedCell>` plus a grid `StyleTable` with collection; mutation goes through `set_cell` / `update`; `uniform` replaces `plain`; `CompactRow::encode_from` reads packed cells, compares runs by style id, and leaves out the structural bits. `check_integrity` gains checks for id validity and the grapheme bit. Gate: the instruction cut is in the range the prototype measured for A.
4. **Screen page.** `Grid.rows` becomes a `ScreenPage`. Scroll, IL, DL and regions become slot rotations; graphemes go into the `GraphemeStore`. Add a test that IL/DL/scroll allocate nothing, using the counting allocator pattern from `memory.rs`. Gate: scroll and scroll-region instruction cuts; kill rule if the cumulative pty_flood gain is under half of the prototype's prediction for these levers.
5. **History pages.** `VecDeque<CompactRow>` becomes `History`: the uniform and cells row forms, per-row eviction, `RowView` over history rows, reflow and height resize written into pages, and heads for anchors and marks. The round-trip proptest now checks `push(row).view().to_row() == row` with the same generator, including the flags only direct edits create. Gate: all of the GO criteria, on the real code.
6. **Cleanup.** Remove `CompactRow`, the `Cow<Row>` shims and the reference module. Regenerate the baselines and update the memory budgets downward, explaining why. Write `docs/perf/<date>-page-storage.md`, rerun the AWS T4 comparison (`compare.sh`, X11 and Wayland, against Ghostty), and update this ADR's status.

## Risks

- **The gain does not appear end to end.** Fewer instructions have failed to show up in wall time before (gathering reads; the 2026-11-06 ring buffer). Mitigation: the PTY-harness A/B in the prototype, and the kill rule at slice 4.
- **Cold memory.** History appends land in pages last touched 10,000 lines ago, which is the effect that halved throughput on 2026-11-06. The volume per row is about the same as today's compact rows (about 100 bytes, written sequentially) and the screen page shrinks threefold, but this has to be measured at 300 columns and with 100k lines of history.
- **Correctness of a core rewrite.** The invariants are many: wide and spacer pairs, grapheme extras, `used` and `uniform`, marks, anchors, reflow, the cluster `Tail`. Mitigation: the slices, the differential oracle, the extended `check_integrity`, and the existing proptests on both feed paths.
- **Style collection bugs.** Rewriting ids in place has to keep the cached cursor id, the history remaps and `LastPrint` consistent. A generation counter invalidates the caches; a proptest forces collection every N prints.
- **sgr-dense regression** from interning. Mitigation: cache the last few template→id pairs. The gate is +3%.
- **Memory growth for CJK, emoji and truecolor-heavy scrollback** (about 1.7x or more; no budget case covers it). Add a CJK budget case in slice 5; a UTF-8 uniform row form is the fallback.
- **API churn** for renderer code that holds `&Cell`. It is confined to the workspace; no ADR commits to ultty-vt's Rust API.
- **Benchmark noise** on a shared workstation. Use interleaved rounds and report the spread, and treat the T4 rerun as the confirmation.

## Open questions

1. Should the scrollback limit become a byte budget, as Ghostty's is, rather than a line count, now that a row's size varies by about 10x between its forms? It would change configuration semantics.
2. Is a third history row form (UTF-8 plus one style plus a wide bitmap) worth having for CJK memory?
3. Should the renderer cache gain a (page uid, slot, row version) fast path ahead of the content comparison, and is that safe for panes and the alternate screen?
4. History page size: 256 rows, or a byte cap? How large should the free list be, and should `clear_history` return memory to the allocator?
5. Should the write loop use explicit SIMD if autovectorisation does not happen, or is the scalar loop enough?
6. Are the 16 reserved bits in the cell worth spending (for example on DECSCA protection), or should they be kept free?
7. Could cell-form history rows go to a compact UTF-8 form during idle periods to save memory, without touching the I/O thread during floods? Not planned; it depends on question 1.

## Consequences

- The VT core's per-byte cost falls for every corpus that scrolls, most for plain and short-line output; the parser is untouched.
- Scrollback costs less memory than today for every budget case, and scrolled-back rows, search and `text --scrollback` read history without decoding it.
- `Row` stops being the storage type and becomes an owned snapshot; readers use `RowView`.
- Closing the rest of the gap to Ghostty after this would mean the parser and handler dispatch (P and the 3.9 instructions per byte of parsing) and the kernel's 4 KiB reads, which this ADR does not address.
