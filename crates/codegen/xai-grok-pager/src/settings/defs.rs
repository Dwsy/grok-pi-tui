//! Default settings catalog — declares every user-tunable preference
//! registered in the settings modal.
//!
//! Defaults come from `UiConfig::default()` for SHELL/SHARED settings.
//! The `defaults_match_ui_config_default` test enforces this.

use super::registry::{
    DynamicEnumSource, EnumChoice, SettingCategory, SettingKind, SettingMeta, SettingOwner,
    StringValidator,
};
use crate::appearance::ScrollMode;
use crate::appearance::TextSelection;
use crate::appearance::permission_cursor::DefaultSelectedPermission;

use xai_grok_shell::agent::config::UiConfig;
use xai_grok_shell::util::config::DISPLAY_REFRESH_DEFAULT_AUTO_CADENCE_ENABLED;
use xai_grok_tools::implementations::grok_build::ask_user_question;

// ---------------------------------------------------------------------------
// Int bounds for `max_thoughts_width`.
//
// Stored as `u16` in `UiConfig`, exposed as `i64` for registry uniformity.
// 40 = min readable width on 80-col terminal; 500 = max before
// "obviously wrong" territory. `pub(crate)` so the dispatcher's clamp
// and the shell helper's defensive clamp share these bounds.
pub(crate) const MAX_THOUGHTS_WIDTH_MIN: i64 = 40;
pub(crate) const MAX_THOUGHTS_WIDTH_MAX: i64 = 500;

/// Registry key for `max_thoughts_width`. Shared between the registry
/// definition and the live-wrap-preview gate in the int stepper.
pub(crate) const MAX_THOUGHTS_WIDTH_KEY: &str = "max_thoughts_width";

const PI_EVAL_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "v1",
        display: "v1",
        description: "Legacy Eval: persistent Python and JavaScript kernels.",
    },
    EnumChoice {
        canonical: "v2",
        display: "v2",
        description: "Eval Bridge v2: JavaScript-only host RPC runtime.",
    },
];

// ---------------------------------------------------------------------------
// Theme choice catalogs.
//
// Canonical names MUST match `ThemeKind::display_name()`.
// Shared by `theme`, `auto_dark_theme`, and `auto_light_theme`;
// auto-* sub-pickers drop "auto" to avoid circular reference.
// Bounded by `MAX_PICKER_CHOICES`.
// ---------------------------------------------------------------------------

/// Full theme catalog including the "auto" meta-variant. Used by `theme` only.
const THEME_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "auto",
        display: "Auto",
        description: "Follow system dark/light appearance.",
    },
    EnumChoice {
        canonical: "pi:transparent",
        display: "Transparent",
        description: "Dark transparent; terminal bg shows through.",
    },
    EnumChoice {
        canonical: "pi:transparent-light",
        display: "Transparent Light",
        description: "Light transparent; terminal bg shows through.",
    },
    EnumChoice {
        canonical: "groknight",
        display: "Grok Night",
        description: "Neutral dark with magenta accent.",
    },
    EnumChoice {
        canonical: "grokday",
        display: "Grok Day",
        description: "Light theme for bright environments.",
    },
    EnumChoice {
        canonical: "tokyonight",
        display: "Tokyo Night",
        description: "Dark + blue-tinted; needs truecolor.",
    },
    // ASCII "Rose Pine Moon" (not "Rosé") for cross-terminal compatibility.
    EnumChoice {
        canonical: "rosepine-moon",
        display: "Rose Pine Moon",
        description: "Muted dark with mauve accents; needs truecolor.",
    },
    EnumChoice {
        canonical: "oscura-midnight",
        display: "Oscura Midnight",
        description: "Deep dark with warm accents; needs truecolor.",
    },
];

// ---------------------------------------------------------------------------
// Permission-mode catalog.
//
// Persisted values map onto runtime flags:
//   "always-approve" ↔ yolo_mode = true  (auto-approve all)
//   "auto"           ↔ auto_mode = true  (LLM classifier; not full yolo)
//   "ask"            ↔ both false (interactive prompts)
//   "default"        ↔ both false (agent's default — currently Ask)
//
// Canonical strings match `load_permission_mode`. `supports_preview:
// false` because toggling YOLO drains the permission queue (unsafe
// for per-keystroke preview).
//
// Adding new modes requires: (1) `PermissionModeKind` variant,
// (2) `EnumChoice` here, (3) `set_yolo_mode_inner` update,
// (4) `load_permission_mode` arm, (5) tests. `Plan` is excluded —
// it lives on its own `plan_mode` setting.
// ---------------------------------------------------------------------------

// Choice order: safe → classifier → unsafe (Default → Ask → Auto → Always approve).
// "Always approve" at the end creates a speed bump against
// accidental selection.
const PERMISSION_MODE_CHOICES: &[EnumChoice] = &[
    // "default" = agent's default behavior. Same as "ask" at runtime;
    // distinct on disk and in the modal indicator.
    EnumChoice {
        canonical: "default",
        display: "Default",
        description: "Use the agent's default permission behavior (currently equivalent to Ask).",
    },
    EnumChoice {
        canonical: "ask",
        display: "Ask",
        description: "Prompt for permission before tool actions.",
    },
    EnumChoice {
        canonical: "auto",
        display: "Auto",
        description: "LLM classifier approves safe tools; dangerous actions may still prompt or deny.",
    },
    EnumChoice {
        canonical: "always-approve",
        display: "Always approve",
        description: "Auto-approve every tool action. Skips ALL permission prompts.",
    },
];

// ---------------------------------------------------------------------------
// Coding-data-sharing catalog.
//
// Persisted in auth metadata (`AuthEntry::coding_data_retention_opt_out`),
// NOT config.toml. Two choices only — the pager has no `Option`/`Unset`
// representation for this field.
//
// `supports_preview: false` — toggling fires an async ACP call that
// can fail. Commit on Enter only.
// ---------------------------------------------------------------------------

const CTRL_O_TOOL_EXPANSION_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "write_edit",
        display: "Write and edit",
        description: "Expand write and edit diffs (default).",
    },
    EnumChoice {
        canonical: "all_tools",
        display: "All tool output",
        description: "Expand every tool output block.",
    },
];

// The setting's own description carries the full explanation, so the choices
// are bare labels — an empty description collapses each to a single line.
const CODING_DATA_SHARING_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "opt-in",
        display: "Opt in",
        description: "",
    },
    EnumChoice {
        canonical: "opt-out",
        display: "Opt out",
        description: "",
    },
];

// ---------------------------------------------------------------------------
// Plan-mode catalog.
//
// PAGER-owned, per-session, ACP-mediated via `session/set_mode`.
// NOT persisted to config.toml — resets every session start.
//
// Uses `on`/`off` canonical strings (not the shell's `plan`/`default`
// wire ids). `Ask` mode is intentionally not exposed here — it's
// only reachable via Shift+Tab.
//
// `supports_preview: false` — toggling fires an ACP request that
// gates tool dispatch. Commit on Enter only.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Default-selected-permission catalog.
//
// Persisted to `[ui].default_selected_permission` in config.toml. Controls
// which row the cursor preselects on the FIRST permission prompt of a
// session; after the user confirms any prompt, the cursor sticks to the
// last-used option kind. `always_allow_all_sessions` (the effective default)
// lands the cursor on the "Always allow on all sessions" / enable-always-approve
// row explicitly, via `is_enable_always_approve_option` — not via index 0; the
// other three map onto `acp::PermissionOptionKind::{AllowOnce, AllowAlways,
// Reject*}`.
//
// `supports_preview: false` — permission prompts aren't open in the modal
// background, so there's no live preview surface.
// ---------------------------------------------------------------------------

// Order matches the live permission prompt rendering (YOLO -> always-allow
// -> allow-once -> reject) so the picker mirrors what the user sees on the
// real prompt.
// Canonicals + display labels come from `DefaultSelectedPermission` (the
// single source of truth) so this table can never drift from the parser,
// the dispatch toast, or the cursor logic.
const DEFAULT_SELECTED_PERMISSION_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: DefaultSelectedPermission::AlwaysAllowAllSessions.as_canonical(),
        display: DefaultSelectedPermission::AlwaysAllowAllSessions.display(),
        description: "",
    },
    EnumChoice {
        canonical: DefaultSelectedPermission::AllowCommandAlways.as_canonical(),
        display: DefaultSelectedPermission::AllowCommandAlways.display(),
        description: "",
    },
    EnumChoice {
        canonical: DefaultSelectedPermission::AllowOnce.as_canonical(),
        display: DefaultSelectedPermission::AllowOnce.display(),
        description: "",
    },
    EnumChoice {
        canonical: DefaultSelectedPermission::Reject.as_canonical(),
        display: DefaultSelectedPermission::Reject.display(),
        description: "",
    },
];

const PLAN_MODE_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "off",
        display: "Off",
        description: "Agent runs tools and edits files directly (default).",
    },
    EnumChoice {
        canonical: "on",
        display: "On",
        description: "Agent summarises a plan and asks for approval before running tools.",
    },
];

// ---------------------------------------------------------------------------
// Mermaid-rendering catalog.
//
// SHELL-owned: persisted to `[ui].render_mermaid`, with a pager-side
// process-wide cache mirror (`appearance::cache::*_render_mermaid`) for the
// render hot path. Canonicals match `RenderMermaid::as_canonical`.
// ---------------------------------------------------------------------------

const PI_BASH_RUN_DISPLAY_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "command_only",
        display: "Command only",
        description: "Hide Task Name and show the syntax-highlighted command.",
    },
    EnumChoice {
        canonical: "task_name",
        display: "Task Name only",
        description: "Show Task Name and hide the command (default).",
    },
    EnumChoice {
        canonical: "task_name_and_command",
        display: "Task Name + command",
        description: "Show Task Name, then the syntax-highlighted command line.",
    },
];

const RENDER_MERMAID_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "auto",
        display: "Auto",
        description: "Show diagrams with a clickable row to open/copy the rendered image.",
    },
    EnumChoice {
        canonical: "on",
        display: "On",
        description: "Same as auto: always show the clickable affordance row.",
    },
    EnumChoice {
        canonical: "off",
        display: "Off",
        description: "Always show the raw Mermaid source as a code block.",
    },
];

// Scroll-input catalog. SHELL-owned, persisted to `[ui].scroll_mode`.
// Canonical strings match `ScrollMode::as_canonical` (pinned by test).
const SCROLL_MODE_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: ScrollMode::Auto.as_canonical(),
        display: "Auto-detect",
        description: "Detect wheel vs trackpad per gesture from event timing. Default.",
    },
    EnumChoice {
        canonical: ScrollMode::Wheel.as_canonical(),
        display: "Mouse wheel",
        description: "Always treat scrolling as wheel notches (fixed lines per tick).",
    },
    EnumChoice {
        canonical: ScrollMode::Trackpad.as_canonical(),
        display: "Trackpad",
        description: "Always treat scrolling as a trackpad (fractional accumulation).",
    },
];

const TEXT_SELECTION_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: TextSelection::Flash.as_canonical(),
        display: "Flash after copy",
        description: "Brief highlight on mouse-up, then clear. Double-click toggles fold. Default.",
    },
    EnumChoice {
        canonical: TextSelection::Hold.as_canonical(),
        display: "Hold until dismissed",
        description: "Keep the selection visible until Esc, click, or scroll. Double-click toggles fold.",
    },
    EnumChoice {
        canonical: TextSelection::WordSelect.as_canonical(),
        display: "Word select (terminal-like)",
        description: "Double-click selects & copies a word, triple-click a line; selection stays until dismissed.",
    },
];

// Hunk-tracker-mode catalog. SHELL-owned, persisted to `[ui].hunk_tracker_mode`.
// `disabled` is accepted as an alias for `off` at parse time but not surfaced
// as a choice.
const HUNK_TRACKER_MODE_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "agent_only",
        display: "Agent only",
        description: "Track only files the agent edits (default).",
    },
    EnumChoice {
        canonical: "all_dirty",
        display: "All dirty",
        description: "Track every git-dirty file, including external edits.",
    },
    EnumChoice {
        canonical: "off",
        display: "Off",
        description: "Disable hunk tracking entirely. Also disables LOC tracking.",
    },
];

const SCREEN_MODE_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "fullscreen",
        display: "Fullscreen",
        description: "Open plain grok in the standard fullscreen TUI. Default when unset.",
    },
    EnumChoice {
        canonical: "minimal",
        display: "Minimal",
        description: "Open plain grok in scrollback-native (minimal) mode.",
    },
];

// Voice-capture-mode catalog. SHELL-owned, persisted to `[ui].voice_capture_mode`.
// `hold` is gated on `kitty_releases_reported`; `effective_enum_choices` hides it
// elsewhere, and it falls back to `toggle` at runtime. "Kitty-protocol terminal"
// in the copy below is a deliberate user-facing simplification: Alacritty <= 0.14
// negotiates the protocol yet never reports releases, so hold stays hidden there.
const VOICE_CAPTURE_MODE_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "toggle",
        display: "Toggle",
        description: "Ctrl+Space / F8 starts dictation; press again (or Esc/Enter) to stop.",
    },
    EnumChoice {
        canonical: "hold",
        display: "Hold to talk",
        description: "Hold Ctrl+Space / F8 to record, release to stop. Needs a Kitty-protocol terminal.",
    },
];

// Voice STT language choices for the settings modal.
//
// Concrete codes must match `xai_grok_voice::STT_LANGUAGES` (official Grok STT
// catalog — https://docs.x.ai/developers/model-capabilities/audio/speech-to-text).
// `auto` is client-only; the voice crate resolves it to a concrete code before
// the STT handshake. Order: English (default), System, then remaining languages
// A–Z by English name. A registry unit test locks this list to the voice crate.
const VOICE_STT_LANGUAGE_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "en",
        display: "English",
        description: "",
    },
    EnumChoice {
        canonical: "auto",
        display: "System",
        description: "Use the system locale when it is a supported STT language; otherwise English.",
    },
    EnumChoice {
        canonical: "ar",
        display: "Arabic",
        description: "",
    },
    EnumChoice {
        canonical: "cs",
        display: "Czech",
        description: "",
    },
    EnumChoice {
        canonical: "da",
        display: "Danish",
        description: "",
    },
    EnumChoice {
        canonical: "nl",
        display: "Dutch",
        description: "",
    },
    EnumChoice {
        canonical: "fil",
        display: "Filipino",
        description: "",
    },
    EnumChoice {
        canonical: "fr",
        display: "French",
        description: "",
    },
    EnumChoice {
        canonical: "de",
        display: "German",
        description: "",
    },
    EnumChoice {
        canonical: "hi",
        display: "Hindi",
        description: "",
    },
    EnumChoice {
        canonical: "id",
        display: "Indonesian",
        description: "",
    },
    EnumChoice {
        canonical: "it",
        display: "Italian",
        description: "",
    },
    EnumChoice {
        canonical: "ja",
        display: "Japanese",
        description: "",
    },
    EnumChoice {
        canonical: "ko",
        display: "Korean",
        description: "",
    },
    EnumChoice {
        canonical: "mk",
        display: "Macedonian",
        description: "",
    },
    EnumChoice {
        canonical: "ms",
        display: "Malay",
        description: "",
    },
    EnumChoice {
        canonical: "fa",
        display: "Persian",
        description: "",
    },
    EnumChoice {
        canonical: "pl",
        display: "Polish",
        description: "",
    },
    EnumChoice {
        canonical: "pt",
        display: "Portuguese",
        description: "",
    },
    EnumChoice {
        canonical: "ro",
        display: "Romanian",
        description: "",
    },
    EnumChoice {
        canonical: "ru",
        display: "Russian",
        description: "",
    },
    EnumChoice {
        canonical: "es",
        display: "Spanish",
        description: "",
    },
    EnumChoice {
        canonical: "sv",
        display: "Swedish",
        description: "",
    },
    EnumChoice {
        canonical: "th",
        display: "Thai",
        description: "",
    },
    EnumChoice {
        canonical: "tr",
        display: "Turkish",
        description: "",
    },
    EnumChoice {
        canonical: "vi",
        display: "Vietnamese",
        description: "",
    },
];

/// Concrete-only theme catalog (excludes "auto"). Used by both
/// `auto_dark_theme` and `auto_light_theme`. No dark/light filtering —
/// the user can pair any theme with any system-appearance bucket.
const CONCRETE_THEME_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "pi:transparent",
        display: "Transparent",
        description: "Dark transparent; terminal bg shows through.",
    },
    EnumChoice {
        canonical: "pi:transparent-light",
        display: "Transparent Light",
        description: "Light transparent; terminal bg shows through.",
    },
    EnumChoice {
        canonical: "groknight",
        display: "Grok Night",
        description: "Neutral dark with magenta accent.",
    },
    EnumChoice {
        canonical: "grokday",
        display: "Grok Day",
        description: "Light theme for bright environments.",
    },
    EnumChoice {
        canonical: "tokyonight",
        display: "Tokyo Night",
        description: "Dark + blue-tinted; needs truecolor.",
    },
    EnumChoice {
        canonical: "rosepine-moon",
        display: "Rose Pine Moon",
        description: "Muted dark with mauve accents; needs truecolor.",
    },
    EnumChoice {
        canonical: "oscura-midnight",
        display: "Oscura Midnight",
        description: "Deep dark with warm accents; needs truecolor.",
    },
];

/// Child settings shown inside the "Show contextual hints" group sub-sheet.
/// Keys match the `[ui.contextual_hints]` serde fields (namespaced so they stay
/// globally unique — bare `plan_mode` collides with the plan-mode enum row).
/// They are registered as normal Bool settings but hidden from the top-level
/// list (`build_rows` skips any key that is a group child).
const CONTEXTUAL_HINTS_CHILDREN: &[&str] = &[
    "contextual_hints.undo",
    "contextual_hints.plan_mode",
    "contextual_hints.image_input",
    "contextual_hints.send_now",
    "contextual_hints.small_screen",
    "contextual_hints.word_select",
    "contextual_hints.ssh_wrap",
];

const RECAP_MODELS_CHILDREN: &[&str] = &["recap_model", "recap_model_2", "recap_model_3"];

const BTW_MODELS_CHILDREN: &[&str] = &["btw_model", "btw_model_2", "btw_model_3"];

const PI_BUILTIN_TOOLS_CHILDREN: &[&str] = &[
    "pi_builtin_tools.read",
    "pi_builtin_tools.bash",
    "pi_builtin_tools.edit",
    "pi_builtin_tools.write",
    "pi_builtin_tools.grep",
    "pi_builtin_tools.find",
    "pi_builtin_tools.ls",
    "pi_builtin_tools.eval",
];

/// Build the catalog. Called once at process start via
/// `SettingsRegistry::defaults()`.
pub fn default_settings() -> Vec<SettingMeta> {
    // Shell schema defaults, used as registry source of truth.
    let ui_default = UiConfig::default();

    vec![
        SettingMeta {
            key: "compact_mode",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shared,
            label: "Compact mode",
            description: "Reduce padding around messages for more content density. \
                          Auto-enabled while the terminal is 20 rows or shorter.",
            keywords: &[
                "compact", "density", "padding", "tight", "small", "screen", "auto",
            ],
            kind: SettingKind::Bool {
                default: ui_default.compact_mode,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "screen_mode",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Default screen mode",
            description: "How plain grok opens next time: Fullscreen (default when unset) or \
                          Minimal. Writes [ui] screen_mode in config.toml. Restart required. \
                          Switch this session only with /minimal or /fullscreen.",
            keywords: &[
                "screen",
                "mode",
                "minimal",
                "fullscreen",
                "full",
                "scrollback",
                "native",
                "alt-screen",
                "render",
                "default",
            ],
            kind: SettingKind::Enum {
                default: "fullscreen",
                choices: SCREEN_MODE_CHOICES,
                supports_preview: false,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "show_timestamps",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shared,
            label: "Show timestamps",
            description: "Show clock time next to user messages and agent responses.",
            keywords: &["timestamps", "time", "clock", "date"],
            kind: SettingKind::Bool {
                // `Option<bool>` — `None` treated as `true`.
                default: ui_default.show_timestamps.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "show_timeline",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shared,
            label: "Timeline sidebar",
            description: "Per-turn tick rail in place of the scrollbar: hover previews a turn, click jumps to it.",
            keywords: &["timeline", "sidebar", "ticks", "turns", "navigator", "rail"],
            kind: SettingKind::Bool {
                // Single source: UiConfig::SHOW_TIMELINE_DEFAULT (opt-in).
                default: ui_default.show_timeline_enabled(),
            },
            restart_required: false,
            // Minimal mode has no interactive scrollback pane for the rail.
            hidden_in_minimal: true,
            external_only: false,
        },
        SettingMeta {
            key: "page_flip_on_send",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shared,
            label: "Snap prompt to top on send",
            description: "When you send a prompt, scroll it to the top of the screen so the \
                          response starts on a fresh page (default). Turn off to leave the scroll \
                          position unchanged when you send.",
            keywords: &[
                "page", "flip", "send", "prompt", "scroll", "top", "jump", "auto", "snap",
            ],
            kind: SettingKind::Bool {
                default: ui_default.page_flip_on_send_enabled(),
            },
            restart_required: false,
            hidden_in_minimal: true,
            external_only: false,
        },
        SettingMeta {
            key: "combine_queued_prompts",
            category: SettingCategory::Editor,
            owner: SettingOwner::Shared,
            label: "Combine queued prompts",
            description: "Merge consecutive plain follow-ups into one model turn \
                          (TUI shows one bubble each). Stops at bash, slash commands, \
                          cron, expanded skills, image follow-ups, or a row under edit. \
                          Default off; applies on local drain and shell promote.",
            keywords: &["queue", "combine", "batch", "follow-up", "merge", "pending"],
            kind: SettingKind::Bool {
                default: ui_default.combine_queued_prompts.unwrap_or(false),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "confirm_before_rewind",
            category: SettingCategory::Editor,
            owner: SettingOwner::Shared,
            label: "Confirm before rewind",
            description: "Ask before rewinding conversation history. Turn off to rewind \
                          immediately when you pick a turn.",
            keywords: &["rewind", "confirm", "undo", "history", "ask", "prompt"],
            kind: SettingKind::Bool {
                default: ui_default.confirm_before_rewind_enabled(),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            // Persisted key stays `simple_mode`; the user-facing label
            // distinguishes the PROMPT vim-mode (this setting) from the
            // scrollback `vim_mode` keybindings below.
            key: "simple_mode",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shared,
            label: "Disable vim input mode",
            description: "Use plain readline-style input instead of vim keys in the prompt. Experimental.",
            keywords: &[
                "simple",
                "ascii",
                "minimal",
                "plain",
                "vim",
                "readline",
                "experimental",
                "editor",
                "input",
                "prompt",
            ],
            kind: SettingKind::Bool {
                // `Option<bool>` — `None` treated as `true`.
                default: ui_default.simple_mode.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned, persisted to `[ui].vim_mode` in config.toml.
        // Defaults to the same value main's `appearance::persist::VIM_MODE_DEFAULT`
        // shipped with. Bundled next to `simple_mode` because they pair up:
        // simple_mode controls the input editor's vim behaviour,
        // vim_mode controls the scrollback's vim behaviour.
        SettingMeta {
            key: "vim_mode",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Vim scrollback navigation",
            description: "Enable vim keys (h/j/k/l, gg/G, /) for navigating the scrollback. Does not affect the input prompt.",
            keywords: &[
                "vim",
                "scrollback",
                "navigation",
                "hjkl",
                "keys",
                "keybindings",
                "scroll",
            ],
            kind: SettingKind::Bool {
                default: ui_default.vim_mode.unwrap_or(false),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // --- theme + auto themes ---------------------------------------------
        SettingMeta {
            key: "theme",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shared,
            label: "Theme",
            description: "Color theme for the pager UI.",
            keywords: &[
                "theme",
                "color",
                "colour",
                "palette",
                "appearance",
                "dark",
                "light",
            ],
            kind: SettingKind::Enum {
                // `Option<String>` — `None` resolved to "groknight".
                default: "groknight",
                choices: THEME_CHOICES,
                supports_preview: true,
            },
            restart_required: false,
            hidden_in_minimal: true,
            external_only: false,
        },
        SettingMeta {
            key: "auto_dark_theme",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shared,
            label: "Auto dark theme",
            description: "Theme to use when the system is in dark mode (only with theme=auto).",
            keywords: &["auto", "dark", "theme", "system", "appearance", "night"],
            kind: SettingKind::Enum {
                // `Option<String>` — `None` falls back to "groknight".
                default: "groknight",
                choices: CONCRETE_THEME_CHOICES,
                supports_preview: true,
            },
            restart_required: false,
            hidden_in_minimal: true,
            external_only: false,
        },
        SettingMeta {
            key: "auto_light_theme",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shared,
            label: "Auto light theme",
            description: "Theme to use when the system is in light mode (only with theme=auto).",
            keywords: &["auto", "light", "theme", "system", "appearance", "day"],
            kind: SettingKind::Enum {
                // `Option<String>` — `None` falls back to "grokday".
                default: "grokday",
                choices: CONCRETE_THEME_CHOICES,
                supports_preview: true,
            },
            restart_required: false,
            hidden_in_minimal: true,
            external_only: false,
        },
        // SHELL-owned: persisted to `[ui].render_mermaid`, with a pager-side
        // process-wide cache mirror (like `vim_mode`). Default pinned to "auto"
        // by `defaults_match_ui_config_default`.
        SettingMeta {
            key: "render_mermaid",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Render Mermaid diagrams",
            description: "How ```mermaid code blocks are shown: auto/on add a clickable row to \
                          open the rendered diagram; off shows the raw source.",
            keywords: &[
                "mermaid",
                "diagram",
                "diagrams",
                "render",
                "flowchart",
                "graph",
                "chart",
            ],
            kind: SettingKind::Enum {
                default: "auto",
                choices: RENDER_MERMAID_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // Security-relevant: "always-approve" bypasses all permission prompts.
        // Modal reads live state from `PagerLocalSnapshot.yolo_mode`
        // (not `ui.permission_mode`) to reflect Ctrl+O toggles immediately.
        SettingMeta {
            key: "permission_mode",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Permission mode",
            description: "Default uses the agent's built-in behavior; \
                          Ask prompts for each tool action; \
                          Auto uses an LLM classifier for risky tools; \
                          Always approve grants all permissions automatically.",
            keywords: &[
                "permission",
                "approve",
                "yolo",
                "agent",
                "always",
                "ask",
                "auto",
                "classifier",
                "tool",
                "danger",
            ],
            kind: SettingKind::Enum {
                default: "ask",
                choices: PERMISSION_MODE_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned `[ui].remember_tool_approvals`. Gates the per-tool
        // "Always allow …" prompt options. `restart_required` — resolved at
        // permission-manager spawn (also fed by env/requirements/managed/remote settings).
        SettingMeta {
            key: "remember_tool_approvals",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Remember tool approvals",
            description: "Show \"Always allow\" options in permission prompts so you can stop \
                          being re-asked about a specific command or tool. Applies in ask and \
                          auto; Always-approve still skips all prompts. Restart required.",
            keywords: &[
                "permission",
                "approve",
                "approval",
                "always",
                "allow",
                "remember",
                "tool",
                "command",
                "kubectl",
                "ask",
                "again",
                "whitelist",
            ],
            kind: SettingKind::Bool {
                default: ui_default.remember_tool_approvals.unwrap_or(false),
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: false,
        },
        // PAGER-owned; default pinned by `defaults_match_pager_state`.
        SettingMeta {
            key: "multiline_mode",
            category: SettingCategory::Editor,
            owner: SettingOwner::Pager,
            label: "Multiline",
            description: "When on, Enter inserts a newline and Shift+Enter sends. Resets each session.",
            keywords: &["multiline", "newline", "input", "editor", "enter"],
            kind: SettingKind::Bool { default: false },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned. Reads from `pager.current_model_name` (not
        // `cfg.models.default`) so the modal reflects `/model` switches.
        // Empty-string default = "no opinion" / use shell's resolution.
        SettingMeta {
            key: "default_model",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Default model",
            description: "Model used for new sessions. Changing this also switches the active session. Pick `(no override)` to clear.",
            keywords: &["model", "default", "agent", "llm", "grok", "switch"],
            kind: SettingKind::DynamicEnum {
                default: "",
                source: DynamicEnumSource::ActiveModelCatalog,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHARED. `u16` in UiConfig, widened to `i64` for registry.
        // Width changes apply on the next render frame.
        SettingMeta {
            key: MAX_THOUGHTS_WIDTH_KEY,
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shared,
            label: "Max thoughts width",
            description: "Column width budget for the agent's thoughts panel (40-500, default 120).",
            keywords: &[
                "thoughts",
                "width",
                "max",
                "thinking",
                "panel",
                "reasoning",
                "columns",
            ],
            kind: SettingKind::Int {
                default: ui_default.max_thoughts_width as i64,
                min: MAX_THOUGHTS_WIDTH_MIN,
                max: MAX_THOUGHTS_WIDTH_MAX,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned: `[ui].show_thinking_blocks` + process-wide cache. Default ON.
        SettingMeta {
            key: "show_thinking_blocks",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Show thinking blocks",
            description: "Show agent thinking/reasoning blocks in the scrollback while streaming.",
            keywords: &[
                "thinking",
                "reasoning",
                "thoughts",
                "blocks",
                "show",
                "hide",
            ],
            kind: SettingKind::Bool {
                default: ui_default.show_thinking_blocks.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned: `[ui].thinking_border_colors` + process-wide cache.
        // Default ON; applies to the next render frame.
        SettingMeta {
            key: "thinking_border_colors",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Thinking border colors",
            description: "Color the normal prompt border based on the selected thinking/reasoning level.",
            keywords: &[
                "thinking",
                "reasoning",
                "effort",
                "prompt",
                "border",
                "color",
                "level",
            ],
            kind: SettingKind::Bool {
                default: ui_default.thinking_border_colors.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned: `[ui].prompt_suggestions` + process-wide cache. Default ON.
        // The `GROK_PROMPT_SUGGESTIONS` env var overrides at runtime.
        SettingMeta {
            key: "prompt_suggestions",
            category: SettingCategory::Editor,
            owner: SettingOwner::Shell,
            label: "Prompt suggestions",
            description: "After each turn, predict your likely next prompt and show it as \
                          ghost text in the input (Tab to accept). Uses a small model call \
                          per turn.",
            keywords: &[
                "prompt",
                "suggestion",
                "suggestions",
                "autocomplete",
                "ghost",
                "tab",
                "predict",
                "next",
            ],
            kind: SettingKind::Bool {
                default: ui_default.prompt_suggestions.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // PAGER-owned, persisted to `[prompt].cursor` in pager.toml.
        // Live-applied to every agent through `AppView::set_appearance`.
        SettingMeta {
            key: "prompt_cursor",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Pager,
            label: "Prompt cursor",
            description: "Cursor in the input box: native, block, underline, bar, or one single-column character.",
            keywords: &[
                "prompt", "input", "cursor", "caret", "symbol", "glyph", "block", "bar",
            ],
            kind: SettingKind::String {
                default: "native",
                validator: StringValidator::PromptCursor,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // PAGER-owned, persisted to `[scrollback.scroll].respect_manual_folds`
        // in pager.toml (NOT config.toml). Live value is the appearance
        // config (`AppView::set_appearance` fans changes out to every agent);
        // the flag is read at use time, so no restart.
        SettingMeta {
            key: "respect_manual_folds",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Pager,
            label: "Respect manual folds",
            description: "Keep manually folded blocks as-is while streaming and stop \
                          auto-scroll when expanding a block. Experimental.",
            keywords: &[
                "fold", "pin", "collapse", "expand", "thinking", "follow", "scroll",
            ],
            kind: SettingKind::Bool {
                default: crate::appearance::ScrollConfig::default().respect_manual_folds,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned: `[ui].group_tool_verbs` + process-wide cache. Default ON.
        SettingMeta {
            key: "group_tool_verbs",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Group tool calls",
            description: "Fold consecutive read/search/list tool calls and subagent rows into \
                          one summary row; finished thoughts fold into the group too.",
            keywords: &[
                "group", "tool", "verbs", "fold", "collapse", "read", "search", "summary",
                "thinking", "subagent",
            ],
            kind: SettingKind::Bool {
                default: ui_default.group_tool_verbs.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned: `[ui].collapsed_edit_blocks` + process-wide cache.
        // Default OFF (rollout flag; remote settings / managed config can enable).
        SettingMeta {
            key: "collapsed_edit_blocks",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Collapsed edit blocks",
            description: "Show edits as one-line +N/-M diffstat summaries and merge \
                          back-to-back edits to the same file into one block; expand a \
                          row to see the diffs.",
            keywords: &[
                "edit",
                "edits",
                "diff",
                "diffstat",
                "collapse",
                "collapsed",
                "summary",
                "expand",
                "one-line",
                "merge",
                "coalesce",
            ],
            kind: SettingKind::Bool {
                default: ui_default.collapsed_edit_blocks.unwrap_or(false),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // PAGER-owned, grok-pi-only renderer preference. Kept process-local so
        // enabling the experimental layout does not expand the shared shell
        // config schema. Default OFF; narrow terminals still render unified.
        SettingMeta {
            key: "side_by_side_edit",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Pager,
            label: "Side-by-side edit diffs",
            description: "Show old and new edit content in parallel columns when the terminal is wide enough; narrow layouts stay unified.",
            keywords: &[
                "edit",
                "diff",
                "side-by-side",
                "split",
                "parallel",
                "old",
                "new",
                "columns",
                "pi",
            ],
            kind: SettingKind::Bool {
                default: crate::appearance::cache::SIDE_BY_SIDE_EDIT_DEFAULT,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        // SHELL-owned: `[ui].ctrl_o_tool_expansion`. Only grok-pi consumes this
        // setting; normal Grok retains its existing Ctrl+O behavior.
        SettingMeta {
            key: "ctrl_o_tool_expansion",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Ctrl+O tool expansion",
            description: "Choose whether Ctrl+O expands write/edit diffs or all tool output in grok-pi.",
            keywords: &[
                "ctrl+o", "tool", "expand", "collapse", "write", "edit", "all",
            ],
            kind: SettingKind::Enum {
                default: "write_edit",
                choices: CTRL_O_TOOL_EXPANSION_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned: `[ui].pi_bash_run_display`; only grok-pi supplies
        // `task_name`, and the Execute renderer reads the live cache.
        SettingMeta {
            key: "pi_bash_run_display",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Bash run display",
            description: "Choose whether Bash/run cards show Task Name, command, or both.",
            keywords: &[
                "bash", "run", "command", "task", "name", "header", "batch", "pi",
            ],
            kind: SettingKind::Enum {
                default: "task_name",
                choices: PI_BASH_RUN_DISPLAY_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        // SHELL-owned: `[ui].pi_bash_command_format`; display-only.
        SettingMeta {
            key: "pi_bash_command_format",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Format Bash/Eval display",
            description: "Display Bash commands and Eval code with readable line breaks in permission, run-detail, and Eval popup views. Executed inputs are unchanged. Default off.",
            keywords: &[
                "bash",
                "eval",
                "command",
                "code",
                "format",
                "pretty",
                "operator",
                "line",
                "break",
                "permission",
                "popup",
                "pi",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_bash_command_format,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        // SHELL-owned: `[ui].write_edit_hover_popups`; display-only.
        SettingMeta {
            key: "write_edit_hover_popups",
            category: SettingCategory::Popups,
            owner: SettingOwner::Shell,
            label: "Write/Edit hover popups",
            description: "Show a bounded hover popup with expanded Write and Edit details from collapsed tool rows. Default on.",
            keywords: &[
                "write", "edit", "popup", "hover", "tool", "diff", "preview", "details",
            ],
            kind: SettingKind::Bool {
                default: ui_default.write_edit_hover_popups,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned: `[ui.display_refresh].auto_cadence_enabled`. Restart-
        // required (cadence pinned at startup); hidden in minimal.
        SettingMeta {
            key: "display_refresh_auto_cadence",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Match display refresh rate",
            description: "On high-refresh displays, the TUI will stream/scroll faster \
                          to match the display. Off keeps the classic ~60 Hz cadence. \
                          Restart required.",
            keywords: &[
                "display", "refresh", "rate", "hz", "cadence", "fps", "smooth", "scroll", "stream",
                "high", "120", "144",
            ],
            kind: SettingKind::Bool {
                // Nested Option: None inherits DISPLAY_REFRESH_DEFAULT_AUTO_CADENCE_ENABLED.
                default: ui_default
                    .display_refresh
                    .auto_cadence_enabled
                    .unwrap_or(DISPLAY_REFRESH_DEFAULT_AUTO_CADENCE_ENABLED),
            },
            restart_required: true,
            hidden_in_minimal: true,
            external_only: false,
        },
        // SHELL-owned, persisted to `[ui].scroll_speed` in config.toml.
        SettingMeta {
            key: "scroll_speed",
            category: SettingCategory::Mouse,
            owner: SettingOwner::Shell,
            label: "Scroll speed",
            description: "Mouse-wheel and trackpad scroll speed multiplier (1-100). Higher = faster.",
            keywords: &[
                "scroll", "speed", "mouse", "wheel", "trackpad", "fast", "slow",
            ],
            kind: SettingKind::Int {
                default: ui_default.scroll_speed.unwrap_or(50) as i64,
                min: 1,
                max: 100,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned `auto` | `wheel` | `trackpad` on `[ui].scroll_mode`.
        SettingMeta {
            key: "scroll_mode",
            category: SettingCategory::Mouse,
            owner: SettingOwner::Shell,
            label: "Scroll input",
            description: "Force wheel or trackpad scroll behavior when auto-detection \
                          misreads your device.",
            keywords: &[
                "scroll", "mode", "wheel", "trackpad", "mouse", "detect", "force", "input",
            ],
            kind: SettingKind::Enum {
                default: ui_default
                    .scroll_mode
                    .as_deref()
                    .and_then(ScrollMode::from_canonical)
                    .unwrap_or_default()
                    .as_canonical(),
                choices: SCROLL_MODE_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned, persisted to `[ui].scroll_lines`. One knob for BOTH
        // wheel and trackpad lines-per-tick; the registered default 3 matches
        // most terminal profiles, but until the user first commits a value
        // the per-terminal profile stays in charge (cache unset → no override).
        SettingMeta {
            key: "scroll_lines",
            category: SettingCategory::Mouse,
            owner: SettingOwner::Shell,
            label: "Scroll lines",
            description: "Lines per scroll tick for both wheel and trackpad (1-10). \
                          Until set, each terminal's own profile applies.",
            keywords: &[
                "scroll", "lines", "tick", "notch", "wheel", "trackpad", "mouse",
            ],
            kind: SettingKind::Int {
                default: ui_default.scroll_lines.map(i64::from).unwrap_or(3),
                min: 1,
                max: 10,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned: `[ui].invert_scroll` + process-wide cache. Default OFF.
        SettingMeta {
            key: "invert_scroll",
            category: SettingCategory::Mouse,
            owner: SettingOwner::Shell,
            label: "Invert scroll",
            description: "Reverse vertical scroll direction (natural scrolling).",
            keywords: &[
                "invert",
                "scroll",
                "natural",
                "direction",
                "reverse",
                "mouse",
                "trackpad",
            ],
            kind: SettingKind::Bool {
                default: ui_default.invert_scroll.unwrap_or(false),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned `flash` | `hold` on `[ui].keep_text_selection`.
        SettingMeta {
            key: "keep_text_selection",
            category: SettingCategory::Mouse,
            owner: SettingOwner::Shell,
            label: "Text selection",
            description: "How long in-app selection stays on screen and what double-click does (fold vs. select & copy a word). For your terminal or multiplexer's own selection, hold Shift while dragging (native copy).",
            keywords: &[
                "selection",
                "drag",
                "copy",
                "flash",
                "hold",
                "shift",
                "native",
                "mouse",
                "tmux",
                "double",
                "double-click",
                "word",
                "terminal",
            ],
            kind: SettingKind::Enum {
                default: TextSelection::Flash.as_canonical(),
                choices: TEXT_SELECTION_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned. Persisted in auth metadata (not config.toml).
        // Reads from `PagerLocalSnapshot.coding_data_sharing_opt_out`.
        // Default "opt-out" matches `AuthEntry::coding_data_retention_opt_out = true`
        // (safer consumer default; server enrichment may still opt the user in).
        // ZDR / non-admin guards are enforced at dispatch time.
        // Do not put "telemetry" in keywords — that word is the config-file
        // analytics toggle (Monitoring / Configuration docs).
        SettingMeta {
            key: "coding_data_sharing",
            category: SettingCategory::Privacy,
            owner: SettingOwner::Shell,
            label: "Coding data, retention, and training",
            description: "Opt-in to provide SpaceXAI the ability to retain and train on \
                          coding data, e.g., prompts, traces, & metrics, for training and \
                          debugging purposes. We may still collect simple user metrics, \
                          e.g. how many times you use the product or a feature.",
            keywords: &[
                "privacy",
                "data",
                "sharing",
                "coding",
                "retention",
                "training",
                "opt-in",
                "opt-out",
            ],
            kind: SettingKind::Enum {
                default: "opt-out",
                choices: CODING_DATA_SHARING_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned, persisted to `[ui].default_selected_permission` in
        // config.toml. Read by the pager via `appearance::permission_cursor`.
        // Canonical `always_allow_all_sessions` (the effective default) lands
        // the first prompt's cursor on the enable-always-approve row;
        // subsequent prompts stick to the last-used kind.
        SettingMeta {
            key: "default_selected_permission",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Default selected permission",
            description: "Which row the cursor preselects on permission prompts.",
            keywords: &[
                "permission",
                "approval",
                "cursor",
                "preselect",
                "default",
                "sticky",
                "last",
                "used",
                "yes",
                "no",
                "reject",
                "allow",
            ],
            kind: SettingKind::Enum {
                default: DefaultSelectedPermission::AlwaysAllowAllSessions.as_canonical(),
                choices: DEFAULT_SELECTED_PERMISSION_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned `[toolset.ask_user_question].timeout_enabled`. Surfaces
        // the user-config layer of the tiered timeout gate (requirements/env/
        // managed/remote settings feed the effective value at agent build); the
        // default is the resolver-shared const. `restart_required` — resolved
        // when an agent is built, like `remember_tool_approvals`.
        SettingMeta {
            key: "toolset.ask_user_question.timeout_enabled",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Ask-Question timeout",
            description: "When on, the ask_user_question tool will time out after a set period \
                          of time instead of infinitely blocking.",
            keywords: &[
                "ask",
                "question",
                "questionnaire",
                "timeout",
                "ask_user_question",
                "block",
                "wait",
                "forever",
                "tool",
            ],
            kind: SettingKind::Bool {
                default: ask_user_question::DEFAULT_ASK_USER_QUESTION_TIMEOUT_ENABLED,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: false,
        },
        // PAGER-owned, ACP-mediated. Reads from
        // `PagerLocalSnapshot.plan_mode_active`. Default "off" matches
        // `AgentView::new`'s `plan_mode_active = false`.
        SettingMeta {
            key: "plan_mode",
            category: SettingCategory::Agent,
            owner: SettingOwner::Pager,
            label: "Plan mode",
            description: "When on, the agent summarises a plan before running tools or making edits.",
            keywords: &[
                "plan", "mode", "agent", "summary", "approval", "review", "session",
            ],
            kind: SettingKind::Enum {
                default: "off",
                choices: PLAN_MODE_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned startup-time settings (restart_required: true).
        // The running pager doesn't re-read these mid-session.
        SettingMeta {
            key: "show_tips",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Show tips",
            description: "Show the tip-of-the-day banner on startup. Restart required.",
            keywords: &[
                "tips", "tip", "show", "banner", "welcome", "startup", "launch",
            ],
            kind: SettingKind::Bool { default: true },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: false,
        },
        // Contextual hints: one Advanced row that opens a sub-sheet of per-tip
        // toggles. Applies live (restart_required: false); the group carries no
        // value and its children are hidden from the top-level list.
        SettingMeta {
            key: "contextual_hints",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Show contextual hints",
            description: "Show brief, in-context keyboard hints as you work; \
                          toggle each one individually.",
            keywords: &[
                "contextual",
                "hints",
                "tips",
                "undo",
                "plan",
                "nudge",
                "image",
                "clipboard",
                "ephemeral",
                "send",
                "interject",
                "queue",
                // Child-specific terms: the per-tip children are hidden from the
                // top-level list, so mirror their search words here to keep a
                // query like "ctrl+z" or "shift+tab" from dead-ending.
                "ctrl+z",
                "draft",
                "wipe",
                "mode",
                "shift+tab",
                "paste",
                "input",
                "enter",
                "follow-up",
                "small",
                "screen",
                "compact",
                "ssh",
                "wrap",
                "remote",
            ],
            kind: SettingKind::Group {
                children: CONTEXTUAL_HINTS_CHILDREN,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "auto_update",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Auto-update",
            description: "Automatically download and install pager updates on startup. \
                          Restart required.",
            keywords: &[
                "auto", "update", "updates", "upgrade", "version", "install", "channel",
            ],
            kind: SettingKind::Bool { default: true },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned, persisted to `[ui].hunk_tracker_mode`. Restart-required:
        // the mode is read once when the session connects.
        SettingMeta {
            key: "hunk_tracker_mode",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Hunk tracker",
            description: "Which file changes the agent tracks as hunks. \
                          Off disables tracking (and LOC stats) entirely. \
                          Restart required.",
            keywords: &[
                "hunk", "tracker", "tracking", "diff", "changes", "git", "loc", "off", "disable",
            ],
            kind: SettingKind::Enum {
                default: "agent_only",
                choices: HUNK_TRACKER_MODE_CHOICES,
                supports_preview: false,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned, persisted to `[ui].voice_keybind_enabled`. Default ON —
        // `None` (inherit) reads as `true`. Disables only the Ctrl+Space / F8
        // chord; `/voice` (and Esc / the recording-row `[stop]`) keep working.
        SettingMeta {
            key: "voice_keybind_enabled",
            category: SettingCategory::Editor,
            owner: SettingOwner::Shell,
            label: "Voice shortcut",
            description: "Enable the Ctrl+Space / F8 shortcut for voice dictation. \
                          When off, the keys are ignored; /voice still starts \
                          dictation.",
            keywords: &[
                "voice",
                "dictation",
                "mic",
                "microphone",
                "speech",
                "stt",
                "keybinding",
                "hotkey",
                "ctrl+space",
                "f8",
                "disable",
            ],
            kind: SettingKind::Bool {
                default: ui_default.voice_keybind_enabled.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned, persisted to `[ui].voice_capture_mode`. The `hold` choice
        // is hidden on terminals without key-release reporting (see
        // `effective_enum_choices`) and falls back to `toggle` at runtime.
        SettingMeta {
            key: "voice_capture_mode",
            category: SettingCategory::Editor,
            owner: SettingOwner::Shell,
            label: "Voice capture",
            description: "How the voice chord (Ctrl+Space / F8) behaves: Toggle \
                          (press to start/stop) or Hold to talk (hold to record, \
                          release to stop; needs a Kitty-protocol terminal).",
            keywords: &[
                "voice",
                "dictation",
                "dictate",
                "mic",
                "microphone",
                "speech",
                "stt",
                "toggle",
                "hold",
                "ctrl+space",
                "f8",
                "push-to-talk",
            ],
            kind: SettingKind::Enum {
                default: "hold",
                choices: VOICE_CAPTURE_MODE_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // SHELL-owned, persisted to `[ui].voice_stt_language`. Live-applied to
        // the next voice capture (no restart). Default English; System (`auto`)
        // follows the process locale when it maps to a Grok STT language.
        // Catalog = official STT languages (see xai_grok_voice::STT_LANGUAGES).
        SettingMeta {
            key: "voice_stt_language",
            category: SettingCategory::Editor,
            owner: SettingOwner::Shell,
            label: "Voice language",
            description: "Speech-to-text language for voice dictation (Grok STT). \
                          English by default; System uses your locale when supported. \
                          Sets formatting language for numbers and currencies.",
            keywords: &["voice", "language", "locale", "dictation", "stt", "speech"],
            kind: SettingKind::Enum {
                default: "en",
                choices: VOICE_STT_LANGUAGE_CHOICES,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // Contextual-hint children (hidden from the top-level list; reached via
        // the group sub-sheet). Default ON — `None` (inherit) reads as `true`.
        SettingMeta {
            key: "contextual_hints.undo",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Undo",
            description: "Remind you that Ctrl+Z restores the prompt after you clear it.",
            keywords: &["undo", "ctrl+z", "draft", "wipe", "hint"],
            kind: SettingKind::Bool {
                default: ui_default.contextual_hints.undo.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "contextual_hints.plan_mode",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Plan mode",
            description: "Suggest plan mode (Ctrl+Shift+T) when your prompt looks like a \
                          planning request.",
            keywords: &["plan", "mode", "nudge", "ctrl+shift+t", "hint"],
            kind: SettingKind::Bool {
                default: ui_default.contextual_hints.plan_mode.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "contextual_hints.image_input",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Image input",
            description: "Offer to paste an image when one is on the clipboard and the \
                          model accepts images.",
            keywords: &["image", "clipboard", "paste", "input", "hint"],
            kind: SettingKind::Bool {
                default: ui_default.contextual_hints.image_input.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "contextual_hints.send_now",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Send now",
            description: "After you queue a follow-up mid-turn, remind you that Enter \
                          on an empty prompt sends the top queued item now.",
            keywords: &[
                "send",
                "now",
                "interject",
                "queue",
                "follow-up",
                "enter",
                "empty",
                "hint",
            ],
            kind: SettingKind::Bool {
                default: ui_default.contextual_hints.send_now.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "contextual_hints.small_screen",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Small screen",
            description: "Suggest /compact-mode once per run when the terminal \
                          is short on rows.",
            keywords: &["small", "screen", "compact", "space", "rows", "hint"],
            kind: SettingKind::Bool {
                default: ui_default.contextual_hints.small_screen.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "contextual_hints.word_select",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "Word select",
            description: "After double-clicking conversation text while Text selection \
                          is fold/nav, remind you that Word select lives in Settings.",
            keywords: &[
                "word",
                "select",
                "double",
                "double-click",
                "click",
                "fold",
                "selection",
                "settings",
                "hint",
            ],
            kind: SettingKind::Bool {
                default: ui_default.contextual_hints.word_select.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "contextual_hints.ssh_wrap",
            category: SettingCategory::Advanced,
            owner: SettingOwner::Shell,
            label: "SSH wrap",
            description: "Show a `/doctor` tip when an SSH session is not using `grok wrap`.",
            keywords: &[
                "ssh",
                "wrap",
                "remote",
                "clipboard",
                "restore",
                "startup",
                "hint",
            ],
            kind: SettingKind::Bool {
                default: ui_default.contextual_hints.ssh_wrap.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // ── TodoGate (runtime turn-end backstop) ──────────────────────
        //
        // Only the CLI flag (`--todo-gate`) is wired. Settings-modal
        // entries for `[reminder.todo_gate]` are deferred — the modal
        // dispatcher requires per-key action arms in
        // `settings_modal.rs` + `app/dispatch.rs` + `settings/registry.rs`
        // that don't yet have a place to land.
        // SHELL-owned. `restart_required: false` — the config-reloader
        // rebroadcasts UI changes; mid-session forks pick up new values.
        // Empty-string default = "no opinion" / use shell's resolution.
        SettingMeta {
            key: "fork_secondary_model",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Fork secondary model",
            description: "Model used for the secondary agent when forking. Pick `(no override)` to clear.",
            keywords: &[
                "fork",
                "secondary",
                "model",
                "agent",
                "subagent",
                "branch",
                "models",
            ],
            kind: SettingKind::DynamicEnum {
                default: "",
                source: DynamicEnumSource::ActiveModelCatalog,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "pi_bash",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi Bash bridge",
            description: "Enable grok-pi's enhanced Bash bridge for the next session. Off restores stock Pi Bash; Eval remains independent.",
            keywords: &["pi", "bash", "bridge", "extension", "runtime"],
            kind: SettingKind::Bool {
                default: ui_default.pi_bash,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_eval",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Eval bridge version",
            description: "Choose the Eval runtime for the next grok-pi session. v1 supports Python + JavaScript; v2 is JavaScript-only.",
            keywords: &[
                "pi",
                "eval",
                "v1",
                "v2",
                "javascript",
                "python",
                "runtime",
                "bridge",
            ],
            kind: SettingKind::Enum {
                default: "v1",
                choices: PI_EVAL_CHOICES,
                supports_preview: false,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_eval_v2_only",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Eval v2 only",
            description: "Force Eval Bridge v2 and hide every other Pi tool for the next grok-pi session. Explicit CLI --tools/--no-tools still takes precedence.",
            keywords: &[
                "pi", "eval", "v2", "only", "isolate", "tools", "hide", "sandbox",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_eval_v2_only,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        // External Pi profile resource manager. This is a Pager navigation row,
        // not a Grok-shell setting, so its Group form only supplies the native
        // chevron presentation; settings-modal input maps it to OpenPiConfig.
        SettingMeta {
            key: "pi_builtin_tools",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi built-in tools",
            description: "Choose the Pi built-in tools for the next grok-pi session. Existing extension and custom tools stay enabled.",
            keywords: &[
                "pi",
                "tools",
                "read",
                "bash",
                "edit",
                "write",
                "grep",
                "find",
                "ls",
                "eval",
                "python",
                "javascript",
                "search",
            ],
            kind: SettingKind::Group {
                children: PI_BUILTIN_TOOLS_CHILDREN,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_builtin_tools.read",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Read",
            description: "Allow Pi to read files.",
            keywords: &["pi", "tool", "read"],
            kind: SettingKind::Bool {
                default: ui_default.pi_builtin_tools.read,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_builtin_tools.bash",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Bash",
            description: "Allow Pi to run shell commands.",
            keywords: &["pi", "tool", "bash", "shell"],
            kind: SettingKind::Bool {
                default: ui_default.pi_builtin_tools.bash,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_builtin_tools.edit",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Edit",
            description: "Allow Pi to patch existing files.",
            keywords: &["pi", "tool", "edit"],
            kind: SettingKind::Bool {
                default: ui_default.pi_builtin_tools.edit,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_builtin_tools.write",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Write",
            description: "Allow Pi to create or overwrite files.",
            keywords: &["pi", "tool", "write"],
            kind: SettingKind::Bool {
                default: ui_default.pi_builtin_tools.write,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_builtin_tools.grep",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Grep",
            description: "Allow Pi to search file contents with ripgrep.",
            keywords: &["pi", "tool", "grep", "search", "ripgrep"],
            kind: SettingKind::Bool {
                default: ui_default.pi_builtin_tools.grep,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_builtin_tools.find",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Find",
            description: "Allow Pi to locate files by glob.",
            keywords: &["pi", "tool", "find", "files", "glob"],
            kind: SettingKind::Bool {
                default: ui_default.pi_builtin_tools.find,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_builtin_tools.ls",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Ls",
            description: "Allow Pi to list directory contents.",
            keywords: &["pi", "tool", "ls", "list", "directory"],
            kind: SettingKind::Bool {
                default: ui_default.pi_builtin_tools.ls,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_builtin_tools.eval",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Eval",
            description: "Expose Pi's Eval tool. The grok-pi Eval bridge version is configured separately by Eval bridge version.",
            keywords: &[
                "pi",
                "tool",
                "eval",
                "python",
                "javascript",
                "kernel",
                "repl",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_builtin_tools.eval,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "psm_resume_index",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "PSM resume index",
            description: "Use Pi Session Manager (must be running) for Pi /resume: SQLite catalog, Ctrl+F full-text search, and message preview. Off = Pi JSONL list only.",
            keywords: &["pi", "psm", "resume", "session", "sqlite", "index"],
            kind: SettingKind::Bool {
                default: ui_default.psm_resume_index,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "pi_tree_file_rollback",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi tree file rollback",
            description: "Track write/edit preimages and allow file-only rollback from SessionTree. Takes effect for new grok-pi sessions.",
            keywords: &[
                "pi",
                "tree",
                "rollback",
                "rewind",
                "checkpoint",
                "file",
                "undo",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_tree_file_rollback,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_tree_skip_summary_prompt",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi tree skip summary prompt",
            description: "Skip the \"Summarize branch?\" prompt when navigating the session tree. When on, Enter navigates immediately without summarization.",
            keywords: &[
                "pi",
                "tree",
                "summary",
                "summarize",
                "branch",
                "prompt",
                "navigate",
                "skip",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_tree_skip_summary_prompt,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_herdr",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi Herdr integration",
            description: "Report grok-pi lifecycle and native Pi session state to Herdr. Disabled by default; enable only when Herdr is in use. Takes effect for new grok-pi sessions.",
            keywords: &[
                "pi",
                "herdr",
                "agent",
                "lifecycle",
                "workspace",
                "pane",
                "status",
                "integration",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_herdr,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_subagents",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi subagents",
            description: crate::native_feature_conflicts::f2_description_with_conflicts(
                "Enable the built-in Pi child-session subagents (`spawn_subagent`, native Tasks/child views). Default on; takes effect for new grok-pi sessions.",
                "pi_subagents",
            ),
            keywords: &[
                "pi",
                "subagent",
                "subagents",
                "child",
                "agent",
                "spawn_subagent",
                "tasks",
                "delegate",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_subagents,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_workflows",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi workflows",
            description: crate::native_feature_conflicts::f2_description_with_conflicts(
                "Enable upstream Rhai workflows in grok-pi (tool `workflow`, deep-research, .grok/workflows). Takes effect for new grok-pi sessions.",
                "pi_workflows",
            ),
            keywords: &[
                "pi",
                "workflow",
                "workflows",
                "rhai",
                "deep-research",
                "agent",
                "orchestration",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_workflows,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_todo",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi todo",
            description: crate::native_feature_conflicts::f2_description_with_conflicts(
                "Enable grok-pi's built-in structured `todo` tool and native TodoPane projection. Default on; takes effect for new grok-pi sessions.",
                "pi_todo",
            ),
            keywords: &["pi", "todo", "todos", "task", "tasks", "plan", "progress"],
            kind: SettingKind::Bool {
                default: ui_default.pi_todo,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_goal",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi goal mode",
            description: crate::native_feature_conflicts::f2_description_with_conflicts(
                "Enable Grok-style /goal autonomous loop in grok-pi (status bar + update_goal). Takes effect for new grok-pi sessions.",
                "pi_goal",
            ),
            keywords: &["pi", "goal", "/goal", "autonomous", "update_goal", "agent"],
            kind: SettingKind::Bool {
                default: ui_default.pi_goal,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_loop",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi /loop scheduler",
            description: "Enable Grok-style /loop recurring prompts in grok-pi (tasks pane + scheduler_create). Takes effect for new grok-pi sessions.",
            keywords: &[
                "pi",
                "loop",
                "/loop",
                "scheduler",
                "cron",
                "recurring",
                "agent",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_loop,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_ask_user_question",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Q&A",
            description: crate::native_feature_conflicts::f2_description_with_conflicts(
                "Grok Build asks the right questions to nail the details. Enable native ask_user_question → QuestionView. Takes effect for new grok-pi sessions.",
                "pi_ask_user_question",
            ),
            keywords: &[
                "pi",
                "qa",
                "q&a",
                "ask",
                "question",
                "ask_user_question",
                "questionnaire",
                "interview",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_ask_user_question,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_ask_user_question_notifications",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Q&A desktop notifications",
            description: "Notify you when a native Q&A question arrives while grok-pi is unfocused. Applies immediately; default on.",
            keywords: &[
                "pi",
                "qa",
                "q&a",
                "ask",
                "question",
                "notification",
                "desktop",
                "focus",
                "unfocused",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_ask_user_question_notifications,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_btw",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi /btw side questions",
            description: crate::native_feature_conflicts::f2_description_with_conflicts(
                "Enable native /btw side questions in grok-pi (overlay + Pi complete()). Takes effect for new grok-pi sessions.",
                "pi_btw",
            ),
            keywords: &["pi", "btw", "side", "question", "side-question", "overlay"],
            kind: SettingKind::Bool {
                default: ui_default.pi_btw,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_cache_graph",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi cache graph in Context",
            description: "Show cache hit graph/stats (keys 1/2/3/s/e) inside the Context modal. Matches pi-cache-graph; default on.",
            keywords: &["pi", "cache", "graph", "context", "stats", "hit"],
            kind: SettingKind::Bool {
                default: ui_default.pi_cache_graph,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_config_skill",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi config skill",
            description: "Load grok-pi's embedded configuration skill by default so the agent can explain F2 settings, config.toml, skills, themes, and feature flags. Restart required.",
            keywords: &[
                "pi",
                "grok-pi",
                "config",
                "configuration",
                "skill",
                "skills",
                "f2",
                "settings",
                "docs",
                "guide",
                "default",
                "disable",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_config_skill,
            },
            restart_required: true,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "pi_user_markdown",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Markdown user messages",
            description: "Render grok-pi user prompts with the agent markdown renderer (expanded, user chrome preserved). Off restores classic collapsible plain-text prompts. Default on.",
            keywords: &["pi", "user", "markdown", "prompt", "message"],
            kind: SettingKind::Bool {
                default: ui_default.pi_user_markdown,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_at_search_hidden",
            category: SettingCategory::Editor,
            owner: SettingOwner::Shell,
            label: "Hidden files in @ search",
            description: "Include hidden/dotfile entries in plain `@` file search while still respecting ignore rules, matching pi-main's fd `--hidden` behavior. `@!` additionally reveals gitignored project files while `.git`, `node_modules`, and common dependency/package stores remain excluded. Default on.",
            keywords: &["pi", "at", "file", "search", "hidden", "dotfile", "gitignore"],
            kind: SettingKind::Bool {
                default: ui_default.pi_at_search_hidden,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_keep_multi_agent",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Keep multi-agent on /new",
            description: "When on, `/new` keeps the current agent tab alive so the dashboard can switch back to it (Pi re-loads its session on demand). Default off: `/new` drops the current agent tab and starts fresh — other idle agent tabs in the dashboard are left untouched.",
            keywords: &[
                "pi",
                "new",
                "session",
                "multi",
                "agent",
                "dashboard",
                "replace",
                "keep",
                "preserve",
            ],
            kind: SettingKind::Bool {
                default: ui_default.pi_keep_multi_agent,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "show_other_tool_args",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Other tool args",
            description: "When expanded, show raw_input JSON on all Other/generic tool cards (not just fabric_exec). Collapsed stays name-only. Default off.",
            keywords: &[
                "tool",
                "args",
                "arguments",
                "raw_input",
                "other",
                "fabric",
                "fabric_exec",
                "json",
            ],
            kind: SettingKind::Bool {
                default: ui_default.show_other_tool_args,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "review_file_tree",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Review file tree",
            description: "Default /review-* left pane to a cwd-relative tree (compacts Java package chains). Off = flat basenames. Press t in the modal to toggle; persists here.",
            keywords: &["review", "tree", "files", "code review", "package", "java"],
            kind: SettingKind::Bool {
                default: ui_default.review_file_tree,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "review_include_reads",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Review include reads",
            description: "Include session read tool ops in /review-* file list (right pane reuses read viewer, not diff). Off = edit/write only. Press r in the modal to toggle; persists here.",
            keywords: &["review", "read", "files", "code review", "filter"],
            kind: SettingKind::Bool {
                default: ui_default.review_include_reads,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "pi_config",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Pi resources",
            description: "Review and enable Pi extensions, skills, prompts, and themes. Project resources require Pi project trust.",
            keywords: &[
                "pi",
                "config",
                "extensions",
                "skills",
                "prompts",
                "themes",
                "resources",
            ],
            kind: SettingKind::Group { children: &[] },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        // Session recap (auto away-recap + /recap model override).
        // Auto toggle lives on `[ui].session_recap` (mirrors notification opt-in).
        SettingMeta {
            key: "session_recap",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Session recap",
            description: "Show an automatic \"where was I\" recap when you return after being away. \
Manual /recap still works when the agent advertises sessionRecap.",
            keywords: &["recap", "session", "summary", "away", "return", "auto"],
            kind: SettingKind::Bool {
                default: ui_default.session_recap.unwrap_or(true),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "recap_mermaid",
            category: SettingCategory::Agent,
            owner: SettingOwner::Shell,
            label: "Recap Mermaid diagrams",
            description: "Allow Recap to include an optional Markdown Mermaid diagram. The diagram is rendered when the recap is expanded.",
            keywords: &["recap", "mermaid", "diagram", "summary", "graph"],
            kind: SettingKind::Bool {
                default: ui_default.recap_mermaid.unwrap_or(false),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "progress_bar",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Terminal tab progress",
            description: "Show OSC 9;4 progress indicators in the terminal tab bar.",
            keywords: &["progress", "terminal", "tab", "osc", "9;4", "indicator"],
            kind: SettingKind::Bool {
                default: ui_default.progress_bar.unwrap_or(false),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "remote_tui_footer",
            category: SettingCategory::Appearance,
            owner: SettingOwner::Shell,
            label: "Remote TUI footer",
            description: "Show keyboard hints below experimental Remote TUI frames.",
            keywords: &["remote", "tui", "footer", "keyboard", "hints"],
            kind: SettingKind::Bool {
                default: ui_default.remote_tui_footer.unwrap_or(false),
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "recap_models",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Recap models",
            description: "Primary + fallback models for session recap (/recap and auto away-recap). Empty slots use the active session model or skip.",
            keywords: &["recap", "model", "summary", "session", "models", "fallback"],
            kind: SettingKind::Group {
                children: RECAP_MODELS_CHILDREN,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "btw_models",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Btw models",
            description: "Primary + fallback models for /btw side questions. Empty slots use the active session model or skip.",
            keywords: &["btw", "model", "side", "question", "fallback"],
            kind: SettingKind::Group {
                children: BTW_MODELS_CHILDREN,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "recap_model",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Primary",
            description: "Primary recap model. Empty = active session model.",
            keywords: &["recap", "model", "summary", "session", "models"],
            kind: SettingKind::DynamicEnum {
                default: "",
                source: DynamicEnumSource::ActiveModelCatalog,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "recap_model_2",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Fallback 2",
            description: "Tried if primary fails. Empty = skip.",
            keywords: &["recap", "model", "fallback", "summary"],
            kind: SettingKind::DynamicEnum {
                default: "",
                source: DynamicEnumSource::ActiveModelCatalog,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "recap_model_3",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Fallback 3",
            description: "Tried if primary and fallback 2 fail. Empty = skip.",
            keywords: &["recap", "model", "fallback", "summary"],
            kind: SettingKind::DynamicEnum {
                default: "",
                source: DynamicEnumSource::ActiveModelCatalog,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: false,
        },
        SettingMeta {
            key: "btw_model",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Primary",
            description: "Primary /btw model. Empty = active session model.",
            keywords: &["btw", "model", "side", "question"],
            kind: SettingKind::DynamicEnum {
                default: "",
                source: DynamicEnumSource::ActiveModelCatalog,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "btw_model_2",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Fallback 2",
            description: "Tried if primary fails. Empty = skip.",
            keywords: &["btw", "model", "fallback"],
            kind: SettingKind::DynamicEnum {
                default: "",
                source: DynamicEnumSource::ActiveModelCatalog,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
        SettingMeta {
            key: "btw_model_3",
            category: SettingCategory::Models,
            owner: SettingOwner::Shell,
            label: "Fallback 3",
            description: "Tried if primary and fallback 2 fail. Empty = skip.",
            keywords: &["btw", "model", "fallback"],
            kind: SettingKind::DynamicEnum {
                default: "",
                source: DynamicEnumSource::ActiveModelCatalog,
                supports_preview: false,
            },
            restart_required: false,
            hidden_in_minimal: false,
            external_only: true,
        },
    ]
}
