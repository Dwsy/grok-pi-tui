/**
 * Keyfile transport and adapter meta — the side channel between this extension
 * and the grok-pi adapter (keys in, layout metadata out). Not Pi RPC.
 */

import { closeSync, existsSync, openSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir as osTmpdir } from "node:os";
import { join } from "node:path";
import type { ActiveHost, RemoteTuiDemoUi, RemoteTuiLayout } from "./shared.ts";
import { LAYOUT_WIDGET_KEY, META_NAME } from "./shared.ts";

export function metaPath(): string {
  return process.env.PI_GROK_REMOTE_TUI_META || join(osTmpdir(), META_NAME);
}

export function writeMeta(meta: { id: string; keysPath: string } | null): void {
  const path = metaPath();
  try {
    if (meta === null) {
      if (existsSync(path)) unlinkSync(path);
      return;
    }
    writeFileSync(path, JSON.stringify(meta), "utf8");
  } catch {
    /* ignore */
  }
}

export function publishRemoteTuiLayout(ui: RemoteTuiDemoUi, layout: RemoteTuiLayout | undefined): void {
  ui.setWidget(
    LAYOUT_WIDGET_KEY,
    layout ? [JSON.stringify(layout)] : undefined,
  );
}

export function ensureKeyFile(path: string): void {
  try {
    closeSync(openSync(path, "a"));
  } catch {
    /* ignore */
  }
}

export function drainKeys(host: ActiveHost): void {
  if (host.closed) return;
  try {
    if (!existsSync(host.keysPath)) return;
    const buf = readFileSync(host.keysPath, "utf8");
    if (buf.length <= host.keyOffset) return;
    // fs.watch can fire mid-write. Only consume complete JSONL records.
    const end = buf.lastIndexOf("\n") + 1;
    if (end <= host.keyOffset) return;
    const chunk = buf.slice(host.keyOffset, end);
    host.keyOffset = end;
    for (const line of chunk.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      let msg: { id?: string; op?: string; data?: string };
      try {
        msg = JSON.parse(trimmed) as typeof msg;
      } catch {
        continue;
      }
      if (host.closed) return;
      if (msg.id !== host.id) continue;
      if (msg.op === "cancel") {
        host.close(undefined);
        return;
      }
      if (msg.op === "input" && typeof msg.data === "string") {
        host.handleInput(msg.data);
      }
    }
  } catch {
    /* ignore */
  }
}
