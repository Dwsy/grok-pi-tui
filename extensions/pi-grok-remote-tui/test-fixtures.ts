// Bun module mocks are process-wide; both projection suites need the same exports.
type SettingItem = {
  id: string;
  label: string;
  description?: string;
  currentValue: string;
  values?: string[];
};

/** Minimal SettingsList stand-in: space/enter cycles, esc cancels. */
class MockSettingsList {
  private items: SettingItem[];
  private index = 0;
  private onChange: (id: string, newValue: string) => void;
  private onCancel: () => void;

  constructor(
    items: SettingItem[],
    _maxVisible: number,
    _theme: unknown,
    onChange: (id: string, newValue: string) => void,
    onCancel: () => void,
  ) {
    this.items = items;
    this.onChange = onChange;
    this.onCancel = onCancel;
  }

  invalidate() {}
  render() {
    return this.items.map((item) => `${item.label}=${item.currentValue}`);
  }

  handleInput(data: string) {
    if (data === "\x1b[A") {
      this.index = this.index === 0 ? this.items.length - 1 : this.index - 1;
      return;
    }
    if (data === "\x1b[B") {
      this.index = this.index === this.items.length - 1 ? 0 : this.index + 1;
      return;
    }
    if (data === " " || data === "\r") {
      const item = this.items[this.index]!;
      const values = item.values ?? ["on", "off"];
      const next = values[(values.indexOf(item.currentValue) + 1) % values.length]!;
      item.currentValue = next;
      this.onChange(item.id, next);
      return;
    }
    if (data === "\x1b") {
      this.onCancel();
    }
  }
}

export const piTuiMock = {
  CURSOR_MARKER: "\x1b_pi:c\x07",
  isKeyRelease: (data: string) => /:3(?:u|~|[ABCDHF])/.test(data),
  KeybindingsManager: class {
    matches() {
      return false;
    }
  },
  TUI_KEYBINDINGS: {},
  setKeybindings: () => {},
  SettingsList: MockSettingsList,
};
