import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { Writable } from "node:stream";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

import {
  BorderType,
  Box,
  CAP_APPEND_CONTENT,
  CAP_INLINE_SURFACES,
  CAP_LAYOUT_FLEXBOX,
  CAP_REGION_METADATA,
  CAP_SEMANTICS,
  Display,
  FlexDirection,
  LayoutTrigger,
  Live,
  MOD_CTRL,
  NamedColor,
  OverflowAxis,
  PROTOCOL_VERSION,
  ProtocolError,
  ROOT_REGION,
  Role,
  Text,
  TextAlign,
  TextOverflow,
  TextWrap,
  SurfaceEventKind,
  UlttyClient,
  borderAll,
  color,
  decodeAppMessage,
  decodeTerminalMessage,
  defaultConstraints,
  defaultSemantics,
  defaultStyle,
  dim,
  encodeAppMessage,
  encodeTerminalMessage,
  peekFrameLength,
  render,
  renderAnsi,
  type AppMessage,
  type TerminalMessage,
} from "../src/index.ts";

interface ConformanceVector {
  name: string;
  direction: "app" | "terminal";
  hex: string;
  expected: Record<string, unknown>;
}

function hexToBytes(hex: string): Uint8Array {
  assert.equal(hex.length % 2, 0);
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

const here = path.dirname(fileURLToPath(import.meta.url));
const vectorsPath = path.resolve(
  here,
  "../../../crates/ultty-protocol/tests/conformance/v1.json",
);
const vectors: ConformanceVector[] = JSON.parse(
  fs.readFileSync(vectorsPath, "utf8"),
);

describe("v1 wire conformance suite", () => {
  it("loads all v1 conformance vectors", () => {
    assert.ok(vectors.length >= 32, `expected >= 32 vectors, got ${vectors.length}`);
  });

  for (const vec of vectors) {
    it(`vector: ${vec.name}`, () => {
      const raw = hexToBytes(vec.hex);
      const expected = vec.expected;

      if (expected.type === "reject") {
        assert.throws(
          () => {
            if (vec.direction === "app") {
              decodeAppMessage(raw);
            } else {
              decodeTerminalMessage(raw);
            }
          },
          (err: unknown) => {
            assert.ok(err instanceof ProtocolError, `expected ProtocolError, got ${err}`);
            assert.equal(err.code, expected.error);
            return true;
          },
        );
        return;
      }

      if (vec.direction === "app") {
        const { message, bytesRead } = decodeAppMessage(raw);
        assert.equal(bytesRead, raw.length);
        assert.equal(bytesToHex(encodeAppMessage(message)), vec.hex);

        if (expected.type === "client_hello") {
          assert.deepEqual(message, {
            type: "client_hello",
            version: expected.version,
            requestedCapabilities: expected.requested_capabilities,
          });
        } else if (expected.type === "begin_frame") {
          assert.deepEqual(message, {
            type: "begin_frame",
            frameId: expected.frame_id,
          });
        } else if (expected.type === "end_frame") {
          assert.deepEqual(message, {
            type: "end_frame",
            frameId: expected.frame_id,
          });
        } else if (expected.type === "create_region") {
          assert.equal(message.type, "create_region");
          assert.equal(message.id, expected.id);
          assert.equal(message.parent, expected.parent);
          assert.equal(message.insertIndex, expected.insert_index);
          assert.equal(message.constraints.display, Display.Flex);
          assert.equal(message.constraints.flexDirection, FlexDirection.Row);
          assert.equal(message.constraints.flexGrow, expected.flex_grow);
          assert.deepEqual(message.constraints.width, {
            kind: "percent",
            value: expected.width_percent,
          });
          assert.equal(message.constraints.overflow.y, OverflowAxis.Scroll);
          const rgb = expected.fg_rgb as [number, number, number];
          assert.deepEqual(message.style.fg, color.rgb(rgb[0], rgb[1], rgb[2]));
          assert.equal(message.style.bold, expected.bold);
        } else if (expected.type === "delete_region") {
          assert.deepEqual(message, {
            type: "delete_region",
            id: expected.region,
          });
        } else if (expected.type === "text") {
          assert.equal(message.type, "set_content");
          assert.equal(message.id, expected.region);
          assert.equal(message.content.kind, "text");
          assert.equal(message.content.content.text, expected.text);
        } else if (expected.type === "rich_text") {
          assert.equal(message.type, "set_content");
          assert.equal(message.id, expected.region);
          assert.equal(message.content.kind, "rich_text");
          assert.deepEqual(
            message.content.content.spans.map((s) => s.text),
            expected.spans,
          );
        } else if (expected.type === "append_content") {
          assert.deepEqual(message, {
            type: "append_content",
            id: expected.region,
            text: expected.text,
          });
        } else if (expected.type === "set_region_metadata") {
          assert.deepEqual(message, {
            type: "set_region_metadata",
            id: expected.region,
            metadata: expected.metadata,
          });
        } else if (expected.type === "markdown") {
          assert.equal(message.type, "set_content");
          assert.equal(message.id, expected.region);
          assert.equal(message.content.kind, "widget");
          assert.equal(message.content.widget.kind, "markdown");
          assert.equal(message.content.widget.source, expected.source);
        } else if (expected.type === "code") {
          assert.equal(message.type, "set_content");
          assert.equal(message.id, expected.region);
          assert.equal(message.content.kind, "widget");
          assert.equal(message.content.widget.kind, "code");
          assert.equal(message.content.widget.source, expected.source);
          assert.equal(message.content.widget.language, expected.language);
          assert.equal(message.content.widget.lineNumbers, expected.line_numbers);
        } else if (expected.type === "create_surface") {
          assert.equal(message.type, "create_surface");
          assert.equal(message.id, expected.region);
          assert.equal(message.rows, expected.rows);
        } else if (expected.type === "close_surface") {
          assert.deepEqual(message, {
            type: "close_surface",
            id: expected.region,
          });
        } else if (expected.type === "set_semantics") {
          assert.equal(message.type, "set_semantics");
          assert.equal(message.id, expected.region);
          assert.equal(message.semantics.role, Role.Log);
          assert.equal(message.semantics.label, expected.label);
          assert.equal(message.semantics.live, Live.Polite);
          assert.equal(message.semantics.busy, expected.busy);
        } else if (expected.type === "scroll") {
          assert.deepEqual(message, {
            type: "scroll",
            id: expected.region,
            x: expected.x,
            y: expected.y,
            animate: expected.animate,
          });
        } else if (expected.type === "disconnect") {
          assert.deepEqual(message, { type: "disconnect" });
        } else if (expected.type === "pong") {
          assert.deepEqual(message, { type: "pong" });
        } else {
          assert.fail(`unhandled app vector type: ${String(expected.type)}`);
        }
      } else {
        const { message, bytesRead } = decodeTerminalMessage(raw);
        assert.equal(bytesRead, raw.length);
        assert.equal(bytesToHex(encodeTerminalMessage(message)), vec.hex);

        if (expected.type === "server_hello") {
          assert.deepEqual(message, {
            type: "server_hello",
            version: expected.version,
            capabilities: expected.capabilities,
          });
        } else if (expected.type === "session_ready") {
          assert.deepEqual(message, {
            type: "session_ready",
            sessionId: BigInt(String(expected.session_id)),
            rootRegion: expected.root_region,
            width: expected.width,
            height: expected.height,
          });
        } else if (expected.type === "surface_event") {
          assert.deepEqual(message, {
            type: "surface_event",
            id: expected.region,
            kind: SurfaceEventKind.Placed,
          });
        } else if (expected.type === "layout_computed") {
          assert.equal(message.type, "layout_computed");
          assert.equal(message.frameId, expected.frame_id);
          assert.equal(message.trigger, LayoutTrigger.TreeChange);
          assert.equal(message.regions.length, 1);
          assert.equal(message.regions[0].id, expected.region);
          assert.equal(message.regions[0].width, expected.width);
          assert.equal(message.regions[0].height, expected.height);
          assert.equal(message.regions[0].contentWidth, expected.content_width);
          assert.equal(message.regions[0].contentHeight, expected.content_height);
          assert.equal(message.regions[0].visible, expected.visible);
        } else if (expected.type === "input_key_down") {
          assert.equal(message.type, "input");
          assert.equal(message.event.regionId, expected.region);
          assert.deepEqual(message.event.eventType, {
            kind: "key_down",
            key: { kind: "char", char: expected.char },
            modifiers: expected.modifiers,
          });
          assert.equal(message.event.modifiers, expected.modifiers);
        } else if (expected.type === "resize") {
          assert.deepEqual(message, {
            type: "resize",
            event: { width: expected.width, height: expected.height },
          });
        } else if (expected.type === "error") {
          assert.deepEqual(message, {
            type: "error",
            code: expected.code,
            message: expected.message,
          });
        } else if (expected.type === "ping") {
          assert.deepEqual(message, { type: "ping" });
        } else {
          assert.fail(`unhandled terminal vector type: ${String(expected.type)}`);
        }
      }
    });
  }
});

describe("codec roundtrips", () => {
  it("roundtrips CreateRegion, SetContent, SetSemantics, and Scroll", () => {
    const messages: AppMessage[] = [
      {
        type: "create_region",
        id: 7,
        parent: ROOT_REGION,
        insertIndex: -1,
        constraints: defaultConstraints({
          display: Display.Flex,
          flexDirection: FlexDirection.Row,
          flexGrow: 1,
          width: dim.percent(100),
          height: dim.cells(24),
          overflow: { x: OverflowAxis.Hidden, y: OverflowAxis.Scroll },
        }),
        style: defaultStyle({
          fg: color.rgb(200, 220, 255),
          bg: color.named(NamedColor.Background),
          border: borderAll(BorderType.Rounded, 1),
          bold: true,
        }),
      },
      {
        type: "set_content",
        id: 7,
        content: {
          kind: "text",
          content: {
            text: "Hello 日本語",
            wrap: TextWrap.Word,
            align: TextAlign.Left,
            overflow: TextOverflow.Ellipsis,
          },
        },
      },
      {
        type: "set_semantics",
        id: 7,
        semantics: defaultSemantics({
          role: Role.Log,
          label: "Transcript",
          live: Live.Polite,
          busy: true,
        }),
      },
      { type: "scroll", id: 7, x: 0, y: 12.5, animate: true },
    ];

    for (const msg of messages) {
      const encoded = encodeAppMessage(msg);
      const { message, bytesRead } = decodeAppMessage(encoded);
      assert.equal(bytesRead, encoded.length);
      assert.deepEqual(message, msg);
    }
  });

  it("roundtrips TerminalMessage layout, input, and surface events", () => {
    const messages: TerminalMessage[] = [
      {
        type: "layout_computed",
        frameId: 3,
        trigger: LayoutTrigger.TreeChange,
        regions: [
          {
            id: 1,
            x: 0,
            y: 0,
            width: 80,
            height: 24,
            contentX: 1,
            contentY: 1,
            contentWidth: 78,
            contentHeight: 22,
            visible: true,
          },
        ],
      },
      {
        type: "input",
        event: {
          regionId: 1,
          eventType: {
            kind: "key_down",
            key: { kind: "char", char: "c" },
            modifiers: MOD_CTRL,
          },
          x: 0,
          y: 0,
          modifiers: MOD_CTRL,
        },
      },
      { type: "resize", event: { width: 120, height: 40 } },
    ];

    for (const msg of messages) {
      const encoded = encodeTerminalMessage(msg);
      const { message, bytesRead } = decodeTerminalMessage(encoded);
      assert.equal(bytesRead, encoded.length);
      assert.deepEqual(message, msg);
    }
  });
});

describe("UlttyClient and Ink-style renderer", () => {
  it("completes handshake, handles ping/pong, and mounts Ink tree over a Unix socket", async () => {
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "ultty-ts-"));
    const sockPath = path.join(tmpDir, "ultty.sock");
    const received: AppMessage[] = [];

    const server = net.createServer((conn) => {
      let buf = new Uint8Array(0);
      conn.on("data", (chunk: Buffer) => {
        const next = new Uint8Array(buf.length + chunk.length);
        next.set(buf, 0);
        next.set(chunk, buf.length);
        buf = next;

        while (true) {
          const len = peekFrameLength(buf);
          if (len === null) break;
          const frame = buf.subarray(0, len);
          buf = buf.subarray(len);
          const { message } = decodeAppMessage(frame);
          received.push(message);

          if (message.type === "client_hello") {
            conn.write(
              encodeTerminalMessage({
                type: "server_hello",
                version: PROTOCOL_VERSION,
                capabilities:
                  CAP_LAYOUT_FLEXBOX |
                  CAP_APPEND_CONTENT |
                  CAP_REGION_METADATA |
                  CAP_INLINE_SURFACES |
                  CAP_SEMANTICS,
              }),
            );
            conn.write(
              encodeTerminalMessage({
                type: "session_ready",
                sessionId: 99n,
                rootRegion: ROOT_REGION,
                width: 100,
                height: 30,
              }),
            );
            // Send a keepalive Ping to verify automatic Pong response.
            conn.write(encodeTerminalMessage({ type: "ping" }));
          } else if (message.type === "end_frame") {
            conn.write(
              encodeTerminalMessage({
                type: "layout_computed",
                frameId: message.frameId,
                trigger: LayoutTrigger.TreeChange,
                regions: [],
              }),
            );
          }
        }
      });
    });

    await new Promise<void>((resolve) => server.listen(sockPath, resolve));
    try {
      const tree = Box(
        { id: "root", flexDirection: "column", padding: 1 },
        Text("Header", { id: "hdr", bold: true, color: "cyan" }),
        Box(
          {
            id: "log",
            borderStyle: "single",
            overflowY: "scroll",
            role: Role.Log,
            label: "Logs",
            live: Live.Polite,
          },
          Text("Line 1\nLine 2"),
        ),
      );

      const app = await render(tree, { socketPath: sockPath });
      assert.equal(app.mode, "native");
      assert.ok(app.client instanceof UlttyClient);
      assert.equal(app.client.sessionId, 99n);
      assert.equal(app.client.viewportWidth, 100);
      assert.equal(app.client.viewportHeight, 30);

      await app.rerender(Box({ id: "root" }, Text("Updated")));
      app.unmount();

      await new Promise((r) => setTimeout(r, 30));
      const types = received.map((m) => m.type);
      assert.ok(types.includes("client_hello"));
      assert.ok(types.includes("pong"));
      assert.ok(types.includes("create_region"));
      assert.ok(types.includes("set_content"));
      assert.ok(types.includes("set_region_metadata"));
      assert.ok(types.includes("set_semantics"));
      assert.ok(types.includes("end_frame"));
      assert.ok(types.includes("delete_region"));
      assert.ok(types.includes("disconnect"));
    } finally {
      server.close();
      fs.rmSync(tmpDir, { recursive: true, force: true });
    }
  });

  it("falls back cleanly to styled ANSI output outside Ultty", async () => {
    const chunks: string[] = [];
    const output = new Writable({
      write(chunk, _enc, cb) {
        chunks.push(String(chunk));
        cb();
      },
    });

    const node = Box(
      { flexDirection: "column", borderStyle: "round", borderColor: "cyan" },
      Text("Agent Output", { bold: true, color: "green" }),
      Text("All checks passed"),
    );

    const app = await render(node, { forceAnsi: true, output, columns: 40 });
    assert.equal(app.mode, "ansi");
    assert.equal(app.client, null);
    app.unmount();

    const rendered = chunks.join("");
    assert.ok(rendered.includes("╭"));
    assert.ok(rendered.includes("Agent Output"));
    assert.ok(rendered.includes("All checks passed"));
    assert.ok(renderAnsi(node, 40).includes("╰"));
  });
});
