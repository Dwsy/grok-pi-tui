import { expect, test } from "bun:test";
import { appendFileSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { TuiMouseEvent } from "@earendil-works/pi-tui";
import { drainKeys, metaPath } from "./transport.ts";
import type { ActiveHost } from "./shared.ts";

test("key transport buffers partial records and ignores stale component input/cancel", () => {
  const dir = mkdtempSync(join(tmpdir(), "remote-tui-transport-test-"));
  const keysPath = join(dir, "keys.jsonl");
  const inputs: string[] = [];
  const mouseEvents: TuiMouseEvent[] = [];
  const host = {
    id: "current", keysPath, keyOffset: 0, keyBuffer: Buffer.alloc(0), closed: false,
    close: () => { host.closed = true; },
    handleInput: (data: string) => inputs.push(data),
    handleMouse: (event: TuiMouseEvent) => mouseEvents.push(event),
  } as ActiveHost;
  try {
    const partial = '{"id":"current","op":"input","data":"a"';
    writeFileSync(keysPath, partial);
    drainKeys(host);
    expect(host.keyOffset).toBe(Buffer.byteLength(partial));
    expect(host.keyBuffer.toString("utf8")).toBe(partial);
    appendFileSync(keysPath, '}\n');
    drainKeys(host);
    expect(inputs).toEqual(["a"]);
    appendFileSync(keysPath, [
      { id: "old", op: "input", data: "s" },
      { id: "old", op: "cancel" },
      { id: "current", op: "input", data: "\x1b" },
    ].map((event) => JSON.stringify(event) + "\n").join(""));
    drainKeys(host);
    expect(host.closed).toBe(false);
    expect(inputs).toEqual(["a", "\x1b"]);
    const mouse = (type: TuiMouseEvent["type"], x: number, wheelDelta?: number): TuiMouseEvent => ({
      type,
      button: type === "wheel" ? "none" : "left",
      x,
      y: 1,
      screenX: x,
      screenY: 1,
      width: 80,
      height: 24,
      shift: false,
      alt: false,
      ctrl: false,
      ...(wheelDelta === undefined ? {} : { wheelDelta }),
    });
    appendFileSync(keysPath, [
      { id: "old", op: "mouse", event: mouse("press", 1) },
      { id: "current", op: "mouse", event: mouse("move", 2) },
      { id: "current", op: "mouse", event: mouse("move", 4) },
      { id: "current", op: "mouse", event: mouse("wheel", 4, -1) },
    ].map((event) => JSON.stringify(event) + "\n").join(""));
    drainKeys(host);
    expect(mouseEvents.map((event) => [event.type, event.x, event.wheelDelta])).toEqual([
      ["move", 4, undefined],
      ["wheel", 4, -1],
    ]);
    appendFileSync(keysPath, JSON.stringify({ id: "current", op: "cancel" }) + "\n");
    drainKeys(host);
    expect(host.closed).toBe(true);
  } finally {
    rmSync(dir, { recursive: true });
  }
});

test("instance metadata path overrides the legacy shared file", () => {
  const previous = process.env.PI_GROK_REMOTE_TUI_META;
  try {
    process.env.PI_GROK_REMOTE_TUI_META = "/instance-one/active.json";
    expect(metaPath()).toBe("/instance-one/active.json");
    process.env.PI_GROK_REMOTE_TUI_META = "/instance-two/active.json";
    expect(metaPath()).toBe("/instance-two/active.json");
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI_META;
    else process.env.PI_GROK_REMOTE_TUI_META = previous;
  }
});
