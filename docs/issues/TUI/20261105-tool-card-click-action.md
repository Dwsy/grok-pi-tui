# F2 tool-card click action

## Requirement

Make tool-card clicks configurable through F2. Choices: select only (default), expand/collapse, and open the native details viewer (same action as Enter). The third choice was explicitly confirmed as click-to-open. Settings apply live and persist in the product-isolated user config.

## Design

Use the shared settings registry and existing PersistSetting/rollback path. Add `[ui].tool_card_click_action = "select" | "toggle" | "open_viewer"`. Only native tool-call entries use this policy. Conversation selection, text dragging, inline links/media, and special non-tool cards retain their existing handlers. The native Enter/OpenBlockViewer path remains the details surface, including native group-header handling.

## Progress

- [x] Locate current click folding, F2 registry, live cache, persistence and viewer action.
- [x] Implement the three modes and settings wiring.
- [ ] Verify click/drag/group behavior, settings defaults and relevant build checks.

## Validation

Pending.
