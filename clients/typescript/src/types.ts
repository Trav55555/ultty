/**
 * Wire protocol constants, message unions, layout constraints, styles, and events
 * matching crates/ultty-protocol v1.
 */

export const FRAME_MAGIC = new Uint8Array([0x55, 0x54]); // "UT"
export const FRAME_MAGIC_LE = 0x5455;
export const FRAME_HEADER_LEN = 8;

export const PROTOCOL_VERSION = 1;
export const MIN_PROTOCOL_VERSION = 1;

export const ROOT_REGION = 0;

export const MIN_SURFACE_ROWS = 1;
export const MAX_SURFACE_ROWS = 128;

export const MAX_STRING_LEN = 1024 * 1024;
export const MAX_PAYLOAD_LEN = 16 * 1024 * 1024;
export const MAX_REGION_COUNT = 10_000;
export const MAX_TRACK_COUNT = 1_000;
export const MAX_SPAN_COUNT = 10_000;
export const MAX_METADATA_PAIRS = 1024;
export const MAX_METADATA_TOTAL_BYTES = 1024 * 1024;
export const MAX_IMAGE_DIMENSION = 16_384;
export const MAX_SEMANTICS_STRING_LEN = 4 * 1024;
export const MAX_SEMANTICS_TOTAL_BYTES = 8 * 1024;

export const CAP_LAYOUT_FLEXBOX = 0x0001;
export const CAP_WIDGET_SCROLL = 0x0010;
export const CAP_APPEND_CONTENT = 0x0020;
export const CAP_STREAM_LIFECYCLE = 0x0040;
export const CAP_REGION_METADATA = 0x0080;
export const CAP_INLINE_SURFACES = 0x0100;
export const CAP_SEMANTICS = 0x0200;
export const CAP_INPUT_CURSOR = 0x0400;
export const CAP_OVERRIDE_PUSH = 0x0800;
export const CAP_SEMANTIC_ACTIONS = 0x1000;

export const CAP_ALL_KNOWN =
  CAP_LAYOUT_FLEXBOX |
  CAP_WIDGET_SCROLL |
  CAP_APPEND_CONTENT |
  CAP_STREAM_LIFECYCLE |
  CAP_REGION_METADATA |
  CAP_INLINE_SURFACES |
  CAP_SEMANTICS |
  CAP_INPUT_CURSOR |
  CAP_OVERRIDE_PUSH |
  CAP_SEMANTIC_ACTIONS;

export const CLIENT_DEFAULT_CAPABILITIES =
  CAP_ALL_KNOWN & ~(CAP_OVERRIDE_PUSH | CAP_SEMANTIC_ACTIONS);

export const EVENT_MOUSE = 0x01;
export const EVENT_KEY = 0x02;
export const EVENT_FOCUS = 0x04;
export const EVENT_SCROLL = 0x08;

export const MOD_NONE = 0x00;
export const MOD_SHIFT = 0x01;
export const MOD_CTRL = 0x02;
export const MOD_ALT = 0x04;
export const MOD_META = 0x08;

export const TRANSITION_OPACITY = 0x01;
export const TRANSITION_BG = 0x02;
export const TRANSITION_FG = 0x04;
export const TRANSITION_BORDER_COLOR = 0x08;
export const TRANSITION_ALL = 0x0f;

export const MsgType = {
  // App -> Terminal
  CLIENT_HELLO: 0x0001,
  CREATE_REGION: 0x0010,
  DELETE_REGION: 0x0012,
  REPARENT_REGION: 0x0013,
  SET_REGION_METADATA: 0x0014,
  CREATE_SURFACE: 0x0015,
  CLOSE_SURFACE: 0x0016,
  SET_CONTENT: 0x0020,
  SET_STYLE: 0x0021,
  SET_CONSTRAINTS: 0x0022,
  APPEND_CONTENT: 0x0023,
  STREAM_START: 0x0024,
  STREAM_END: 0x0025,
  SET_SEMANTICS: 0x0026,
  BEGIN_FRAME: 0x0030,
  END_FRAME: 0x0031,
  SCROLL: 0x0040,
  FOCUS: 0x0041,
  BLUR: 0x0042,
  SUBSCRIBE: 0x0050,
  UNSUBSCRIBE: 0x0051,
  CLIPBOARD_GET: 0x0060,
  CLIPBOARD_SET: 0x0061,
  PONG: 0x0070,
  OVERRIDE_RESULT: 0x0080,
  ACTION_RESULT: 0x0081,
  DISCONNECT: 0x00ff,

  // Terminal -> App
  SERVER_HELLO: 0x0001,
  SESSION_READY: 0x0002,
  LAYOUT_COMPUTED: 0x0010,
  INPUT: 0x0020,
  RESIZE: 0x0022,
  FOCUS_EVENT: 0x0023,
  SCROLL_EVENT: 0x0024,
  CLIPBOARD_CONTENT: 0x0025,
  SURFACE_EVENT: 0x0026,
  PING: 0x0070,
  SET_OVERRIDE: 0x0080,
  INVOKE_ACTION: 0x0081,
  ERROR: 0x00ff,
} as const;

export const NamedColor = {
  Black: 0,
  Red: 1,
  Green: 2,
  Yellow: 3,
  Blue: 4,
  Magenta: 5,
  Cyan: 6,
  White: 7,
  BrightBlack: 8,
  BrightRed: 9,
  BrightGreen: 10,
  BrightYellow: 11,
  BrightBlue: 12,
  BrightMagenta: 13,
  BrightCyan: 14,
  BrightWhite: 15,
  Foreground: 16,
  Background: 17,
} as const;
export type NamedColor = (typeof NamedColor)[keyof typeof NamedColor];

export type Color =
  | { kind: "named"; color: NamedColor }
  | { kind: "rgb"; r: number; g: number; b: number }
  | { kind: "indexed"; index: number };

export const BorderType = {
  None: 0,
  Thin: 1,
  Thick: 2,
  Double: 3,
  Rounded: 4,
  Dashed: 5,
  Ascii: 6,
} as const;
export type BorderType = (typeof BorderType)[keyof typeof BorderType];

export interface BorderEdges {
  top: boolean;
  right: boolean;
  bottom: boolean;
  left: boolean;
}

export interface BorderStyle {
  style: BorderType;
  edges: BorderEdges;
  radius: number;
}

export const Easing = {
  Linear: 0,
  EaseIn: 1,
  EaseOut: 2,
  EaseInOut: 3,
} as const;
export type Easing = (typeof Easing)[keyof typeof Easing];

export interface Transition {
  durationMs: number;
  easing: Easing;
  properties: number;
}

export interface Style {
  fg: Color | null;
  bg: Color | null;
  border: BorderStyle;
  borderColor: Color | null;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  dim: boolean;
  opacity: number;
  transition: Transition | null;
}

export const Display = {
  Block: 0,
  Flex: 1,
  Grid: 2,
  None: 3,
} as const;
export type Display = (typeof Display)[keyof typeof Display];

export const FlexDirection = {
  Row: 0,
  RowReverse: 1,
  Column: 2,
  ColumnReverse: 3,
} as const;
export type FlexDirection = (typeof FlexDirection)[keyof typeof FlexDirection];

export const FlexWrap = {
  NoWrap: 0,
  Wrap: 1,
  WrapReverse: 2,
} as const;
export type FlexWrap = (typeof FlexWrap)[keyof typeof FlexWrap];

export const JustifyContent = {
  Start: 0,
  End: 1,
  Center: 2,
  SpaceBetween: 3,
  SpaceAround: 4,
  SpaceEvenly: 5,
} as const;
export type JustifyContent =
  (typeof JustifyContent)[keyof typeof JustifyContent];

export const AlignItems = {
  Start: 0,
  End: 1,
  Center: 2,
  Baseline: 3,
  Stretch: 4,
} as const;
export type AlignItems = (typeof AlignItems)[keyof typeof AlignItems];

export const AlignSelf = {
  Auto: 0,
  Start: 1,
  End: 2,
  Center: 3,
  Baseline: 4,
  Stretch: 5,
} as const;
export type AlignSelf = (typeof AlignSelf)[keyof typeof AlignSelf];

export const AlignContent = {
  Start: 0,
  End: 1,
  Center: 2,
  SpaceBetween: 3,
  SpaceAround: 4,
  SpaceEvenly: 5,
  Stretch: 6,
} as const;
export type AlignContent = (typeof AlignContent)[keyof typeof AlignContent];

export type Dimension =
  | { kind: "auto" }
  | { kind: "cells"; value: number }
  | { kind: "percent"; value: number }
  | { kind: "fr"; value: number }
  | { kind: "vw"; value: number }
  | { kind: "vh"; value: number }
  | { kind: "min_content" }
  | { kind: "max_content" };

export interface Edges<T> {
  top: T;
  right: T;
  bottom: T;
  left: T;
}

export interface Size<T> {
  width: T;
  height: T;
}

export const OverflowAxis = {
  Visible: 0,
  Hidden: 1,
  Scroll: 2,
  Auto: 3,
} as const;
export type OverflowAxis = (typeof OverflowAxis)[keyof typeof OverflowAxis];

export interface Overflow {
  x: OverflowAxis;
  y: OverflowAxis;
}

export type GridLine =
  | { kind: "auto" }
  | { kind: "line"; value: number }
  | { kind: "span"; value: number };

export interface GridPlacement {
  start: GridLine;
  end: GridLine;
}

export type TrackSizingFunction =
  | { kind: "single"; dim: Dimension }
  | { kind: "minmax"; min: Dimension; max: Dimension }
  | { kind: "repeat"; count: number; dims: Dimension[] };

export interface Constraints {
  display: Display;
  flexDirection: FlexDirection;
  flexWrap: FlexWrap;
  justifyContent: JustifyContent;
  alignItems: AlignItems;
  alignSelf: AlignSelf;
  alignContent: AlignContent;
  flexGrow: number;
  flexShrink: number;
  flexBasis: Dimension;
  width: Dimension;
  height: Dimension;
  minWidth: Dimension;
  minHeight: Dimension;
  maxWidth: Dimension;
  maxHeight: Dimension;
  aspectRatio: number | null;
  margin: Edges<Dimension>;
  padding: Edges<Dimension>;
  gap: Size<Dimension>;
  overflow: Overflow;
  gridTemplateColumns: TrackSizingFunction[];
  gridTemplateRows: TrackSizingFunction[];
  gridColumn: GridPlacement;
  gridRow: GridPlacement;
}

export const TextWrap = {
  None: 0,
  Char: 1,
  Word: 2,
} as const;
export type TextWrap = (typeof TextWrap)[keyof typeof TextWrap];

export const TextAlign = {
  Left: 0,
  Center: 1,
  Right: 2,
} as const;
export type TextAlign = (typeof TextAlign)[keyof typeof TextAlign];

export const TextOverflow = {
  Clip: 0,
  Ellipsis: 1,
  Fade: 2,
} as const;
export type TextOverflow = (typeof TextOverflow)[keyof typeof TextOverflow];

export interface TextContent {
  text: string;
  wrap: TextWrap;
  align: TextAlign;
  overflow: TextOverflow;
}

export interface Span {
  text: string;
  style: Style;
}

export interface RichText {
  spans: Span[];
}

export const SpinnerStyle = {
  Braille: 0,
  Dots: 1,
  Line: 2,
  Bounce: 3,
} as const;
export type SpinnerStyle = (typeof SpinnerStyle)[keyof typeof SpinnerStyle];

export const ImageScale = {
  Fit: 0,
  Fill: 1,
  Stretch: 2,
  None: 3,
} as const;
export type ImageScale = (typeof ImageScale)[keyof typeof ImageScale];

export const ButtonStyle = {
  Default: 0,
  Primary: 1,
  Danger: 2,
  Success: 3,
} as const;
export type ButtonStyle = (typeof ButtonStyle)[keyof typeof ButtonStyle];

export type WidgetContent =
  | {
      kind: "input";
      value: string;
      placeholder: string;
      multiline: boolean;
      password: boolean;
      cursor: number | null;
    }
  | { kind: "markdown"; source: string }
  | { kind: "code"; source: string; language: string; lineNumbers: boolean }
  | {
      kind: "progress";
      value: number;
      label: string | null;
      animated: boolean;
      indeterminate: boolean;
    }
  | { kind: "spinner"; label: string | null; style: SpinnerStyle }
  | {
      kind: "image";
      width: number;
      height: number;
      scale: ImageScale;
      data: Uint8Array;
    }
  | { kind: "button"; label: string; style: ButtonStyle };

export type Content =
  | { kind: "none" }
  | { kind: "text"; content: TextContent }
  | { kind: "rich_text"; content: RichText }
  | { kind: "widget"; widget: WidgetContent };

export const MouseButton = {
  Left: 0,
  Middle: 1,
  Right: 2,
  Button4: 3,
  Button5: 4,
} as const;
export type MouseButton = (typeof MouseButton)[keyof typeof MouseButton];

export type Key =
  | { kind: "char"; char: string }
  | { kind: "function"; number: number }
  | { kind: "escape" }
  | { kind: "enter" }
  | { kind: "tab" }
  | { kind: "backspace" }
  | { kind: "delete" }
  | { kind: "insert" }
  | { kind: "home" }
  | { kind: "end" }
  | { kind: "page_up" }
  | { kind: "page_down" }
  | { kind: "up" }
  | { kind: "down" }
  | { kind: "left" }
  | { kind: "right" };

export type InputType =
  | { kind: "mouse_down"; button: MouseButton }
  | { kind: "mouse_up"; button: MouseButton }
  | { kind: "mouse_move" }
  | { kind: "mouse_scroll"; deltaX: number; deltaY: number }
  | { kind: "key_down"; key: Key; modifiers: number }
  | { kind: "key_up"; key: Key; modifiers: number }
  | { kind: "text_input"; text: string };

export interface InputEvent {
  regionId: number;
  eventType: InputType;
  x: number;
  y: number;
  modifiers: number;
}

export interface ResizeEvent {
  width: number;
  height: number;
}

export interface FocusEvent {
  regionId: number;
  focused: boolean;
}

export interface ScrollEvent {
  regionId: number;
  scrollX: number;
  scrollY: number;
}

export const LayoutTrigger = {
  Resize: 0,
  ConstraintChange: 1,
  ContentChange: 2,
  TreeChange: 3,
} as const;
export type LayoutTrigger = (typeof LayoutTrigger)[keyof typeof LayoutTrigger];

export interface ComputedRegionLayout {
  id: number;
  x: number;
  y: number;
  width: number;
  height: number;
  contentX: number;
  contentY: number;
  contentWidth: number;
  contentHeight: number;
  visible: boolean;
}

export const SurfaceEventKind = {
  Placed: 0,
  Closed: 1,
  Evicted: 2,
  Hidden: 3,
  Shown: 4,
} as const;
export type SurfaceEventKind =
  (typeof SurfaceEventKind)[keyof typeof SurfaceEventKind];

export const Role = {
  Auto: 0,
  Group: 1,
  Text: 2,
  Heading: 3,
  Log: 4,
  Status: 5,
  Alert: 6,
  Dialog: 7,
  Input: 8,
  Button: 9,
  Checkbox: 10,
  Switch: 11,
  Progress: 12,
  List: 13,
  ListItem: 14,
  TabList: 15,
  Tab: 16,
  Tree: 17,
  TreeItem: 18,
  Image: 19,
  Link: 20,
  Table: 21,
  Row: 22,
  Cell: 23,
  ColumnHeader: 24,
  ComboBox: 25,
  Option: 26,
} as const;
export type Role = (typeof Role)[keyof typeof Role];

export const Live = {
  Off: 0,
  Polite: 1,
  Assertive: 2,
} as const;
export type Live = (typeof Live)[keyof typeof Live];

export const Toggled = {
  Unset: 0,
  False: 1,
  True: 2,
  Mixed: 3,
} as const;
export type Toggled = (typeof Toggled)[keyof typeof Toggled];

export const Expanded = {
  Unset: 0,
  Collapsed: 1,
  Expanded: 2,
} as const;
export type Expanded = (typeof Expanded)[keyof typeof Expanded];

export interface Semantics {
  role: Role;
  label: string | null;
  description: string | null;
  live: Live;
  busy: boolean;
  hidden: boolean;
  readOnly: boolean;
  disabled: boolean;
  atomic: boolean;
  modal: boolean;
  selected: boolean;
  toggled: Toggled;
  expanded: Expanded;
  value: { now: number; min: number; max: number } | null;
  valueText: string | null;
  position: { index: number; size: number } | null;
  level: number;
  labelledBy: number | null;
  describedBy: number | null;
  controls: number | null;
}

export type AppMessage =
  | { type: "client_hello"; version: number; requestedCapabilities: number }
  | {
      type: "create_region";
      id: number;
      parent: number;
      insertIndex: number;
      constraints: Constraints;
      style: Style;
    }
  | { type: "set_constraints"; id: number; constraints: Constraints }
  | { type: "set_style"; id: number; style: Style }
  | { type: "set_content"; id: number; content: Content }
  | { type: "append_content"; id: number; text: string }
  | { type: "stream_start"; id: number; streamId: number }
  | { type: "stream_end"; id: number; streamId: number }
  | {
      type: "set_region_metadata";
      id: number;
      metadata: Array<[string, string]>;
    }
  | { type: "set_semantics"; id: number; semantics: Semantics }
  | { type: "delete_region"; id: number }
  | {
      type: "create_surface";
      id: number;
      rows: number;
      constraints: Constraints;
      style: Style;
    }
  | { type: "close_surface"; id: number }
  | {
      type: "reparent_region";
      id: number;
      newParent: number;
      insertIndex: number;
    }
  | { type: "begin_frame"; frameId: number }
  | { type: "end_frame"; frameId: number }
  | { type: "scroll"; id: number; x: number; y: number; animate: boolean }
  | { type: "focus"; id: number }
  | { type: "blur" }
  | { type: "subscribe"; id: number; events: number }
  | { type: "unsubscribe"; id: number; events: number }
  | { type: "clipboard_get" }
  | { type: "clipboard_set"; text: string }
  | { type: "disconnect" }
  | { type: "pong" }
  | { type: "override_result"; requestId: number; error: string | null }
  | { type: "action_result"; requestId: number; error: string | null };

export type TerminalMessage =
  | { type: "server_hello"; version: number; capabilities: number }
  | {
      type: "session_ready";
      sessionId: bigint;
      rootRegion: number;
      width: number;
      height: number;
    }
  | {
      type: "layout_computed";
      frameId: number | null;
      trigger: LayoutTrigger;
      regions: ComputedRegionLayout[];
    }
  | { type: "input"; event: InputEvent }
  | { type: "resize"; event: ResizeEvent }
  | { type: "focus"; event: FocusEvent }
  | { type: "scroll"; event: ScrollEvent }
  | { type: "clipboard_content"; text: string | null }
  | { type: "surface_event"; id: number; kind: SurfaceEventKind }
  | { type: "ping" }
  | {
      type: "set_override";
      requestId: number;
      pushed: { format: number; source: string } | null;
    }
  | {
      type: "invoke_action";
      requestId: number;
      frameId: number;
      id: number;
      widgetId: string;
      action: number;
    }
  | { type: "error"; code: number; message: string };

export const dim = {
  auto: (): Dimension => ({ kind: "auto" }),
  cells: (value: number): Dimension => ({ kind: "cells", value }),
  percent: (value: number): Dimension => ({ kind: "percent", value }),
  fr: (value: number): Dimension => ({ kind: "fr", value }),
  vw: (value: number): Dimension => ({ kind: "vw", value }),
  vh: (value: number): Dimension => ({ kind: "vh", value }),
  minContent: (): Dimension => ({ kind: "min_content" }),
  maxContent: (): Dimension => ({ kind: "max_content" }),
} as const;

export const color = {
  named: (c: NamedColor): Color => ({ kind: "named", color: c }),
  rgb: (r: number, g: number, b: number): Color => ({ kind: "rgb", r, g, b }),
  indexed: (index: number): Color => ({ kind: "indexed", index }),
} as const;

export function defaultBorder(): BorderStyle {
  return {
    style: BorderType.None,
    edges: { top: false, right: false, bottom: false, left: false },
    radius: 0,
  };
}

export function borderAll(style: BorderType, radius = 0): BorderStyle {
  return {
    style,
    edges: { top: true, right: true, bottom: true, left: true },
    radius,
  };
}

export function defaultStyle(overrides: Partial<Style> = {}): Style {
  return {
    fg: null,
    bg: null,
    border: defaultBorder(),
    borderColor: null,
    bold: false,
    italic: false,
    underline: false,
    dim: false,
    opacity: 1.0,
    transition: null,
    ...overrides,
  };
}

export function defaultConstraints(
  overrides: Partial<Constraints> = {},
): Constraints {
  return {
    display: Display.Block,
    flexDirection: FlexDirection.Column,
    flexWrap: FlexWrap.NoWrap,
    justifyContent: JustifyContent.Start,
    alignItems: AlignItems.Stretch,
    alignSelf: AlignSelf.Auto,
    alignContent: AlignContent.Stretch,
    flexGrow: 0,
    flexShrink: 0,
    flexBasis: dim.auto(),
    width: dim.auto(),
    height: dim.auto(),
    minWidth: dim.auto(),
    minHeight: dim.auto(),
    maxWidth: dim.auto(),
    maxHeight: dim.auto(),
    aspectRatio: null,
    margin: {
      top: dim.cells(0),
      right: dim.cells(0),
      bottom: dim.cells(0),
      left: dim.cells(0),
    },
    padding: {
      top: dim.auto(),
      right: dim.auto(),
      bottom: dim.auto(),
      left: dim.auto(),
    },
    gap: { width: dim.auto(), height: dim.auto() },
    overflow: { x: OverflowAxis.Visible, y: OverflowAxis.Visible },
    gridTemplateColumns: [],
    gridTemplateRows: [],
    gridColumn: { start: { kind: "auto" }, end: { kind: "auto" } },
    gridRow: { start: { kind: "auto" }, end: { kind: "auto" } },
    ...overrides,
  };
}

export function defaultSemantics(
  overrides: Partial<Semantics> = {},
): Semantics {
  return {
    role: Role.Auto,
    label: null,
    description: null,
    live: Live.Off,
    busy: false,
    hidden: false,
    readOnly: false,
    disabled: false,
    atomic: false,
    modal: false,
    selected: false,
    toggled: Toggled.Unset,
    expanded: Expanded.Unset,
    value: null,
    valueText: null,
    position: null,
    level: 0,
    labelledBy: null,
    describedBy: null,
    controls: null,
    ...overrides,
  };
}

export function negotiateProtocolVersion(
  clientVersion: number,
  serverVersion: number,
): number | null {
  if (
    clientVersion < MIN_PROTOCOL_VERSION ||
    serverVersion < MIN_PROTOCOL_VERSION
  ) {
    return null;
  }
  return Math.min(clientVersion, serverVersion);
}
