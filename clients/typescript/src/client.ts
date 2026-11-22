/**
 * Async Unix-socket client for the Ultty native terminal protocol.
 */

import { EventEmitter } from "node:events";
import net from "node:net";
import process from "node:process";

import {
  decodeTerminalMessage,
  encodeAppMessage,
  peekFrameLength,
  ProtocolError,
} from "./codec.ts";
import {
  CLIENT_DEFAULT_CAPABILITIES,
  EVENT_KEY,
  EVENT_MOUSE,
  EVENT_SCROLL,
  PROTOCOL_VERSION,
  ROOT_REGION,
  negotiateProtocolVersion,
  type AppMessage,
  type ComputedRegionLayout,
  type Constraints,
  type Content,
  type Semantics,
  type Style,
  type TerminalMessage,
} from "./types.ts";

export interface ConnectOptions {
  socketPath?: string;
  capabilities?: number;
  timeoutMs?: number;
}

interface PendingWaiter {
  predicate: (msg: TerminalMessage) => boolean;
  resolve: (msg: TerminalMessage) => void;
  reject: (err: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

/**
 * Detect whether `ULTTY_SOCKET` is available for this process.
 * Returns null inside multiplexers (`TMUX` or `STY`) where the socket belongs
 * to an outer terminal.
 */
export function detectSocketPath(): string | null {
  if (process.env.TMUX || process.env.STY) return null;
  const path = process.env.ULTTY_SOCKET;
  return path && path.length > 0 ? path : null;
}

export class UlttyClient extends EventEmitter {
  private readonly socket: net.Socket;
  private readBuf: Uint8Array = new Uint8Array(0);
  private waiters: PendingWaiter[] = [];
  private handshakeQueue: TerminalMessage[] = [];
  private handshakeComplete = false;
  private nextRegionId = 1;
  private nextFrameId = 0;
  private closed = false;

  sessionId = 0n;
  rootRegion = ROOT_REGION;
  viewportWidth = 80;
  viewportHeight = 24;
  negotiatedVersion = PROTOCOL_VERSION;
  negotiatedCapabilities = 0;

  private constructor(socket: net.Socket) {
    super();
    this.socket = socket;
    this.socket.on("data", (chunk: Buffer) => this.onData(chunk));
    this.socket.on("close", () => this.onClose());
    this.socket.on("error", (err) => this.failAll(err));
  }

  /**
   * Connect to Ultty via `options.socketPath` or `ULTTY_SOCKET` and complete
   * the `ClientHello` / `ServerHello` / `SessionReady` handshake.
   */
  static async connect(options: ConnectOptions = {}): Promise<UlttyClient> {
    const socketPath = options.socketPath ?? detectSocketPath();
    if (!socketPath) {
      throw new Error("ULTTY_SOCKET is not set");
    }
    const timeoutMs = options.timeoutMs ?? 5000;
    const requestedCaps = options.capabilities ?? CLIENT_DEFAULT_CAPABILITIES;

    const socket = await new Promise<net.Socket>((resolve, reject) => {
      const s = net.createConnection({ path: socketPath });
      const timer = setTimeout(() => {
        s.destroy();
        reject(new Error(`timed out connecting to ${socketPath}`));
      }, timeoutMs);
      s.once("connect", () => {
        clearTimeout(timer);
        resolve(s);
      });
      s.once("error", (err) => {
        clearTimeout(timer);
        reject(err);
      });
    });

    const client = new UlttyClient(socket);
    try {
      client.send({
        type: "client_hello",
        version: PROTOCOL_VERSION,
        requestedCapabilities: requestedCaps,
      });

      const hello = await client.waitFor(
        (m): m is Extract<TerminalMessage, { type: "server_hello" }> =>
          m.type === "server_hello",
        timeoutMs,
      );
      const negotiated = negotiateProtocolVersion(
        PROTOCOL_VERSION,
        hello.version,
      );
      if (negotiated === null) {
        throw new ProtocolError(
          "invalid_data",
          `unsupported server protocol version ${hello.version}`,
        );
      }
      client.negotiatedVersion = negotiated;
      client.negotiatedCapabilities = hello.capabilities & requestedCaps;

      const ready = await client.waitFor(
        (m): m is Extract<TerminalMessage, { type: "session_ready" }> =>
          m.type === "session_ready",
        timeoutMs,
      );
      client.sessionId = ready.sessionId;
      client.rootRegion = ready.rootRegion;
      client.viewportWidth = ready.width;
      client.viewportHeight = ready.height;
      client.handshakeComplete = true;
      for (const queued of client.handshakeQueue.splice(0)) {
        client.emit("message", queued);
        client.emit(queued.type, queued);
      }
      return client;
    } catch (err) {
      client.close();
      throw err;
    }
  }

  isClosed(): boolean {
    return this.closed;
  }

  supportsCapability(bit: number): boolean {
    return (this.negotiatedCapabilities & bit) !== 0;
  }

  allocRegionId(): number {
    return this.nextRegionId++;
  }

  send(msg: AppMessage): void {
    if (this.closed) {
      throw new Error("UlttyClient connection is closed");
    }
    this.socket.write(encodeAppMessage(msg));
  }

  beginFrame(): number {
    const frameId = ++this.nextFrameId;
    this.send({ type: "begin_frame", frameId });
    return frameId;
  }

  async endFrame(
    frameId: number,
    timeoutMs = 5000,
  ): Promise<ComputedRegionLayout[]> {
    const replyPromise = this.waitFor(
      (m): m is Extract<TerminalMessage, { type: "layout_computed" }> =>
        m.type === "layout_computed",
      timeoutMs,
    );
    this.send({ type: "end_frame", frameId });
    const reply = await replyPromise;
    return reply.regions;
  }

  createRegion(
    parent: number,
    constraints: Constraints,
    style: Style,
    insertIndex = -1,
  ): number {
    const id = this.allocRegionId();
    this.send({
      type: "create_region",
      id,
      parent,
      insertIndex,
      constraints,
      style,
    });
    return id;
  }

  setConstraints(id: number, constraints: Constraints): void {
    this.send({ type: "set_constraints", id, constraints });
  }

  setStyle(id: number, style: Style): void {
    this.send({ type: "set_style", id, style });
  }

  setContent(id: number, content: Content): void {
    this.send({ type: "set_content", id, content });
  }

  appendContent(id: number, text: string): void {
    this.send({ type: "append_content", id, text });
  }

  setMetadata(id: number, metadata: Array<[string, string]>): void {
    this.send({ type: "set_region_metadata", id, metadata });
  }

  setSemantics(id: number, semantics: Semantics): void {
    this.send({ type: "set_semantics", id, semantics });
  }

  deleteRegion(id: number): void {
    this.send({ type: "delete_region", id });
  }

  scroll(id: number, x: number, y: number, animate = false): void {
    this.send({ type: "scroll", id, x, y, animate });
  }

  focus(id: number): void {
    this.send({ type: "focus", id });
  }

  blur(): void {
    this.send({ type: "blur" });
  }

  subscribe(
    id: number = this.rootRegion,
    events: number = EVENT_KEY | EVENT_MOUSE | EVENT_SCROLL,
  ): void {
    this.send({ type: "subscribe", id, events });
  }

  unsubscribe(id: number, events: number): void {
    this.send({ type: "unsubscribe", id, events });
  }

  disconnect(): void {
    if (!this.closed) {
      try {
        this.send({ type: "disconnect" });
      } catch {
        // Ignore write error while tearing down.
      }
      this.close();
    }
  }

  close(): void {
    if (!this.closed) {
      this.closed = true;
      this.socket.end();
      this.socket.destroy();
    }
  }

  private waitFor<T extends TerminalMessage>(
    predicate: (msg: TerminalMessage) => msg is T,
    timeoutMs: number,
  ): Promise<T> {
    const queuedIdx = this.handshakeQueue.findIndex(predicate);
    if (queuedIdx >= 0) {
      const [found] = this.handshakeQueue.splice(queuedIdx, 1);
      return Promise.resolve(found as T);
    }
    return new Promise<T>((resolve, reject) => {
      if (this.closed) {
        reject(new Error("UlttyClient connection is closed"));
        return;
      }
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((w) => w !== waiter);
        reject(new Error(`timed out waiting for terminal reply (${timeoutMs}ms)`));
      }, timeoutMs);
      const waiter: PendingWaiter = {
        predicate,
        resolve: (msg) => resolve(msg as T),
        reject,
        timer,
      };
      this.waiters.push(waiter);
    });
  }

  private onData(chunk: Buffer): void {
    const combined = new Uint8Array(this.readBuf.length + chunk.length);
    combined.set(this.readBuf, 0);
    combined.set(chunk, this.readBuf.length);
    this.readBuf = combined;

    while (true) {
      let frameLen: number | null;
      try {
        frameLen = peekFrameLength(this.readBuf);
      } catch (err) {
        this.failAll(err instanceof Error ? err : new Error(String(err)));
        this.close();
        return;
      }
      if (frameLen === null) return;

      const frameBytes = this.readBuf.subarray(0, frameLen);
      this.readBuf = this.readBuf.subarray(frameLen);

      let msg: TerminalMessage;
      try {
        msg = decodeTerminalMessage(frameBytes).message;
      } catch (err) {
        this.failAll(err instanceof Error ? err : new Error(String(err)));
        this.close();
        return;
      }

      if (msg.type === "ping") {
        if (!this.closed) this.send({ type: "pong" });
        continue;
      }
      if (msg.type === "resize") {
        this.viewportWidth = msg.event.width;
        this.viewportHeight = msg.event.height;
      }

      const waiterIdx = this.waiters.findIndex((w) => w.predicate(msg));
      if (waiterIdx >= 0) {
        const [waiter] = this.waiters.splice(waiterIdx, 1);
        clearTimeout(waiter.timer);
        waiter.resolve(msg);
      } else if (!this.handshakeComplete) {
        this.handshakeQueue.push(msg);
      } else {
        this.emit("message", msg);
        this.emit(msg.type, msg);
      }
    }
  }

  private onClose(): void {
    this.closed = true;
    this.failAll(new Error("connection closed"));
    this.emit("close");
  }

  private failAll(err: Error): void {
    const pending = this.waiters.splice(0);
    for (const w of pending) {
      clearTimeout(w.timer);
      w.reject(err);
    }
  }
}
