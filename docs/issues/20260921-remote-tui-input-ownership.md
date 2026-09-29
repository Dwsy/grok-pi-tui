# Remote TUI input ownership

## Problem and scope

Pi custom components displayed in Grok-Pi must own keyboard input while active.
Reported symptom: letter actions such as A/S reach the composer. The exact live
trigger is not yet reproduced. Inspection also found global shortcut precedence,
forced Escape cancellation, and a cross-process shared keyfile metadata path.

## Plan

1. Keep modal input ahead of extension shortcuts; forward Escape to the component
   and reserve Ctrl+Shift+Escape for host cancellation. Never fall through to the
   composer for unsupported keys while the remote component owns input.
2. Publish explicit component identity before the first frame, reject stale input,
   and isolate transport metadata per Grok-Pi instance.
3. Preserve the native Pi runtime. Ordinary widgets do not acquire modal input.
   Native Pi question dialogs opened by a component temporarily receive input.
4. Add focused Pager and transport regressions, run the extension tests and Rust
   checks, and record the actual verification boundaries before publishing.

## Status

- Implementation complete: explicit lifecycle messages, modal shortcut priority,
  ordinary Escape forwarding, emergency cancellation, child-focus preservation,
  native-dialog visibility, and per-instance/component transport validation.
- Live PTY investigation additionally found one-task-per-key dispatch reordering
  `deepseek` and fs.watch delaying a burst tail. Keyboard notifications now enqueue
  and execute in order; a 50ms drain supplements filesystem notifications.
- TypeScript: 13 tests passed; entry bundle builds successfully.
- Production Rust check passed. Adapter transport regression and embedded-module
  materialization test each passed.
- Pager lib regression execution is blocked by 110 unrelated test-target compile
  errors (timeline helpers, outdated TasksPane calls and AppView fixtures).
- Product build passed. PTY smoke verified settings `s`, model search `deepseek`,
  rapid Down/Enter, Escape back, emergency close and restored composer input.
- User's exact original scene and live multi-terminal acceptance remain pending.

## Manual acceptance

Using the built binary, open `/remote-tui` and a third-party `ctx.ui.custom()`
component. Keep a composer draft, press letter actions, navigate and paste into
the component, open/close a native input dialog, then cancel back to the composer.
The draft must remain intact. Ordinary Escape must follow the component's own
back/cancel behavior; Ctrl+Shift+Escape must release a stuck remote component.
Repeat with two terminals and during reload. Test lower-case keys separately
from Shift-modified keys: plugins must parse the terminal sequence correctly.

## Follow-up: modified key presses

Shop settings exposed a remaining encoder defect: `kitty_sequence()` returned
`None` for Press, including Shift+S, so modal capture consumed the key without
forwarding it. Encode modified presses, preserve plain Shift+letter presses as
uppercase text for Pi components using literal actions, and keep repeat/release
events distinct. Validate with a standalone integration target (production
library, not the broken lib-test fixtures) and the actual Shop settings component
using synthetic profiles without writing user configuration.

Validation: standalone key integration tests failed 2/3 before the fix and
passed 3/3 after it; product build passed. PTY runs of the real Shop panel
returned `save` for `s`, raw `S` and Kitty `Shift+S`. No configuration was saved
by the probe. Full Shop transport/lifecycle behavior is outside this key test.
