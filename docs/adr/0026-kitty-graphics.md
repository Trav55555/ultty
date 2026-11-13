# ADR 0026: Kitty Graphics Protocol

## Status

**Proposed (not scheduled)** 2026-11-12. This records a possible feature for
deciding later; slices 1 to 3 are not scheduled.

Slice 0, the sixel fixes that matter whether or not the rest goes ahead, is
done: #129, #130, #132, #133 and #134, merged 2026-11-13. "What Ultty has
today" below describes the code before those fixes, at 86d9d91.

## Context

### Why

`docs/feature-parity.md` ranks the missing Kitty graphics protocol as gap 3
of 10. Ghostty 1.4 supports it, including animation and relative placement.
Ultty consumes `ESC _ G … ESC \` and drops it (`ApcKind::Other` in
`crates/ultty-vt/src/ansi/mod.rs`). The compat scorecard row says "No — use
Sixel" (`docs/compat-scorecard.md`).

The programs that need it are those that speak only Kitty's protocol, or
prefer it:

- `kitten icat`, and image previews in yazi, ranger and lf;
- Neovim plugins such as image.nvim, and others that use Unicode
  placeholders so images survive tmux and scrolling;
- timg and chafa, which pick Kitty when it is detected and fall back to
  sixel or block characters;
- notebook and plotting front ends (for example matplotlib Kitty backends,
  and euporie).

This list comes from the programs' own documentation, not from tests run
here. None of these tools is installed on the dev machine.

Sixel cannot do what these programs rely on:

- **No ids.** A sixel cannot be moved, re-placed or deleted, so an app must
  redraw the cells. Kitty lets an app upload an image once and place it
  many times.
- **No compression.** Sixel is a palette format of at most 256 colours,
  sent as text. A photo is large and loses quality. Kitty takes PNG or raw
  RGB/RGBA, optionally zlib-compressed.
- **No layering.** Kitty has z-index, so an image can sit under or over
  text.
- **No placeholders.** Sixel pixels are lost when tmux or a pager redraws.
  Kitty's Unicode placeholders are ordinary text that tmux passes through.

### What Ultty has today

Sixel works for the simple case: decode, place at the cursor, draw. Checked
on `main` at 86d9d91, with a throwaway probe test (not committed) for the
behaviour:

- **Decode.** `decode_sixel` (`crates/ultty-vt/src/grid/image.rs`) caps
  each side at `MAX_SIXEL_DIM` = 16,384 and the total at
  `MAX_SIXEL_PIXELS` = 16 Mi pixels, and stores RGBA.
- **Store.** There is one `ImageStore` per `Terminal`, which is per pane
  (`terminal.rs`, field `images`). Main and alternate screens share it. It
  is capped at `MAX_IMAGE_STORE_BYTES` = 256 MiB and
  `MAX_IMAGE_STORE_COUNT` = 1,024. The oldest image is evicted first.
  `check_integrity` asserts both caps.
- **Position.** `TerminalImage` holds `start_col` and `start_row`, which
  are screen coordinates fixed when the image is placed
  (`term_handler.rs`, `handle_sixel_image`). Nothing updates them.
- **Render.** `Renderer::render_images` (`crates/ultty/src/renderer/image.rs`)
  draws each image at `start_row * cell_height + tab_bar_height`. It draws
  after cell backgrounds and before text, so in Kitty terms the z-index is
  "under text". Only the one-pane frame paths call it
  (`render_with_overlay` and `render_with_native_sessions` in
  `renderer/mod.rs`).

The probe and the code show these gaps:

1. **Images do not scroll.** After six line feeds on a 5-row screen, the
   image was still at `(0, 0)`. The renderer ignores the scroll offset, so
   an image stays put while text scrolls under it, and while the user
   scrolls back.
2. **Clearing does not clear images.** After `ED 2` and `ED 3` the store
   still held the image. Entering the alternate screen (1049) shows the
   main screen's images, because the store is shared. Only RIS clears it
   (`reset_auxiliary_state`).
3. **Large sixels print as text.** DCS strings are capped at 64 KiB
   (`MAX_STRING_BUFFER`, `ansi/osc.rs`). At the cap, `dcs_string` calls
   `reset()`, which goes back to ground, so the rest of the sixel prints as
   text. A 70,000-byte sixel filled the screen with `~`.
4. **No images in split panes or pane screenshots.** `encode_panes`
   (`renderer/panes.rs`) does not draw terminal images. It is used for
   split tabs and for `capture_panes_screenshot`.
5. **The texture cache mixes up tabs.** `Renderer::image_cache` is keyed
   by `ImageId` alone. Every `Terminal` numbers its images from 0, and
   `upload_image` returns early when the key is already present. A second
   tab's image 0 can therefore draw with the first tab's texture. The
   cache is cleared only when a tab is closed (`app/input.rs`). When it
   holds more than 64 textures it evicts arbitrary `HashMap` keys, and it
   has no byte cap.
6. **The PTY reports no pixel size.** `PtySize::with_pixels` exists
   (`pty/master.rs`), but every spawn and resize uses `PtySize::new`, so
   `TIOCGWINSZ` returns `ws_xpixel = ws_ypixel = 0`. Many image tools read
   the cell size from there. XTWINOPS 14 and 16 do answer
   (`Terminal::cell_pixel_size`).

A Kitty query today gets no reply (probe: `a=q` gave no responses). DA1
replies `?62;4;22c` (`PRIMARY_DEVICE_ATTRIBUTES`, `term_handler.rs`), so
clients that probe "query, then DA1" correctly see no support.

Session restore (`crates/ultty/src/session.rs`) saves tab titles,
directories and layout, not screen contents. Images have nothing to
restore.

### Overlap with the native protocol

Native apps can already show images: `Content::Image` carries
`ImageWidget` (RGBA only, with a scale mode;
`crates/ultty-protocol/src/content.rs`). The spec caps it at 16 MiB of data
and 16,384 pixels a side (`docs/PROTOCOL_SPEC.md` §3 limits, §8.4). The
image is laid out with the region and drawn by the GPU. Accessibility
gives it the `Image` role, with its label as alt text (ADR 0021). It works
over the socket and in-band (ADR 0018). Its ANSI-mode fallback in `ulttui`
draws a bordered box with a caption, not the image
(`crates/ulttui/src/widgets/image.rs`, `render_ansi`).

So native apps are covered. The gap is the large set of existing programs
that will never speak Ultty's protocol. ADR 0020 already rejected
Kitty-style placement for inline surfaces and noted Kitty graphics as "a
separate gap".

ADR 0004 puts terminal correctness before visual richness. Gaps 1 to 3
above are correctness bugs in what Ultty already advertises (DA1 includes
4, sixel). They come first.

## Decision (proposed)

Add the Kitty graphics protocol in slices, each usable on its own. Slice 0
is a prerequisite. A later slice may never be built if demand does not
show up.

### Slice 0: make the existing image path correct

Done. These fixes were needed for sixel on their own, and slice 1 builds on
the same placement model.

| PR | Fix |
|---|---|
| #129 | `TIOCGWINSZ` reports pixel size; an oversized DCS is swallowed to ST (sixel strings get 8 MiB) |
| #130 | Texture cache keyed by terminal and image, LRU with a 256 MiB byte cap |
| #132 | One image store per screen; ED 2, leaving the alternate screen and RIS clear images (following foot) |
| #133 | Images pinned to text lines: scroll, scrollback, eviction, ED 3, reflow, clipping |
| #134 | Images drawn in split panes and pane screenshots |

Left over from slice 0: ED 0/1 and EL do not delete images; an image taller
than the space below the cursor is not scrolled into view; reflow places an
image by cell offset, so wide-character spacing can move it by a row; mode
47 clears like 1047/1049, which was not checked against foot.

The list as planned:

- Pin images to text lines so that they scroll, sit in scrollback and
  follow reflow (see Design, "Placements in the grid").
- Give each screen its own image store, so the alternate screen starts
  with no images. RIS, `ED 2` and `ED 3` clear images as Kitty specifies.
- When a DCS string passes its cap, consume the rest of the string instead
  of printing it, the way `ApcKind::Oversized` already does for APC.
- Draw terminal images in `encode_panes`, clipped to the pane, so split
  panes and pane screenshots show them.
- Key the texture cache by `(terminal, image)`. Evict textures whose image
  is gone, and add a byte cap.
- Report pixel size through `TIOCGWINSZ`: use `PtySize::with_pixels` on
  spawn and on resize.

### Slice 1: direct transmission, placement, delete, query

**Framing.** An APC string whose first byte is `G` becomes a new
`ApcKind::Graphics`. It is collected, up to a per-string cap (see Resource
limits), and dispatched as an `Action::Graphics` that carries the control
data and the payload. Control keys are parsed into a typed command. Unknown
keys are ignored. A malformed value gets an `EINVAL` reply, unless the
quiet mode suppresses it.

**Actions supported:**

| `a=` | Meaning |
|---|---|
| `t` | Transmit and store. This is the default when `a` is absent; I could not confirm the default in the spec text I read, so check it. |
| `T` | Transmit and place. |
| `p` | Place an image that was sent earlier. |
| `d` | Delete. |
| `q` | Query: decode and validate, reply, store nothing. |

**Keys supported:**

- `i`, `I`, `p`, `q`.
- `f` = `24`, `32` or `100`.
- `t=d`.
- `o=z`.
- `m`.
- `s`, `v`: required for `f=24/32`.
- `S`: the data size when `o=z` is used with PNG.
- `x`, `y`, `w`, `h`: the source rectangle.
- `X`, `Y`: the pixel offset inside the first cell.
- `c`, `r`: the display size in cells.
- `z`.
- `C`.
- `N`: the transient hint, used for eviction order only.

**Delete (`d=`):** `a A i I n N c C p P q Q r R x X y Y z Z`. Lowercase
removes placements. Uppercase also frees images that are left with no
placement. Any delete aborts an upload still in progress (spec). `f`/`F`
(frames) and a bare `a=d` follow the spec.

**Replies.** Replies are sent for `a=t/T/p/q` when `i` or `I` is set:
`ESC _G i=<id>[,I=<n>][,p=<pid>];OK ESC \`, or `;<CODE>:<fixed text>`.
`q=1` suppresses `OK`. `q=2` suppresses errors too. `i` together with `I`
is `EINVAL`.

**Refused in slice 1, each with an error reply:**

- **`t=f` (file), `t=t` (temp file) and `t=s` (shared memory).** These
  are refused with `EBADF`, the one code the spec mandates for every
  image-read failure, plus fixed text saying the medium is not supported.
  The reasons:
  1. They make the terminal read local files or shared memory chosen by
     whatever writes to the PTY. That includes a `cat`ed file and a remote
     host over ssh. The spec has to add symlink, special-file and
     sensitive-path rules for this, and to require one indistinguishable
     error for every failure, so that the reply does not reveal whether a
     path exists (spec, "transmission medium" notes). Ultty refuses
     `file:` links and OSC 52 reads for the same reason
     (`docs/security-audit.md`).
  2. `t=t` and `t=s` delete or unlink the object after reading it, so the
     PTY's writer could make the terminal delete files.
  3. The only gain is speed for local clients, and only for large images.
     Direct transmission works for local and remote clients alike.
  4. Clients fall back. `kitten icat` uses direct transmission over ssh.
     I have not checked how timg and chafa fall back when a medium is
     refused, so slice 1's smoke test must check it.
- **Formats `f=8` and `f=16` (indexed, kitty 0.50).** Refused with
  `EINVAL`.
- **Relative placements (`P`, `Q`, `H`, `V`).** Refused with `EINVAL`.
- **Virtual placements (`U=1`).** Refused with `EINVAL` until slice 2.
- **Animation actions (`a=f`, `a=a`, `a=c`).** Refused with `EINVAL`.
- **Feature queries (`e=`, kitty 0.50).** Answered with `EINVAL` for now.
  I am not sure what a client expects from a terminal that lacks feature
  queries. Check against kitty's own behaviour before building.

Slice 1 also adds a scorecard row, "Kitty graphics (direct)", and
`ulttyctl` exposure of placements for tests (see Testing).

### Slice 2: Unicode placeholders

This slice is for tmux and Neovim.

- `a=p,U=1` (or `a=T,U=1`) creates a virtual placement of `c` × `r` cells.
- Cells whose base character is U+10EEEE are drawn as part of an image.
  The image comes from the cell's foreground colour (24-bit, or 8-bit for
  ids below 256) plus an optional third diacritic for the high byte. The
  placement id comes from the underline colour. The row and column come
  from the first two diacritics, through the diacritics table in kitty's
  `rowcolumn-diacritics.txt`. Missing diacritics are inferred from the
  cell to the left, under the spec's three rules.
- Deletes affect virtual placements only for `d = i I r R n N` (spec).

Placeholders are text, so scroll, reflow, scrollback, tmux redraws and
the alternate screen all work through the grid unchanged. The grid
already stores combining marks as grapheme extras on the cell (ADR 0022
§4).

Relative placements (`P`, `Q`, `H`, `V`) can join this slice if
image.nvim or similar needs them. They are still refused otherwise.

### Slice 3: animation, only if demand appears

`a=f` (frames, with deltas and blending), `a=a` (playback: `s`, `v`,
`c`), `a=c` (composition), and feature queries `e=f/a/c`. Kitty stores
frames on disk under a separate quota, 5× the base one (spec, "Storage
quotas"). Ultty would keep frames in memory under the same per-screen
quota. This slice is not scheduled. Its trigger would be a named program
that users run in Ultty and that is broken without it.

## Design

### Image store

Each `Buffer` (main and alternate) gets its own `GraphicsStore`, replacing
the per-`Terminal` `ImageStore`:

```text
GraphicsStore {
  images:     HashMap<ImageId, StoredImage>   // internal id, as today
  by_client:  HashMap<u32, ImageId>           // Kitty i=
  by_number:  HashMap<u32, Vec<ImageId>>      // Kitty I=, newest last
  placements: Vec<Placement>                  // sorted by line, then z
  upload:     Option<Upload>                  // the one chunked upload in flight
  bytes, quota
}
StoredImage { width, height, rgba: Arc<[u8]>, client_id, number, transient, last_used }
Placement {
  image, placement_id,                        // p=, 0 = anonymous
  line: u64, col: u16,                        // top-left cell; line = evicted + absolute index
  cols, rows,                                 // display span in cells
  x_off, y_off,                               // X=, Y= pixels inside the first cell
  src: Rect,                                  // x, y, w, h
  z: i32,
}
```

- Sixel images go into the same store. They have no client id and get one
  anonymous placement at z = −1.
- `ImageId` stays internal. Kitty ids and numbers map onto it, so a Kitty
  client cannot collide with a sixel. Placing with the same `(i, p)`
  replaces the old placement, as the spec requires. Re-transmitting an id
  replaces the image's data and drops its placements.
- `rgba` is an `Arc` so the renderer can hold it without copying.

### Placements in the grid and scrollback

A placement is pinned to a **line number**: `Buffer::evicted` plus the
row's absolute index. The `Buffer` docs say this numbers a line for good
(`buffer.rs`). Scrolling into history therefore moves nothing. The
renderer maps line numbers to visual rows with the same arithmetic as
`visible_row`, as inline surfaces do (ADR 0020 §4).

- **Eviction from history.** A placement whose last line is evicted is
  dropped, and its image becomes unreferenced (see Resource limits for the
  eviction order).
- **Scroll regions.** For scrolls inside a region that does not start at
  row 0 (vim, less), the spec says that only images entirely inside the
  margins scroll, and the others are clipped. The scroll path shifts
  placements whose rows lie inside the region and clips those that cross
  it. The check is one branch on "this screen has placements", so the hot
  scroll path in ADR 0022 pays nothing when a screen has no images.
- **Reflow** (a width change) moves a placement's top-left cell to where
  that cell's source row went. `place_anchors_after_reflow` receives this
  map (`made_at`) for anchor rows only. It would have to cover rows that
  carry placements as well. The image keeps its cell span and is not
  rewrapped. I have not confirmed how kitty or Ghostty move images on a
  width change. A placement whose row falls past the history limit is
  dropped.
- **Height resize** goes through the same line numbers and needs no
  special case.
- **Cell size change** (zoom, font change). Placements keep their cell
  span, and the image is scaled to it, so text layout does not change.
  Kitty may recompute the span from pixels. Open question 4.

Rows do not change: there is no per-cell or per-row image field, and the
page layout in ADR 0022 is untouched. The cost is one sorted side table
per screen, which is empty for almost every pane.

### Z-index

The renderer draws three image passes, using the spec's thresholds:

| z | Drawn |
|---|---|
| z < −1,073,741,824 | before cell backgrounds |
| −1,073,741,824 ≤ z < 0 | after backgrounds, before text (where sixel draws today) |
| z ≥ 0 | after text, before the cursor and overlays |

Within a pass, images are sorted by z, then by image id (spec). Each image
is one textured quad clipped to the pane. Batching all quads of a pass
into one draw call per texture replaces today's render pass per image.

### Behaviour across terminal actions

These rules follow the spec's "Interaction with other terminal actions"
section.

| Action | Effect on placements |
|---|---|
| Line feed, scroll | They move with their lines (by line number). |
| Region scroll (`DECSTBM`, IL/DL) | They shift inside the region and are clipped at its margins. Whether IL/DL count as scrolls for images is not settled; follow kitty. |
| `ED 2` | Placements on the visible rows are deleted. `ED 3` deletes those in scrollback. |
| Other erases (`EL`, `ECH`, `DCH`, `ED 0/1`) | No effect. Per the spec, only delete commands remove graphics. |
| 1049 / 1047 enter | The alternate store is cleared, as its text is. Main placements stay in the main store. |
| RIS | Both stores are cleared. |
| Width resize | Placements follow reflow (above). |
| Split, pane resize | Each pane is its own `Terminal`. Images are clipped to the pane rectangle. |
| Session restore | Nothing to do. Screen contents are not saved. |

### Screenshots, accessibility, search and selection

- **Screenshots.** Both screenshot paths draw images, after the slice 0
  fix to `encode_panes`.
- **Accessibility.** In slice 1, each visible placement adds one
  unlabelled `Image` node, with its bounds, under its pane's `Terminal`
  node (`crates/ultty/src/a11y/tree.rs`). Kitty has no alt text. A screen
  reader then announces that an image exists, not what it shows. Open
  question: skip the node instead.
- **Search, `text`, `last-command`, selection and copy.** These read text
  only, and placements do not change it. In slice 2, placeholder cells are
  text, and copying them would put U+10EEEE and diacritics on the
  clipboard. The proposal is to copy them as nothing. Open question 3.

### Where decoding lives

The graphics state machine (store, placements, replies) belongs in
`ultty-vt`, next to sixel, so that `conformance.rs`, `scorecard.rs` and
the `vt_terminal` fuzzer reach it. `ultty-vt` has no I/O and only four
dependencies today. PNG and zlib decoding would add `png` (already a
dependency of `ultty`, for screenshots: 0.17, plus 0.18 through `arboard`
→ `image`) and `miniz_oxide` (already in the tree through `png`/`flate2`).
Both are pure Rust with no I/O.

The alternative is a decoder trait that `ultty` injects. That keeps
`ultty-vt` lean, but fuzzing it needs both crates. Open question 1.

Decoding runs on the thread that parses PTY output. At the caps below, a
worst-case PNG takes tens of milliseconds to decode, and the pane stalls
for that long. That is acceptable for slice 1, and should be measured.

## Security

- **No local reads.** Only `t=d` is accepted. Ultty never opens, reads,
  unlinks or `shm_open`s a path that arrives in terminal output. This is a
  rule in the code, not a setting, like the other rows in
  `docs/security-audit.md` "Runtime hardening".
- **New untrusted parsers.** Base64, zlib and PNG decoding of bytes that
  any PTY writer controls, including a `cat`ed file or an ssh peer.
  - **Base64.** Use the workspace `base64` crate, with strict decoding.
    Chunks other than the last must be a multiple of 4 bytes (spec).
  - **zlib (`o=z`).** Use `miniz_oxide`'s inflate with an output limit
    equal to the expected decoded size. For `f=24/32` that is `s × v × 3`
    or `4`, exactly. For PNG it is the `S` key, capped at the transmitted
    cap. Output past the limit is a decompression bomb: stop, reply
    `EINVAL`, keep nothing.
  - **PNG.** Use `png::Decoder::new_with_limits` with
    `Limits { bytes: per-image decoded cap }`. Read the header first, and
    reject dimensions over 16,384 a side or more than `MAX_IMAGE_PIXELS`
    in total before allocating the frame. Expand palette and 16-bit
    images to RGBA8 (`Transformations::EXPAND | STRIP_16`). Decode only
    the first frame of an APNG.
  - **No `unsafe`** in the new code. Any decode error becomes a reply
    code, never a panic. Because of `panic = "abort"` in release
    (`CLAUDE.md`), a panic in a decoder would take down every tab.
- **Replies must not leak.** A reply contains only the numbers the client
  sent (`i`, `I`, `p`), parsed and printed again as integers, plus a code
  and fixed ASCII text from a table. Payload bytes, paths, internal ids,
  sizes or other state never appear in a reply. A reply also never
  reflects bytes from the request, so a `cat`ed file cannot use a reply to
  type arbitrary text into the shell. The worst it can do is make the
  terminal type a well-formed `ESC _G…ESC \`, which shells ignore.
  Queries never store anything (spec).
- **Fuzzing.** Add a `kitty_graphics` target in `fuzz/fuzz_targets/` (see
  Testing).
- **Audit.** Add a row to `docs/security-audit.md` "Runtime hardening":
  "Terminal output making Ultty read, delete or map local files (Kitty
  `t=f/t/s`)" → "Only direct transmission; other media get `EBADF`."

## Resource limits

All limits are per screen (main and alternate each), matching Ghostty and
kitty:

| Limit | Proposed value | Reason |
|---|---|---|
| Transmitted bytes per image (after base64, before zlib) | 32 MiB | Twice the native protocol's 16 MiB payload cap. A 4K PNG photo fits. |
| Decoded size per image | 16,384 a side, `MAX_IMAGE_PIXELS` = 16 Mi pixels (64 MiB RGBA) | Same as `MAX_SIXEL_PIXELS`, which then serves both protocols. |
| APC string per chunk | 64 KiB | The spec says chunks are at most 4,096 bytes. This leaves room for clients that send larger ones; whether any do is not checked. Past the cap, `ApcKind::Oversized`: the string is consumed and dropped. |
| In-flight upload | One per screen, capped at the transmitted cap | The spec forbids interleaving other graphics commands with an upload. A new `i` without `m` continuing, a delete, or RIS abandons it. A chunk that would pass the cap aborts the upload with `EFBIG` (if kitty uses that code; unchecked) and frees the buffer. |
| Upload idle timeout | None | A text-only check cannot tell a slow ssh link from an abandoned upload. The cap bounds the memory. |
| Store quota (decoded bytes) | 256 MiB per screen by default. Config `images.storage_limit`; 0 disables both protocols. | Keeps today's `MAX_IMAGE_STORE_BYTES`. Kitty uses 320 MB per buffer; Ghostty's `image-storage-limit` defaults to 320 MB per screen. |
| Images per screen | 1,024 (today's `MAX_IMAGE_STORE_COUNT`) | Bounds the map sizes. |
| Placements per screen | 4,096 | Bounds the side table and the per-frame quad count. |
| GPU texture cache | Byte cap, for example 256 MiB per window, LRU | Today it caps the count (64) but not the bytes. |

**Eviction** runs when a new image would pass the quota:

1. images with no placements, transient (`N=1`) first, oldest first (spec:
   images without placements go first);
2. images whose only placements are in scrollback, oldest first;
3. the oldest image.

The new image itself is never evicted to make room for itself. An image
larger than the whole quota is refused with an error reply.

**Fit with the memory budgets.** The text budgets in
`crates/ultty/benches/memory.rs` are about 1.2 MB for 10,000 typical lines
at 120x40, and about 10.5 MB for sgr-dense. One 1920×1080 image is
8.3 MB of RGBA, so images will dominate any pane that uses them. That is
why the quota is a hard cap per screen and is configurable.

With two screens per pane, the worst case is 512 MiB per pane, which is
the same order as kitty and Ghostty. It is also the same as today's
256 MiB per terminal, which sixel can already fill. A process-wide cap
across panes is open question 2.

Add memory bench cases:

- `kitty_image_1080p_placed`: the budget is the pixel bytes plus 5%, and
  no in-flight buffer is left after the upload.
- `kitty_quota_churn`: upload 2× the quota, and the live heap stays under
  quota + 5%.

## Testing

- **Conformance** (`crates/ultty-vt/tests/conformance.rs`), one test per
  spec rule:
  - transmit then place; `a=T`;
  - same `(i, p)` replaces; `I` gets a fresh id and reply;
  - every `d=` value, lowercase and uppercase;
  - `q=1` and `q=2`; `i` with `I` gives `EINVAL`;
  - each refused medium gives `EBADF` with no file access;
  - `ED 2`, 1049 and RIS clear;
  - placements scroll into history and are dropped on eviction;
  - region scroll clips; cursor movement after placement, and `C=1`.

  Slice 0 adds the sixel tests that the probe shows failing today: scroll,
  `ED 2`, alternate screen, and a large DCS not printing.
- **Scorecard** (`scorecard.rs`): add `scorecard_kitty_graphics`, which
  queries, gets `OK`, transmits a 1×1 PNG and places it. The compat
  scorecard row switches to "Yes (direct transmission)". The
  feature-parity row and gap 3 are updated.
- **Fuzz.** Add `fuzz/fuzz_targets/kitty_graphics.rs`. It builds
  structured Kitty commands from the input (keys from a fixed set, values
  from fuzz bytes), with a payload that is fuzz bytes, optionally
  zlib-compressed and always base64-encoded. It splits them into chunks
  and interleaves resizes and text. After each step it asserts
  `check_integrity` (extended to cover stores, quotas and placements) and
  that every reply matches the reply grammar. The existing `vt_terminal`
  target reaches the framing but rarely produces a valid payload, so it
  is not enough on its own.

  A second target, `image_decode`, feeds raw bytes to the PNG and zlib
  decoders with the production limits. It asserts no panic, and peak
  allocation within the cap, measured with a counting allocator as in the
  memory bench.
- **Proptests** for chunking, in `ultty-vt`:
  - any split of a valid upload into chunks (multiples of 4), and any
    split of the byte stream across `feed` calls, gives the same stored
    image as one unchunked command;
  - text between chunks is printed and does not corrupt the upload;
  - a delete in the middle aborts the upload;
  - going over the cap aborts the upload, frees its buffer and prints
    nothing.
- **Render checks.** There is no pixel-golden harness today. The
  canonical visual path is `ulttyctl screenshot` (`docs/visual-regression.md`).
  Add an e2e script that places solid-colour images at known z-indexes
  and cells, takes a screenshot, and samples pixels:
  - under-text vs over-text (a glyph pixel is the image colour or the
    text colour);
  - below-background (hidden by a cell with a non-default background);
  - after scrolling one line, the image is one cell height higher;
  - in a split pane, clipped to the pane.

  Sampling pixels is robust to font changes, where full PNG goldens would
  not be.
- **Real programs.** Add `scripts/e2e/kitty-graphics.sh`, run under
  `scripts/e2e/run-headless.sh`:
  - `kitten icat --transfer-mode=stream` on a fixture PNG;
  - `timg -pk` and `chafa -f kitty` on the same file;
  - for each, `ulttyctl` shows one placement of the right cell size;
  - a screenshot shows non-background pixels in the expected rectangle;
  - nothing printed text into the grid.

  The tools are not installed on the dev machine, so the CI image needs
  them. The script skips with a message if they are missing, and fails in
  CI. In slice 2, the same script runs `kitten icat --unicode-placeholder`
  inside tmux with `allow-passthrough on`.
- **ulttyctl support.** To make these assertable, add
  `ulttyctl images [--pane N] --json`, listing placements as
  `{image, placement, line, col, cols, rows, z, width, height}`. No pixel
  data is returned.

## Alternatives considered

- **Do nothing (sixel only).** This costs nothing, and Ultty stays ahead
  of Ghostty and Alacritty on sixel. Kitty-only programs still show
  nothing, and gap 3 stays open. Slice 0 is worth doing even under this
  option, because the sixel bugs are real.
- **The full spec at once,** including file, temp-file and shared-memory
  media, animation and relative placement. That matches Ghostty sooner.
  It roughly doubles the work, and it adds the only part of the protocol
  that reads local files. The other features have no named user here yet.
- **Steer apps to the native protocol** (`Content::Image`). This is
  already the better path for ulttui apps. It has layout, accessibility
  labels and works in-band. But the programs above are not going to adopt
  a protocol that only Ultty speaks. It is complementary, not an
  alternative. One cheap bridge, out of scope here: ulttui's ANSI mode
  could emit Kitty graphics for `Image` in terminals that support it.
- **Keep decoded images compressed** (store the PNG, decode on draw).
  That uses less resident memory for photos, but costs CPU on every
  texture upload and complicates eviction. Rejected for slice 1. The
  quota bounds memory either way.

## Consequences

- Programs that speak only Kitty's protocol show images in Ultty, over
  ssh too, and with slice 2 inside tmux and Neovim.
- Sixel becomes correct: images scroll, clear, stay off the alternate
  screen, show in split panes, and large sixels stop printing as text.
- `ultty-vt` gains two pure-Rust decoding dependencies (unless open
  question 1 goes the other way), and the project gains two fuzz targets
  to keep running.
- The attack surface grows by base64, zlib and PNG parsing. File access
  does not grow.
- Image memory is capped per screen and is configurable. A pane that uses
  images will use many times the memory of a text-only pane.
- Three image passes per frame replace one, at no cost when a pane has no
  images.

## Slices and rough effort

These are estimates for one engineer who knows the codebase, before any
prototype. Each slice also needs `cargo test --workspace`, clippy with
`-D warnings`, `fmt --check`, and its own check.

| Slice | Contents | Effort | Check |
|---|---|---|---|
| 0 (done) | Line-pinned placements and the per-screen store for sixel; clear rules; DCS overflow; images in `encode_panes`; cache key and byte cap; PTY pixel size | 4–6 days (done in #129–#134) | The new sixel conformance tests; split-pane screenshot sampling |
| 1 | APC `G` framing; commands; direct `f=24/32/100` with `o=z`; chunking; place, delete, query; replies; z passes; quotas and eviction; `ulttyctl images`; fuzz and proptests; scorecard; security-audit row | 8–12 days | Conformance, proptests, fuzz runs clean for 1 hour, kitty-graphics.sh |
| 2 | Virtual placements; placeholder rendering; diacritics table; copy rule; optional relative placements | 4–6 days (plus 2–3 for relative placement) | Placeholder conformance; tmux e2e |
| 3 | Animation | 6–10 days | Not planned |

## Open questions

1. **Where decoding lives.** In `ultty-vt` with `png` and `miniz_oxide`,
   which is simpler and fully fuzzable there, or behind a trait that
   `ultty` provides, which keeps `ultty-vt`'s dependencies at four?
2. **Process-wide cap.** Should there be a cap across all panes (for
   example 1 GiB), on top of the per-screen quota? Otherwise ten panes
   could hold 5 GiB.
3. **Copying placeholder cells** (slice 2). Copy them as nothing, as
   spaces, or as the raw characters? Raw is what tmux will see anyway.
   I have not checked what kitty does.
4. **Cell size changes.** Should a placement keep its cell span, or keep
   its pixel size and recompute the span? Check kitty's and Ghostty's
   behaviour before building.
5. **Feature queries (`e=`).** Do clients that send them handle `EINVAL`
   from a terminal without the feature, or do they expect a particular
   reply? I am not sure, from the spec text alone.
6. **Error codes.** The spec text I read names `ENOENT`, `EINVAL`,
   `EBADF`, `ETOODEEP`, `ECYCLE`, `ENOPARENT` and (for composition)
   `ENOSPC`. I did not confirm the codes for "data too large" or "not
   enough data" (`EFBIG`, `ENODATA` are guesses). Use kitty's actual
   strings.
7. **Accessibility.** Is an unlabelled `Image` node per placement useful
   to screen reader users, or noise? ADR 0021's review could decide.
8. **Decode stalls.** Is a stall of tens of milliseconds on the parse
   thread acceptable for a 16-megapixel PNG, or should decoding move off
   that thread with a deferred reply?
