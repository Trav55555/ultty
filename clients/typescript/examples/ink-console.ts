#!/usr/bin/env node
/**
 * Ink-style agent console example.
 *
 * Renders natively via the Ultty binary wire protocol when `ULTTY_SOCKET` is
 * present, and falls back to styled ANSI output in any other terminal.
 *
 * Run with `--once` to render a single frame and exit immediately.
 */

import process from "node:process";

import { Box, Live, Role, Text, render, type InkNode } from "../src/index.ts";

function buildView(logs: string[], step: number): InkNode {
  return Box(
    {
      id: "root",
      flexDirection: "column",
      padding: 1,
      gap: 1,
    },
    Box(
      {
        id: "header",
        borderStyle: "round",
        borderColor: "cyan",
        paddingX: 1,
        flexDirection: "row",
      },
      Text("Ultty TypeScript Console", { color: "brightCyan", bold: true }),
      Text(`  step ${step}  |  native + ANSI fallback`, { dimColor: true }),
    ),
    Box(
      {
        id: "body",
        flexDirection: "row",
        flexGrow: 1,
        gap: 1,
      },
      Box(
        {
          id: "transcript",
          borderStyle: "single",
          borderColor: "blue",
          paddingX: 1,
          flexGrow: 1,
          overflowY: "scroll",
          role: Role.Log,
          label: "Agent Transcript",
          live: Live.Polite,
        },
        Text(logs.join("\n"), { wrap: "word" }),
      ),
      Box(
        {
          id: "sidebar",
          width: 24,
          borderStyle: "single",
          borderColor: "gray",
          paddingX: 1,
          flexDirection: "column",
        },
        Text("Status", { color: "yellow", bold: true }),
        Text("Mode: connected", { color: "green" }),
        Text(`Lines: ${logs.length}`, { dimColor: true }),
      ),
    ),
    Box(
      {
        id: "footer",
        paddingX: 1,
      },
      Text("Press Ctrl+C to exit", { dimColor: true }),
    ),
  );
}

async function main(): Promise<void> {
  const once = process.argv.includes("--once");
  const forceAnsi = process.argv.includes("--ansi");
  const logs = [
    "[info] Connected to workspace",
    "[tool] cargo test -p ultty-protocol",
    "[ok]   11/11 v1 wire conformance vectors verified",
  ];

  const app = await render(buildView(logs, 1), { forceAnsi });

  if (once) {
    app.unmount();
    return;
  }

  let step = 1;
  const interval = setInterval(() => {
    step += 1;
    logs.push(`[tick] streaming update #${step}`);
    void app.rerender(buildView(logs, step));
    if (step >= 5) {
      clearInterval(interval);
    }
  }, 1000);

  const shutdown = (): void => {
    clearInterval(interval);
    app.unmount();
    process.exit(0);
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

void main();
