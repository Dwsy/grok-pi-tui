/**
 * Reusable virtualized list selection, viewport scroll, and double-click
 * state controller for bordered TS TUI overlays.
 */

export const DEFAULT_DOUBLE_CLICK_MS = 450;

export class ListController {
  selected = 0;
  scroll = 0;
  private lastClickIndex = -1;
  private lastClickTime = 0;
  private readonly rowsPerItem: number;
  private readonly doubleClickMs: number;

  constructor(options: { rowsPerItem?: number; doubleClickMs?: number } = {}) {
    this.rowsPerItem = Math.max(1, options.rowsPerItem ?? 1);
    this.doubleClickMs = options.doubleClickMs ?? DEFAULT_DOUBLE_CLICK_MS;
  }

  itemsPerPage(bodyHeight: number): number {
    return Math.max(1, Math.floor(bodyHeight / this.rowsPerItem));
  }

  clamp(count: number, bodyHeight: number): void {
    if (count <= 0) {
      this.selected = 0;
      this.scroll = 0;
      return;
    }
    this.selected = Math.max(0, Math.min(count - 1, this.selected));
    const perPage = this.itemsPerPage(bodyHeight);
    if (this.selected < this.scroll) {
      this.scroll = this.selected;
    } else if (this.selected >= this.scroll + perPage) {
      this.scroll = this.selected - perPage + 1;
    }
    const maxScroll = Math.max(0, count - perPage);
    this.scroll = Math.max(0, Math.min(this.scroll, maxScroll));
  }

  select(next: number, count: number, bodyHeight: number): boolean {
    const prev = this.selected;
    if (count <= 0) {
      this.selected = 0;
      this.scroll = 0;
      return prev !== 0;
    }
    this.selected = Math.max(0, Math.min(count - 1, next));
    this.clamp(count, bodyHeight);
    return this.selected !== prev;
  }

  move(delta: number, count: number, bodyHeight: number): boolean {
    return this.select(this.selected + delta, count, bodyHeight);
  }

  page(direction: -1 | 1, count: number, bodyHeight: number): boolean {
    return this.move(direction * this.itemsPerPage(bodyHeight), count, bodyHeight);
  }

  /**
   * Handle a left-click on item `index`. Returns `"confirm"` on a double-click
   * within `doubleClickMs`, otherwise `"select"`.
   */
  handleClick(
    index: number,
    count: number,
    bodyHeight: number,
    nowMs: number = Date.now(),
  ): "select" | "confirm" | "none" {
    if (index < 0 || index >= count) return "none";
    if (index === this.lastClickIndex && nowMs - this.lastClickTime <= this.doubleClickMs) {
      this.lastClickIndex = -1;
      this.lastClickTime = 0;
      this.select(index, count, bodyHeight);
      return "confirm";
    }
    this.lastClickIndex = index;
    this.lastClickTime = nowMs;
    this.select(index, count, bodyHeight);
    return "select";
  }

  /** Map a 0-based body-relative row offset to an item index. */
  indexAtBodyRow(relRow: number, count: number): number {
    if (relRow < 0) return -1;
    const idx = this.scroll + Math.floor(relRow / this.rowsPerItem);
    return idx >= 0 && idx < count ? idx : -1;
  }

  reset(): void {
    this.selected = 0;
    this.scroll = 0;
    this.lastClickIndex = -1;
    this.lastClickTime = 0;
  }
}
