/**
 * Demo capability lab — native Pi SettingsList multi-toggle projecting onto
 * native Grok surfaces (header/footer widgets, status, title, editor text).
 * Works under the real TUI and the remote custom() host.
 */

import {
  SettingsList,
  type Component,
  type SettingItem,
  type SettingsListTheme,
} from "@earendil-works/pi-tui";
import type { RemoteTuiDemoUi } from "./shared.ts";

export const DEMO_ITEMS = [
  { key: "header", label: "Header widget", description: "aboveEditor native surface" },
  { key: "footer", label: "Footer widget", description: "belowEditor native surface" },
  { key: "status", label: "Status bar", description: "setStatus fire-and-forget" },
  { key: "title", label: "Window title", description: "setTitle fire-and-forget" },
  { key: "editor", label: "Prompt editor", description: "setEditorText fire-and-forget" },
] as const;

export type DemoKey = (typeof DEMO_ITEMS)[number]["key"];

export function applyDemoCapabilities(ui: RemoteTuiDemoUi, keys: DemoKey[]): void {
  const selected = new Set(keys);
  const labels = DEMO_ITEMS.filter((item) => selected.has(item.key)).map((item) => item.label);
  const summary = labels.length > 0 ? labels.join(", ") : "none";

  // Align with Pi setWidget semantics: plain multi-line frames above/below
  // the editor. No synthetic "Esc closes" chrome — Esc is host cancellation.
  ui.setWidget(
    "remote_tui_demo_header",
    selected.has("header")
      ? [
          "\x1b[1mRemote TUI demo header\x1b[0m",
          `\x1b[2m${summary}\x1b[0m`,
        ]
      : undefined,
    { placement: "aboveEditor" },
  );
  ui.setWidget(
    "remote_tui_demo_footer",
    selected.has("footer")
      ? [
          `\x1b[2mFooter · ${labels.length} selected: ${summary}\x1b[0m`,
        ]
      : undefined,
    { placement: "belowEditor" },
  );
  if (selected.has("status")) {
    ui.setStatus?.("remote-tui-demo", `Remote TUI demo: ${summary}`);
  } else {
    ui.setStatus?.("remote-tui-demo");
  }
  if (selected.has("title")) {
    ui.setTitle?.("Remote TUI capability lab");
  }
  if (selected.has("editor")) {
    ui.setEditorText?.("Remote TUI demo applied — type here or press Esc to close.");
  }
}

function demoSettingsTheme(theme: {
  fg: (color: string, text: string) => string;
  bold?: (text: string) => string;
}): SettingsListTheme {
  return {
    label: (text, selected) => (selected ? theme.fg("accent", text) : text),
    value: (text, selected) =>
      selected ? theme.fg("accent", text) : theme.fg("dim", text),
    description: (text) => theme.fg("dim", text),
    cursor: theme.fg("accent", "→ "),
    hint: (text) => theme.fg("dim", text),
  };
}

/** Native Pi SettingsList multi-toggle — works under real TUI and remote host. */
export function createDemoSelector(
  tui: { requestRender: () => void },
  theme: {
    fg: (color: string, text: string) => string;
    bold?: (text: string) => string;
  },
  done: (result: string | undefined) => void,
  onChange: (keys: DemoKey[]) => void,
): Component {
  const enabled = new Set<DemoKey>();
  const items: SettingItem[] = DEMO_ITEMS.map((item) => ({
    id: item.key,
    label: item.label,
    description: item.description,
    currentValue: "off",
    values: ["on", "off"],
  }));

  const selectedKeys = (): DemoKey[] =>
    DEMO_ITEMS.map((item) => item.key).filter((key) => enabled.has(key));

  const list = new SettingsList(
    items,
    DEMO_ITEMS.length + 1,
    demoSettingsTheme(theme),
    (id, newValue) => {
      if (newValue === "on") enabled.add(id as DemoKey);
      else enabled.delete(id as DemoKey);
      onChange(selectedKeys());
    },
    () => {
      const keys = selectedKeys();
      done(keys.length > 0 ? keys.join(",") : undefined);
    },
  );

  const bold = theme.bold ?? ((text: string) => text);
  return {
    invalidate() {
      list.invalidate();
    },
    render(width: number) {
      return [
        theme.fg("accent", bold("Remote TUI capability lab")),
        theme.fg("dim", "Enter/Space toggle · Esc close"),
        "",
        ...list.render(width),
      ];
    },
    handleInput(data: string) {
      list.handleInput(data);
      tui.requestRender();
    },
  };
}
