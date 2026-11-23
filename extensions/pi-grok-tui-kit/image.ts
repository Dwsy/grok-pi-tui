/**
 * High-performance image probing, terminal graphics protocol detection (Herdr,
 * Kitty, Ghostty, WezTerm, Warp, iTerm2), and cached Truecolor half-block (`▀`)
 * fallback renderer for TS TUI overlays.
 *
 * Design:
 * - Zero external JS dependencies: binary header probing reads at most 64 KB to
 *   extract exact pixel dimensions for PNG, JPEG, GIF, WebP, BMP, and SVG in <1 ms.
 * - Native HD overlay: in `grok-pi` under Herdr / Kitty / Ghostty / WezTerm /
 *   Warp / iTerm2, components emit an overlay-local rect via
 *   `setWidget("__pi_grok_remote_tui_image__", [JSON])`, and Rust's `xai-grok-pager`
 *   renders the full-resolution image via `overlay::static_centered_read_viewer`
 *   with a 64 MB bounded cache and zero steady-state retransmission.
 * - Universal fallback: when no native graphics protocol is active, falls back
 *   to an LRU-cached Truecolor `▀` half-block preview decoded via `ffmpeg`/`magick`.
 */

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

/** Must match `handle_pi_ui_widget` in `xai-grok-pager/src/app/acp_handler/mod.rs`. */
export const REMOTE_TUI_IMAGE_WIDGET_KEY = "__pi_grok_remote_tui_image__";

export type ImageProtocol = "kitty" | "iterm2" | "halfblock";

export interface TerminalImageCapability {
  protocol: ImageProtocol;
  label: string;
}

export interface ImageMetadata {
  format: string;
  width: number;
  height: number;
  sizeBytes: number;
}

export interface RemoteTuiImageSpec {
  path: string;
  /** 0-based cell X offset relative to the active Remote TUI overlay rect. */
  x: number;
  /** 0-based cell Y offset relative to the active Remote TUI overlay rect. */
  y: number;
  width: number;
  height: number;
}

const IMAGE_EXTENSIONS = new Set([
  ".png",
  ".jpg",
  ".jpeg",
  ".gif",
  ".webp",
  ".bmp",
  ".svg",
  ".ico",
  ".tif",
  ".tiff",
  ".avif",
  ".heic",
]);

const HEADER_PROBE_BYTES = 64 * 1024;
const HALF_BLOCK_CACHE_LIMIT = 48;
const halfBlockCache = new Map<string, string[] | null>();

export function isImageFilePath(filePath: string): boolean {
  const ext = path.extname(filePath).toLowerCase();
  return IMAGE_EXTENSIONS.has(ext);
}

/**
 * Detect whether the current terminal supports native HD image overlays.
 * Matches `xai-grok-pager-render/src/terminal/image.rs`, including **Herdr**
 * (`HERDR_ENV=1` or `TERM_PROGRAM=herdr`) which forwards Kitty graphics.
 */
export function detectTerminalImageProtocol(
  env: NodeJS.ProcessEnv = process.env,
): TerminalImageCapability {
  if (process.platform === "win32") {
    return { protocol: "halfblock", label: "Half-Block" };
  }

  const termProgram = (env.TERM_PROGRAM ?? "").toLowerCase();
  const term = (env.TERM ?? "").toLowerCase();
  const lcTerminal = (env.LC_TERMINAL ?? "").toLowerCase();

  // Nested real tmux/screen without passthrough drops graphics; Herdr speaks Kitty.
  if (env.HERDR_ENV || termProgram === "herdr") {
    return { protocol: "kitty", label: "HD · Herdr/Kitty" };
  }
  if (env.KITTY_WINDOW_ID || term.includes("kitty")) {
    return { protocol: "kitty", label: "HD · Kitty" };
  }
  if (termProgram === "ghostty" || term.includes("ghostty")) {
    return { protocol: "kitty", label: "HD · Ghostty" };
  }
  if (termProgram === "wezterm") {
    return { protocol: "kitty", label: "HD · WezTerm" };
  }
  if (termProgram === "warpterminal") {
    return { protocol: "kitty", label: "HD · Warp" };
  }
  if (termProgram === "iterm.app" || lcTerminal === "iterm2") {
    return { protocol: "iterm2", label: "HD · iTerm2" };
  }

  return { protocol: "halfblock", label: "Half-Block" };
}

function parsePngDimensions(buf: Buffer): { width: number; height: number } | null {
  if (buf.length < 24) return null;
  if (
    buf[0] !== 0x89 ||
    buf[1] !== 0x50 ||
    buf[2] !== 0x4e ||
    buf[3] !== 0x47
  ) {
    return null;
  }
  const width = buf.readUInt32BE(16);
  const height = buf.readUInt32BE(20);
  return width > 0 && height > 0 ? { width, height } : null;
}

function parseGifDimensions(buf: Buffer): { width: number; height: number } | null {
  if (buf.length < 10) return null;
  const sig = buf.subarray(0, 6).toString("ascii");
  if (sig !== "GIF87a" && sig !== "GIF89a") return null;
  const width = buf.readUInt16LE(6);
  const height = buf.readUInt16LE(8);
  return width > 0 && height > 0 ? { width, height } : null;
}

function parseBmpDimensions(buf: Buffer): { width: number; height: number } | null {
  if (buf.length < 26 || buf[0] !== 0x42 || buf[1] !== 0x4d) return null;
  const width = buf.readInt32LE(18);
  const height = Math.abs(buf.readInt32LE(22));
  return width > 0 && height > 0 ? { width, height } : null;
}

function parseWebpDimensions(buf: Buffer): { width: number; height: number } | null {
  if (buf.length < 30) return null;
  if (
    buf.subarray(0, 4).toString("ascii") !== "RIFF" ||
    buf.subarray(8, 12).toString("ascii") !== "WEBP"
  ) {
    return null;
  }
  const chunk = buf.subarray(12, 16).toString("ascii");
  if (chunk === "VP8X" && buf.length >= 30) {
    const width = 1 + (buf[24]! | (buf[25]! << 8) | (buf[26]! << 16));
    const height = 1 + (buf[27]! | (buf[28]! << 8) | (buf[29]! << 16));
    return { width, height };
  }
  if (chunk === "VP8 " && buf.length >= 30) {
    const width = buf.readUInt16LE(26) & 0x3fff;
    const height = buf.readUInt16LE(28) & 0x3fff;
    return width > 0 && height > 0 ? { width, height } : null;
  }
  if (chunk === "VP8L" && buf.length >= 25) {
    const bits = buf.readUInt32LE(21);
    const width = (bits & 0x3fff) + 1;
    const height = ((bits >> 14) & 0x3fff) + 1;
    return { width, height };
  }
  return null;
}

function parseJpegDimensions(buf: Buffer): { width: number; height: number } | null {
  if (buf.length < 4 || buf[0] !== 0xff || buf[1] !== 0xd8) return null;
  let offset = 2;
  while (offset + 4 < buf.length) {
    if (buf[offset] !== 0xff) {
      offset++;
      continue;
    }
    while (offset < buf.length && buf[offset] === 0xff) offset++;
    if (offset >= buf.length) break;
    const marker = buf[offset++]!;
    if (marker === 0xd9 || marker === 0xda) break; // EOI / SOS
    if (offset + 2 > buf.length) break;
    const segLen = buf.readUInt16BE(offset);
    if (segLen < 2 || offset + segLen > buf.length) break;
    // SOF0..SOF3, SOF5..SOF7, SOF9..SOF11, SOF13..SOF15
    const isSof =
      (marker >= 0xc0 && marker <= 0xc3) ||
      (marker >= 0xc5 && marker <= 0xc7) ||
      (marker >= 0xc9 && marker <= 0xcb) ||
      (marker >= 0xcd && marker <= 0xcf);
    if (isSof && segLen >= 7) {
      const height = buf.readUInt16BE(offset + 3);
      const width = buf.readUInt16BE(offset + 5);
      return width > 0 && height > 0 ? { width, height } : null;
    }
    offset += segLen;
  }
  return null;
}

function parseSvgDimensions(buf: Buffer): { width: number; height: number } | null {
  const head = buf.subarray(0, Math.min(buf.length, 4096)).toString("utf8");
  const svgTag = head.match(/<svg\b[^>]*>/i)?.[0];
  if (!svgTag) return null;
  const vb = svgTag.match(/viewBox\s*=\s*["'][\s,]*[\d.-]+[\s,]+[\d.-]+[\s,]+([\d.]+)[\s,]+([\d.]+)/i);
  if (vb) {
    const width = Math.round(Number(vb[1]));
    const height = Math.round(Number(vb[2]));
    if (width > 0 && height > 0) return { width, height };
  }
  const wMatch = svgTag.match(/\bwidth\s*=\s*["']([\d.]+)/i);
  const hMatch = svgTag.match(/\bheight\s*=\s*["']([\d.]+)/i);
  if (wMatch && hMatch) {
    const width = Math.round(Number(wMatch[1]));
    const height = Math.round(Number(hMatch[1]));
    if (width > 0 && height > 0) return { width, height };
  }
  return null;
}

/**
 * Probe image format & pixel dimensions by reading only the file header (<= 64 KB).
 */
export function probeImageMetadata(filePath: string): ImageMetadata | null {
  try {
    const stat = fs.statSync(filePath);
    if (!stat.isFile()) return null;
    const readLen = Math.min(stat.size, HEADER_PROBE_BYTES);
    const buf = Buffer.alloc(readLen);
    const fd = fs.openSync(filePath, "r");
    try {
      fs.readSync(fd, buf, 0, readLen, 0);
    } finally {
      fs.closeSync(fd);
    }

    const ext = path.extname(filePath).toLowerCase();
    const png = parsePngDimensions(buf);
    if (png) return { format: "PNG", ...png, sizeBytes: stat.size };

    const jpeg = parseJpegDimensions(buf);
    if (jpeg) return { format: "JPEG", ...jpeg, sizeBytes: stat.size };

    const gif = parseGifDimensions(buf);
    if (gif) return { format: "GIF", ...gif, sizeBytes: stat.size };

    const webp = parseWebpDimensions(buf);
    if (webp) return { format: "WebP", ...webp, sizeBytes: stat.size };

    const bmp = parseBmpDimensions(buf);
    if (bmp) return { format: "BMP", ...bmp, sizeBytes: stat.size };

    if (ext === ".svg") {
      const svg = parseSvgDimensions(buf);
      return {
        format: "SVG",
        width: svg?.width ?? 0,
        height: svg?.height ?? 0,
        sizeBytes: stat.size,
      };
    }

    if (IMAGE_EXTENSIONS.has(ext)) {
      return {
        format: ext.slice(1).toUpperCase(),
        width: 0,
        height: 0,
        sizeBytes: stat.size,
      };
    }
    return null;
  } catch {
    return null;
  }
}

/**
 * Compute cell dimensions (cols, rows) that fit an image inside `(maxCols, maxRows)`
 * preserving aspect ratio (assuming ~1:2 terminal cell aspect ratio).
 */
export function fitImageCells(
  imgW: number,
  imgH: number,
  maxCols: number,
  maxRows: number,
): { cols: number; rows: number } {
  const safeCols = Math.max(4, Math.floor(maxCols));
  const safeRows = Math.max(2, Math.floor(maxRows));
  if (imgW <= 0 || imgH <= 0) {
    return { cols: safeCols, rows: safeRows };
  }
  // Each terminal cell is ~2 pixels tall per 1 pixel wide in half-block space.
  const pxMaxW = safeCols;
  const pxMaxH = safeRows * 2;
  const scale = Math.min(pxMaxW / imgW, pxMaxH / imgH, 1);
  const cols = Math.max(2, Math.min(safeCols, Math.round(imgW * scale)));
  const pxH = Math.max(2, Math.min(pxMaxH, Math.round(imgH * scale)));
  const rows = Math.max(1, Math.ceil(pxH / 2));
  return { cols, rows };
}

/**
 * Convert raw RGB24 pixel buffer (`width * height * 3` bytes) into ANSI
 * Truecolor Unicode upper half-block (`▀`) lines (`Math.ceil(height / 2)` rows).
 */
export function rgb24ToHalfBlockLines(
  rgb: Buffer,
  pxWidth: number,
  pxHeight: number,
): string[] {
  const lines: string[] = [];
  const stride = pxWidth * 3;
  for (let y = 0; y < pxHeight; y += 2) {
    let row = "";
    const topOff = y * stride;
    const botOff = (y + 1 < pxHeight ? y + 1 : y) * stride;
    for (let x = 0; x < pxWidth; x++) {
      const iTop = topOff + x * 3;
      const iBot = botOff + x * 3;
      const tr = rgb[iTop] ?? 0;
      const tg = rgb[iTop + 1] ?? 0;
      const tb = rgb[iTop + 2] ?? 0;
      const br = rgb[iBot] ?? 0;
      const bg = rgb[iBot + 1] ?? 0;
      const bb = rgb[iBot + 2] ?? 0;
      row += `\x1b[38;2;${tr};${tg};${tb}m\x1b[48;2;${br};${bg};${bb}m▀`;
    }
    lines.push(`${row}\x1b[0m`);
  }
  return lines;
}

/**
 * Decode and render a Truecolor `▀` half-block preview with LRU caching.
 * Uses `ffmpeg` (or `magick` fallback) with a tight 350 ms timeout so UI
 * navigation stays responsive even on very large files.
 */
export function renderHalfBlockPreview(
  filePath: string,
  maxCols: number,
  maxRows: number,
  meta?: ImageMetadata | null,
): string[] | null {
  try {
    const stat = fs.statSync(filePath);
    const target = fitImageCells(meta?.width ?? 0, meta?.height ?? 0, maxCols, maxRows);
    const pxW = target.cols;
    const pxH = target.rows * 2;
    const cacheKey = `${filePath}:${stat.mtimeMs}:${pxW}x${pxH}`;

    if (halfBlockCache.has(cacheKey)) {
      const hit = halfBlockCache.get(cacheKey) ?? null;
      halfBlockCache.delete(cacheKey);
      halfBlockCache.set(cacheKey, hit);
      return hit;
    }

    const expectedBytes = pxW * pxH * 3;
    let rgb: Buffer | null = null;

    const ff = spawnSync(
      "ffmpeg",
      [
        "-v",
        "error",
        "-i",
        filePath,
        "-vf",
        `scale=${pxW}:${pxH}:flags=fast_bilinear`,
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "pipe:1",
      ],
      { timeout: 350, maxBuffer: expectedBytes + 65536 },
    );
    if (ff.status === 0 && Buffer.isBuffer(ff.stdout) && ff.stdout.length >= expectedBytes) {
      rgb = ff.stdout;
    } else {
      const mg = spawnSync(
        "magick",
        [filePath, "-resize", `${pxW}x${pxH}!`, "-depth", "8", "rgb:-"],
        { timeout: 350, maxBuffer: expectedBytes + 65536 },
      );
      if (mg.status === 0 && Buffer.isBuffer(mg.stdout) && mg.stdout.length >= expectedBytes) {
        rgb = mg.stdout;
      }
    }

    const rendered = rgb ? rgb24ToHalfBlockLines(rgb, pxW, pxH) : null;
    if (halfBlockCache.size >= HALF_BLOCK_CACHE_LIMIT) {
      const oldest = halfBlockCache.keys().next().value;
      if (oldest !== undefined) halfBlockCache.delete(oldest);
    }
    halfBlockCache.set(cacheKey, rendered);
    return rendered;
  } catch {
    return null;
  }
}
