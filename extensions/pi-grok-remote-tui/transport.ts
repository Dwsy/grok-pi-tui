/**
 * Keyfile transport and adapter meta — the side channel between this extension
 * and the grok-pi adapter (keys in, layout metadata out). Not Pi RPC.
 */

import {
  closeSync,
  existsSync,
  fstatSync,
  openSync,
  readFileSync,
  readSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir as osTmpdir } from "node:os";
import { join } from "node:path";
import type { TuiMouseEvent } from "@earendil-works/pi-tui";
import type { ActiveHost, RemoteTuiDemoUi, RemoteTuiLayout } from "./shared.ts";
import { LAYOUT_WIDGET_KEY, META_NAME } from "./shared.ts";

function isMouseEvent(value: unknown): value is TuiMouseEvent {
  if (!value || typeof value !== "object") return false;
  const event = value as Record<string, unknown>;
  return (
    ["press", "release", "move", "drag", "click", "wheel"].includes(String(event.type)) &&
    ["left", "middle", "right", "none"].includes(String(event.button)) &&
    ["x", "y", "screenX", "screenY", "width", "height"].every(
      (key) => typeof event[key] === "number" && Number.isFinite(event[key]),
    ) &&
    ["shift", "alt", "ctrl"].every((key) => typeof event[key] === "boolean") &&
    (event.wheelDelta === undefined || typeof event.wheelDelta === "number") &&
    (event.clickCount === undefined || typeof event.clickCount === "number")
  );
}

export function metaPath(): string {
  return process.env.PI_GROK_REMOTE_TUI_META || join(osTmpdir(), META_NAME);
}

export function viewportPath(): string {
  return `${metaPath()}.viewport.json`;
}

export function readLatestViewport(): { cols: number; rows: number } | undefined {
  try {
    const path = viewportPath();
    if (!existsSync(path)) return undefined;
    const parsed = JSON.parse(readFileSync(path, "utf8")) as { cols?: unknown; rows?: unknown };
    const cols = Number(parsed?.cols);
    const rows = Number(parsed?.rows);
    if (Number.isFinite(cols) && cols > 0 && Number.isFinite(rows) && rows > 0) {
      return { cols: Math.floor(cols), rows: Math.floor(rows) };
    }
  } catch {
    /* ignore */
  }
  return undefined;
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
    const fd = openSync(host.keysPath, "r");
    let chunk = Buffer.alloc(0);
    try {
      const size = fstatSync(fd).size;
      if (size < host.keyOffset) {
        // A host may recreate the per-session file after a transient restart.
        host.keyOffset = 0;
        host.keyBuffer = Buffer.alloc(0);
      }
      const remaining = size - host.keyOffset;
      if (remaining <= 0) return;
      const buffer = Buffer.alloc(remaining);
      const bytesRead = readSync(fd, buffer, 0, remaining, host.keyOffset);
      host.keyOffset += bytesRead;
      chunk = Buffer.concat([host.keyBuffer ?? Buffer.alloc(0), buffer.subarray(0, bytesRead)]);
    } finally {
      closeSync(fd);
    }
    // Keep an incomplete tail separately and read only newly appended bytes on
    // the next poll; mouse motion can otherwise turn every poll into an O(n)
    // read of the whole session history.
    const end = chunk.lastIndexOf(0x0a) + 1;
    host.keyBuffer = Buffer.from(chunk.subarray(end));
    if (end === 0) return;
    const messages: Array<{
      id?: string;
      op?: string;
      data?: string;
      event?: unknown;
      cols?: number;
      rows?: number;
    }> = [];
    for (const line of chunk.subarray(0, end).toString("utf8").split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        messages.push(JSON.parse(trimmed) as (typeof messages)[number]);
      } catch {
        continue;
      }
    }
    for (let index = 0; index < messages.length; index++) {
      if (host.closed) return;
      let msg = messages[index];
      // Coalesce consecutive hover `move` or same-button `drag` bursts within a
      // single drain to their latest coordinates while keeping press/release/click
      // boundaries strictly ordered.
      if (
        msg.op === "mouse" &&
        isMouseEvent(msg.event) &&
        (msg.event.type === "move" || msg.event.type === "drag")
      ) {
        const kind = msg.event.type;
        const btn = msg.event.button;
        while (index + 1 < messages.length) {
          const next = messages[index + 1];
          if (!next) break;
          const nextEvent = next.event;
          if (
            next.id !== msg.id ||
            next.op !== "mouse" ||
            !isMouseEvent(nextEvent) ||
            nextEvent.type !== kind ||
            (kind === "drag" && nextEvent.button !== btn)
          ) {
            break;
          }
          msg = next;
          index++;
        }
      }
      if (msg.id !== host.id) continue;
      if (msg.op === "cancel") {
        host.close(undefined);
        return;
      }
      if (msg.op === "resize") {
        while (index + 1 < messages.length) {
          const next = messages[index + 1];
          if (!next || next.id !== msg.id || next.op !== "resize") break;
          msg = next;
          index++;
        }
        const cols = Number(msg.cols);
        const rows = Number(msg.rows);
        if (Number.isFinite(cols) && cols > 0 && Number.isFinite(rows) && rows > 0) {
          host.handleResize(Math.floor(cols), Math.floor(rows));
        }
        continue;
      }
      if (msg.op === "input" && typeof msg.data === "string") {
        host.handleInput(msg.data);
      }
      if (msg.op === "mouse" && isMouseEvent(msg.event)) {
        host.handleMouse(msg.event);
      }
    }
  } catch {
    /* ignore */
  }
}
