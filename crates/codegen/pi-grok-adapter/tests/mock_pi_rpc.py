#!/usr/bin/env python3
"""Deterministic stand-in for Pi's current JSONL RPC mode."""
from __future__ import annotations

import json
import sys
from typing import Any


def emit(value: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def success(request: dict[str, Any], data: Any = ...) -> None:
    value: dict[str, Any] = {
        "id": request.get("id"),
        "type": "response",
        "command": request.get("type", "unknown"),
        "success": True,
    }
    if data is not ...:
        value["data"] = data
    emit(value)


def failure(request: dict[str, Any], message: str) -> None:
    emit({
        "id": request.get("id"),
        "type": "response",
        "command": request.get("type", "unknown"),
        "success": False,
        "error": message,
    })


state = {
    "model": {
        "provider": "mock",
        "id": "pi-test",
        "name": "Mock Pi",
        "reasoning": True,
        "input": ["text", "image"],
        "contextWindow": 131072,
        "thinkingLevelMap": {
            "off": "off",
            "minimal": "minimal",
            "low": "low",
            "medium": "medium",
            "high": "high",
            "xhigh": "xhigh",
        },
    },
    "thinkingLevel": "medium",
    "isStreaming": False,
    "isCompacting": False,
    "steeringMode": "all",
    "followUpMode": "all",
    "sessionFile": "/tmp/mock-pi-session.jsonl",
    "sessionId": "mock-pi-session",
    "sessionName": "Mock Pi session",
    "autoCompactionEnabled": True,
    "messageCount": 2,
    "pendingMessageCount": 0,
}

for raw in sys.stdin:
    try:
        request = json.loads(raw)
    except json.JSONDecodeError as exc:
        emit({"type": "response", "command": "parse", "success": False, "error": str(exc)})
        continue

    kind = request.get("type")
    if kind == "extension_ui_response":
        # Pi intentionally emits no acknowledgement for interactive UI replies.
        continue
    if kind == "get_state":
        success(request, state)
    elif kind == "get_available_models":
        success(request, {"models": [state["model"], {
            "provider": "mock",
            "id": "pi-fast",
            "name": "Mock Pi Fast",
            "reasoning": False,
            "input": ["text"],
            "contextWindow": 65536,
        }]})
    elif kind == "get_commands":
        success(request, {"commands": [
            {
                "name": "review",
                "description": "Review current changes",
                "source": "extension",
                "sourceInfo": {"path": "/tmp/mock-extension.ts", "origin": "extension"},
            },
            {
                "name": "skill:explain",
                "description": "Explain code",
                "source": "skill",
                "sourceInfo": {"path": "/tmp/SKILL.md", "origin": "skill"},
            },
        ]})
    elif kind == "get_messages":
        success(request, {"messages": [
            {"role": "user", "content": [{"type": "text", "text": "restored question"}]},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "restored reasoning"},
                {"type": "text", "text": "restored answer"},
            ]},
        ]})
    elif kind == "prompt":
        success(request)
        emit({"type": "extension_ui_request", "id": "ui-notify", "method": "notify",
              "message": "Mock notification", "notifyType": "info"})
        emit({"type": "extension_ui_request", "id": "ui-status", "method": "setStatus",
              "statusKey": "mock", "statusText": "Mock extension active"})
        emit({"type": "extension_ui_request", "id": "ui-widget", "method": "setWidget",
              "widgetKey": "mock-widget", "widgetLines": ["line one", "line two"],
              "widgetPlacement": "aboveEditor"})
        emit({"type": "extension_ui_request", "id": "ui-title", "method": "setTitle",
              "title": "Mock Pi"})
        emit({"type": "extension_ui_request", "id": "ui-editor-text", "method": "set_editor_text",
              "text": "prefilled prompt"})
        emit({"type": "extension_ui_request", "id": "ui-select", "method": "select",
              "title": "Select", "options": ["first", "second"], "timeout": 10000})
        emit({"type": "extension_ui_request", "id": "ui-confirm", "method": "confirm",
              "title": "Confirm", "message": "Continue?", "timeout": 10000})
        emit({"type": "extension_ui_request", "id": "ui-input", "method": "input",
              "title": "Input", "placeholder": "value", "timeout": 10000})
        emit({"type": "extension_ui_request", "id": "ui-editor", "method": "editor",
              "title": "Editor", "prefill": "multi\nline"})
        emit({"type": "agent_start"})
        emit({"type": "turn_start"})
        emit({"type": "message_start", "message": {"role": "assistant", "content": []}})
        emit({"type": "message_update", "message": {"role": "assistant", "content": []},
              "assistantMessageEvent": {"type": "thinking_delta", "contentIndex": 0,
                                         "delta": "plan", "partial": {"role": "assistant", "content": []}}})
        emit({"type": "message_update", "message": {"role": "assistant", "content": []},
              "assistantMessageEvent": {"type": "text_delta", "contentIndex": 1,
                                         "delta": "smoke ", "partial": {"role": "assistant", "content": []}}})
        emit({"type": "tool_execution_start", "toolCallId": "tool-1", "toolName": "read",
              "args": {"path": "README.md"}})
        emit({"type": "tool_execution_update", "toolCallId": "tool-1", "toolName": "read",
              "args": {"path": "README.md"}, "partialResult": {"content": [{"type": "text", "text": "partial"}]}})
        emit({"type": "tool_execution_end", "toolCallId": "tool-1", "toolName": "read",
              "result": {"content": [{"type": "text", "text": "done"}]}, "isError": False})
        emit({"type": "tool_execution_start", "toolCallId": "tool-2", "toolName": "codemode",
              "args": {"code": "return await tools.read({ path: 'README.md' });"}})
        emit({"type": "tool_execution_update", "toolCallId": "tool-2", "toolName": "codemode",
              "args": {"code": "return await tools.read({ path: 'README.md' });"},
              "partialResult": {"content": [], "details": {"calls": [
                  {"id": "tool-2/1", "name": "read", "args": "{ \"path\": \"README.md\" }",
                   "status": "running"}]}}})
        emit({"type": "tool_execution_start", "toolCallId": "tool-2/1", "toolName": "read",
              "args": {"path": "README.md"}, "parentToolCallId": "tool-2"})
        emit({"type": "tool_execution_end", "toolCallId": "tool-2/1", "toolName": "read",
              "result": {"content": [{"type": "text", "text": "README"}]}, "isError": False,
              "parentToolCallId": "tool-2"})
        emit({"type": "tool_execution_update", "toolCallId": "tool-2", "toolName": "codemode",
              "args": {"code": "return await tools.read({ path: 'README.md' });"},
              "partialResult": {"content": [], "details": {"calls": [
                  {"id": "tool-2/1", "name": "read", "args": "{ \"path\": \"README.md\" }",
                   "status": "ok", "durationMs": 12}]}}})
        emit({"type": "tool_execution_end", "toolCallId": "tool-2", "toolName": "codemode",
              "result": {"content": [
                  {"type": "text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                  {"type": "text", "text": "README"}],
                  "details": {"calls": [
                      {"id": "tool-2/1", "name": "read", "args": "{ \"path\": \"README.md\" }",
                       "status": "ok", "durationMs": 12}]}},
              "isError": False})
        emit({"type": "message_update", "message": {"role": "assistant", "content": []},
              "assistantMessageEvent": {"type": "text_delta", "contentIndex": 1,
                                         "delta": "passed", "partial": {"role": "assistant", "content": []}}})
        emit({"type": "message_end", "message": {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "plan"},
            {"type": "text", "text": "smoke passed"},
        ]}})
        emit({"type": "turn_end", "message": {"role": "assistant", "content": []}, "toolResults": []})
        emit({"type": "agent_end", "messages": [], "willRetry": False})
        emit({"type": "agent_settled"})
    elif kind == "new_session":
        success(request, {"cancelled": False})
    elif kind in {"abort", "compact", "set_model", "set_thinking_level", "set_session_name"}:
        success(request, {} if kind == "compact" else ...)
    else:
        failure(request, f"mock does not implement {kind}")
