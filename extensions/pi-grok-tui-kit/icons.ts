/**
 * Shared Nerd Font devicon glyphs & terminal capability detection
 * (NvChad `nvim-tree.lua` + `nvim-web-devicons` palette with clean ASCII fallback).
 */

import path from "node:path";

export interface FileIconSpec {
  glyph: string;
  color: "accent" | "success" | "warning" | "error" | "dim" | "default";
}

const NERD_EXT_ICONS: Record<string, FileIconSpec> = {
  rs: { glyph: "", color: "warning" },
  ts: { glyph: "", color: "accent" },
  tsx: { glyph: "", color: "accent" },
  mts: { glyph: "", color: "accent" },
  cts: { glyph: "", color: "accent" },
  js: { glyph: "", color: "warning" },
  jsx: { glyph: "", color: "accent" },
  mjs: { glyph: "", color: "warning" },
  cjs: { glyph: "", color: "warning" },
  json: { glyph: "", color: "warning" },
  jsonc: { glyph: "", color: "warning" },
  toml: { glyph: "", color: "warning" },
  yaml: { glyph: "", color: "error" },
  yml: { glyph: "", color: "error" },
  md: { glyph: "", color: "accent" },
  mdx: { glyph: "", color: "accent" },
  lua: { glyph: "", color: "accent" },
  vim: { glyph: "", color: "success" },
  py: { glyph: "", color: "warning" },
  go: { glyph: "", color: "accent" },
  java: { glyph: "", color: "error" },
  kt: { glyph: "", color: "accent" },
  swift: { glyph: "", color: "warning" },
  c: { glyph: "", color: "accent" },
  h: { glyph: "", color: "accent" },
  cpp: { glyph: "", color: "accent" },
  hpp: { glyph: "", color: "accent" },
  sh: { glyph: "", color: "success" },
  bash: { glyph: "", color: "success" },
  zsh: { glyph: "", color: "success" },
  fish: { glyph: "", color: "success" },
  html: { glyph: "", color: "error" },
  css: { glyph: "", color: "accent" },
  scss: { glyph: "", color: "error" },
  vue: { glyph: "", color: "success" },
  svelte: { glyph: "", color: "warning" },
  sql: { glyph: "", color: "warning" },
  db: { glyph: "", color: "warning" },
  sqlite: { glyph: "", color: "warning" },
  png: { glyph: "󰋩", color: "accent" },
  jpg: { glyph: "󰋩", color: "accent" },
  jpeg: { glyph: "󰋩", color: "accent" },
  webp: { glyph: "󰋩", color: "accent" },
  gif: { glyph: "󰋩", color: "accent" },
  svg: { glyph: "󰜡", color: "warning" },
  pdf: { glyph: "", color: "error" },
  lock: { glyph: "", color: "dim" },
  env: { glyph: "", color: "warning" },
  txt: { glyph: "󰈙", color: "dim" },
  log: { glyph: "󰌱", color: "dim" },
};

const NERD_NAME_ICONS: Record<string, FileIconSpec> = {
  "cargo.toml": { glyph: "", color: "warning" },
  "cargo.lock": { glyph: "", color: "dim" },
  "package.json": { glyph: "", color: "success" },
  "tsconfig.json": { glyph: "", color: "accent" },
  "dockerfile": { glyph: "󰡨", color: "accent" },
  "makefile": { glyph: "", color: "warning" },
  ".gitignore": { glyph: "", color: "error" },
  ".gitmodules": { glyph: "", color: "error" },
  "license": { glyph: "", color: "warning" },
  "readme.md": { glyph: "", color: "accent" },
};

/**
 * Detect whether the active terminal likely has Nerd Font glyphs available.
 * Explicit env overrides (`PI_GROK_NERD_ICONS=0|1`, `NERD_FONT=0|1`) win first.
 */
export function detectNerdFontSupport(env: NodeJS.ProcessEnv = process.env): boolean {
  const explicit = env.PI_GROK_NERD_ICONS ?? env.NERD_FONT ?? env.PI_NERD_ICONS;
  if (explicit !== undefined) {
    return explicit !== "0" && explicit.toLowerCase() !== "false";
  }
  const termProgram = (env.TERM_PROGRAM ?? "").toLowerCase();
  const term = (env.TERM ?? "").toLowerCase();
  return (
    termProgram.includes("ghostty") ||
    termProgram.includes("iterm") ||
    termProgram.includes("wezterm") ||
    termProgram.includes("warp") ||
    termProgram.includes("alacritty") ||
    termProgram.includes("otty") ||
    termProgram.includes("vscode") ||
    termProgram.includes("herdr") ||
    term.includes("kitty") ||
    term.includes("xterm-256color") ||
    Boolean(env.KITTY_WINDOW_ID) ||
    Boolean(env.WEZTERM_EXECUTABLE)
  );
}

/** Resolve file/folder icon glyph (NvChad nvim-tree + devicons, or ASCII fallback). */
export function getFileIcon(
  name: string,
  options: { isDir?: boolean; expanded?: boolean; isSymlink?: boolean; nerdIcons: boolean },
): FileIconSpec {
  const { isDir = false, expanded = false, isSymlink = false, nerdIcons } = options;
  if (!nerdIcons) {
    if (isDir) return { glyph: expanded ? "▾" : "▸", color: "accent" };
    if (isSymlink) return { glyph: "@", color: "accent" };
    return { glyph: "·", color: "dim" };
  }
  if (isDir) {
    if (isSymlink) return { glyph: "", color: "accent" };
    return { glyph: expanded ? "" : "", color: "accent" };
  }
  const lower = name.toLowerCase();
  if (NERD_NAME_ICONS[lower]) return NERD_NAME_ICONS[lower]!;
  const ext = path.extname(lower).slice(1);
  if (ext && NERD_EXT_ICONS[ext]) return NERD_EXT_ICONS[ext]!;
  return { glyph: "󰈚", color: "default" };
}
