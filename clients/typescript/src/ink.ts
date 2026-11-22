/**
 * Ink-style declarative flex tree helpers with native Ultty rendering and
 * graceful ANSI fallback for other terminals.
 */

import process from "node:process";

import { detectSocketPath, UlttyClient } from "./client.ts";
import {
  BorderType,
  CAP_REGION_METADATA,
  CAP_SEMANTICS,
  Display,
  FlexDirection,
  Live,
  NamedColor,
  OverflowAxis,
  Role,
  TextAlign,
  TextOverflow,
  TextWrap,
  borderAll,
  color as makeColor,
  defaultConstraints,
  defaultSemantics,
  defaultStyle,
  dim,
  type Color,
  type Constraints,
  type Content,
  type Style,
} from "./types.ts";

export type ColorName =
  | "black"
  | "red"
  | "green"
  | "yellow"
  | "blue"
  | "magenta"
  | "cyan"
  | "white"
  | "gray"
  | "brightRed"
  | "brightGreen"
  | "brightYellow"
  | "brightBlue"
  | "brightMagenta"
  | "brightCyan"
  | "brightWhite"
  | "foreground"
  | "background";

export type ColorInput = ColorName | [number, number, number] | Color;

export type BorderInput = "none" | "single" | "double" | "round" | "bold" | "ascii";

export interface BoxProps {
  id?: string;
  flexDirection?: "row" | "column";
  flexGrow?: number;
  width?: number | `${number}%`;
  height?: number | `${number}%`;
  padding?: number;
  paddingX?: number;
  paddingY?: number;
  gap?: number;
  borderStyle?: BorderInput;
  borderColor?: ColorInput;
  backgroundColor?: ColorInput;
  overflowY?: "visible" | "hidden" | "scroll" | "auto";
  role?: Role;
  label?: string;
  live?: Live;
}

export interface TextProps {
  id?: string;
  color?: ColorInput;
  backgroundColor?: ColorInput;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
  dimColor?: boolean;
  wrap?: "none" | "char" | "word";
  align?: "left" | "center" | "right";
  flexGrow?: number;
  overflowY?: "visible" | "hidden" | "scroll" | "auto";
  role?: Role;
  label?: string;
  live?: Live;
}

export type InkNode =
  | { kind: "box"; props: BoxProps; children: InkNode[] }
  | { kind: "text"; props: TextProps; text: string };

export function Box(props: BoxProps, ...children: InkNode[]): InkNode {
  return { kind: "box", props, children };
}

export function Text(text: string, props: TextProps = {}): InkNode {
  return { kind: "text", props, text };
}

const NAMED_MAP: Record<ColorName, NamedColor> = {
  black: NamedColor.Black,
  red: NamedColor.Red,
  green: NamedColor.Green,
  yellow: NamedColor.Yellow,
  blue: NamedColor.Blue,
  magenta: NamedColor.Magenta,
  cyan: NamedColor.Cyan,
  white: NamedColor.White,
  gray: NamedColor.BrightBlack,
  brightRed: NamedColor.BrightRed,
  brightGreen: NamedColor.BrightGreen,
  brightYellow: NamedColor.BrightYellow,
  brightBlue: NamedColor.BrightBlue,
  brightMagenta: NamedColor.BrightMagenta,
  brightCyan: NamedColor.BrightCyan,
  brightWhite: NamedColor.BrightWhite,
  foreground: NamedColor.Foreground,
  background: NamedColor.Background,
};

function resolveColor(input?: ColorInput): Color | null {
  if (!input) return null;
  if (typeof input === "string") return makeColor.named(NAMED_MAP[input]);
  if (Array.isArray(input)) return makeColor.rgb(input[0], input[1], input[2]);
  return input;
}

function resolveDim(value?: number | `${number}%`) {
  if (value === undefined) return dim.auto();
  if (typeof value === "number") return dim.cells(value);
  return dim.percent(Number.parseFloat(value));
}

function resolveBorder(input?: BorderInput) {
  switch (input) {
    case "single":
      return borderAll(BorderType.Thin);
    case "double":
      return borderAll(BorderType.Double);
    case "round":
      return borderAll(BorderType.Rounded, 1);
    case "bold":
      return borderAll(BorderType.Thick);
    case "ascii":
      return borderAll(BorderType.Ascii);
    default:
      return borderAll(BorderType.None);
  }
}

function resolveOverflow(v?: "visible" | "hidden" | "scroll" | "auto"): OverflowAxis {
  switch (v) {
    case "hidden":
      return OverflowAxis.Hidden;
    case "scroll":
      return OverflowAxis.Scroll;
    case "auto":
      return OverflowAxis.Auto;
    default:
      return OverflowAxis.Visible;
  }
}

function nodeToProtocol(
  node: InkNode,
  isRoot: boolean,
): { constraints: Constraints; style: Style; content: Content } {
  if (node.kind === "box") {
    const p = node.props;
    const padX = p.paddingX ?? p.padding ?? 0;
    const padY = p.paddingY ?? p.padding ?? 0;
    const constraints = defaultConstraints({
      display: Display.Flex,
      flexDirection:
        p.flexDirection === "row" ? FlexDirection.Row : FlexDirection.Column,
      flexGrow: p.flexGrow ?? 0,
      width: isRoot ? dim.percent(100) : resolveDim(p.width),
      height: isRoot ? dim.percent(100) : resolveDim(p.height),
      padding: {
        top: dim.cells(padY),
        right: dim.cells(padX),
        bottom: dim.cells(padY),
        left: dim.cells(padX),
      },
      gap:
        p.gap !== undefined
          ? { width: dim.cells(p.gap), height: dim.cells(p.gap) }
          : { width: dim.auto(), height: dim.auto() },
      overflow: {
        x: OverflowAxis.Hidden,
        y: resolveOverflow(p.overflowY),
      },
    });
    const style = defaultStyle({
      bg:
        resolveColor(p.backgroundColor) ??
        (isRoot ? makeColor.named(NamedColor.Background) : null),
      border: p.borderStyle ? resolveBorder(p.borderStyle) : defaultStyle().border,
      borderColor: resolveColor(p.borderColor),
    });
    return { constraints, style, content: { kind: "none" } };
  }

  const p = node.props;
  const constraints = defaultConstraints({
    display: Display.Block,
    flexGrow: p.flexGrow ?? 0,
    width: isRoot ? dim.percent(100) : dim.auto(),
    height: isRoot ? dim.percent(100) : dim.auto(),
    overflow: { x: OverflowAxis.Hidden, y: resolveOverflow(p.overflowY) },
  });
  const style = defaultStyle({
    fg: resolveColor(p.color),
    bg: resolveColor(p.backgroundColor),
    bold: p.bold ?? false,
    italic: p.italic ?? false,
    underline: p.underline ?? false,
    dim: p.dimColor ?? false,
  });
  const wrap =
    p.wrap === "none"
      ? TextWrap.None
      : p.wrap === "char"
        ? TextWrap.Char
        : TextWrap.Word;
  const align =
    p.align === "center"
      ? TextAlign.Center
      : p.align === "right"
        ? TextAlign.Right
        : TextAlign.Left;
  const content: Content = {
    kind: "text",
    content: { text: node.text, wrap, align, overflow: TextOverflow.Clip },
  };
  return { constraints, style, content };
}

function ansiFg(c: Color | null): string {
  if (!c) return "";
  if (c.kind === "rgb") return `\x1b[38;2;${c.r};${c.g};${c.b}m`;
  if (c.kind === "indexed") return `\x1b[38;5;${c.index}m`;
  if (c.color <= 7) return `\x1b[${30 + c.color}m`;
  if (c.color <= 15) return `\x1b[${90 + (c.color - 8)}m`;
  return "";
}

function ansiBg(c: Color | null): string {
  if (!c) return "";
  if (c.kind === "rgb") return `\x1b[48;2;${c.r};${c.g};${c.b}m`;
  if (c.kind === "indexed") return `\x1b[48;5;${c.index}m`;
  if (c.color <= 7) return `\x1b[${40 + c.color}m`;
  if (c.color <= 15) return `\x1b[${100 + (c.color - 8)}m`;
  return "";
}

function wrapText(text: string, width: number): string[] {
  const safeWidth = Math.max(1, width);
  const out: string[] = [];
  for (const rawLine of text.split("\n")) {
    if (rawLine.length <= safeWidth) {
      out.push(rawLine);
      continue;
    }
    let remaining = rawLine;
    while (remaining.length > safeWidth) {
      out.push(remaining.slice(0, safeWidth));
      remaining = remaining.slice(safeWidth);
    }
    out.push(remaining);
  }
  return out;
}

function padVisible(line: string, visibleLen: number, width: number): string {
  const pad = Math.max(0, width - visibleLen);
  return line + " ".repeat(pad);
}

function stripAnsi(s: string): string {
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

function borderGlyphs(style?: BorderInput): [string, string, string, string, string, string] {
  switch (style) {
    case "double":
      return ["╔", "╗", "╚", "╝", "═", "║"];
    case "round":
      return ["╭", "╮", "╰", "╯", "─", "│"];
    case "bold":
      return ["┏", "┓", "┗", "┛", "━", "┃"];
    case "ascii":
      return ["+", "+", "+", "+", "-", "|"];
    default:
      return ["┌", "┐", "└", "┘", "─", "│"];
  }
}

/**
 * Render an `InkNode` tree to an ANSI string for terminals without Ultty native support.
 */
export function renderAnsi(node: InkNode, width = 80): string {
  return renderNodeLines(node, Math.max(4, width)).join("\n");
}

function renderNodeLines(node: InkNode, width: number): string[] {
  if (node.kind === "text") {
    const p = node.props;
    const prefix =
      (p.bold ? "\x1b[1m" : "") +
      (p.dimColor ? "\x1b[2m" : "") +
      (p.italic ? "\x1b[3m" : "") +
      (p.underline ? "\x1b[4m" : "") +
      ansiFg(resolveColor(p.color)) +
      ansiBg(resolveColor(p.backgroundColor));
    const reset = prefix.length > 0 ? "\x1b[0m" : "";
    return wrapText(node.text, width).map((line) => `${prefix}${line}${reset}`);
  }

  const p = node.props;
  const hasBorder = p.borderStyle !== undefined && p.borderStyle !== "none";
  const padX = p.paddingX ?? p.padding ?? 0;
  const padY = p.paddingY ?? p.padding ?? 0;
  let boxWidth = width;
  if (typeof p.width === "number") boxWidth = Math.min(width, p.width);
  else if (typeof p.width === "string" && p.width.endsWith("%")) {
    boxWidth = Math.max(4, Math.floor((width * Number.parseFloat(p.width)) / 100));
  }
  const innerWidth = Math.max(1, boxWidth - (hasBorder ? 2 : 0) - padX * 2);

  let bodyLines: string[] = [];
  if (p.flexDirection === "row" && node.children.length > 0) {
    const gap = p.gap ?? 1;
    const totalGap = gap * Math.max(0, node.children.length - 1);
    const fixedTotal = node.children.reduce(
      (acc, c) =>
        acc + (c.kind === "box" && typeof c.props.width === "number" ? c.props.width : 0),
      0,
    );
    const flexChildren = node.children.filter(
      (c) => !(c.kind === "box" && typeof c.props.width === "number"),
    );
    const remWidth = Math.max(1, innerWidth - fixedTotal - totalGap);
    const flexSlot = Math.max(
      1,
      Math.floor(remWidth / Math.max(1, flexChildren.length)),
    );

    const cols = node.children.map((child) => {
      const cw =
        child.kind === "box" && typeof child.props.width === "number"
          ? child.props.width
          : flexSlot;
      return { width: cw, lines: renderNodeLines(child, cw) };
    });
    const maxRows = cols.reduce((m, c) => Math.max(m, c.lines.length), 0);
    const gapStr = " ".repeat(gap);
    for (let row = 0; row < maxRows; row++) {
      const parts = cols.map((c) => {
        const line = c.lines[row] ?? "";
        return padVisible(line, stripAnsi(line).length, c.width);
      });
      bodyLines.push(parts.join(gapStr));
    }
  } else {
    const gap = p.gap ?? 0;
    node.children.forEach((child, idx) => {
      if (idx > 0 && gap > 0) {
        for (let g = 0; g < gap; g++) bodyLines.push("");
      }
      bodyLines.push(...renderNodeLines(child, innerWidth));
    });
  }

  if (typeof p.height === "number") {
    const maxBody = Math.max(1, p.height - (hasBorder ? 2 : 0) - padY * 2);
    if (bodyLines.length > maxBody) {
      bodyLines = bodyLines.slice(bodyLines.length - maxBody);
    }
  }

  const padded: string[] = [];
  const leftPad = " ".repeat(padX);
  const rightPad = " ".repeat(padX);
  const fullInner = innerWidth + padX * 2;
  for (let i = 0; i < padY; i++) padded.push(" ".repeat(fullInner));
  for (const line of bodyLines) {
    const content = padVisible(line, stripAnsi(line).length, innerWidth);
    padded.push(`${leftPad}${content}${rightPad}`);
  }
  for (let i = 0; i < padY; i++) padded.push(" ".repeat(fullInner));

  if (!hasBorder) return padded;

  const [tl, tr, bl, br, h, v] = borderGlyphs(p.borderStyle);
  const bc = ansiFg(resolveColor(p.borderColor));
  const reset = bc.length > 0 ? "\x1b[0m" : "";
  const horiz = h.repeat(fullInner);
  return [
    `${bc}${tl}${horiz}${tr}${reset}`,
    ...padded.map((line) => `${bc}${v}${reset}${line}${bc}${v}${reset}`),
    `${bc}${bl}${horiz}${br}${reset}`,
  ];
}

export interface RenderOptions {
  socketPath?: string;
  forceAnsi?: boolean;
  output?: NodeJS.WritableStream;
  columns?: number;
}

export interface RenderedApp {
  readonly mode: "native" | "ansi";
  readonly client: UlttyClient | null;
  rerender(node: InkNode): Promise<void>;
  unmount(): void;
}

/**
 * Render an Ink-style node tree. Uses the Ultty native protocol when connected
 * via `ULTTY_SOCKET`, and falls back to ANSI rendering everywhere else.
 */
export async function render(
  initialNode: InkNode,
  options: RenderOptions = {},
): Promise<RenderedApp> {
  const out = options.output ?? process.stdout;
  const socketPath = options.forceAnsi
    ? null
    : (options.socketPath ?? detectSocketPath());

  if (socketPath) {
    try {
      const client = await UlttyClient.connect({ socketPath });
      let currentRootChild: number | null = null;

      const mountTree = async (node: InkNode): Promise<void> => {
        const frameId = client.beginFrame();
        if (currentRootChild !== null) {
          client.deleteRegion(currentRootChild);
        }
        const emitNode = (n: InkNode, parent: number, isRoot: boolean): number => {
          const { constraints, style, content } = nodeToProtocol(n, isRoot);
          const id = client.createRegion(parent, constraints, style);
          if (content.kind !== "none") {
            client.setContent(id, content);
          }
          if (n.props.id && client.supportsCapability(CAP_REGION_METADATA)) {
            client.setMetadata(id, [["id", n.props.id]]);
          }
          if (
            client.supportsCapability(CAP_SEMANTICS) &&
            (n.props.role !== undefined ||
              n.props.label !== undefined ||
              n.props.live !== undefined)
          ) {
            client.setSemantics(
              id,
              defaultSemantics({
                role: n.props.role ?? Role.Auto,
                label: n.props.label ?? null,
                live: n.props.live ?? Live.Off,
              }),
            );
          }
          if (n.kind === "box") {
            for (const child of n.children) {
              emitNode(child, id, false);
            }
          }
          return id;
        };

        currentRootChild = emitNode(node, client.rootRegion, true);
        await client.endFrame(frameId);
      };

      await mountTree(initialNode);
      return {
        mode: "native",
        client,
        rerender: mountTree,
        unmount: () => client.disconnect(),
      };
    } catch {
      // Fall back cleanly to ANSI if the socket is unavailable.
    }
  }

  const getColumns = (): number =>
    options.columns ??
    ("columns" in out && typeof out.columns === "number" ? out.columns : 80);

  const drawAnsi = async (node: InkNode): Promise<void> => {
    out.write(`${renderAnsi(node, getColumns())}\n`);
  };

  await drawAnsi(initialNode);
  return {
    mode: "ansi",
    client: null,
    rerender: drawAnsi,
    unmount: () => {},
  };
}
