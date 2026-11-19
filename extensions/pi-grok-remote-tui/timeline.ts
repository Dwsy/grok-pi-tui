/** Timeline configuration and tool formatting over the grok-pi extension bridge. */
import { randomUUID } from "node:crypto";
import { mkdirSync, readdirSync, readFileSync, rmSync, unlinkSync, watch } from "node:fs";
import { dirname, join } from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import {
  Input, SelectList, SettingsList, matchesKey, truncateToWidth,
  type Component, type SettingItem, type TuiMouseEvent, type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import { ensureRemoteTuiHost, type installCustomPatch } from "./host.ts";

import { TIMELINE_TOOL_GROUPS, formatTimelineBatch, plain, type FormatRequest } from "./timeline-format.ts";

const STATUS_KEY = "__pi_grok_timeline__";
const FILE_TOOLS = ["edit", "write"];

export interface TimelineContents {
  user_prompts: boolean;
  compactions: boolean;
  tools: string[];
}

const defaults = (): TimelineContents => ({ user_prompts: true, compactions: true, tools: [] });

interface ConfigReply {
  op: "config";
  host: string;
  id: string;
  contents: TimelineContents;
  error?: string;
}

function openBridge(ui: ExtensionContext["ui"], metaPath: string) {
  const host = randomUUID();
  const directory = join(dirname(metaPath), `timeline-${host}`);
  mkdirSync(directory, { mode: 0o700 });
  const publish = (message: object) => ui.setStatus(STATUS_KEY, JSON.stringify(message));
  const pending = new Map<string, (reply?: ConfigReply) => void>();
  const drain = () => {
    for (const name of readdirSync(directory)) {
      if (!name.endsWith(".json")) continue;
      try {
        const path = join(directory, name);
        const data = readFileSync(path, "utf8");
        unlinkSync(path);
        const message = JSON.parse(data) as ConfigReply | FormatRequest;
        if (message.op === "config" && message.host === host) {
          pending.get(message.id)?.(message);
        } else if (message.op === "format" && message.scope.host === host) {
          publish(formatTimelineBatch(message));
        }
      } catch {
        // A malformed request has no effect on the panel or Pager fallback.
      }
    }
  };
  const watcher = watch(directory, { persistent: false }, () => {
    try {
      drain();
    } catch {
      // The extension may have been shut down before the callback ran.
    }
  });
  watcher.on("error", () => {
    for (const complete of pending.values()) complete();
  });
  const ready = () => publish({ op: "ready", host });
  return {
    ready,
    request(op: "get" | "save", contents?: TimelineContents): Promise<TimelineContents> {
      const id = randomUUID();
      return new Promise((resolve, reject) => {
        const complete = (reply?: ConfigReply) => {
          clearTimeout(timer);
          pending.delete(id);
          if (!reply) reject(new Error("Pager did not answer the Timeline request."));
          else if (reply.error) reject(new Error(reply.error));
          else resolve(reply.contents);
        };
        const timer = setTimeout(complete, 5000);
        pending.set(id, complete);
        publish({ op, host, id, contents });
      });
    },
    dispose() {
      watcher.close();
      for (const complete of pending.values()) complete();
      rmSync(directory, { recursive: true, force: true });
    },
  };
}

type PanelTheme = Parameters<Parameters<ExtensionContext["ui"]["custom"]>[0]>[1];

export function createTimelinePanel(
  tui: { requestRender(): void; terminal: { rows: number } },
  theme: PanelTheme,
  initial: TimelineContents,
  toolNames: readonly string[],
  save: (contents: TimelineContents) => Promise<TimelineContents>,
  done: (saved: boolean | undefined) => void,
): Component & { dispose(): void } {
  let draft = { ...initial, tools: [...initial.tools] };
  let saving = false;
  let closed = false;
  let error: string | undefined;
  let input: Input | undefined;
  let suggestions: SelectList | undefined;
  let list: SettingsList;
  let listRows = 0;
  let suggestionRows = 0;
  const maxRows = (reserved: number) => Math.max(1, Math.min(10, tui.terminal.rows - reserved));
  const accent = (text: string) => theme.fg("accent", text);
  const dim = (text: string) => theme.fg("dim", text);

  function close(saved?: boolean) {
    closed = true;
    done(saved);
  }

  function saveDraft() {
    saving = true;
    error = undefined;
    tui.requestRender();
    void save(draft).then(
      () => { if (!closed) close(true); },
      (reason: unknown) => {
        if (closed) return;
        saving = false;
        error = plain(reason instanceof Error ? reason.message : String(reason));
        tui.requestRender();
      },
    );
  }

  function acceptTool(name: string) {
    if (!name) return;
    if (!draft.tools.includes(name)) draft.tools.push(name);
    input = undefined;
    suggestions = undefined;
    const group = TIMELINE_TOOL_GROUPS.find((item) => item.tools.includes(name));
    const selected = FILE_TOOLS.includes(name)
      ? name
      : group?.tools.every((tool) => draft.tools.includes(tool)) ? group.id : `tool:${name}`;
    buildList(selected);
  }

  function updateSuggestions() {
    const name = input?.getValue().trim() ?? "";
    const candidates = toolNames.filter((tool) => tool !== name && !draft.tools.includes(tool)
      && tool.toLowerCase().includes(name.toLowerCase()));
    const items = [
      ...(name ? [{ value: name, label: name, description: "Use this name" }] : []),
      ...candidates.map((tool) => ({ value: tool, label: plain(tool) })),
    ];
    suggestions = new SelectList(items, maxRows(7), {
      selectedPrefix: accent,
      selectedText: accent,
      description: dim,
      scrollInfo: dim,
      noMatch: dim,
    }, { maxPrimaryColumnWidth: 48 });
    suggestions.onSelect = (item) => acceptTool(item.value);
  }

  function buildList(selection?: string) {
    const toggle = (id: string, label: string, active: boolean): SettingItem =>
      ({ id, label, currentValue: active ? "on" : "off", values: ["on", "off"] });
    const groupItems: SettingItem[] = TIMELINE_TOOL_GROUPS.map(({ id, label, tools }) => {
      const count = tools.filter((tool) => draft.tools.includes(tool)).length;
      return {
        id, label, values: ["on", "off"],
        currentValue: count === tools.length ? "on" : count > 0 ? `${count}/${tools.length}` : "off",
      };
    });
    const bundled = new Set([
      ...FILE_TOOLS,
      ...TIMELINE_TOOL_GROUPS.filter((group) => group.tools.every((tool) => draft.tools.includes(tool)))
        .flatMap((group) => group.tools),
    ]);
    const items: SettingItem[] = [
      toggle("user_prompts", "User prompts", draft.user_prompts),
      toggle("compactions", "Context compactions", draft.compactions),
      ...FILE_TOOLS.map((tool) => toggle(tool, `${tool} · prompt summary`, draft.tools.includes(tool))),
      ...groupItems,
      ...draft.tools.filter((tool) => !bundled.has(tool)).map((tool) => ({
        id: `tool:${tool}`, label: tool, currentValue: "Remove", values: ["Remove"],
      })),
      { id: "add", label: "Add tool", currentValue: "", values: [""] },
      { id: "save", label: "Save", currentValue: "", values: [""] },
      { id: "defaults", label: "Restore defaults", currentValue: "", values: [""] },
    ];
    list = new SettingsList(items, maxRows(7), {
      label: (text, selected) => selected ? accent(text) : text,
      value: (text, selected) => selected ? accent(text) : dim(text),
      description: dim,
      cursor: accent("→ "),
      hint: dim,
    }, (id, value) => {
      if (id === "user_prompts" || id === "compactions") {
        draft[id] = value === "on";
      } else if (FILE_TOOLS.includes(id)) {
        if (value === "on") draft.tools.push(id);
        else draft.tools = draft.tools.filter((tool) => tool !== id);
      } else if (id.startsWith("tool:")) {
        draft.tools = draft.tools.filter((tool) => tool !== id.slice(5));
        buildList("add");
      } else if (id === "add") {
        input = new Input();
        input.focused = true;
        updateSuggestions();
      } else if (id === "save") {
        saveDraft();
      } else if (id === "defaults") {
        draft = defaults();
        error = undefined;
        buildList("save");
      } else {
        const group = TIMELINE_TOOL_GROUPS.find((item) => item.id === id);
        if (group) {
          draft.tools = value === "on"
            ? [...new Set([...draft.tools, ...group.tools])]
            : draft.tools.filter((tool) => !group.tools.includes(tool));
          buildList(id);
        }
      }
    }, close);
    if (selection) list.selectItem(selection);
  }
  buildList();

  function listMouse(target: SettingsList | SelectList, event: TuiMouseEvent): TuiMouseEventResult | undefined {
    // Remote TUI synthesizes click only after a handled press. Leave selection
    // alone on press so a windowed list cannot shift out from under the pointer.
    if (event.type === "press" && event.button === "left") return { handled: true, render: false };
    if (event.type === "click") return target.handleMouse?.(event);
    if (event.type !== "wheel" || !event.wheelDelta) return undefined;
    let render = false;
    const step = { ...event, wheelDelta: Math.sign(event.wheelDelta) };
    for (let i = 0; i < Math.min(32, Math.abs(event.wheelDelta)); i++) {
      render = target.handleMouse?.(step)?.render === true || render;
    }
    return { handled: true, render };
  }

  return {
    invalidate() {
      list.invalidate();
      input?.invalidate();
      suggestions?.invalidate();
    },
    render(width) {
      const title = accent(theme.bold(input ? "Add Timeline tool" : "Timeline contents"));
      const hint = input ? "Type a tool name or choose a suggestion" : "Enter/Space select · Esc discard";
      if (input) {
        const candidates = suggestions?.render(width) ?? [];
        suggestionRows = candidates.length;
        return [title, dim(hint), ...input.render(width), ...candidates];
      }
      const rows = list.render(width);
      listRows = rows.length;
      const footer = saving ? dim("Saving…") : error ? theme.fg("error", error) : "";
      return [title, dim(hint), ...rows, ...(footer ? [truncateToWidth(footer, width)] : [])];
    },
    handleInput(data) {
      if (saving) return;
      if (matchesKey(data, "escape")) close();
      else if (!input) list.handleInput(data);
      else if ((["up", "down", "pageUp", "pageDown", "enter"] as const).some((key) => matchesKey(data, key))) {
        suggestions?.handleInput(data);
      } else {
        const before = input.getValue();
        input.handleInput(data);
        if (input.getValue() !== before) updateSuggestions();
      }
      tui.requestRender();
    },
    handleMouse(event: TuiMouseEvent) {
      if (saving) return { handled: true };
      if (event.y < 2) return undefined;
      if (input) {
        if (event.y === 2) {
          input.focused = true;
          return input.handleMouse?.({ ...event, y: 0 });
        }
        if (suggestions && event.y < 3 + suggestionRows) {
          return listMouse(suggestions, { ...event, y: event.y - 3, height: suggestionRows });
        }
      } else if (event.y < 2 + listRows) {
        return listMouse(list, { ...event, y: event.y - 2, height: listRows });
      }
      return undefined;
    },
    dispose() { closed = true; },
  };
}

export function installTimeline(pi: ExtensionAPI): void {
  const metaPath = process.env.PI_GROK_REMOTE_TUI_META;
  if (!metaPath) return;
  let bridge: ReturnType<typeof openBridge> | undefined;
  let bridgeUi: ExtensionContext["ui"] | undefined;
  const ensureBridge = (ui: ExtensionContext["ui"]) => {
    if (!bridge || bridgeUi !== ui) {
      bridge?.dispose();
      bridge = openBridge(ui, metaPath);
      bridgeUi = ui;
      bridge.ready();
    }
    return bridge;
  };
  pi.on("session_start", (_event, ctx) => { ensureBridge(ctx.ui); });
  pi.on("session_shutdown", () => {
    bridge?.dispose();
    bridge = undefined;
    bridgeUi = undefined;
  });
  pi.registerCommand("timeline-config", {
    description: "Choose which entries appear in the Timeline sidebar",
    handler: async (_args, ctx) => {
      ensureRemoteTuiHost(ctx.ui as Parameters<typeof installCustomPatch>[0]);
      try {
        const currentBridge = ensureBridge(ctx.ui);
        currentBridge.ready();
        const contents = await currentBridge.request("get");
        const tools = [...new Set(pi.getAllTools().map(({ name }) => name))];
        await ctx.ui.custom<boolean | undefined>((tui, theme, _keys, done) =>
          createTimelinePanel(tui, theme, contents, tools, (rules) => currentBridge.request("save", rules), done),
        { overlay: true });
      } catch (error) {
        ctx.ui.notify(`Timeline configuration: ${error instanceof Error ? error.message : String(error)}`, "error");
      }
    },
  });
}
