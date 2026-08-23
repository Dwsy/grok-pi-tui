use serde::{Deserialize, Serialize};
use xai_grok_config_types::DisplayRefreshSettings;

fn default_true() -> bool {
    true
}

fn default_pi_eval() -> String {
    "v1".to_string()
}

fn default_pi_eval_v2_language() -> String {
    "js".to_string()
}

fn default_pi_eval_v2_display_mode() -> String {
    "effects".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub max_thoughts_width: u16,
    /// Pi built-in tool preferences for the grok-pi external profile. The
    /// default preserves Pi's own default tool set; F2 writes this as a group.
    #[serde(default, skip_serializing_if = "PiBuiltinTools::is_default")]
    pub pi_builtin_tools: PiBuiltinTools,
    /// Enable grok-pi's enhanced Bash bridge. This is separate from
    /// `pi_builtin_tools.bash`: disabling it restores stock Pi Bash behavior
    /// while leaving Eval runtime injection independent.
    /// Default on; takes effect for new grok-pi sessions only.
    #[serde(default = "default_true")]
    pub pi_bash: bool,
    /// Format Bash commands and Eval code for display in detail/popup views.
    /// Display-only; executed inputs remain unchanged.
    /// Default off; F2 can enable without restart.
    #[serde(default)]
    pub pi_bash_command_format: bool,
    /// Show hover popups with expanded Write/Edit details from collapsed tool rows.
    /// Default on; display-only and live-applied.
    #[serde(default = "default_true")]
    pub write_edit_hover_popups: bool,
    /// Select the Eval bridge generation independently of `pi_bash`.
    /// `v1` keeps the legacy Python + JavaScript runtime; `v2` uses the host-RPC runtime.
    /// Default v1; takes effect for new grok-pi sessions only.
    #[serde(default = "default_pi_eval")]
    pub pi_eval: String,
    /// Select Eval v2 languages: `js`, `py`, or `all`.
    /// Default `js` preserves the pre-selector v2 behavior.
    #[serde(default = "default_pi_eval_v2_language")]
    pub pi_eval_v2_language: String,
    /// Select Eval v2 presentation: `effects` hides orchestration source while
    /// `legacy` keeps the source/result card. Display-only and live-applied.
    #[serde(default = "default_pi_eval_v2_display_mode")]
    pub pi_eval_v2_display_mode: String,
    /// Force Eval Bridge v2 and allow only the Eval tool in the Pi registry.
    /// This is a restart-required grok-pi isolation mode; it does not mutate
    /// the stored `pi_eval` or per-tool preferences underneath it.
    #[serde(default)]
    pub pi_eval_v2_only: bool,
    /// Use Pi Session Manager for external Pi `/resume`: SQLite catalog,
    /// Ctrl+F full-text search, and message preview. Requires PSM running.
    /// Disabled by default; off → Pi JSONL list only (no PSM SQLite paths).
    #[serde(default)]
    pub psm_resume_index: bool,
    /// Track write/edit preimages and allow file-only rollback from SessionTree.
    /// Default off; takes effect for new grok-pi sessions only.
    #[serde(default)]
    pub pi_tree_file_rollback: bool,
    /// Skip the "Summarize branch?" prompt when navigating the session tree.
    /// When true, Enter navigates immediately without summarization.
    #[serde(default)]
    pub pi_tree_skip_summary_prompt: bool,
    /// Report authoritative Pi lifecycle and session state to Herdr.
    /// Default off; takes effect for new sessions only.
    #[serde(default)]
    pub pi_herdr: bool,
    /// Enable the built-in Pi child-session subagent bridge.
    /// Default on; takes effect for new grok-pi sessions only.
    #[serde(default = "default_true")]
    pub pi_subagents: bool,
    /// Enable optional Subagents V2 team tools (`spawn_team`, stable agent
    /// paths, peer messaging) on top of Pi subagents.
    /// Default off; takes effect for new grok-pi sessions only.
    #[serde(default)]
    pub pi_subagents_v2: bool,
    /// Enable upstream-compatible Rhai workflows in grok-pi (xai-workflow + Pi spawn).
    /// Default off; takes effect for new grok-pi sessions only.
    #[serde(default)]
    pub pi_workflows: bool,
    /// Enable grok-pi's built-in structured `todo` tool and native TodoPane projection.
    /// Default on; takes effect for new grok-pi sessions only.
    #[serde(default = "default_true")]
    pub pi_todo: bool,
    /// Select the todo extension V2 runtime (cross-version migration from V1).
    /// Default off = V1; takes effect for new grok-pi sessions only.
    #[serde(default)]
    pub pi_todo_v2: bool,
    /// Enable Grok-style `/goal` loop for grok-pi (GoalHost + update_goal).
    /// Default off; takes effect for new grok-pi sessions only.
    #[serde(default)]
    pub pi_goal: bool,
    /// Enable Grok-style `/loop` scheduled recurring prompts for grok-pi.
    /// Default off; takes effect for new grok-pi sessions only.
    #[serde(default)]
    pub pi_loop: bool,
    /// Enable native Q&A (`ask_user_question` → Grok QuestionView) in grok-pi.
    /// Default off; takes effect for new grok-pi sessions only.
    #[serde(default)]
    pub pi_ask_user_question: bool,
    /// Notify the desktop when native Q&A arrives while grok-pi is unfocused.
    /// Default on; applies immediately.
    #[serde(default = "default_true")]
    pub pi_ask_user_question_notifications: bool,
    /// Enable native `/btw` side questions for grok-pi (Pi extension + x.ai/btw).
    /// Default off; takes effect for new grok-pi sessions only.
    #[serde(default)]
    pub pi_btw: bool,
    /// Show pi-cache-graph views (1/2/3/s/e) inside the Context modal.
    /// Default on for grok-pi; F2 can disable without restart.
    #[serde(default = "default_true")]
    pub pi_cache_graph: bool,
    /// Load the bundled grok-pi configuration skill so the agent can answer
    /// questions about grok-pi settings, F2 toggles, and config files.
    /// Default on; takes effect for new grok-pi sessions only.
    #[serde(default = "default_true")]
    pub pi_config_skill: bool,
    /// Render grok-pi user prompts with the agent markdown renderer (no
    /// collapse). Default on; F2 can disable to restore classic collapsible
    /// plain-text prompts. Applies immediately.
    #[serde(default = "default_true")]
    pub pi_user_markdown: bool,
    /// Include hidden (dotfile) entries in plain `@` file search by default,
    /// aligning with pi-main's fd `--hidden` behavior: dotfiles are listed while
    /// ignore rules stay active. `@!` additionally reveals gitignored project
    /// files, but dependency/package stores such as `.git` and `node_modules`
    /// remain hard-excluded. Default on; applies live to the current @ walker.
    #[serde(default = "default_true")]
    pub pi_at_search_hidden: bool,
    /// Keep previous agent tabs alive when `/new` starts a fresh session.
    /// Default off: `/new` fully replaces the current session (old agent tabs
    /// are dropped). When on, `/new` preserves old agents so the dashboard can
    /// switch back to them (Pi re-loads their session on demand). Applies
    /// immediately — no restart required.
    #[serde(default)]
    pub pi_keep_multi_agent: bool,
    /// Show `raw_input` args on Other/generic tool cards when expanded.
    /// Default off; F2 can enable without restart. Not fabric-only.
    #[serde(default)]
    pub show_other_tool_args: bool,
    /// Default file-list layout in `/review-*` modal: tree (cwd-relative,
    /// compact Java packages) vs flat basenames. Default off; `t` in modal
    /// toggles and persists via F2 `review_file_tree`.
    #[serde(default)]
    pub review_file_tree: bool,
    /// Include session `read` tool ops in `/review-*` file list. Default off;
    /// `r` in modal toggles and persists via F2 `review_include_reads`.
    #[serde(default)]
    pub review_include_reads: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Model ID to use for the secondary agent when forking.
    /// Defaults to the main default model (from default_models.json).
    pub fork_secondary_model: String,
    /// Optional model for display-only session recap (`/recap` + auto away recap).
    /// Empty string = fall through to slot 2/3 or the active session model.
    /// Written by F2 settings.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recap_model: String,
    /// Recap fallback model slot 2 (tried after `recap_model`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recap_model_2: String,
    /// Recap fallback model slot 3 (tried after `recap_model_2`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recap_model_3: String,
    /// Optional model for native `/btw` side questions (slot 1).
    /// Empty = fall through to slot 2/3 or the active session model.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub btw_model: String,
    /// `/btw` fallback model slot 2.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub btw_model_2: String,
    /// `/btw` fallback model slot 3.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub btw_model_3: String,
    /// Auto session-recap when returning from away. `None` = on (default).
    /// Manual `/recap` still follows agent capability (`sessionRecap`); this only
    /// gates the automatic return-from-away path. Written by F2 settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_recap: Option<bool>,
    /// Allow recap generation to include an optional Markdown Mermaid diagram.
    /// `None` = off (default). Written by F2 settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recap_mermaid: Option<bool>,
    /// Show OSC 9;4 progress indicators in the terminal tab bar.
    /// `None` = off (default). Written by F2 settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_bar: Option<bool>,
    /// Show the experimental Remote TUI footer below its projected frame.
    /// `None` = off (default). Written by F2 settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_tui_footer: Option<bool>,
    /// YOLO mode. Read by `util::config`, declared here for `serde_ignored`.
    #[serde(default)]
    pub yolo: bool,
    /// UI theme alias. Read by `util::config`, declared here for `serde_ignored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_theme: Option<String>,
    /// Compact mode. Read by pager, declared here for `serde_ignored`.
    #[serde(default)]
    pub compact_mode: bool,
    /// Simple mode. Read by pager, declared here for `serde_ignored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simple_mode: Option<bool>,
    /// Read by `load_permission_mode()`. Declared for `serde_ignored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Legacy name for `permission_mode`. Declared for `serde_ignored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
    /// Which permission option the cursor preselects on the **first**
    /// permission prompt of a session. One of `allow_once`, `allow_always`,
    /// or `reject`. After the first prompt, the cursor sticks to the user's
    /// last-used option kind. When unset, the first prompt preselects the
    /// "Always allow on all sessions" (enable-always-approve) row. Read by
    /// the pager's permission view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_selected_permission: Option<String>,
    /// Written by the pager's appearance persist module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_timestamps: Option<bool>,
    /// Timeline sidebar (per-turn tick rail in place of the scrollbar).
    /// `None` = off (client default; opt-in). Written by the pager's settings modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_timeline: Option<bool>,
    /// Snap a just-sent prompt to the viewport top. `None` = on (default).
    /// Written by the pager's settings modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_flip_on_send: Option<bool>,
    /// Ask before rewinding conversation history. `None` = on (default).
    /// Written by the pager's settings modal / rewind "Yes, and don't ask again".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_before_rewind: Option<bool>,
    /// Theme to use when the OS is in dark mode. Written by the pager's theme persist module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_dark_theme: Option<String>,
    /// Theme to use when the OS is in light mode. Written by the pager's theme persist module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_light_theme: Option<String>,
    /// Mouse-wheel and trackpad scroll speed multiplier (1–100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_speed: Option<u8>,
    /// Force scroll input classification (`auto` | `wheel` | `trackpad`).
    /// Written by the pager's settings modal; unset defaults to `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_mode: Option<String>,
    /// Invert vertical scroll direction ("natural" scrolling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert_scroll: Option<bool>,
    /// Lines per scroll tick, applied to BOTH wheel and trackpad pricing
    /// (1–10). Unset keeps the per-terminal scroll profile's values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_lines: Option<u8>,
    /// Vim-style scrollback navigation (hjkl, gg/G, /).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vim_mode: Option<bool>,
    /// How ` ```mermaid ` code blocks are rendered (`auto` | `on` | `off`).
    /// Written by the pager's settings modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_mermaid: Option<String>,
    /// Hunk-tracker mode the pager advertises to the agent (`agent_only` |
    /// `all_dirty` | `off`). Written by the pager's settings modal; read at
    /// connect time (CLI `--hunk-tracker-mode` / `GROK_HUNK_TRACKER` override
    /// it). `off` disables hunk tracking entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunk_tracker_mode: Option<String>,
    /// Voice capture chord behavior: `toggle` or `hold` (hold-to-talk; needs a
    /// Kitty-protocol terminal, else falls back to toggle). Written by the
    /// settings modal; unset defaults to `hold`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_capture_mode: Option<String>,
    /// Speech-to-text language preference for voice dictation. A Grok STT
    /// catalog code (`en`, `es`, `ja`, … — see xAI STT supported languages) or
    /// `auto` (system locale, resolved at connect). Written by the settings
    /// modal; unset leaves `[voice].language` / default `en`. When set, overrides
    /// `[voice].language` for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_stt_language: Option<String>,
    /// Whether the Ctrl+Space / F8 voice-dictation shortcut is active. Written
    /// by the settings modal; unset defaults to `true` (shortcut on). When
    /// `false` the chord is ignored — `/voice` still starts dictation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_keybind_enabled: Option<bool>,
    /// When `true`, registers `Ctrl+R` (while scrollback is focused) to toggle
    /// terminal mouse reporting (mouse capture) so users can hand selection back
    /// to the terminal for native click-drag copy/paste. Opt-in only; unset/false
    /// leaves mouse reporting always on with no toggle shortcut. The prompt keeps
    /// `Ctrl+R` for history search — focus scrollback (Esc/Tab) first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse_reporting_toggle: Option<bool>,
    /// When cancelling a parent turn with running subagents: `always_stop` stops
    /// them without prompting, `always_continue` leaves them running without
    /// prompting. Unset/`ask` shows the cancel-turn picker. Written by the pager
    /// when the user picks "Always stop" / "Always continue".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_subagents_on_turn_cancel: Option<String>,
    /// User knob for the `remember_tool_approvals` gate: when `true`, permission
    /// prompts show the granular per-tool "Always allow …" options. Written by
    /// the settings modal; requirements/env/managed/remote settings also feed the
    /// effective gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember_tool_approvals: Option<bool>,
    /// In-app drag selection highlight: `flash` | `hold` (legacy bool accepted).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_keep_text_selection"
    )]
    pub keep_text_selection: Option<String>,
    /// Legacy TTL ms; only `Some(0)` counts when `keep_text_selection` is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_highlight_duration_ms: Option<u64>,
    /// Show agent thinking/reasoning blocks in the TUI scrollback.
    /// `None` = on (client default). Written by the pager's settings modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_thinking_blocks: Option<bool>,
    /// Color the normal prompt border according to the selected thinking/
    /// reasoning effort. `None` = on (client default). Written by the pager's
    /// F2 settings modal and applied live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_border_colors: Option<bool>,
    /// Fold runs of consecutive non-destructive tool calls (reads, searches,
    /// lists) into one transcript row. `None` = on (client default). Written
    /// by the pager's settings modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_tool_verbs: Option<bool>,
    /// Show Edit tool calls as a collapsed one-line `+N/-M` diffstat summary
    /// by default (expand for the diff). `None` = off (client default).
    /// Written by the pager's settings modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_edit_blocks: Option<bool>,
    /// Scope toggled by Ctrl+O in a grok-pi session: `write_edit` (default)
    /// expands write and edit output; `all_tools` expands every tool output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctrl_o_tool_expansion: Option<String>,
    /// Header content for grok-pi Bash/run tool cards:
    /// `command_only` | `task_name` (default) | `task_name_and_command`.
    /// Written by the pager's F2 settings modal and applied live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_bash_run_display: Option<String>,
    /// Next-prompt suggestions (tab autocomplete ghost text) after each turn.
    /// `None` = on (client default). Written by the pager's settings modal;
    /// the `GROK_PROMPT_SUGGESTIONS` env var overrides at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_suggestions: Option<bool>,
    /// Startup cursor style: `None` (default) inherits the terminal's own
    /// style; `Some(true)` forces the legacy blinking block, `Some(false)` a
    /// steady block. Config-file-only knob (no /settings row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_blink: Option<bool>,
    /// `"fullscreen"` | `"minimal"`; unset → product default fullscreen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_mode: Option<String>,
    /// Retired hidden opt-in for terminal-like double/triple-click word/line
    /// selection. Superseded by `keep_text_selection = "word_select"`. Still
    /// read only when `keep_text_selection` is unset; Settings clears this on
    /// write. `"word_select"` | unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_click_action: Option<String>,
    /// Per-tip contextual-hint opt-outs (`[ui.contextual_hints]`). Each `None`
    /// inherits the remote/default (on); `Some` is a user-explicit choice that
    /// beats the remote tier. Skipped on the wire when untouched so the section
    /// only appears once a user toggles a tip.
    #[serde(default, skip_serializing_if = "ContextualHints::is_default")]
    pub contextual_hints: ContextualHints,
    /// Combine consecutive queued follow-ups into one turn. `None` = off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combine_queued_prompts: Option<bool>,
    /// Display-refresh probe + auto-cadence (`[ui.display_refresh]`). Per-field
    /// `None` inherits remote/default; skipped when untouched.
    #[serde(default, skip_serializing_if = "DisplayRefreshSettings::is_default")]
    pub display_refresh: DisplayRefreshSettings,
}

/// User-config opt-outs for the per-tip contextual hints, serialized as
/// `[ui.contextual_hints]`. Per-field `None` means "inherit remote/default";
/// `Some(bool)` is a user-explicit choice (needed so the resolver can let it
/// beat the remote tier).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextualHints {
    /// Undo tip (Ctrl+Z after a substantial draft wipe).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<bool>,
    /// Plan-mode nudge (typing a planning keyword).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode: Option<bool>,
    /// Clipboard-image input tip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_input: Option<bool>,
    /// Send-now tip after queuing a mid-turn follow-up (InterjectPrompt chord).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_now: Option<bool>,
    /// Small-screen tip (`/compact-mode` hint on smallish terminals).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub small_screen: Option<bool>,
    /// Word-select tip after double-clicking scrollback while Text selection
    /// is still fold/nav (`flash` / `hold`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_select: Option<bool>,
    /// SSH wrap session-load tip (recommend `grok wrap ssh` when the session
    /// runs over SSH without an OSC 52 sink).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_wrap: Option<bool>,
}

impl ContextualHints {
    /// True when no tip has a user-explicit value (all inherit). Lets the
    /// section stay absent from `config.toml` until the user toggles a tip.
    pub fn is_default(&self) -> bool {
        self.undo.is_none()
            && self.plan_mode.is_none()
            && self.image_input.is_none()
            && self.send_now.is_none()
            && self.small_screen.is_none()
            && self.word_select.is_none()
            && self.ssh_wrap.is_none()
    }
}

const DEFAULT_MAX_THOUGHTS_WIDTH: u16 = 120;

/// Per-tool grok-pi preferences. These are applied by the bundled Pi
/// extension at session startup, not by the Pager itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PiBuiltinTools {
    pub read: bool,
    pub bash: bool,
    pub edit: bool,
    pub write: bool,
    pub grep: bool,
    pub find: bool,
    pub ls: bool,
    pub eval: bool,
}

impl PiBuiltinTools {
    pub fn is_default(&self) -> bool {
        self.read
            && self.bash
            && self.edit
            && self.write
            && !self.grep
            && !self.find
            && !self.ls
            && !self.eval
    }
}

impl Default for PiBuiltinTools {
    fn default() -> Self {
        Self {
            read: true,
            bash: true,
            edit: true,
            write: true,
            grep: false,
            find: false,
            ls: false,
            eval: false,
        }
    }
}

fn deserialize_keep_text_selection<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bool(bool),
        Str(String),
    }

    Ok(
        Option::<Raw>::deserialize(deserializer)?.map(|raw| match raw {
            Raw::Bool(true) => "hold".to_string(),
            Raw::Bool(false) => "flash".to_string(),
            Raw::Str(s) => s,
        }),
    )
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            max_thoughts_width: DEFAULT_MAX_THOUGHTS_WIDTH,
            pi_builtin_tools: PiBuiltinTools::default(),
            pi_bash: true,
            pi_bash_command_format: false,
            write_edit_hover_popups: true,
            pi_eval: default_pi_eval(),
            pi_eval_v2_language: default_pi_eval_v2_language(),
            pi_eval_v2_display_mode: default_pi_eval_v2_display_mode(),
            pi_eval_v2_only: false,
            psm_resume_index: false,
            pi_tree_file_rollback: false,
            pi_tree_skip_summary_prompt: false,
            pi_herdr: false,
            pi_subagents: true,
            pi_subagents_v2: false,
            pi_workflows: false,
            pi_todo: true,
            pi_todo_v2: false,
            pi_goal: false,
            pi_loop: false,
            pi_ask_user_question: false,
            pi_ask_user_question_notifications: true,
            pi_btw: false,
            pi_cache_graph: true,
            pi_config_skill: true,
            pi_user_markdown: true,
            pi_at_search_hidden: true,
            pi_keep_multi_agent: false,
            show_other_tool_args: false,
            review_file_tree: false,
            review_include_reads: false,
            theme: None,
            fork_secondary_model: xai_grok_models::default_model().to_string(),
            recap_model: String::new(),
            recap_model_2: String::new(),
            recap_model_3: String::new(),
            btw_model: String::new(),
            btw_model_2: String::new(),
            btw_model_3: String::new(),
            session_recap: None,
            recap_mermaid: None,
            progress_bar: None,
            remote_tui_footer: None,
            yolo: false,
            ui_theme: None,
            compact_mode: false,
            simple_mode: None,
            permission_mode: None,
            approval_mode: None,
            default_selected_permission: None,
            show_timestamps: None,
            show_timeline: None,
            page_flip_on_send: None,
            confirm_before_rewind: None,
            auto_dark_theme: None,
            auto_light_theme: None,
            scroll_speed: None,
            scroll_mode: None,
            invert_scroll: None,
            scroll_lines: None,
            vim_mode: None,
            render_mermaid: None,
            hunk_tracker_mode: None,
            voice_capture_mode: None,
            voice_stt_language: None,
            voice_keybind_enabled: None,
            mouse_reporting_toggle: None,
            remember_tool_approvals: None,
            cancel_subagents_on_turn_cancel: None,
            keep_text_selection: None,
            selection_highlight_duration_ms: None,
            show_thinking_blocks: None,
            thinking_border_colors: None,
            group_tool_verbs: None,
            collapsed_edit_blocks: None,
            ctrl_o_tool_expansion: None,
            pi_bash_run_display: None,
            prompt_suggestions: None,
            cursor_blink: None,
            screen_mode: None,
            double_click_action: None,
            contextual_hints: ContextualHints::default(),
            combine_queued_prompts: None,
            display_refresh: DisplayRefreshSettings::default(),
        }
    }
}

impl UiConfig {
    /// The single source of truth for the timeline-sidebar default (opt-in).
    /// Flip this one line to change the default everywhere.
    ///
    // TODO: migrate the other boolean UI settings (show_timestamps,
    // simple_mode, show_thinking_blocks, …) to the same const + resolver
    // pattern. They currently duplicate their default literal across
    // cache.rs / config.rs / defs.rs / setters.rs / registry.rs and rely on
    // the registry drift-guard test to catch mismatches.
    pub const SHOW_TIMELINE_DEFAULT: bool = false;

    /// Resolved timeline-sidebar setting: the configured value, or
    /// [`Self::SHOW_TIMELINE_DEFAULT`] when unset. The one place the default
    /// is applied — every layer (cache, appearance config, settings modal)
    /// reads through here so they cannot drift.
    pub fn show_timeline_enabled(&self) -> bool {
        self.show_timeline.unwrap_or(Self::SHOW_TIMELINE_DEFAULT)
    }

    /// Default for [`Self::page_flip_on_send`] when unset.
    pub const PAGE_FLIP_ON_SEND_DEFAULT: bool = true;

    pub fn page_flip_on_send_enabled(&self) -> bool {
        self.page_flip_on_send
            .unwrap_or(Self::PAGE_FLIP_ON_SEND_DEFAULT)
    }

    /// Default for [`Self::confirm_before_rewind`] when unset.
    pub const CONFIRM_BEFORE_REWIND_DEFAULT: bool = true;

    pub fn confirm_before_rewind_enabled(&self) -> bool {
        self.confirm_before_rewind
            .unwrap_or(Self::CONFIRM_BEFORE_REWIND_DEFAULT)
    }

    /// True when the highlight should not timer-dismiss (`hold` / `word_select`,
    /// or legacy duration 0).
    pub fn keep_text_selection_enabled(&self) -> bool {
        if let Some(ref s) = self.keep_text_selection {
            return s == "hold" || s == "word_select";
        }
        matches!(self.selection_highlight_duration_ms, Some(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_flip_on_send_defaults_on() {
        assert!(UiConfig::default().page_flip_on_send_enabled());
        let off = UiConfig {
            page_flip_on_send: Some(false),
            ..Default::default()
        };
        assert!(!off.page_flip_on_send_enabled());
    }

    #[test]
    fn confirm_before_rewind_defaults_on() {
        assert!(UiConfig::default().confirm_before_rewind_enabled());
        let off = UiConfig {
            confirm_before_rewind: Some(false),
            ..Default::default()
        };
        assert!(!off.confirm_before_rewind_enabled());
    }

    #[test]
    fn keep_text_selection_enabled_precedence() {
        let mut ui = UiConfig::default();
        assert!(!ui.keep_text_selection_enabled());

        ui.selection_highlight_duration_ms = Some(0);
        assert!(ui.keep_text_selection_enabled());

        ui.selection_highlight_duration_ms = Some(150);
        assert!(!ui.keep_text_selection_enabled());

        ui.selection_highlight_duration_ms = Some(0);
        ui.keep_text_selection = Some("flash".into());
        assert!(!ui.keep_text_selection_enabled());

        ui.keep_text_selection = Some("hold".into());
        ui.selection_highlight_duration_ms = Some(999);
        assert!(ui.keep_text_selection_enabled());

        ui.keep_text_selection = Some("hold".into());
        ui.selection_highlight_duration_ms = None;
        assert!(ui.keep_text_selection_enabled());

        // `word_select` implies hold (persistent highlight).
        ui.keep_text_selection = Some("word_select".into());
        ui.selection_highlight_duration_ms = None;
        assert!(ui.keep_text_selection_enabled());
    }

    #[test]
    fn keep_text_selection_deserializes_legacy_bool_and_string() {
        let from_true: UiConfig = serde_json::from_str(r#"{"keep_text_selection": true}"#).unwrap();
        assert_eq!(from_true.keep_text_selection.as_deref(), Some("hold"));

        let from_false: UiConfig =
            serde_json::from_str(r#"{"keep_text_selection": false}"#).unwrap();
        assert_eq!(from_false.keep_text_selection.as_deref(), Some("flash"));

        let from_hold: UiConfig =
            serde_json::from_str(r#"{"keep_text_selection": "hold"}"#).unwrap();
        assert_eq!(from_hold.keep_text_selection.as_deref(), Some("hold"));

        let from_flash: UiConfig =
            serde_json::from_str(r#"{"keep_text_selection": "flash"}"#).unwrap();
        assert_eq!(from_flash.keep_text_selection.as_deref(), Some("flash"));
    }

    #[test]
    fn display_refresh_nested_deserialize() {
        let ui: UiConfig = serde_json::from_str(
            r#"{"display_refresh": {"auto_cadence_enabled": true, "floor_ms": 7, "probe_enabled": false}}"#,
        )
        .unwrap();
        assert_eq!(ui.display_refresh.auto_cadence_enabled, Some(true));
        assert_eq!(ui.display_refresh.floor_ms, Some(7));
        assert_eq!(ui.display_refresh.probe_enabled, Some(false));
        assert!(!ui.display_refresh.is_default());
    }

    #[test]
    fn display_refresh_default_is_skipped_shape() {
        assert!(DisplayRefreshSettings::default().is_default());
        let ui = UiConfig::default();
        assert!(ui.display_refresh.is_default());
    }
}
