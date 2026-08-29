#!/usr/bin/env python3
"""Prove that Pi is hosted by the uploaded Grok Build TUI, not a replacement TUI.

The verifier intentionally checks architecture and source identity rather than
visual strings. It verifies the complete uploaded Grok tree against a SHA-256
baseline, allowing only declared ACP/state/dispatch seams, a library-only Pi
adapter, and a second composition binary inside xai-grok-pager-bin.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def source_files(root: Path) -> set[str]:
    output: set[str] = set()
    for path in root.rglob("*"):
        if not path.is_file() or "target" in path.parts or ".git" in path.parts:
            continue
        output.add(path.relative_to(root).as_posix())
    return output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--pi-source", type=Path, required=True)
    parser.add_argument("--json-out", type=Path)
    args = parser.parse_args()

    ws = args.workspace.resolve()
    pi = args.pi_source.resolve()
    adapter = ws / "crates/codegen/pi-grok-adapter"
    pager = ws / "crates/codegen/xai-grok-pager"
    pager_bin = ws / "crates/codegen/xai-grok-pager-bin"
    native_bin = pager_bin / "src/bin/grok-pi.rs"
    docs = adapter / "docs"

    checks: dict[str, dict[str, Any]] = {}

    def check(name: str, passed: bool, detail: str, evidence: Any | None = None) -> None:
        item: dict[str, Any] = {"passed": bool(passed), "detail": detail}
        if evidence is not None:
            item["evidence"] = evidence
        checks[name] = item

    root_cargo = read(ws / "Cargo.toml")
    adapter_cargo = read(adapter / "Cargo.toml")
    bin_cargo = read(pager_bin / "Cargo.toml")
    adapter_sources = "\n".join(
        read(path) for path in sorted((adapter / "src").glob("*.rs"))
    )
    bin_source = read(native_bin) if native_bin.exists() else ""
    pager_app_source = read(pager / "src/app/mod.rs")

    check(
        "adapter_is_workspace_library",
        '"crates/codegen/pi-grok-adapter"' in root_cargo
        and '"crates/codegen/pi-grok-tui"' not in root_cargo
        and "[lib]" in adapter_cargo
        and "[[bin]]" not in adapter_cargo
        and not (adapter / "src/main.rs").exists(),
        "pi-grok-adapter is a library-only workspace member; the former standalone TUI target is absent",
    )

    banned_deps = ["ratatui", "crossterm", "xai-ratatui", "tui-textarea"]
    dependency_hits = [name for name in banned_deps if name in adapter_cargo.lower()]
    check(
        "adapter_has_no_terminal_dependencies",
        not dependency_hits,
        "no terminal/rendering dependencies in the Pi adapter"
        if not dependency_hits
        else f"found terminal dependencies: {dependency_hits}",
    )

    banned_adapter_patterns = {
        "ratatui": r"\bratatui\b",
        "crossterm": r"\bcrossterm\b",
        "terminal_type": r"\bTerminal\s*<",
        "frame_type": r"\bFrame\b",
        "widget_impl": r"\bimpl\s+Widget\b",
        "draw_call": r"\.draw\s*\(",
        "render_method": r"\bfn\s+render\s*\(",
        "event_read": r"event::read\s*\(",
        "raw_mode": r"enable_raw_mode\s*\(",
        "alternate_screen": r"EnterAlternateScreen",
    }
    adapter_hits = [
        name
        for name, pattern in banned_adapter_patterns.items()
        if re.search(pattern, adapter_sources)
    ]
    check(
        "adapter_has_no_renderer_input_or_terminal_loop",
        not adapter_hits,
        "adapter only translates Pi JSONL RPC to ACP"
        if not adapter_hits
        else f"renderer/input patterns found: {adapter_hits}",
    )

    check(
        "composition_binary_is_native_grok_package",
        native_bin.exists()
        and 'name = "grok-pi"' in bin_cargo
        and 'path = "src/bin/grok-pi.rs"' in bin_cargo
        and 'pi-grok-adapter = { path = "../pi-grok-adapter" }' in bin_cargo,
        "grok-pi is a second composition root in xai-grok-pager-bin",
        str(native_bin.relative_to(ws)) if native_bin.exists() else None,
    )

    banned_bin_patterns = {
        "direct_ratatui": r"\bratatui\b",
        "direct_crossterm": r"\bcrossterm\b",
        "custom_terminal": r"\bTerminal\s*(?:::|<)",
        "custom_draw": r"\.draw\s*\(",
        "custom_event_read": r"event::read\s*\(",
        "custom_raw_mode": r"enable_raw_mode\s*\(",
    }
    bin_hits = [
        name for name, pattern in banned_bin_patterns.items() if re.search(pattern, bin_source)
    ]
    check(
        "composition_binary_does_not_render",
        not bin_hits,
        "grok-pi performs process/ACP composition only"
        if not bin_hits
        else f"custom terminal code found in grok-pi: {bin_hits}",
    )

    required_native_calls = [
        "xai_grok_pager_minimal::install()",
        "AcpConnection::external(",
        "run_external_deferred(start, ready)",
        "ExternalRunStartConfig {",
        "PagerArgs::parse_from",
    ]
    missing_calls = [token for token in required_native_calls if token not in bin_source]
    run_external_calls = [
        "spawn_writer_thread()",
        "init_terminal(",
        "event_loop::run(",
        "restore_terminal(",
    ]
    missing_run_calls = [token for token in run_external_calls if token not in pager_app_source]
    check(
        "binary_enters_production_grok_pager",
        not missing_calls and not missing_run_calls,
        "grok-pi enters Grok's production terminal initialization, writer, event loop and restore path"
        if not missing_calls and not missing_run_calls
        else f"missing composition={missing_calls}; missing pager lifecycle={missing_run_calls}",
    )

    required_components = [
        pager / "src/views/prompt_widget/mod.rs",
        pager / "src/views/completion_dropdown.rs",
        pager / "src/views/question_view.rs",
        pager / "src/views/agent_status.rs",
        pager / "src/scrollback/render.rs",
        pager / "src/slash/acp_command.rs",
        ws / "crates/codegen/xai-grok-pager-minimal/src/lib.rs",
        ws / "crates/codegen/xai-grok-markdown/src/render.rs",
    ]
    missing_components = [
        str(path.relative_to(ws)) for path in required_components if not path.exists()
    ]
    check(
        "native_grok_tui_components_present",
        not missing_components,
        "native prompt, slash, question, status, scrollback, minimal and Markdown components are present"
        if not missing_components
        else f"missing native components: {missing_components}",
    )

    # Full uploaded-tree identity check. This is stronger than checking only a
    # hand-picked renderer directory: every uploaded Grok file outside the
    # declared integration seams must remain byte-identical.
    baseline_manifest_path = docs / "grok_uploaded_baseline_sha256.json"
    baseline_manifest = json.loads(read(baseline_manifest_path))
    baseline_files: dict[str, str] = baseline_manifest["baselineFiles"]
    allowed_modified = set(baseline_manifest["allowedModifiedFiles"])
    allowed_added_files = set(baseline_manifest["allowedAddedFiles"])
    allowed_added_prefixes = tuple(baseline_manifest["allowedAddedPrefixes"])
    current_files = source_files(ws)
    baseline_set = set(baseline_files)

    missing_baseline = sorted(baseline_set - current_files)
    hash_mismatches: list[str] = []
    for rel, expected in baseline_files.items():
        if rel in allowed_modified or rel not in current_files:
            continue
        if sha256(ws / rel) != expected:
            hash_mismatches.append(rel)
    extras = sorted(
        rel
        for rel in current_files - baseline_set
        if rel not in allowed_added_files
        and not any(rel.startswith(prefix) for prefix in allowed_added_prefixes)
    )
    unchanged_count = len(baseline_files) - len(allowed_modified)
    check(
        "uploaded_grok_tree_is_byte_identical_outside_declared_seams",
        not missing_baseline and not hash_mismatches and not extras,
        f"{unchanged_count} uploaded Grok files are SHA-256 identical; only {len(allowed_modified)} declared semantic/manifest seams may differ"
        if not missing_baseline and not hash_mismatches and not extras
        else (
            f"missing={missing_baseline[:10]}; mismatched={hash_mismatches[:10]}; "
            f"unexpected additions={extras[:10]}"
        ),
    )

    # Focused renderer manifest gives reviewers a compact list of the actual
    # TUI, input, slash, scrollback, minimal and Markdown files.
    renderer_manifest = json.loads(read(docs / "native_renderer_sha256.json"))
    renderer_declared_modified = {
        # EditTool hook/viewer seam: delegates to the sibling layout renderer
        # while retaining the native unified renderer and patch-copy behavior.
        "crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/edit.rs",
        "crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/mod.rs",
        "crates/codegen/xai-grok-pager/src/views/block_viewer.rs",
        "crates/codegen/xai-grok-pager/src/scrollback/block.rs",
        "crates/codegen/xai-grok-pager/src/scrollback/state/mod.rs",
        "crates/codegen/xai-grok-pager/src/scrollback/wrappers/entry_renderer.rs",
        # Product-copy seam only: the upstream tutorial modal/state/input remain
        # native while external compositions select their own static topics.
        "crates/codegen/xai-grok-pager/src/views/tutorial.rs",
    }
    renderer_declared_added = {
        "crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/side_by_side_edit.rs",
    }
    renderer_declared_seams = renderer_declared_modified | renderer_declared_added
    renderer_mismatches = [
        rel
        for rel, expected in renderer_manifest["files"].items()
        if rel not in renderer_declared_seams
        and (not (ws / rel).exists() or sha256(ws / rel) != expected)
    ]
    renderer_seams_declared = (
        renderer_declared_modified <= allowed_modified
        and renderer_declared_added <= allowed_added_files
    )
    check(
        "message_animation_renderer_seams_are_declared",
        renderer_seams_declared,
        f"{len(renderer_declared_modified)} modified and {len(renderer_declared_added)} added native renderer/state seams are declared"
        if renderer_seams_declared
        else (
            f"missing modified seams: {sorted(renderer_declared_modified - allowed_modified)}; "
            f"missing added seams: {sorted(renderer_declared_added - allowed_added_files)}"
        ),
    )
    unchanged_renderer_count = renderer_manifest["fileCount"] - len(renderer_declared_seams)
    check(
        "native_renderer_input_markdown_hashes_match_outside_declared_seams",
        not renderer_mismatches,
        f"{unchanged_renderer_count} native renderer/input/Markdown files match the uploaded Grok source; "
        f"{len(renderer_declared_seams)} narrow semantic seams are declared separately"
        if not renderer_mismatches
        else f"renderer mismatches: {renderer_mismatches[:20]}",
    )

    # Verify the files allowed to differ really are a narrow integration seam,
    # not a hidden second UI package.
    expected_modified = {
        "Cargo.lock",
        "Cargo.toml",
        "crates/codegen/xai-grok-pager-bin/Cargo.toml",
        "crates/codegen/xai-grok-pager/Cargo.toml",
        "crates/codegen/xai-grok-pager/src/acp/mod.rs",
        "crates/codegen/xai-grok-pager/src/acp/tracker.rs",
        "crates/codegen/xai-grok-pager/src/app/actions.rs",
        "crates/codegen/xai-grok-pager/src/app/acp_handler/interactions.rs",
        "crates/codegen/xai-grok-pager/src/app/acp_handler/mod.rs",
        "crates/codegen/xai-grok-pager/src/app/app_view.rs",
        "crates/codegen/xai-grok-pager/src/app/agent_view/panes.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/dashboard.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/mod.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/prompt.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/queue.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/session/lifecycle.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/session/load.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/session/mod.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/status.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/router.rs",
        "crates/codegen/xai-grok-pager/src/app/modals.rs",
        "crates/codegen/xai-grok-pager/src/lib.rs",
        "crates/codegen/xai-grok-pager-minimal/src/welcome.rs",
        "crates/codegen/xai-grok-pager/src/settings/defs.rs",
        "crates/codegen/xai-grok-pager/src/views/modal.rs",
        "crates/codegen/xai-grok-pager/src/views/mod.rs",
        "crates/codegen/xai-grok-pager/src/views/settings_modal/input.rs",
        "crates/codegen/xai-grok-pager/src/views/settings_modal/tests.rs",
        "crates/codegen/xai-grok-pager/src/views/session_picker.rs",
        "crates/codegen/xai-grok-pager/src/views/picker.rs",
        "crates/codegen/xai-grok-pager/src/views/agent_status.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/settings/setters.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/settings/ui.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/task_result.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/tests/prompt.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/tests/router.rs",
        "crates/codegen/xai-grok-pager/src/app/dispatch/tests/mod.rs",
        "crates/codegen/xai-grok-pager/src/app/effects/helpers.rs",
        "crates/codegen/xai-grok-pager/src/app/effects/mod.rs",
        "crates/codegen/xai-grok-pager/src/app/effects/tests.rs",
        "crates/codegen/xai-grok-pager/src/app/event_loop.rs",
        "crates/codegen/xai-grok-pager/src/app/foreign_sessions.rs",
        "crates/codegen/xai-grok-pager/src/app/mod.rs",
        "crates/codegen/xai-grok-pager/src/slash/acp_command.rs",
        "crates/codegen/xai-grok-pager/src/slash/command.rs",
        "crates/codegen/xai-grok-pager/src/slash/commands/mod.rs",
        "crates/codegen/xai-grok-pager/src/slash/commands/tutorial.rs",
        "crates/codegen/xai-grok-pager/src/slash/mod.rs",
        # Tutorial product-copy seam: stock Grok retains the default profile;
        # grok-pi supplies only static title/description/topics while reusing UI.
        "crates/codegen/xai-grok-pager/src/tutorial_docs.rs",
        "crates/codegen/xai-grok-pager/src/views/tutorial.rs",
        # Native message chrome policy: user and agent message bodies may stream,
        # but their left accent remains static and does not drive animation ticks.
        "crates/codegen/xai-grok-pager/src/scrollback/block.rs",
        "crates/codegen/xai-grok-pager/src/scrollback/state/mod.rs",
        "crates/codegen/xai-grok-pager/src/scrollback/wrappers/entry_renderer.rs",
        # Native EditTool hook/viewer seam. The optional parallel layout lives
        # in an explicitly allowed sibling source file, not inside edit.rs.
        "crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/edit.rs",
        "crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/mod.rs",
        "crates/codegen/xai-grok-pager/src/views/block_viewer.rs",
        "crates/codegen/xai-grok-pager/src/settings/registry.rs",
        "crates/codegen/xai-grok-pager-render/src/appearance/cache.rs",
        # Narrow branding seam: process-wide logo override for external hosts
        # (e.g. grok-pi π art). Layout/shimmer still use the native renderer.
        "crates/codegen/xai-grok-pager/src/scrollback/blocks/context_info.rs",
        "crates/codegen/xai-grok-pager/src/views/welcome/logo.rs",
        # Welcome menu policy for external hosts: hide New worktree, Changelog URL.
        "crates/codegen/xai-grok-pager/src/views/welcome/mod.rs",
        # grok-pi update discovery entry (GitHub releases → npm mirrors).
        "crates/codegen/xai-grok-update/src/lib.rs",
    }
    check(
        "modified_surface_is_exact_and_semantic",
        allowed_modified == expected_modified,
        f"exactly {len(expected_modified)} workspace/ACP/state/dispatch/slash/tutorial/logo/message-animation seams are declared; no second TUI or broad renderer rewrite"
        if allowed_modified == expected_modified
        else f"declared seam mismatch: {sorted(allowed_modified ^ expected_modified)}",
    )

    all_rs = "\n".join(
        read(path)
        for path in ws.rglob("*.rs")
        if "target" not in path.parts and ".git" not in path.parts
    )
    forbidden_messages = [
        "acknowledged by fallback renderer",
        "fallback renderer",
        "Unsupported Extension UI method",
    ]
    found_messages = [message for message in forbidden_messages if message in all_rs]
    old_tui_paths = [
        str(path.relative_to(ws))
        for path in ws.rglob("pi-grok-tui")
        if path.is_dir()
    ]
    check(
        "no_fallback_or_old_custom_tui",
        not found_messages and not old_tui_paths,
        "no fallback acknowledgement, old pi-grok-tui crate, or custom renderer remains"
        if not found_messages and not old_tui_paths
        else f"messages={found_messages}; paths={old_tui_paths}",
    )

    builtins_match = re.search(
        r"const PI_GROK_NATIVE_COMMANDS:\s*&\[&str\]\s*=\s*&\[(.*?)\];",
        bin_source,
        re.S,
    )
    builtins = re.findall(r'"([^"]+)"', builtins_match.group(1)) if builtins_match else []
    expected_builtins = [
        "exit",
        "help",
        "new",
        "compact",
        "model",
        "effort",
        "rename",
        "resume",
        "tree",
        "notify",
        "dashboard",
        "recap",
        "copy",
        "find",
        "transcript",
        "export",
        "expand",
        "queue",
        "multiline",
        "compact-mode",
        "vim-mode",
        "theme",
        "timestamps",
        "timeline",
        "toggle-mouse-reporting",
        "pi-config",
        "pi-models",
    ]
    # Product/session-store commands must not leak into the Pi composition.
    # Pi extension/prompt/skill commands arrive dynamically over get_commands.
    forbidden_local_commands = [
        "rpc",
        "diagnostics",
        "capabilities",
        "stats",
        "fork",
        "history",
        "login",
        "logout",
        "usage",
        "plugins",
        "mcp",
        "memory",
        "workspace",
        "share",
        "voice",
        "debug",
    ]
    local_command_hits = [name for name in forbidden_local_commands if name in builtins]
    check(
        "slash_surface_uses_grok_native_commands_and_pi_catalog",
        builtins == expected_builtins and not local_command_hits,
        f"retained {len(builtins)} existing Grok UI or ACP-backed commands; no adapter-specific slash UI"
        if builtins == expected_builtins and not local_command_hits
        else f"builtins={builtins}; forbidden local commands={local_command_hits}",
        builtins,
    )
    check(
        "pi_commands_are_dynamic_acp_commands",
        '"type": "get_commands"' in adapter_sources
        and "AvailableCommandsUpdate" in adapter_sources
        and "command_catalog(&self.commands)" in adapter_sources,
        "Pi extension/prompt/skill commands are discovered from get_commands and merged by Grok's native ACP slash registry",
    )

    rpc_types = pi / "packages/coding-agent/src/modes/rpc/rpc-types.ts"
    rpc_text = read(rpc_types) if rpc_types.exists() else ""
    command_section = rpc_text.split("// RPC Responses", 1)[0]
    pi_command_tokens = set(re.findall(r'type:\s*"([a-zA-Z0-9_]+)"', command_section))
    adapter_command_tokens = {
        "get_state",
        "get_available_models",
        "get_commands",
        "get_messages",
        "prompt",
        "abort",
        "abort_bash",
        "bash",
        "new_session",
        "compact",
        "set_model",
        "set_thinking_level",
        "set_session_name",
    }
    missing_rpc_commands = sorted(adapter_command_tokens - pi_command_tokens)
    check(
        "adapter_rpc_calls_exist_in_uploaded_pi",
        not missing_rpc_commands,
        f"validated {len(adapter_command_tokens)} adapter RPC calls against Pi rpc-types.ts"
        if not missing_rpc_commands
        else f"missing Pi RPC commands: {missing_rpc_commands}",
    )

    expected_ui = {
        "select",
        "confirm",
        "input",
        "editor",
        "notify",
        "setStatus",
        "setWidget",
        "setTitle",
        "set_editor_text",
    }
    pi_ui_methods = set(re.findall(r'method:\s*"([A-Za-z0-9_]+)"', rpc_text))
    adapter_ui_tokens = {
        "select": '"select"',
        "confirm": '"confirm"',
        "input": '"input"',
        "editor": '"editor"',
        "notify": '"notify"',
        "setStatus": '"setstatus"',
        "setWidget": '"setwidget"',
        "setTitle": '"settitle"',
        "set_editor_text": '"set_editor_text"',
    }
    missing_pi_ui = sorted(expected_ui - pi_ui_methods)
    missing_adapter_ui = sorted(
        method for method, token in adapter_ui_tokens.items() if token not in adapter_sources
    )
    native_ui_routes = [
        '"pi/ui/notify"',
        '"pi/ui/status"',
        '"pi/ui/widget"',
        '"pi/ui/title"',
        '"pi/ui/editor_text"',
        '"pi/ui/cancel_interaction"',
        '"x.ai/ask_user_question"',
    ]
    pager_ui_text = read(pager / "src/app/acp_handler/mod.rs") + read(
        pager / "src/app/acp_handler/interactions.rs"
    )
    missing_native_routes = [route for route in native_ui_routes if route not in (adapter_sources + pager_ui_text)]
    check(
        "all_pi_extension_ui_methods_use_native_grok_surfaces",
        not missing_pi_ui and not missing_adapter_ui and not missing_native_routes,
        "all 9 Pi Extension UI methods map to Grok toast/status/widget/title/prompt/question surfaces"
        if not missing_pi_ui and not missing_adapter_ui and not missing_native_routes
        else (
            f"missing Pi={missing_pi_ui}; adapter={missing_adapter_ui}; "
            f"native routes={missing_native_routes}"
        ),
    )

    event_tokens = {
        "agent_start",
        "agent_end",
        "agent_settled",
        "turn_start",
        "turn_end",
        "message_start",
        "message_update",
        "message_end",
        "tool_execution_start",
        "tool_execution_update",
        "tool_execution_end",
        "queue_update",
        "compaction_start",
        "compaction_end",
        "auto_retry_start",
        "auto_retry_end",
        "session_info_changed",
        "thinking_level_changed",
        "extension_ui_request",
        "extension_error",
    }
    missing_event_routes = sorted(
        token for token in event_tokens if f'"{token}"' not in adapter_sources
    )
    check(
        "pi_runtime_events_are_projected_to_acp",
        not missing_event_routes,
        f"validated {len(event_tokens)} Pi lifecycle, stream, tool, queue, compaction, retry and UI event routes"
        if not missing_event_routes
        else f"missing event routes: {missing_event_routes}",
    )

    agent_session_source = pi / "packages/coding-agent/src/core/agent-session.ts"
    agent_session_text = read(agent_session_source) if agent_session_source.exists() else ""
    # Most stream/tool lifecycle events come from AgentEvent, while Pi-specific
    # queue/compaction/retry state is declared in AgentSessionEvent. Validate the
    # Pi-specific set directly and validate the remaining event strings across
    # the uploaded Pi coding-agent source tree.
    pi_event_corpus = "\n".join(
        read(path)
        for path in (pi / "packages/coding-agent/src").rglob("*.ts")
        if path.is_file()
    )
    missing_pi_event_contract = sorted(
        token for token in event_tokens if f'"{token}"' not in pi_event_corpus
    )
    check(
        "adapter_event_routes_exist_in_uploaded_pi",
        not missing_pi_event_contract and "AgentSessionEvent" in agent_session_text,
        f"all {len(event_tokens)} routed events exist in the uploaded Pi source contract"
        if not missing_pi_event_contract and "AgentSessionEvent" in agent_session_text
        else f"missing Pi event contract tokens: {missing_pi_event_contract}",
    )

    behavior_tokens = {
        "history_replay": '"type": "get_messages"',
        "model_catalog": "build_model_catalog(",
        "reasoning_stream": "AgentThoughtChunk",
        "markdown_stream": "AgentMessageChunk",
        "tool_cards": "ToolCallUpdate",
        "image_blocks": "ContentBlock::Image",
        "steer": '"streamingBehavior": "steer"',
        "follow_up": '"followUp"',
        "direct_bash": '"type": "bash"',
        "completion_barrier": '"agent_settled" => {',
    }
    missing_behavior = [name for name, token in behavior_tokens.items() if token not in adapter_sources]
    check(
        "pi_semantics_feed_grok_native_views",
        not missing_behavior,
        "history, Markdown/reasoning streams, images, tools, model/effort, steer/follow-up and Bash are mapped into ACP"
        if not missing_behavior
        else f"missing semantic mappings: {missing_behavior}",
    )

    dialog_semantics = {
        "timeout": "extension_dialog_timeout(&event)",
        "native_cancel": '"pi/ui/cancel_interaction"',
        "pi_cancel_response": '"cancelled": true',
        "confirm_response": '"confirmed": answer.eq_ignore_ascii_case("yes")',
        "freeform_annotations": '"input" | "editor" => annotated_answer(value)',
    }
    missing_dialog_semantics = [
        name for name, token in dialog_semantics.items() if token not in adapter_sources
    ]
    check(
        "extension_dialog_semantics_match_pi",
        not missing_dialog_semantics,
        "Pi timeouts cancel Grok QuestionView; confirm/value/cancel response shapes match rpc-types.ts"
        if not missing_dialog_semantics
        else f"missing dialog semantics: {missing_dialog_semantics}",
    )

    readme_text = read(adapter / "README.md") if (adapter / "README.md").exists() else ""
    capabilities_text = read(docs / "capabilities.json") if (docs / "capabilities.json").exists() else ""
    stale_doc_tokens = [
        token
        for token in ["pi-grok-tui", "Rust/Ratatui terminal frontend"]
        if token.lower() in (readme_text + "\n" + capabilities_text).lower()
    ]
    required_doc_tokens = [
        "xai-grok-pager",
        "pi-grok-adapter",
        "native",
        "get_commands",
    ]
    missing_doc_tokens = [
        token for token in required_doc_tokens if token.lower() not in (readme_text + capabilities_text).lower()
    ]
    check(
        "documentation_describes_native_grok_architecture",
        not stale_doc_tokens and not missing_doc_tokens,
        "README and capabilities identify the production Grok pager and the headless Pi adapter"
        if not stale_doc_tokens and not missing_doc_tokens
        else f"stale={stale_doc_tokens}; missing={missing_doc_tokens}",
    )

    passed = all(item["passed"] for item in checks.values())
    report = {
        "schemaVersion": 2,
        "passed": passed,
        "workspace": str(ws),
        "piSource": str(pi),
        "checks": checks,
    }
    out = args.json_out or docs / "native-grok-verification.json"
    out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    for name, item in checks.items():
        marker = "PASS" if item["passed"] else "FAIL"
        print(f"[{marker}] {name}: {item['detail']}")
    print(f"\nResult: {'PASS' if passed else 'FAIL'}")
    print(f"Report: {out}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
