# pi_user_markdown — grok-pi user prompt markdown rendering

**Status:** done
**Date:** 2026-08-05

## Summary

Add an F2 setting (`pi_user_markdown`, default **on**) so grok-pi user messages render with the agent markdown renderer while preserving `UserPromptBlock` folding. Collapsed/truncated prompts use the classic 3-line preview; expanding the block switches to Markdown rendering. Turning the setting off keeps the classic plain-text renderer without changing the current fold state.

## Scope

- External-agent (grok-pi) profile only (`external_only` setting).
- Applies immediately on toggle (no restart).
- Preserves user prompt chrome (prefix, accent band, background).

## Implementation

| Layer | Change |
|---|---|
| `[ui].pi_user_markdown` | `UiConfig` field, default `true` |
| Appearance cache | `load_pi_user_markdown` / `set_pi_user_markdown` |
| F2 | Agent → **Markdown user messages** |
| Setter | `set_pi_user_markdown` + `apply_pi_user_markdown_flip` on all agent/subagent scrollbacks |
| `UserPromptBlock` | Lazy `MarkdownContent`; `use_agent_renderer()` = external profile ∧ cache flag; Markdown is used for expanded mode while collapsed/truncated mode reuses the classic 3-line preview |
| Persist | `helpers.rs` → `set_pi_user_markdown` |

## Verification

```bash
./scripts/cargo-shared.sh test -p xai-grok-pager --lib -- scrollback::blocks::user
./scripts/cargo-shared.sh test -p xai-grok-pager --lib -- settings::registry
./scripts/cargo-shared.sh test -p xai-grok-pager-render --lib -- appearance::cache
./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi
```

2026-08-17 folding regression fix:

- `scrollback::blocks::user::tests`: 45 passed, including Markdown-on folding and collapsed preview.
- `pi_user_markdown_flip_preserves_fold_state`: passed.
- `cargo check -p xai-grok-pager-bin --bin grok-pi`: passed.
- `git diff --check`: passed.
