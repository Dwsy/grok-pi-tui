# Bug: Switching model mid-response leaves the turn stuck on "Thinking…" with no reasoning block

## Summary

When you switch the model (`NextModel` / `/model` / `SwitchModel`) while the
agent is already streaming a response, grok-pi flips the status line to
"Thinking…" even though no reasoning content is being produced. The turn stays
in that state while plain text streams, and the spurious "Thinking" activity is
never cleared.

## Steps to reproduce

1. Start a response in grok-pi (any model, thinking enabled).
2. While the response is streaming, switch the model (e.g. `NextModel`).
3. The status line shows "Thinking…" but the scrollback has no thinking block
   and the model is actually emitting ordinary text.

## Root cause

Two coupled defects in the pager's ACP update tracker
(`xai-grok-pager/src/acp/tracker.rs`, `AcpUpdateTracker::handle_update`, the
`streamStartMs` boundary block):

1. **Stale empty thinking block survives a stream boundary.**
   On a new `streamStartMs`, the code only calls `finish_thinking` when the
   current thinking block has content (`thinking_has_content`). A pre-created
   *empty* block short-circuits the boundary: `finish_thinking` is skipped, and
   because `current_thinking.is_some()` is still true, `pre_create_thinking` is
   skipped too. The stale empty block stays registered as "the active thinking".

2. **`activity()` reports `Thinking` whenever `current_thinking.is_some()`.**
   `TurnActivity::Thinking` (`tracker.rs` `activity()`) is returned whenever a
   thinking block is registered, even if it never received a single token. So
   the dangling empty block keeps the status line on "Thinking…".

### Trigger path

The model-switch RPC (`x.ai/session/set_model` ->
`pi-grok-adapter` `set_session_model`) issues `set_model` / `set_thinking_level`
**mid-turn**. The continuation under the new model starts a fresh stream
segment before the previous segment emitted its first token, so a new
`streamStartMs` crosses the boundary while the pre-created thinking block is
still empty — hitting defect 1 and leaving the status stuck per defect 2.

## Expected behavior

Switching models mid-response must not fabricate a reasoning phase. The status
should reflect the actual stream (or the pre-created empty thinking block
should be dropped on the boundary), and "Thinking…" should only appear when
real thinking tokens are flowing.

## Suggested direction

- At a stream boundary, always release the current thinking block (empty ones
  are removed by `finish_thinking`'s empty-block cleanup), not only when it has
  content.
- Or gate the pre-create on the stream actually producing thought chunks, so a
  plain-text continuation never leaves a dangling empty block.

## Environment

- grok-pi (pi-grok-build), `origin Dwsy/grok-pi`
- Pi adapter + native Pager (`xai-grok-pager`)