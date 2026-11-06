# File and skill preview input routing

## Requirement

Continue Pi session 01a1a1e5-1599-76c9-998b-944010fd37c9: make @file mouse preview usable, expose the preview gestures in the footer, and support skill preview from the slash dropdown. Keep the native LineViewer surface and the ordinary Ctrl-L model picker outside preview contexts.

## Evidence before this repair

- The previous uncommitted change added chip Click handling and skill-row preview, but did not change AgentView's agent-screen shortcut promotion.
- In app/agent_view/prompt.rs, Ctrl-L reaches the ModelPicker registry branch before PromptWidget::handle_key can create a file viewer request. Widget-only tests cannot catch this.
- Running grok-pi processes (including PID 49219 started 2026-11-04 10:01:36 CST) predate the target/debug/grok-pi build timestamp 10:03:07 CST. Rebuilding does not update those processes.
- The @file dropdown has no preview hint before acceptance; the existing hint only appears beside an accepted chip. Mouse preview is not advertised.
- Current checkout contains unrelated uncommitted work; preserve it and limit edits to this behavior.

## Plan

1. Add AgentView regression coverage reproducing Ctrl-L theft and covering native mouse/skill paths.
2. Route file preview before global shortcut promotion and align footer hints with the real available actions.
3. Run focused tests and rebuild grok-pi; exercise a fresh terminal instance without sending a model prompt.

## Additional live findings

The live skill fixture appears in the slash dropdown, but Ctrl-L does nothing and the preview hint is absent. Source tracing identified two upstream breaks before mouse handling: `PiCommand` and `command_catalog` discard Pi skill source/path metadata, and `selected_skill_path` resolves the typed query rather than the selected candidate. The repair must carry the real RPC skill identity into native ACP metadata and resolve the selected row, including partial queries and arrow selection.

## Enhanced-keyboard compatibility

The native shortcut matcher preserves Shift on punctuation. The file dropdown and accepted-chip preview now both recognize colon events with no modifiers or Shift. The colon regression covers both forms, and terminal acceptance includes an explicit CSI-u Shift-colon event.

## Validation

Before-fix terminal reproduction: accepting @README.md then pressing Ctrl-L opens Pick model; a real --skill preview-fixture row exposes no preview and Ctrl-L is a no-op.

- PASS: 7 AgentView preview input/painted-hitbox regressions, including partial skill queries and arrow navigation, exit 0.
- PASS: 2 existing selected-skill-path accessor tests and 14 ACP command/metadata tests. Skill invocation remains raw slash text delegated to Pi.
- PASS: rustfmt check for all six touched Rust files; scoped diff whitespace check.
- PASS: 3 adapter command catalog regressions, including real sourceInfo identity and legacy path/location.
- Pending: refresh the 7 Pager tests after adding Shift-colon compatibility, production Rust build, and fresh terminal acceptance.
- Build-driver blocker: build.sh's optional pi-main source build fails with existing TypeScript dependency/API mismatches (for example missing TuiMainScreen and runToolCall exports). Runtime defaults to system Pi; verified installed Pi is 1.5.0. Build the Rust composition with cargo-shared.sh and test with explicit --pi-bin pi.

The skill fixture uses append_text to put the cursor after the query, matching actual typing; set_text preserves the old cursor and is not equivalent.
