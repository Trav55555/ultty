/**
 * Binary wire codec for Ultty protocol v1.
 *
 * Frame layout (little-endian):
 *   magic:   2 bytes (0x55 0x54 = "UT")
 *   type:    2 bytes (u16)
 *   length:  4 bytes (u32, payload byte length)
 *   payload: `length` bytes
 */

import {
  FRAME_HEADER_LEN,
  FRAME_MAGIC,
  FRAME_MAGIC_LE,
  MAX_IMAGE_DIMENSION,
  MAX_METADATA_PAIRS,
  MAX_METADATA_TOTAL_BYTES,
  MAX_PAYLOAD_LEN,
  MAX_REGION_COUNT,
  MAX_SEMANTICS_STRING_LEN,
  MAX_SEMANTICS_TOTAL_BYTES,
  MAX_SPAN_COUNT,
  MAX_STRING_LEN,
  MAX_SURFACE_ROWS,
  MAX_TRACK_COUNT,
  MIN_SURFACE_ROWS,
  MOD_NONE,
  MsgType,
  ROOT_REGION,
  type AlignContent,
  type AlignItems,
  type AlignSelf,
  type AppMessage,
  type BorderStyle,
  type BorderType,
  type ButtonStyle,
  type Color,
  type ComputedRegionLayout,
  type Constraints,
  type Content,
  type Dimension,
  type Display,
  type Easing,
  type Edges,
  type Expanded,
  type FlexDirection,
  type FlexWrap,
  type GridLine,
  type GridPlacement,
  type ImageScale,
  type InputEvent,
  type InputType,
  type JustifyContent,
  type Key,
  type LayoutTrigger,
  type Live,
  type MouseButton,
  type NamedColor,
  type Overflow,
  type OverflowAxis,
  type RichText,
  type Role,
  type Semantics,
  type Size,
  type Span,
  type SpinnerStyle,
  type Style,
  type SurfaceEventKind,
  type TerminalMessage,
  type TextAlign,
  type TextContent,
  type TextOverflow,
  type TextWrap,
  type Toggled,
  type TrackSizingFunction,
  type Transition,
  type WidgetContent,
} from "./types.ts";

export type ProtocolErrorCode =
  | "invalid_magic"
  | "unexpected_eof"
  | "invalid_utf8"
  | "invalid_float"
  | "invalid_data"
  | "unknown_message_type";

export class ProtocolError extends Error {
  readonly code: ProtocolErrorCode;

  constructor(code: ProtocolErrorCode, message: string) {
    super(message);
    this.name = "ProtocolError";
    this.code = code;
  }
}

const utf8Encoder = new TextEncoder();
const utf8Decoder = new TextDecoder("utf-8", { fatal: true });

class ByteWriter {
  private buf: Uint8Array;
  private view: DataView;
  private pos = 0;

  constructor(initialCapacity = 256) {
    this.buf = new Uint8Array(initialCapacity);
    this.view = new DataView(this.buf.buffer);
  }

  private ensure(extra: number): void {
    const needed = this.pos + extra;
    if (needed <= this.buf.length) return;
    let next = this.buf.length;
    while (next < needed) next *= 2;
    const grown = new Uint8Array(next);
    grown.set(this.buf.subarray(0, this.pos));
    this.buf = grown;
    this.view = new DataView(this.buf.buffer);
  }

  u8(v: number): void {
    this.ensure(1);
    this.buf[this.pos++] = v & 0xff;
  }

  u16(v: number): void {
    this.ensure(2);
    this.view.setUint16(this.pos, v, true);
    this.pos += 2;
  }

  i16(v: number): void {
    this.ensure(2);
    this.view.setInt16(this.pos, v, true);
    this.pos += 2;
  }

  u32(v: number): void {
    this.ensure(4);
    this.view.setUint32(this.pos, v >>> 0, true);
    this.pos += 4;
  }

  i32(v: number): void {
    this.ensure(4);
    this.view.setInt32(this.pos, v | 0, true);
    this.pos += 4;
  }

  u64(v: bigint): void {
    this.ensure(8);
    this.view.setBigUint64(this.pos, v, true);
    this.pos += 8;
  }

  f32(v: number): void {
    this.ensure(4);
    this.view.setFloat32(this.pos, v, true);
    this.pos += 4;
  }

  bytes(data: Uint8Array): void {
    this.ensure(data.length);
    this.buf.set(data, this.pos);
    this.pos += data.length;
  }

  str(s: string): void {
    const encoded = utf8Encoder.encode(s);
    this.u32(encoded.length);
    this.bytes(encoded);
  }

  optStr(s: string | null): void {
    if (s === null) {
      this.u8(0);
    } else {
      this.u8(1);
      this.str(s);
    }
  }

  optF32(v: number | null): void {
    if (v === null) {
      this.u8(0);
    } else {
      this.u8(1);
      this.f32(v);
    }
  }

  toFrame(typeCode: number): Uint8Array {
    const payloadLen = this.pos;
    const out = new Uint8Array(FRAME_HEADER_LEN + payloadLen);
    out[0] = FRAME_MAGIC[0];
    out[1] = FRAME_MAGIC[1];
    const headerView = new DataView(out.buffer);
    headerView.setUint16(2, typeCode, true);
    headerView.setUint32(4, payloadLen, true);
    out.set(this.buf.subarray(0, payloadLen), FRAME_HEADER_LEN);
    return out;
  }
}

class ByteReader {
  private readonly buf: Uint8Array;
  private readonly view: DataView;
  pos = 0;

  constructor(data: Uint8Array) {
    this.buf = data;
    this.view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  }

  remaining(): number {
    return this.buf.length - this.pos;
  }

  private need(n: number): void {
    if (this.remaining() < n) {
      throw new ProtocolError(
        "unexpected_eof",
        `unexpected EOF: needed ${n} bytes, have ${this.remaining()}`,
      );
    }
  }

  u8(): number {
    this.need(1);
    return this.buf[this.pos++];
  }

  u16(): number {
    this.need(2);
    const v = this.view.getUint16(this.pos, true);
    this.pos += 2;
    return v;
  }

  i16(): number {
    this.need(2);
    const v = this.view.getInt16(this.pos, true);
    this.pos += 2;
    return v;
  }

  u32(): number {
    this.need(4);
    const v = this.view.getUint32(this.pos, true);
    this.pos += 4;
    return v;
  }

  i32(): number {
    this.need(4);
    const v = this.view.getInt32(this.pos, true);
    this.pos += 4;
    return v;
  }

  u64(): bigint {
    this.need(8);
    const v = this.view.getBigUint64(this.pos, true);
    this.pos += 8;
    return v;
  }

  f32(): number {
    this.need(4);
    const v = this.view.getFloat32(this.pos, true);
    this.pos += 4;
    if (!Number.isFinite(v)) {
      throw new ProtocolError("invalid_float", `invalid float: ${v}`);
    }
    return v;
  }

  slice(len: number): Uint8Array {
    this.need(len);
    const out = this.buf.slice(this.pos, this.pos + len);
    this.pos += len;
    return out;
  }

  str(): string {
    const len = this.u32();
    if (len > MAX_STRING_LEN) {
      throw new ProtocolError(
        "invalid_data",
        `string length ${len} exceeds maximum ${MAX_STRING_LEN}`,
      );
    }
    this.need(len);
    const sub = this.buf.subarray(this.pos, this.pos + len);
    this.pos += len;
    try {
      return utf8Decoder.decode(sub);
    } catch (err) {
      throw new ProtocolError(
        "invalid_utf8",
        `invalid UTF-8: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }

  optTag(): boolean {
    const tag = this.u8();
    if (tag === 0) return false;
    if (tag === 1) return true;
    throw new ProtocolError("invalid_data", `invalid option tag: ${tag}`);
  }

  optStr(): string | null {
    return this.optTag() ? this.str() : null;
  }

  optF32(): number | null {
    return this.optTag() ? this.f32() : null;
  }
}

function checkRange(value: number, max: number, label: string): number {
  if (value > max) {
    throw new ProtocolError("invalid_data", `invalid ${label}: ${value}`);
  }
  return value;
}

function encodeColor(w: ByteWriter, c: Color): void {
  switch (c.kind) {
    case "named":
      w.u8(0);
      w.u8(c.color);
      break;
    case "rgb":
      w.u8(1);
      w.u8(c.r);
      w.u8(c.g);
      w.u8(c.b);
      break;
    case "indexed":
      w.u8(2);
      w.u8(c.index);
      break;
  }
}

function decodeColor(r: ByteReader): Color {
  const tag = r.u8();
  if (tag === 0) {
    return {
      kind: "named",
      color: checkRange(r.u8(), 17, "named color") as NamedColor,
    };
  }
  if (tag === 1) {
    return { kind: "rgb", r: r.u8(), g: r.u8(), b: r.u8() };
  }
  if (tag === 2) {
    return { kind: "indexed", index: r.u8() };
  }
  throw new ProtocolError("invalid_data", `invalid color type: ${tag}`);
}

function encodeOptColor(w: ByteWriter, c: Color | null): void {
  if (c === null) {
    w.u8(0);
  } else {
    w.u8(1);
    encodeColor(w, c);
  }
}

function decodeOptColor(r: ByteReader): Color | null {
  return r.optTag() ? decodeColor(r) : null;
}

function encodeBorder(w: ByteWriter, b: BorderStyle): void {
  w.u8(b.style);
  const bits =
    (b.edges.top ? 1 : 0) |
    (b.edges.right ? 2 : 0) |
    (b.edges.bottom ? 4 : 0) |
    (b.edges.left ? 8 : 0);
  w.u8(bits);
  w.u8(b.radius);
}

function decodeBorder(r: ByteReader): BorderStyle {
  const style = checkRange(r.u8(), 6, "border type") as BorderType;
  const bits = r.u8();
  const radius = r.u8();
  return {
    style,
    edges: {
      top: (bits & 1) !== 0,
      right: (bits & 2) !== 0,
      bottom: (bits & 4) !== 0,
      left: (bits & 8) !== 0,
    },
    radius,
  };
}

function encodeTransition(w: ByteWriter, t: Transition): void {
  w.u32(t.durationMs);
  w.u8(t.easing);
  w.u32(t.properties);
}

function decodeTransition(r: ByteReader): Transition {
  const durationMs = r.u32();
  const easing = checkRange(r.u8(), 3, "easing") as Easing;
  const properties = r.u32();
  return { durationMs, easing, properties };
}

function encodeStyle(w: ByteWriter, s: Style): void {
  encodeOptColor(w, s.fg);
  encodeOptColor(w, s.bg);
  encodeBorder(w, s.border);
  encodeOptColor(w, s.borderColor);
  const flags =
    (s.bold ? 1 : 0) |
    (s.italic ? 2 : 0) |
    (s.underline ? 4 : 0) |
    (s.dim ? 8 : 0) |
    (s.transition !== null ? 16 : 0);
  w.u8(flags);
  w.f32(s.opacity);
  if (s.transition !== null) {
    encodeTransition(w, s.transition);
  }
}

function decodeStyle(r: ByteReader): Style {
  const fg = decodeOptColor(r);
  const bg = decodeOptColor(r);
  const border = decodeBorder(r);
  const borderColor = decodeOptColor(r);
  const flags = r.u8();
  const opacity = r.f32();
  const transition = (flags & 16) !== 0 ? decodeTransition(r) : null;
  return {
    fg,
    bg,
    border,
    borderColor,
    bold: (flags & 1) !== 0,
    italic: (flags & 2) !== 0,
    underline: (flags & 4) !== 0,
    dim: (flags & 8) !== 0,
    opacity,
    transition,
  };
}

function encodeDimension(w: ByteWriter, d: Dimension): void {
  switch (d.kind) {
    case "auto":
      w.u8(0);
      break;
    case "cells":
      w.u8(1);
      w.f32(d.value);
      break;
    case "percent":
      w.u8(2);
      w.f32(d.value);
      break;
    case "fr":
      w.u8(3);
      w.f32(d.value);
      break;
    case "vw":
      w.u8(4);
      w.f32(d.value);
      break;
    case "vh":
      w.u8(5);
      w.f32(d.value);
      break;
    case "min_content":
      w.u8(6);
      break;
    case "max_content":
      w.u8(7);
      break;
  }
}

function decodeDimension(r: ByteReader): Dimension {
  switch (r.u8()) {
    case 0:
      return { kind: "auto" };
    case 1:
      return { kind: "cells", value: r.f32() };
    case 2:
      return { kind: "percent", value: r.f32() };
    case 3:
      return { kind: "fr", value: r.f32() };
    case 4:
      return { kind: "vw", value: r.f32() };
    case 5:
      return { kind: "vh", value: r.f32() };
    case 6:
      return { kind: "min_content" };
    case 7:
      return { kind: "max_content" };
    default:
      throw new ProtocolError("invalid_data", "invalid dimension type");
  }
}

function encodeEdgesDim(w: ByteWriter, e: Edges<Dimension>): void {
  encodeDimension(w, e.top);
  encodeDimension(w, e.right);
  encodeDimension(w, e.bottom);
  encodeDimension(w, e.left);
}

function decodeEdgesDim(r: ByteReader): Edges<Dimension> {
  return {
    top: decodeDimension(r),
    right: decodeDimension(r),
    bottom: decodeDimension(r),
    left: decodeDimension(r),
  };
}

function encodeSizeDim(w: ByteWriter, s: Size<Dimension>): void {
  encodeDimension(w, s.width);
  encodeDimension(w, s.height);
}

function decodeSizeDim(r: ByteReader): Size<Dimension> {
  return { width: decodeDimension(r), height: decodeDimension(r) };
}

function encodeOverflow(w: ByteWriter, o: Overflow): void {
  w.u8(o.x);
  w.u8(o.y);
}

function decodeOverflow(r: ByteReader): Overflow {
  return {
    x: checkRange(r.u8(), 3, "overflow axis") as OverflowAxis,
    y: checkRange(r.u8(), 3, "overflow axis") as OverflowAxis,
  };
}

function encodeGridLine(w: ByteWriter, g: GridLine): void {
  switch (g.kind) {
    case "auto":
      w.u8(0);
      break;
    case "line":
      w.u8(1);
      w.i16(g.value);
      break;
    case "span":
      w.u8(2);
      w.u16(g.value);
      break;
  }
}

function decodeGridLine(r: ByteReader): GridLine {
  switch (r.u8()) {
    case 0:
      return { kind: "auto" };
    case 1:
      return { kind: "line", value: r.i16() };
    case 2:
      return { kind: "span", value: r.u16() };
    default:
      throw new ProtocolError("invalid_data", "invalid grid line type");
  }
}

function encodeGridPlacement(w: ByteWriter, p: GridPlacement): void {
  encodeGridLine(w, p.start);
  encodeGridLine(w, p.end);
}

function decodeGridPlacement(r: ByteReader): GridPlacement {
  return { start: decodeGridLine(r), end: decodeGridLine(r) };
}

function encodeTrack(w: ByteWriter, t: TrackSizingFunction): void {
  switch (t.kind) {
    case "single":
      w.u8(0);
      encodeDimension(w, t.dim);
      break;
    case "minmax":
      w.u8(1);
      encodeDimension(w, t.min);
      encodeDimension(w, t.max);
      break;
    case "repeat":
      w.u8(2);
      w.u16(t.count);
      w.u32(t.dims.length);
      for (const d of t.dims) encodeDimension(w, d);
      break;
  }
}

function decodeTrack(r: ByteReader): TrackSizingFunction {
  switch (r.u8()) {
    case 0:
      return { kind: "single", dim: decodeDimension(r) };
    case 1:
      return {
        kind: "minmax",
        min: decodeDimension(r),
        max: decodeDimension(r),
      };
    case 2: {
      const count = r.u16();
      const len = r.u32();
      if (len > MAX_TRACK_COUNT) {
        throw new ProtocolError("invalid_data", `track count ${len} too large`);
      }
      const dims: Dimension[] = [];
      for (let i = 0; i < len; i++) dims.push(decodeDimension(r));
      return { kind: "repeat", count, dims };
    }
    default:
      throw new ProtocolError("invalid_data", "invalid track sizing function");
  }
}

function encodeTracks(w: ByteWriter, tracks: TrackSizingFunction[]): void {
  w.u32(tracks.length);
  for (const t of tracks) encodeTrack(w, t);
}

function decodeTracks(r: ByteReader): TrackSizingFunction[] {
  const len = r.u32();
  if (len > MAX_TRACK_COUNT) {
    throw new ProtocolError("invalid_data", `track count ${len} too large`);
  }
  const out: TrackSizingFunction[] = [];
  for (let i = 0; i < len; i++) out.push(decodeTrack(r));
  return out;
}

function encodeConstraints(w: ByteWriter, c: Constraints): void {
  w.u8(c.display);
  w.u8(c.flexDirection);
  w.u8(c.flexWrap);
  w.u8(c.justifyContent);
  w.u8(c.alignItems);
  w.u8(c.alignSelf);
  w.u8(c.alignContent);
  w.f32(c.flexGrow);
  w.f32(c.flexShrink);
  encodeDimension(w, c.flexBasis);
  encodeDimension(w, c.width);
  encodeDimension(w, c.height);
  encodeDimension(w, c.minWidth);
  encodeDimension(w, c.minHeight);
  encodeDimension(w, c.maxWidth);
  encodeDimension(w, c.maxHeight);
  w.optF32(c.aspectRatio);
  encodeEdgesDim(w, c.margin);
  encodeEdgesDim(w, c.padding);
  encodeSizeDim(w, c.gap);
  encodeOverflow(w, c.overflow);
  encodeTracks(w, c.gridTemplateColumns);
  encodeTracks(w, c.gridTemplateRows);
  encodeGridPlacement(w, c.gridColumn);
  encodeGridPlacement(w, c.gridRow);
}

function decodeConstraints(r: ByteReader): Constraints {
  return {
    display: checkRange(r.u8(), 3, "display") as Display,
    flexDirection: checkRange(r.u8(), 3, "flex direction") as FlexDirection,
    flexWrap: checkRange(r.u8(), 2, "flex wrap") as FlexWrap,
    justifyContent: checkRange(r.u8(), 5, "justify content") as JustifyContent,
    alignItems: checkRange(r.u8(), 4, "align items") as AlignItems,
    alignSelf: checkRange(r.u8(), 5, "align self") as AlignSelf,
    alignContent: checkRange(r.u8(), 6, "align content") as AlignContent,
    flexGrow: r.f32(),
    flexShrink: r.f32(),
    flexBasis: decodeDimension(r),
    width: decodeDimension(r),
    height: decodeDimension(r),
    minWidth: decodeDimension(r),
    minHeight: decodeDimension(r),
    maxWidth: decodeDimension(r),
    maxHeight: decodeDimension(r),
    aspectRatio: r.optF32(),
    margin: decodeEdgesDim(r),
    padding: decodeEdgesDim(r),
    gap: decodeSizeDim(r),
    overflow: decodeOverflow(r),
    gridTemplateColumns: decodeTracks(r),
    gridTemplateRows: decodeTracks(r),
    gridColumn: decodeGridPlacement(r),
    gridRow: decodeGridPlacement(r),
  };
}

function encodeTextContent(w: ByteWriter, t: TextContent): void {
  w.str(t.text);
  w.u8(t.wrap);
  w.u8(t.align);
  w.u8(t.overflow);
}

function decodeTextContent(r: ByteReader): TextContent {
  return {
    text: r.str(),
    wrap: checkRange(r.u8(), 2, "text wrap") as TextWrap,
    align: checkRange(r.u8(), 2, "text align") as TextAlign,
    overflow: checkRange(r.u8(), 2, "text overflow") as TextOverflow,
  };
}

function encodeRichText(w: ByteWriter, rt: RichText): void {
  w.u32(rt.spans.length);
  for (const s of rt.spans) {
    w.str(s.text);
    encodeStyle(w, s.style);
  }
}

function decodeRichText(r: ByteReader): RichText {
  const len = r.u32();
  if (len > MAX_SPAN_COUNT) {
    throw new ProtocolError("invalid_data", `span count ${len} too large`);
  }
  const spans: Span[] = [];
  for (let i = 0; i < len; i++) {
    spans.push({ text: r.str(), style: decodeStyle(r) });
  }
  return { spans };
}

function encodeWidget(w: ByteWriter, widget: WidgetContent): void {
  switch (widget.kind) {
    case "input":
      w.u8(widget.cursor === null ? 0 : 7);
      w.str(widget.value);
      w.str(widget.placeholder);
      w.u8((widget.multiline ? 1 : 0) | (widget.password ? 2 : 0));
      if (widget.cursor !== null) w.u32(widget.cursor);
      break;
    case "markdown":
      w.u8(1);
      w.str(widget.source);
      break;
    case "code":
      w.u8(2);
      w.str(widget.source);
      w.str(widget.language);
      w.u8(widget.lineNumbers ? 1 : 0);
      break;
    case "progress":
      w.u8(3);
      w.f32(widget.value);
      w.optStr(widget.label);
      w.u8(widget.animated ? 1 : 0);
      w.u8(widget.indeterminate ? 1 : 0);
      break;
    case "spinner":
      w.u8(4);
      w.optStr(widget.label);
      w.u8(widget.style);
      break;
    case "image":
      w.u8(5);
      w.u32(widget.width);
      w.u32(widget.height);
      w.u8(widget.scale);
      w.u32(widget.data.length);
      w.bytes(widget.data);
      break;
    case "button":
      w.u8(6);
      w.str(widget.label);
      w.u8(widget.style);
      break;
  }
}

function decodeWidget(r: ByteReader): WidgetContent {
  const tag = r.u8();
  switch (tag) {
    case 0:
    case 7: {
      const value = r.str();
      const placeholder = r.str();
      const flags = r.u8();
      const cursor = tag === 7 ? r.u32() : null;
      return {
        kind: "input",
        value,
        placeholder,
        multiline: (flags & 1) !== 0,
        password: (flags & 2) !== 0,
        cursor,
      };
    }
    case 1:
      return { kind: "markdown", source: r.str() };
    case 2:
      return {
        kind: "code",
        source: r.str(),
        language: r.str(),
        lineNumbers: r.u8() !== 0,
      };
    case 3: {
      const value = r.f32();
      const label = r.optStr();
      const animated = r.remaining() >= 1 ? r.u8() !== 0 : false;
      const indeterminate = r.remaining() >= 1 ? r.u8() !== 0 : false;
      return { kind: "progress", value, label, animated, indeterminate };
    }
    case 4: {
      const label = r.optStr();
      const styleRaw = r.remaining() >= 1 ? r.u8() : 0;
      const style = (styleRaw <= 3 ? styleRaw : 0) as SpinnerStyle;
      return { kind: "spinner", label, style };
    }
    case 5: {
      const width = r.u32();
      const height = r.u32();
      if (width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION) {
        throw new ProtocolError("invalid_data", "image dimensions too large");
      }
      const scale = checkRange(r.u8(), 3, "image scale") as ImageScale;
      const dataLen = r.u32();
      if (dataLen > MAX_PAYLOAD_LEN || dataLen < width * height * 4) {
        throw new ProtocolError("invalid_data", "invalid image data length");
      }
      return { kind: "image", width, height, scale, data: r.slice(dataLen) };
    }
    case 6:
      return {
        kind: "button",
        label: r.str(),
        style: checkRange(r.u8(), 3, "button style") as ButtonStyle,
      };
    default:
      throw new ProtocolError("invalid_data", `invalid widget type: ${tag}`);
  }
}

function encodeContent(w: ByteWriter, c: Content): void {
  switch (c.kind) {
    case "none":
      w.u8(0);
      break;
    case "text":
      w.u8(1);
      encodeTextContent(w, c.content);
      break;
    case "rich_text":
      w.u8(2);
      encodeRichText(w, c.content);
      break;
    case "widget":
      w.u8(3);
      encodeWidget(w, c.widget);
      break;
  }
}

function decodeContent(r: ByteReader): Content {
  switch (r.u8()) {
    case 0:
      return { kind: "none" };
    case 1:
      return { kind: "text", content: decodeTextContent(r) };
    case 2:
      return { kind: "rich_text", content: decodeRichText(r) };
    case 3:
      return { kind: "widget", widget: decodeWidget(r) };
    default:
      throw new ProtocolError("invalid_data", "invalid content type");
  }
}

function encodeSemantics(w: ByteWriter, s: Semantics): void {
  w.u8(s.role);
  w.optStr(s.label);
  w.optStr(s.description);
  w.u8(s.live);
  const flags =
    (s.busy ? 1 : 0) |
    (s.hidden ? 2 : 0) |
    (s.readOnly ? 4 : 0) |
    (s.disabled ? 8 : 0) |
    (s.atomic ? 16 : 0) |
    (s.modal ? 32 : 0) |
    (s.selected ? 64 : 0);
  w.u8(flags);
  w.u8(s.toggled);
  w.u8(s.expanded);
  if (s.value !== null) {
    w.u8(1);
    w.f32(s.value.now);
    w.f32(s.value.min);
    w.f32(s.value.max);
  } else {
    w.u8(0);
  }
  w.optStr(s.valueText);
  if (s.position !== null) {
    w.u8(1);
    w.u32(s.position.index);
    w.u32(s.position.size);
  } else {
    w.u8(0);
  }
  w.u8(s.level);
  for (const id of [s.labelledBy, s.describedBy, s.controls]) {
    if (id !== null) {
      w.u8(1);
      w.u32(id);
    } else {
      w.u8(0);
    }
  }
}

function decodeSemantics(r: ByteReader): Semantics {
  let totalBytes = 0;
  const boundedStr = (): string | null => {
    const s = r.optStr();
    if (s !== null) {
      const len = utf8Encoder.encode(s).length;
      if (len > MAX_SEMANTICS_STRING_LEN) {
        throw new ProtocolError("invalid_data", "semantics string too long");
      }
      totalBytes += len;
      if (totalBytes > MAX_SEMANTICS_TOTAL_BYTES) {
        throw new ProtocolError("invalid_data", "semantics total too large");
      }
    }
    return s;
  };
  const role = checkRange(r.u8(), 26, "semantics role") as Role;
  const label = boundedStr();
  const description = boundedStr();
  const live = checkRange(r.u8(), 2, "semantics live") as Live;
  const flags = r.u8();
  if ((flags & ~0x7f) !== 0) {
    throw new ProtocolError("invalid_data", `invalid semantics flags: ${flags}`);
  }
  const toggled = checkRange(r.u8(), 3, "semantics toggled") as Toggled;
  const expanded = checkRange(r.u8(), 2, "semantics expanded") as Expanded;
  const value = r.optTag()
    ? { now: r.f32(), min: r.f32(), max: r.f32() }
    : null;
  const valueText = boundedStr();
  const position = r.optTag() ? { index: r.u32(), size: r.u32() } : null;
  const level = r.u8();
  const labelledBy = r.optTag() ? r.u32() : null;
  const describedBy = r.optTag() ? r.u32() : null;
  const controls = r.optTag() ? r.u32() : null;
  return {
    role,
    label,
    description,
    live,
    busy: (flags & 1) !== 0,
    hidden: (flags & 2) !== 0,
    readOnly: (flags & 4) !== 0,
    disabled: (flags & 8) !== 0,
    atomic: (flags & 16) !== 0,
    modal: (flags & 32) !== 0,
    selected: (flags & 64) !== 0,
    toggled,
    expanded,
    value,
    valueText,
    position,
    level,
    labelledBy,
    describedBy,
    controls,
  };
}

function encodeKey(w: ByteWriter, key: Key): void {
  switch (key.kind) {
    case "char":
      w.u8(0);
      w.u32(key.char.codePointAt(0) ?? 0);
      break;
    case "function":
      w.u8(1);
      w.u8(key.number);
      break;
    case "escape":
      w.u8(2);
      break;
    case "enter":
      w.u8(3);
      break;
    case "tab":
      w.u8(4);
      break;
    case "backspace":
      w.u8(5);
      break;
    case "delete":
      w.u8(6);
      break;
    case "insert":
      w.u8(7);
      break;
    case "home":
      w.u8(8);
      break;
    case "end":
      w.u8(9);
      break;
    case "page_up":
      w.u8(10);
      break;
    case "page_down":
      w.u8(11);
      break;
    case "up":
      w.u8(12);
      break;
    case "down":
      w.u8(13);
      break;
    case "left":
      w.u8(14);
      break;
    case "right":
      w.u8(15);
      break;
  }
}

function decodeKey(r: ByteReader): Key {
  switch (r.u8()) {
    case 0: {
      const cp = r.u32();
      if (cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) {
        throw new ProtocolError("invalid_data", `invalid char codepoint: ${cp}`);
      }
      return { kind: "char", char: String.fromCodePoint(cp) };
    }
    case 1:
      return { kind: "function", number: r.u8() };
    case 2:
      return { kind: "escape" };
    case 3:
      return { kind: "enter" };
    case 4:
      return { kind: "tab" };
    case 5:
      return { kind: "backspace" };
    case 6:
      return { kind: "delete" };
    case 7:
      return { kind: "insert" };
    case 8:
      return { kind: "home" };
    case 9:
      return { kind: "end" };
    case 10:
      return { kind: "page_up" };
    case 11:
      return { kind: "page_down" };
    case 12:
      return { kind: "up" };
    case 13:
      return { kind: "down" };
    case 14:
      return { kind: "left" };
    case 15:
      return { kind: "right" };
    default:
      throw new ProtocolError("invalid_data", "invalid key variant");
  }
}

function encodeInputType(w: ByteWriter, t: InputType): void {
  switch (t.kind) {
    case "mouse_down":
      w.u8(0);
      w.u8(t.button);
      break;
    case "mouse_up":
      w.u8(1);
      w.u8(t.button);
      break;
    case "mouse_move":
      w.u8(2);
      break;
    case "mouse_scroll":
      w.u8(3);
      w.f32(t.deltaX);
      w.f32(t.deltaY);
      break;
    case "key_down":
      w.u8(4);
      encodeKey(w, t.key);
      w.u8(t.modifiers);
      break;
    case "key_up":
      w.u8(5);
      encodeKey(w, t.key);
      w.u8(t.modifiers);
      break;
    case "text_input":
      w.u8(6);
      w.str(t.text);
      break;
  }
}

function decodeInputType(r: ByteReader): InputType {
  switch (r.u8()) {
    case 0:
      return {
        kind: "mouse_down",
        button: checkRange(r.u8(), 4, "mouse button") as MouseButton,
      };
    case 1:
      return {
        kind: "mouse_up",
        button: checkRange(r.u8(), 4, "mouse button") as MouseButton,
      };
    case 2:
      return { kind: "mouse_move" };
    case 3:
      return { kind: "mouse_scroll", deltaX: r.f32(), deltaY: r.f32() };
    case 4:
      return { kind: "key_down", key: decodeKey(r), modifiers: r.u8() };
    case 5:
      return { kind: "key_up", key: decodeKey(r), modifiers: r.u8() };
    case 6:
      return { kind: "text_input", text: r.str() };
    default:
      throw new ProtocolError("invalid_data", "invalid input type");
  }
}

function encodeInputEvent(w: ByteWriter, e: InputEvent): void {
  w.u32(e.regionId);
  encodeInputType(w, e.eventType);
  w.f32(e.x);
  w.f32(e.y);
  w.u8(e.modifiers);
}

function decodeInputEvent(r: ByteReader): InputEvent {
  const regionId = r.u32();
  const eventType = decodeInputType(r);
  const x = r.f32();
  const y = r.f32();
  const modifiers = r.remaining() > 0 ? r.u8() : MOD_NONE;
  return { regionId, eventType, x, y, modifiers };
}

function decodeHeader(r: ByteReader): { typeCode: number; payloadLen: number } {
  if (r.remaining() < FRAME_HEADER_LEN) {
    throw new ProtocolError(
      "unexpected_eof",
      `short frame header: need ${FRAME_HEADER_LEN}, have ${r.remaining()}`,
    );
  }
  const magic = r.u16();
  if (magic !== FRAME_MAGIC_LE) {
    throw new ProtocolError(
      "invalid_magic",
      `invalid magic: 0x${magic.toString(16)}`,
    );
  }
  const typeCode = r.u16();
  const payloadLen = r.u32();
  if (payloadLen > MAX_PAYLOAD_LEN) {
    throw new ProtocolError(
      "invalid_data",
      `payload length ${payloadLen} exceeds maximum ${MAX_PAYLOAD_LEN}`,
    );
  }
  if (r.remaining() < payloadLen) {
    throw new ProtocolError(
      "unexpected_eof",
      `truncated frame payload: need ${payloadLen}, have ${r.remaining()}`,
    );
  }
  return { typeCode, payloadLen };
}

export function encodeAppMessage(msg: AppMessage): Uint8Array {
  const w = new ByteWriter();
  switch (msg.type) {
    case "client_hello":
      w.u16(msg.version);
      w.u32(msg.requestedCapabilities);
      return w.toFrame(MsgType.CLIENT_HELLO);
    case "create_region":
      w.u32(msg.id);
      w.u32(msg.parent);
      w.i32(msg.insertIndex);
      encodeConstraints(w, msg.constraints);
      encodeStyle(w, msg.style);
      return w.toFrame(MsgType.CREATE_REGION);
    case "set_constraints":
      w.u32(msg.id);
      encodeConstraints(w, msg.constraints);
      return w.toFrame(MsgType.SET_CONSTRAINTS);
    case "set_style":
      w.u32(msg.id);
      encodeStyle(w, msg.style);
      return w.toFrame(MsgType.SET_STYLE);
    case "set_content":
      w.u32(msg.id);
      encodeContent(w, msg.content);
      return w.toFrame(MsgType.SET_CONTENT);
    case "append_content":
      w.u32(msg.id);
      w.str(msg.text);
      return w.toFrame(MsgType.APPEND_CONTENT);
    case "stream_start":
      w.u32(msg.id);
      w.u32(msg.streamId);
      return w.toFrame(MsgType.STREAM_START);
    case "stream_end":
      w.u32(msg.id);
      w.u32(msg.streamId);
      return w.toFrame(MsgType.STREAM_END);
    case "set_region_metadata":
      w.u32(msg.id);
      w.u32(msg.metadata.length);
      for (const [k, v] of msg.metadata) {
        w.str(k);
        w.str(v);
      }
      return w.toFrame(MsgType.SET_REGION_METADATA);
    case "set_semantics":
      w.u32(msg.id);
      encodeSemantics(w, msg.semantics);
      return w.toFrame(MsgType.SET_SEMANTICS);
    case "delete_region":
      w.u32(msg.id);
      return w.toFrame(MsgType.DELETE_REGION);
    case "create_surface":
      w.u32(msg.id);
      w.u16(msg.rows);
      encodeConstraints(w, msg.constraints);
      encodeStyle(w, msg.style);
      return w.toFrame(MsgType.CREATE_SURFACE);
    case "close_surface":
      w.u32(msg.id);
      return w.toFrame(MsgType.CLOSE_SURFACE);
    case "reparent_region":
      w.u32(msg.id);
      w.u32(msg.newParent);
      w.i32(msg.insertIndex);
      return w.toFrame(MsgType.REPARENT_REGION);
    case "begin_frame":
      w.u32(msg.frameId);
      return w.toFrame(MsgType.BEGIN_FRAME);
    case "end_frame":
      w.u32(msg.frameId);
      return w.toFrame(MsgType.END_FRAME);
    case "scroll":
      w.u32(msg.id);
      w.f32(msg.x);
      w.f32(msg.y);
      w.u8(msg.animate ? 1 : 0);
      return w.toFrame(MsgType.SCROLL);
    case "focus":
      w.u32(msg.id);
      return w.toFrame(MsgType.FOCUS);
    case "blur":
      return w.toFrame(MsgType.BLUR);
    case "subscribe":
      w.u32(msg.id);
      w.u32(msg.events);
      return w.toFrame(MsgType.SUBSCRIBE);
    case "unsubscribe":
      w.u32(msg.id);
      w.u32(msg.events);
      return w.toFrame(MsgType.UNSUBSCRIBE);
    case "clipboard_get":
      return w.toFrame(MsgType.CLIPBOARD_GET);
    case "clipboard_set":
      w.str(msg.text);
      return w.toFrame(MsgType.CLIPBOARD_SET);
    case "disconnect":
      return w.toFrame(MsgType.DISCONNECT);
    case "pong":
      return w.toFrame(MsgType.PONG);
    case "override_result":
      w.u32(msg.requestId);
      w.optStr(msg.error);
      return w.toFrame(MsgType.OVERRIDE_RESULT);
    case "action_result":
      w.u32(msg.requestId);
      w.optStr(msg.error);
      return w.toFrame(MsgType.ACTION_RESULT);
  }
}

export function decodeAppMessage(data: Uint8Array): {
  message: AppMessage;
  bytesRead: number;
} {
  const outer = new ByteReader(data);
  const { typeCode, payloadLen } = decodeHeader(outer);
  const payload = data.subarray(FRAME_HEADER_LEN, FRAME_HEADER_LEN + payloadLen);
  const r = new ByteReader(payload);
  let message: AppMessage;

  switch (typeCode) {
    case MsgType.CLIENT_HELLO:
      message = {
        type: "client_hello",
        version: r.u16(),
        requestedCapabilities: r.u32(),
      };
      break;
    case MsgType.CREATE_REGION:
      message = {
        type: "create_region",
        id: r.u32(),
        parent: r.u32(),
        insertIndex: r.i32(),
        constraints: decodeConstraints(r),
        style: decodeStyle(r),
      };
      break;
    case MsgType.SET_CONSTRAINTS:
      message = {
        type: "set_constraints",
        id: r.u32(),
        constraints: decodeConstraints(r),
      };
      break;
    case MsgType.SET_STYLE:
      message = { type: "set_style", id: r.u32(), style: decodeStyle(r) };
      break;
    case MsgType.SET_CONTENT:
      message = { type: "set_content", id: r.u32(), content: decodeContent(r) };
      break;
    case MsgType.APPEND_CONTENT:
      message = { type: "append_content", id: r.u32(), text: r.str() };
      break;
    case MsgType.STREAM_START:
      message = { type: "stream_start", id: r.u32(), streamId: r.u32() };
      break;
    case MsgType.STREAM_END:
      message = { type: "stream_end", id: r.u32(), streamId: r.u32() };
      break;
    case MsgType.SET_REGION_METADATA: {
      const id = r.u32();
      const count = r.u32();
      if (count > MAX_METADATA_PAIRS) {
        throw new ProtocolError("invalid_data", "metadata pair count too large");
      }
      const metadata: Array<[string, string]> = [];
      let total = 0;
      for (let i = 0; i < count; i++) {
        const k = r.str();
        const v = r.str();
        total += utf8Encoder.encode(k).length + utf8Encoder.encode(v).length;
        if (total > MAX_METADATA_TOTAL_BYTES) {
          throw new ProtocolError("invalid_data", "metadata size too large");
        }
        metadata.push([k, v]);
      }
      message = { type: "set_region_metadata", id, metadata };
      break;
    }
    case MsgType.SET_SEMANTICS:
      message = {
        type: "set_semantics",
        id: r.u32(),
        semantics: decodeSemantics(r),
      };
      break;
    case MsgType.DELETE_REGION:
      message = { type: "delete_region", id: r.u32() };
      break;
    case MsgType.CREATE_SURFACE: {
      const id = r.u32();
      if (id === ROOT_REGION) {
        throw new ProtocolError("invalid_data", "surface cannot use root id");
      }
      const rows = r.u16();
      if (rows < MIN_SURFACE_ROWS || rows > MAX_SURFACE_ROWS) {
        throw new ProtocolError("invalid_data", `invalid surface rows: ${rows}`);
      }
      message = {
        type: "create_surface",
        id,
        rows,
        constraints: decodeConstraints(r),
        style: decodeStyle(r),
      };
      break;
    }
    case MsgType.CLOSE_SURFACE: {
      const id = r.u32();
      if (id === ROOT_REGION) {
        throw new ProtocolError("invalid_data", "surface cannot use root id");
      }
      message = { type: "close_surface", id };
      break;
    }
    case MsgType.REPARENT_REGION:
      message = {
        type: "reparent_region",
        id: r.u32(),
        newParent: r.u32(),
        insertIndex: r.i32(),
      };
      break;
    case MsgType.BEGIN_FRAME:
      message = { type: "begin_frame", frameId: r.u32() };
      break;
    case MsgType.END_FRAME:
      message = { type: "end_frame", frameId: r.u32() };
      break;
    case MsgType.SCROLL:
      message = {
        type: "scroll",
        id: r.u32(),
        x: r.f32(),
        y: r.f32(),
        animate: r.u8() !== 0,
      };
      break;
    case MsgType.FOCUS:
      message = { type: "focus", id: r.u32() };
      break;
    case MsgType.BLUR:
      message = { type: "blur" };
      break;
    case MsgType.SUBSCRIBE:
      message = { type: "subscribe", id: r.u32(), events: r.u32() };
      break;
    case MsgType.UNSUBSCRIBE:
      message = { type: "unsubscribe", id: r.u32(), events: r.u32() };
      break;
    case MsgType.CLIPBOARD_GET:
      message = { type: "clipboard_get" };
      break;
    case MsgType.CLIPBOARD_SET:
      message = { type: "clipboard_set", text: r.str() };
      break;
    case MsgType.DISCONNECT:
      message = { type: "disconnect" };
      break;
    case MsgType.PONG:
      message = { type: "pong" };
      break;
    case MsgType.OVERRIDE_RESULT:
      message = {
        type: "override_result",
        requestId: r.u32(),
        error: r.optStr(),
      };
      break;
    case MsgType.ACTION_RESULT:
      message = {
        type: "action_result",
        requestId: r.u32(),
        error: r.optStr(),
      };
      break;
    default:
      throw new ProtocolError(
        "unknown_message_type",
        `unknown app message type: 0x${typeCode.toString(16)}`,
      );
  }

  return { message, bytesRead: FRAME_HEADER_LEN + payloadLen };
}

export function encodeTerminalMessage(msg: TerminalMessage): Uint8Array {
  const w = new ByteWriter();
  switch (msg.type) {
    case "server_hello":
      w.u16(msg.version);
      w.u32(msg.capabilities);
      return w.toFrame(MsgType.SERVER_HELLO);
    case "session_ready":
      w.u64(msg.sessionId);
      w.u32(msg.rootRegion);
      w.u16(msg.width);
      w.u16(msg.height);
      return w.toFrame(MsgType.SESSION_READY);
    case "layout_computed":
      if (msg.frameId !== null) {
        w.u8(1);
        w.u32(msg.frameId);
      } else {
        w.u8(0);
      }
      w.u8(msg.trigger);
      w.u32(msg.regions.length);
      for (const r of msg.regions) {
        w.u32(r.id);
        w.f32(r.x);
        w.f32(r.y);
        w.f32(r.width);
        w.f32(r.height);
        w.f32(r.contentX);
        w.f32(r.contentY);
        w.f32(r.contentWidth);
        w.f32(r.contentHeight);
        w.u8(r.visible ? 1 : 0);
      }
      return w.toFrame(MsgType.LAYOUT_COMPUTED);
    case "input":
      encodeInputEvent(w, msg.event);
      return w.toFrame(MsgType.INPUT);
    case "resize":
      w.u16(msg.event.width);
      w.u16(msg.event.height);
      return w.toFrame(MsgType.RESIZE);
    case "focus":
      w.u32(msg.event.regionId);
      w.u8(msg.event.focused ? 1 : 0);
      return w.toFrame(MsgType.FOCUS_EVENT);
    case "scroll":
      w.u32(msg.event.regionId);
      w.f32(msg.event.scrollX);
      w.f32(msg.event.scrollY);
      return w.toFrame(MsgType.SCROLL_EVENT);
    case "clipboard_content":
      w.optStr(msg.text);
      return w.toFrame(MsgType.CLIPBOARD_CONTENT);
    case "surface_event":
      w.u32(msg.id);
      w.u8(msg.kind);
      return w.toFrame(MsgType.SURFACE_EVENT);
    case "ping":
      return w.toFrame(MsgType.PING);
    case "set_override":
      w.u32(msg.requestId);
      if (msg.pushed !== null) {
        w.u8(1);
        w.u8(msg.pushed.format);
        w.str(msg.pushed.source);
      } else {
        w.u8(0);
      }
      return w.toFrame(MsgType.SET_OVERRIDE);
    case "invoke_action":
      w.u32(msg.requestId);
      w.u32(msg.frameId);
      w.u32(msg.id);
      w.str(msg.widgetId);
      w.u8(msg.action);
      return w.toFrame(MsgType.INVOKE_ACTION);
    case "error":
      w.u16(msg.code);
      w.str(msg.message);
      return w.toFrame(MsgType.ERROR);
  }
}

export function decodeTerminalMessage(data: Uint8Array): {
  message: TerminalMessage;
  bytesRead: number;
} {
  const outer = new ByteReader(data);
  const { typeCode, payloadLen } = decodeHeader(outer);
  const payload = data.subarray(FRAME_HEADER_LEN, FRAME_HEADER_LEN + payloadLen);
  const r = new ByteReader(payload);
  let message: TerminalMessage;

  switch (typeCode) {
    case MsgType.SERVER_HELLO:
      message = {
        type: "server_hello",
        version: r.u16(),
        capabilities: r.u32(),
      };
      break;
    case MsgType.SESSION_READY:
      message = {
        type: "session_ready",
        sessionId: r.u64(),
        rootRegion: r.u32(),
        width: r.u16(),
        height: r.u16(),
      };
      break;
    case MsgType.LAYOUT_COMPUTED: {
      const frameId = r.optTag() ? r.u32() : null;
      const trigger = checkRange(r.u8(), 3, "layout trigger") as LayoutTrigger;
      const count = r.u32();
      if (count > MAX_REGION_COUNT) {
        throw new ProtocolError("invalid_data", "region count too large");
      }
      const regions: ComputedRegionLayout[] = [];
      for (let i = 0; i < count; i++) {
        regions.push({
          id: r.u32(),
          x: r.f32(),
          y: r.f32(),
          width: r.f32(),
          height: r.f32(),
          contentX: r.f32(),
          contentY: r.f32(),
          contentWidth: r.f32(),
          contentHeight: r.f32(),
          visible: r.u8() !== 0,
        });
      }
      message = { type: "layout_computed", frameId, trigger, regions };
      break;
    }
    case MsgType.INPUT:
      message = { type: "input", event: decodeInputEvent(r) };
      break;
    case MsgType.RESIZE:
      message = {
        type: "resize",
        event: { width: r.u16(), height: r.u16() },
      };
      break;
    case MsgType.FOCUS_EVENT:
      message = {
        type: "focus",
        event: { regionId: r.u32(), focused: r.u8() !== 0 },
      };
      break;
    case MsgType.SCROLL_EVENT:
      message = {
        type: "scroll",
        event: { regionId: r.u32(), scrollX: r.f32(), scrollY: r.f32() },
      };
      break;
    case MsgType.CLIPBOARD_CONTENT:
      message = { type: "clipboard_content", text: r.optStr() };
      break;
    case MsgType.SURFACE_EVENT: {
      const id = r.u32();
      if (id === ROOT_REGION) {
        throw new ProtocolError("invalid_data", "surface cannot use root id");
      }
      const kind = checkRange(
        r.u8(),
        4,
        "surface event kind",
      ) as SurfaceEventKind;
      message = { type: "surface_event", id, kind };
      break;
    }
    case MsgType.PING:
      message = { type: "ping" };
      break;
    case MsgType.SET_OVERRIDE: {
      const requestId = r.u32();
      const pushed = r.optTag() ? { format: r.u8(), source: r.str() } : null;
      message = { type: "set_override", requestId, pushed };
      break;
    }
    case MsgType.INVOKE_ACTION:
      message = {
        type: "invoke_action",
        requestId: r.u32(),
        frameId: r.u32(),
        id: r.u32(),
        widgetId: r.str(),
        action: r.u8(),
      };
      break;
    case MsgType.ERROR:
      message = { type: "error", code: r.u16(), message: r.str() };
      break;
    default:
      throw new ProtocolError(
        "unknown_message_type",
        `unknown terminal message type: 0x${typeCode.toString(16)}`,
      );
  }

  return { message, bytesRead: FRAME_HEADER_LEN + payloadLen };
}

/**
 * Peek whether a complete frame is available in `buf`.
 * Returns total frame byte length if complete, or null if more bytes are needed.
 */
export function peekFrameLength(buf: Uint8Array): number | null {
  if (buf.length < FRAME_HEADER_LEN) return null;
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const magic = view.getUint16(0, true);
  if (magic !== FRAME_MAGIC_LE) {
    throw new ProtocolError(
      "invalid_magic",
      `invalid magic: 0x${magic.toString(16)}`,
    );
  }
  const payloadLen = view.getUint32(4, true);
  if (payloadLen > MAX_PAYLOAD_LEN) {
    throw new ProtocolError(
      "invalid_data",
      `payload length ${payloadLen} exceeds maximum ${MAX_PAYLOAD_LEN}`,
    );
  }
  const total = FRAME_HEADER_LEN + payloadLen;
  return buf.length >= total ? total : null;
}
