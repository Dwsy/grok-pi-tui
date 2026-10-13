# ratatui-inline

A Rust library for terminal applications with an inline viewport: a UI pinned to the bottom of the terminal, with history in native scrollback above it.

## What is this?

This crate is for terminal applications where:

- A viewport (UI element) is pinned to the bottom of the terminal
- Content above the viewport becomes part of the terminal's native scrollback
- Users scroll history with the terminal's built-in scroll
- Long lines wrap without truncation
- The viewport stays visible and interactive while history accumulates above

## Key Features

- **Inline viewport** - UI stays at the bottom; content above it goes into scrollback
- **Natural text flow** - Content is printed normally; the terminal wraps and scrolls
- **Zero-copy text processing** - ANSI-aware text segmentation without allocations
- **Line wrapping** - Width boundaries with ANSI sequences
- **Unicode support** - Emoji, CJK, combining characters
- **Terminal resize handling** - CSI `ESC [2J` (clear screen), `ESC [3J` (clear scrollback), `ESC [H` (cursor home), then re-output history
- **Synchronized output** - DCS protocol so the terminal updates once per batch

## Usage

See `examples/inline.rs` for a complete working example.

## Architecture

### Text Processing

ANSI-aware segmentation is zero-copy:

- **anstyle-parse** - ANSI/SGR-aware segmentation for line splitting
- **Zero allocations** - Returns string slices; no copy
- **Single-pass parsing** - One pass with escape-sequence tracking
- **Unicode support** - Width calculation for emoji, CJK, combining characters

### Scrollback Implementation

Natural-flow scrollback:

1. Position the cursor at the viewport top
2. Print content; the terminal wraps
3. Add viewport-height newlines to reserve space
4. Clear and render the viewport

No alternate-screen mode or per-terminal workarounds.

### Line Ending Handling

- **LF (`\n`)** - Next line
- **CRLF (`\r\n`)** - One line break
- **CR (`\r`)** - Cursor to line start (overwrite)

## Design Decisions

### Why Fork ratatui's Terminal?

The standard ratatui Terminal API does not expose internals needed for inline viewport manipulation:

- **Viewport area access** - Current position and dimensions
- **Direct viewport positioning** - Set viewport location
- **Buffer management** - Back-buffer reset and previous-buffer access
- **Resize calculations** - Buffer state during resize

The forked Terminal adds these while keeping ratatui's API.

### Synchronized Output

DCS synchronized output:

- Operations between begin/end markers are atomic
- The terminal updates the display once per batch
- Partial render states are not shown

## Performance

- **Colored JSON**: ~186μs per operation
- **Plain text**: ~75μs per operation
- **Zero allocations** in hot path
- **Single-pass parsing** for all text processing

## Testing

- Text segmentation with ANSI sequences
- Line wrapping and Unicode handling
- Line endings (LF, CRLF, CR)
- Viewport positioning and resizing
- Terminal resize with history re-rendering
- Mock terminal infrastructure for unit testing

### Terminal Resize Strategy

#### The Problem

On the main screen (not alternate screen), resize happens before the app receives SIGWINCH:

- The terminal reflows content before the app sees the signal
- Old viewport borders reflow as garbage text
- Built-in `autoresize()` corrupts scrollback
- Cursor position queries (DSR) race during rapid resize
- Terminals reflow differently

#### The Solution: clear, then re-render

`resize_purge_rerender` writes `\x1b[2J\x1b[3J\x1b[H` (clear screen, clear scrollback, cursor home), re-outputs the full history, and places the viewport from the content height. RIS (`ESC c`) is not used: it does not clear scrollback in iTerm and Terminal.app.

## Dependencies

- `ratatui` - Terminal UI framework (forked Terminal class)
- `crossterm` - Cross-platform terminal manipulation
- `anstyle-parse` - ANSI/SGR-aware line segmentation (production)
- `unicode-width` - Unicode character width calculation
- `termwiz` - **dev-dependency only**; reference splitter for
  `tests/segment_differential.rs` (not linked into shipped binaries)

## References

- [anstyle-parse](https://crates.io/crates/anstyle-parse)
- [Ratatui wrapping discussion](https://github.com/ratatui/ratatui/issues/1426)

## License / attribution

This crate includes a forked `Terminal` implementation derived from [ratatui](https://github.com/ratatui/ratatui)
(MIT / Apache-2.0). See `NOTICE` in this directory and the repository root `THIRD-PARTY-NOTICES`.
