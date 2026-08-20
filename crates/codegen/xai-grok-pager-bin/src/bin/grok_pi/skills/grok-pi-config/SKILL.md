---
name: grok-pi-config
description: Use this when configuring or explaining grok-pi settings, F2 toggles, built-in tools, extensions, skills, themes, Session info, resource policy, or Pi/Grok feature flags.
---
# grok-pi configuration

This embedded skill is the compact starting point for answering grok-pi configuration questions. Use it as a progressive index: answer the immediate question first, then read the relevant detailed docs or config only when exact syntax, defaults, or troubleshooting details are needed.

## Configuration roots

- User state/config home: `$GROK_HOME`, defaulting to `~/.grok-pi`.
- User config file: `~/.grok-pi/config.toml` unless `$GROK_HOME` overrides it.
- Project config directory: `<repo>/.grok-pi`, unless `$GROK_PROJECT_DIR` overrides it.
- User skills: `$GROK_HOME/skills/<name>/SKILL.md`.
- Bundled skills cache: `$GROK_HOME/bundled/skills/<name>/SKILL.md`.
- grok-pi can migrate allowlisted legacy state from `~/.grok`, but do not treat `~/.grok` as the active grok-pi config directory.

## Default loading and F2 control

- This skill is embedded in the grok-pi binary and materialized into `$GROK_HOME/bundled/skills/grok-pi-config/SKILL.md` at startup.
- It loads by default.
- Users can turn it off in F2 with **Pi config skill** (`[ui].pi_config_skill = false`). Restart grok-pi for that change to affect new sessions.
- If the toggle is off, grok-pi removes its managed bundled cache copy before launching the session.

## How to answer grok-pi configuration questions

1. Identify the area: F2 setting, `config.toml`, environment variable, extension, skill, theme, model/provider, session/resume, resource policy, or tool behavior.
2. Give the minimal direct answer first.
3. State whether the change applies live or requires restarting grok-pi.
4. Mention the F2 label/key when a setting exists there.
5. For exact file syntax or edge cases, inspect the current config/docs instead of guessing.

## Common F2 / `[ui]` keys

- `pi_config_skill`: default-on embedded configuration skill.
- `pi_builtin_tools.*`: enable/disable built-in `read`, `bash`, `edit`, `write`, `grep`, `find`, `ls`, and `eval` tools.
- `pi_bash`, `pi_eval`, `pi_eval_v2_only`: Bash/Eval bridge behavior.
- `pi_subagents`, `pi_todo`, `pi_workflows`, `pi_goal`, `pi_loop`, `pi_btw`, `pi_ask_user_question`: native Pi/Grok features.
- `pi_cache_graph`, `pi_user_markdown`, `pi_keep_multi_agent`: UI/session behavior.
- `theme`, `auto_dark_theme`, `auto_light_theme`: appearance.
- `permission_mode`, `remember_tool_approvals`, `default_selected_permission`: approval behavior.

## Progressive loading rule

Do not dump the entire grok-pi option surface into ordinary answers. Start from this skill's compact map; open detailed docs, settings registry, or effective config only when the user's request needs complete syntax, defaults, or diagnostics.
