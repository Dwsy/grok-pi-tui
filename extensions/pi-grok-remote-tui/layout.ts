/**
 * custom() layout resolution — mirrors Pi interactive TUI sizing so projected
 * frames land where the real TUI would place them.
 */

import type { RemoteTuiLayout, UnknownRecord } from "./shared.ts";

export function asRecord(value: unknown): UnknownRecord | undefined {
  return value !== null && typeof value === "object" ? (value as UnknownRecord) : undefined;
}

function validSizeValue(value: unknown): number | string | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return Math.floor(value);
  if (typeof value === "string" && /^\d+(?:\.\d+)?%$/.test(value)) return value;
  return undefined;
}

function resolveSizeValue(value: unknown, reference: number, fallback: number): number {
  if (typeof value === "number" && Number.isFinite(value)) return Math.floor(value);
  if (typeof value === "string") {
    const match = value.match(/^(\d+(?:\.\d+)?)%$/);
    if (match) return Math.floor((reference * Number(match[1])) / 100);
  }
  return fallback;
}

function readOverlayOptions(options: unknown): UnknownRecord {
  const raw = asRecord(options)?.overlayOptions;
  try {
    return asRecord(typeof raw === "function" ? raw() : raw) ?? {};
  } catch {
    return {};
  }
}

/** Mirror Pi custom(): inline by default; explicit `{ overlay: true }` uses popup layout. */
export function resolveRemoteTuiLayout(
  options: unknown,
  terminalWidth: number,
  componentWidth?: unknown,
): RemoteTuiLayout {
  const root = asRecord(options) ?? {};
  const overlay = root.overlay === true;
  const hasOverlayOptions = overlay && root.overlayOptions !== undefined;
  const overlayOptions = hasOverlayOptions ? readOverlayOptions(options) : {};
  const componentWidthValue =
    !hasOverlayOptions &&
    typeof componentWidth === "number" &&
    Number.isFinite(componentWidth) &&
    componentWidth > 0
      ? Math.floor(componentWidth)
      : undefined;
  const defaultWidth = overlay
    ? componentWidthValue ?? Math.min(80, terminalWidth)
    : terminalWidth;
  const width = Math.max(
    1,
    Math.min(
      terminalWidth,
      resolveSizeValue(overlayOptions.width, terminalWidth, defaultWidth),
    ),
  );
  const minWidth = typeof overlayOptions.minWidth === "number" && Number.isFinite(overlayOptions.minWidth)
    ? Math.floor(overlayOptions.minWidth)
    : 0;
  const layout: RemoteTuiLayout = {
    overlay,
    width: Math.max(1, Math.min(terminalWidth, Math.max(width, minWidth))),
  };
  const maxHeight = validSizeValue(overlayOptions.maxHeight);
  if (maxHeight !== undefined) layout.maxHeight = maxHeight;
  if (typeof overlayOptions.anchor === "string") layout.anchor = overlayOptions.anchor;
  const row = validSizeValue(overlayOptions.row);
  const col = validSizeValue(overlayOptions.col);
  if (row !== undefined) layout.row = row;
  if (col !== undefined) layout.col = col;
  for (const [source, target] of [["offsetX", "offsetX"], ["offsetY", "offsetY"]] as const) {
    const value = overlayOptions[source];
    if (typeof value === "number" && Number.isFinite(value)) layout[target] = Math.floor(value);
  }
  return layout;
}

export type ResolvedViewport = {
  width: number;
  rows: number;
  terminalWidth: number;
  layout: RemoteTuiLayout;
};

/** Match Pi interactive TUI's terminal dimensions, then resolve custom layout. */
export function resolveViewport(options?: unknown): ResolvedViewport {
  const envWidth = Number(process.env.PI_GROK_REMOTE_TUI_WIDTH);
  const envRows = Number(process.env.PI_GROK_REMOTE_TUI_ROWS);
  const columnsEnv = Number(process.env.COLUMNS);
  const linesEnv = Number(process.env.LINES);
  const stdoutCols = Number(process.stdout?.columns);
  const stdoutRows = Number(process.stdout?.rows);
  const terminalWidth = Math.max(
    1,
    Math.floor([envWidth, columnsEnv, stdoutCols].find((n) => Number.isFinite(n) && n > 0) ?? 80),
  );
  const rows = Math.max(
    1,
    Math.floor([envRows, linesEnv, stdoutRows].find((n) => Number.isFinite(n) && n > 0) ?? 24),
  );
  const layout = resolveRemoteTuiLayout(options, terminalWidth);
  return { width: layout.width, rows, terminalWidth, layout };
}
