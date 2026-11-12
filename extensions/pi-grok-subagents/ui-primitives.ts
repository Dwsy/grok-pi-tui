/**
 * Pi-tui full-screen configuration primitives for subagent config UIs.
 *
 * Every primitive builds one `ctx.ui.custom` component (projected into Grok
 * Pager by the Remote TUI bridge) with full keyboard and mouse support, so the
 * subagent configuration flows never fall back to chained `ctx.ui.select`
 * prompts. Component factories double as `SettingsList` submenus for one-screen
 * forms. Discovered contract facts (pi-tui 1.5.0):
 * - `SelectList` click selects and activates (`onSelect`), wheel moves selection.
 * - `SettingsList` click/Enter activates a row (cycles `values`, opens `submenu`),
 *   `enableSearch` gives a built-in filter box, `onCancel` fires on Escape, and a
 *   submenu `done(value)` writes the row value and calls `onChange`.
 * - `Input`/`Editor` accept mouse focus; `Input` submits on Enter, `Editor`
 *   exposes `getText()`/`setText()`.
 */

import {
  Container,
  Editor,
  Input,
  SelectList,
  SettingsList,
  Text,
  type Component,
  type SelectItem,
  type SelectListTheme,
  type SettingItem,
  type SettingsListTheme,
  type TUI,
  type TuiMouseEvent,
  type TuiMouseEventResult,
} from "@earendil-works/pi-tui";

/** Result of delegating a mouse event to a nested child (pi-tui container shape). */
type MouseDispatchResult = NonNullable<ReturnType<Container["handleMouse"]>>;
import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

export type UiTheme = {
  fg: (color: string, text: string) => string;
  bold?: (text: string) => string;
};

export type UiTui = {
  requestRender: () => void;
  terminal?: { columns: number; rows: number };
};

type Done<T> = (value: T | undefined) => void;

type Factory<T> = (tui: UiTui, theme: UiTheme, keyboard: unknown, done: (value: T | undefined) => void) => Component;

/** Styled text helpers derived from the host theme. */
export function uiStyles(theme: UiTheme) {
  const bold = theme.bold ?? ((text: string) => text);
  return {
    accent: (text: string) => theme.fg("accent", text),
    dim: (text: string) => theme.fg("dim", text),
    bold,
    title: (text: string) => theme.fg("accent", bold(text)),
    hint: (text: string) => theme.fg("dim", text),
  };
}

export function selectListTheme(theme: UiTheme): SelectListTheme {
  return {
    selectedPrefix: (text) => theme.fg("accent", text),
    selectedText: (text) => theme.fg("accent", text),
    description: (text) => theme.fg("dim", text),
    scrollInfo: (text) => theme.fg("dim", text),
    noMatch: (text) => theme.fg("dim", text),
  };
}

export function settingsListTheme(theme: UiTheme): SettingsListTheme {
  return {
    label: (text, selected) => (selected ? theme.fg("accent", text) : text),
    value: (text, selected) => (selected ? theme.fg("accent", text) : theme.fg("dim", text)),
    description: (text) => theme.fg("dim", text),
    cursor: theme.fg("accent", "→ "),
    hint: (text) => theme.fg("dim", text),
  };
}

/**
 * Vertical stack of children with mouse hit-testing (pi-tui `Container`) and
 * keyboard routing to a designated child. Mouse `focus` results re-target the
 * keyboard to the clicked child so clicking an input field types into it.
 */
export class Panel implements Component {
  readonly container = new Container();
  private keyTarget?: Component & { handleInput: (data: string) => void };

  constructor(children: Component[], keyTarget?: Component & { handleInput: (data: string) => void }) {
    for (const child of children) this.container.addChild(child);
    this.keyTarget = keyTarget;
  }

  setKeyTarget(target: Component & { handleInput: (data: string) => void }): void {
    this.keyTarget = target;
  }

  render(width: number): string[] {
    return this.container.render(width);
  }

  handleInput(data: string): void {
    this.keyTarget?.handleInput(data);
  }

  handleMouse(event: TuiMouseEvent): MouseDispatchResult | undefined {
    const result = this.container.handleMouse(event);
    const target = result?.target.component as (Component & { handleInput?: (data: string) => void }) | undefined;
    if (result?.focus && target?.handleInput) {
      this.keyTarget = target as Component & { handleInput: (data: string) => void };
    }
    return result;
  }

  invalidate(): void {
    this.container.invalidate();
  }
}

export function textLine(text: string): Text {
  return new Text(text);
}

/** Run one full-screen custom component; throws when the host has no custom() host. */
export async function openCustomPanel<T>(
  ctx: ExtensionCommandContext,
  factory: Factory<T>,
  missingMessage = CANCEL_MISSING,
): Promise<T | undefined> {
  let factoryRan = false;
  const wrapped: Factory<T> = (tui, theme, keyboard, done) => {
    factoryRan = true;
    return factory(tui, theme, keyboard, done);
  };
  const result = await (ctx.ui.custom as (factory: Factory<T>) => Promise<T | undefined>)(wrapped);
  if (!factoryRan) {
    ctx.ui.notify(missingMessage, "error");
    throw new Error(missingMessage);
  }
  return result;
}

function visibleRows(tui: UiTui): number {
  const rows = tui.terminal?.rows ?? 24;
  return Math.max(5, rows - 8);
}

function toItems(items: ReadonlyArray<string | SelectItem>): SelectItem[] {
  return items.map((item) => (typeof item === "string" ? { value: item, label: item } : item));
}

const CANCEL_MISSING = "custom() unavailable (RPC without remote host). Run under grok-pi or native Pi TUI.";

/** Single-choice list panel. `done` receives the item value or `undefined` on cancel. */
export function makeSelectPanel(
  theme: UiTheme,
  title: string,
  items: ReadonlyArray<string | SelectItem>,
  done: Done<string>,
  options: { hint?: string; maxVisible?: number } = {},
): Component {
  const styles = uiStyles(theme);
  const selectItems = toItems(items);
  const list = new SelectList(selectItems, options.maxVisible ?? 16, selectListTheme(theme));
  list.onSelect = (item) => done(item.value);
  list.onCancel = () => done(undefined);
  return new Panel(
    [
      textLine(styles.title(title)),
      textLine(styles.hint(options.hint ?? "Enter/click select · Esc cancels")),
      textLine(""),
      list,
    ],
    list,
  );
}

export function piSelect(
  ctx: ExtensionCommandContext,
  title: string,
  items: ReadonlyArray<string | SelectItem>,
  options: { hint?: string } = {},
): Promise<string | undefined> {
  return openCustomPanel<string>(ctx, (tui, theme, _keyboard, done) =>
    makeSelectPanel(theme, title, items, done, { ...options, maxVisible: visibleRows(tui) }), CANCEL_MISSING);
}

export type MultiSelectGroup = {
  label: string;
  items: Array<{ value: string; label: string; description?: string; selected?: boolean }>;
};

/**
 * Grouped checkbox panel. Rows toggle on/off; an explicit Done row confirms and
 * a Cancel row aborts. `maxSelections` reverts selections that exceed the cap.
 */
export function makeMultiSelectPanel(
  theme: UiTheme,
  title: string,
  groups: MultiSelectGroup[],
  done: Done<string[]>,
  options: { maxSelections?: number; hint?: string; doneLabel?: string; maxVisible?: number } = {},
): Component {
  const styles = uiStyles(theme);
  const selected = new Set<string>();
  for (const group of groups) for (const item of group.items) if (item.selected) selected.add(item.value);

  const status = textLine(styles.hint(`${selected.size} selected`));
  const items: SettingItem[] = [];
  for (const group of groups) {
    if (groups.length > 1) items.push({ id: `__group:${group.label}`, label: `── ${group.label} ──`, currentValue: "" });
    for (const item of group.items) {
      items.push({
        id: item.value,
        label: item.label,
        description: item.description,
        currentValue: item.selected ? "on" : "off",
        values: ["on", "off"],
      });
    }
  }
  const DONE_ID = "__done";
  const CANCEL_ID = "__cancel";
  items.push({ id: DONE_ID, label: options.doneLabel ?? "✓ Done", currentValue: "", values: [""] });
  items.push({ id: CANCEL_ID, label: "✕ Cancel", currentValue: "", values: [""] });

  const list = new SettingsList(
    items,
    options.maxVisible ?? 16,
    settingsListTheme(theme),
    (id, newValue) => {
      if (id === DONE_ID) {
        done([...selected]);
        return;
      }
      if (id === CANCEL_ID) {
        done(undefined);
        return;
      }
      if (id.startsWith("__group:")) return;
      if (newValue === "on") {
        if (options.maxSelections !== undefined && selected.size >= options.maxSelections) {
          list.updateValue(id, "off");
          status.setText(styles.hint(`Select at most ${options.maxSelections} items.`));
          return;
        }
        selected.add(id);
      } else {
        selected.delete(id);
      }
      status.setText(styles.hint(`${selected.size} selected`));
    },
    () => done(undefined),
    { enableSearch: true },
  );
  return new Panel(
    [
      textLine(styles.title(title)),
      textLine(styles.hint(options.hint ?? "Enter/click toggles · search filters · Esc cancels")),
      list,
      status,
    ],
    list,
  );
}

export function piMultiSelect(
  ctx: ExtensionCommandContext,
  title: string,
  groups: MultiSelectGroup[],
  options: { maxSelections?: number; hint?: string; doneLabel?: string } = {},
): Promise<string[] | undefined> {
  return openCustomPanel<string[]>(ctx, (tui, theme, _keyboard, done) =>
    makeMultiSelectPanel(theme, title, groups, done, { ...options, maxVisible: visibleRows(tui) }), CANCEL_MISSING);
}

/** Text input panel. `validate` returns an error line to block submission. */
export function makeInputPanel(
  theme: UiTheme,
  title: string,
  done: Done<string>,
  options: { initial?: string; placeholder?: string; hint?: string; validate?: (value: string) => string | undefined } = {},
): Component {
  const styles = uiStyles(theme);
  const status = textLine(styles.hint(options.hint ?? "Enter submits · Esc cancels"));
  const input = new Input({
    placeholder: options.placeholder,
    placeholderStyle: (text) => theme.fg("dim", text),
  });
  if (options.initial !== undefined) {
    input.setValue(options.initial);
    // Park the cursor at the end like the old ctx.ui.input prompt did.
    input.handleInput("\x05");
  }
  input.onSubmit = (value) => {
    const error = options.validate?.(value);
    if (error) {
      status.setText(theme.fg("warning", error));
      return;
    }
    done(value);
  };
  input.onEscape = () => done(undefined);
  input.focused = true;
  return new Panel([textLine(styles.title(title)), input, status], input);
}

export function piInput(
  ctx: ExtensionCommandContext,
  title: string,
  options: { initial?: string; placeholder?: string; hint?: string; validate?: (value: string) => string | undefined } = {},
): Promise<string | undefined> {
  return openCustomPanel<string>(ctx, (_tui, theme, _keyboard, done) =>
    makeInputPanel(theme, title, done, options), CANCEL_MISSING);
}

/**
 * Multi-line editor panel. Escape closes with the current text (mirrors the old
 * `ctx.ui.editor` semantics), Enter inserts a newline.
 */
export function makeEditorPanel(
  tui: TUI,
  theme: UiTheme,
  title: string,
  initial: string,
  done: Done<string>,
): Component {
  const styles = uiStyles(theme);
  const editor = new Editor(tui, {
    borderColor: (text) => theme.fg("dim", text),
    selectList: selectListTheme(theme),
  });
  editor.setText(initial);
  editor.focused = true;
  const wrapper: Component & { handleInput: (data: string) => void } = {
    render: (width: number) => editor.render(width),
    invalidate: () => editor.invalidate(),
    handleMouse: (event: TuiMouseEvent) => editor.handleMouse(event),
    handleInput: (data: string) => {
      if (data === "\x1b") {
        done(editor.getText());
        return;
      }
      editor.handleInput(data);
    },
  };
  return new Panel(
    [textLine(styles.title(title)), textLine(styles.hint("Esc closes and keeps the text")), editor],
    wrapper,
  );
}

export function piEditor(
  ctx: ExtensionCommandContext,
  title: string,
  initial: string,
): Promise<string | undefined> {
  return openCustomPanel<string>(ctx, (tui, theme, _keyboard, done) =>
    makeEditorPanel(tui as unknown as TUI, theme, title, initial, done), CANCEL_MISSING);
}

/** Two-button confirmation panel. `done` receives true when confirmed. */
export function makeConfirmPanel(theme: UiTheme, title: string, message: string, done: Done<boolean>): Component {
  const styles = uiStyles(theme);
  const CONFIRM = "__confirm";
  const CANCEL = "__cancel";
  const list = new SettingsList(
    [
      { id: CONFIRM, label: "✓ Confirm", currentValue: "", values: [""] },
      { id: CANCEL, label: "✕ Cancel", currentValue: "", values: [""] },
    ],
    3,
    settingsListTheme(theme),
    (id) => done(id === CONFIRM),
    () => done(false),
  );
  return new Panel(
    [textLine(styles.title(title)), textLine(message), textLine(styles.hint("Enter/click chooses · Esc cancels")), list],
    list,
  );
}

export function piConfirm(
  ctx: ExtensionCommandContext,
  title: string,
  message: string,
): Promise<boolean | undefined> {
  return openCustomPanel<boolean>(ctx, (_tui, theme, _keyboard, done) =>
    makeConfirmPanel(theme, title, message, done), CANCEL_MISSING);
}
