# PR: HD Image Direct Preview in TS `@file` Explorer (Herdr / Kitty / iTerm2 + Half-Block Fallback)

- **Issue**: `docs/issues/pager/20261123-ts-file-picker-hd-image-preview.md`
- **Scope**: `crates/codegen/xai-grok-pager-render/src/terminal/{image.rs,image/tests.rs}`, `crates/codegen/xai-grok-pager/src/app/{app_view.rs,acp_handler/mod.rs,agent_view/{mod.rs,session.rs,notices.rs,media.rs,render.rs}}`, `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/{mentions_extension.rs,review_extension.rs}`, `extensions/pi-grok-tui-kit/{image.ts,index.ts,kit.test.ts}`, `extensions/pi-grok-mentions/{file-data.ts,file-picker-tui.ts,index.ts,file-picker.test.ts}`

## Summary

Added high-resolution, high-performance direct image preview to the TS `@file` explorer with full **Herdr**, Kitty, Ghostty, WezTerm, Warp, and iTerm2 graphics protocol support, plus an LRU-cached Truecolor Unicode half-block (`▀`) fallback for terminals without native graphics.

## Changes

- **Rust Graphics Overlay & Herdr Support**:
  - `xai-grok-pager-render/src/terminal/image.rs`: Enabled `MultiplexerKind::Herdr` + `GraphicsProtocol::Kitty` in `scrollback_inline_overlay_active_for_brand` (alongside existing `protocol_for_brand_and_mux` Herdr Kitty support), and updated `image/tests.rs`.
  - `xai-grok-pager/src/app/{app_view.rs,acp_handler/mod.rs,agent_view/{mod.rs,session.rs,notices.rs,media.rs,render.rs}}`:
    - Added `RemoteTuiImageOverlay` (`{ path, x, y, width, height }`) transported over the existing `setWidget("__pi_grok_remote_tui_image__", [JSON])` channel (zero `pi-grok-adapter` changes needed).
    - Reused the 64 MB bounded `inline_media_cache`, `MediaFileStamp` negative failure cache, stable per-path `owner_id`, and `overlay::static_centered_read_viewer` in `build_remote_tui_image_post_flush` so steady-state frames transmit zero bytes and closing/switching clears Kitty/Herdr placements and triggers iTerm2 grid repaint cleanly.
- **Shared TS Image Primitives (`extensions/pi-grok-tui-kit/image.ts`)**:
  - `probeImageMetadata`: Zero-dependency header-only binary probe (`<= 64 KB`) for PNG, JPEG, GIF, WebP, BMP, and SVG dimensions.
  - `detectTerminalImageProtocol`: Detects Herdr (`HERDR_ENV` / `TERM_PROGRAM=herdr`), Kitty, Ghostty, WezTerm, Warp, iTerm2, and Half-Block fallback.
  - `renderHalfBlockPreview` / `rgb24ToHalfBlockLines`: Fast LRU-cached Truecolor `▀` half-block renderer (`ffmpeg` / `magick` raw RGB24 pipe with 350 ms timeout).
- **TS `@file` Explorer Integration**:
  - `file-data.ts` detects image files before the NUL-byte binary check and returns `kind: "image"` with `imageMeta` and `absPath`.
  - `file-picker-tui.ts` displays format, pixel dimensions, size, and active protocol badge (`HD · Herdr/Kitty`, `HD · iTerm2`, `Half-Block`), emits diff-checked HD overlay rects when native graphics are available, falls back to Truecolor half-block otherwise, and clears overlays on non-image selection or exit.
  - `index.ts` wires `onImageOverlay` to `ctx.ui.setWidget("__pi_grok_remote_tui_image__", ...)` with `try ... finally` cleanup, and `mentions_extension.rs` / `review_extension.rs` embed `pi-grok-tui-kit/image.ts`.

## Verification

- `bun test --cwd extensions/pi-grok-tui-kit` (all pass, including Herdr/Kitty/iTerm2 detection, binary PNG/GIF/BMP/SVG header probing, and RGB24 half-block encoding)
- `bun test --cwd extensions/pi-grok-mentions` (all pass, including Herdr HD overlay emission, diff-check dedup, and clear-on-switch test)
- `bun test --cwd extensions/pi-grok-review` (all pass)
- `git diff --check` clean
- Local `cargo` commands intentionally skipped per `AGENTS.md`; all Rust types, struct initializers, and `include_str!` paths statically verified.
