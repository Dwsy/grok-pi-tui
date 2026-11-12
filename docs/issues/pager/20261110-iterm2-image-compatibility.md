# iTerm2 image compatibility

## Problem

Images render in Ghostty but iTerm2 does not have equivalent coverage. The existing iTerm2 OSC 1337 encoder uses an invalid `cells` suffix for cell dimensions. Interactive scrollback image overlays are deliberately disabled for iTerm2 because the protocol lacks Kitty placement, crop, and deletion operations.

## Plan

- Correct OSC 1337 cell dimension encoding and verify capability gating and escape generation.
- Evaluate safe reuse of the existing preview overlay for image viewers, including repaint on close and resize.
- Keep scrollback gated until its image lifecycle, clipping, scrolling, and cleanup can be handled safely; document any remaining limitation and validation.

## Status

The Read image viewer now uses the gated iTerm2 preview protocol, while Kitty retains its existing placement path. Successful iTerm2 image writes mark the next frame for a full repaint, including after the viewer closes. During resize and search/input/selection overlays, the image is hidden so the grid can be repainted. This may add redraw cost or flicker on iTerm2.

The OSC 1337 encoder now sends bare cell counts, as required by the protocol. Scrollback images remain gated off for iTerm2.

## Verification

- Scoped rustfmt and `git diff --check` passed.
- `cargo-shared.sh test -p xai-grok-pager-render --lib terminal::image::tests` did not reach test execution after more than 20 minutes of dependency compilation; a stop was requested. The new overlay and Read viewer tests and the binary build/check remain unverified.
- Real iTerm2 visual acceptance has not been performed.
