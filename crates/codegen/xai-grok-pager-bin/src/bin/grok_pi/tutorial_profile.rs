use xai_grok_pager::tutorial_docs::{TutorialProfile, TutorialTopic, set_tutorial_profile};

macro_rules! topic {
    ($file:literal, $title:literal, $blurb:literal) => {
        TutorialTopic {
            title: $title,
            blurb: $blurb,
            content: include_str!(concat!("tutorial/", $file)),
            go_deeper: None,
        }
    };
}

static INTRO_LINES: &[&str] = &[
    "grok-pi combines the Pi agent core with Grok Pager's native terminal UI.",
    "Explore the native workspace, Pi ecosystem, and grok-pi product features.",
];

pub static GROK_PI_TUTORIAL_TOPICS: &[TutorialTopic] = &[
    topic!(
        "01-what-is-grok-pi.md",
        "What Is grok-pi?",
        "Pi owns the agent; Pager owns the terminal"
    ),
    topic!(
        "02-terminal-input.md",
        "Terminal, Prompt & Input",
        "native screens, editing, images, voice and hotkeys"
    ),
    topic!(
        "03-providers-models-thinking.md",
        "Providers, Models & Thinking",
        "multi-provider Pi, local models and reasoning control"
    ),
    topic!(
        "04-tools-diffs-streaming.md",
        "Tools, Streaming & Diffs",
        "tool policy, native cards and structured changes"
    ),
    topic!(
        "05-context-cache-compaction.md",
        "Context, Cache & Compaction",
        "live usage, cache graphs, compact and recap"
    ),
    topic!(
        "06-queue-turn-control.md",
        "Queue & Turn Control",
        "follow-ups, steering, editing and cancellation"
    ),
    topic!(
        "07-sessions-resume.md",
        "Sessions & Resume",
        "local Pi JSONL sessions and rich discovery"
    ),
    topic!(
        "08-tree-branching.md",
        "Session Tree & Branching",
        "navigate, jump, map, fork and clone"
    ),
    topic!(
        "09-review-timeline-rollback.md",
        "Review, Timeline & Rollback",
        "inspect changes and optional file-only recovery"
    ),
    topic!(
        "10-plan-todo.md",
        "Plan Mode & Todo",
        "investigate first, approve, then implement"
    ),
    topic!(
        "11-extensions-dynamic-commands.md",
        "Extensions & Dynamic Commands",
        "tools, commands, shortcuts, events and providers"
    ),
    topic!(
        "12-extension-ui-remote-tui.md",
        "Extension UI & Remote TUI",
        "native dialogs plus experimental custom components"
    ),
    topic!(
        "13-skills-prompts-context-files.md",
        "Skills, Prompts & Context Files",
        "reusable instructions discovered by Pi"
    ),
    topic!(
        "14-themes-packages-resources.md",
        "Themes, Packages & Resources",
        "install with Pi; manage trust and policy in Pager"
    ),
    topic!(
        "15-background-bash-tasks.md",
        "Background Bash & Tasks",
        "move long commands aside without losing control"
    ),
    topic!(
        "16-subagents-dashboard.md",
        "Subagents & Dashboard",
        "child Pi sessions in native task and agent views"
    ),
    topic!(
        "17-optional-automation.md",
        "Optional Interaction & Automation",
        "Q&A, BTW, workflows, goals and loops"
    ),
    topic!(
        "18-operations-isolation-diagnostics.md",
        "Export, Updates & Product State",
        "share, upgrade, isolate and diagnose grok-pi"
    ),
];

pub static GROK_PI_TUTORIAL_PROFILE: TutorialProfile = TutorialProfile {
    title: "Welcome to grok-pi",
    command_description: "Explore grok-pi, Grok Pager, and the Pi ecosystem",
    intro_lines: INTRO_LINES,
    topics: GROK_PI_TUTORIAL_TOPICS,
};

pub fn install() {
    set_tutorial_profile(Some(&GROK_PI_TUTORIAL_PROFILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_is_grok_pi_specific_and_bite_size() {
        assert_eq!(
            xai_grok_pager::tutorial_docs::DEFAULT_TUTORIAL_PROFILE.title,
            "Welcome to Grok Build"
        );
        assert_eq!(GROK_PI_TUTORIAL_PROFILE.title, "Welcome to grok-pi");
        assert_eq!(GROK_PI_TUTORIAL_TOPICS.len(), 18);

        let mut titles = std::collections::HashSet::new();
        for topic in GROK_PI_TUTORIAL_TOPICS {
            assert!(
                titles.insert(topic.title),
                "duplicate topic: {}",
                topic.title
            );
            assert!(
                topic.content.starts_with("# "),
                "missing H1: {}",
                topic.title
            );
            assert!(
                topic.content.lines().count() <= 50,
                "topic too long: {}",
                topic.title
            );
            assert!(
                topic.go_deeper.is_none(),
                "grok-pi topics must not link stock guides"
            );
        }
    }

    #[test]
    fn install_selects_grok_pi_copy_for_the_pager() {
        install();
        let active = xai_grok_pager::tutorial_docs::tutorial_profile();
        assert_eq!(active.title, "Welcome to grok-pi");
        assert_eq!(
            active.command_description,
            "Explore grok-pi, Grok Pager, and the Pi ecosystem"
        );
        assert!(std::ptr::eq(active.topics, GROK_PI_TUTORIAL_TOPICS));
        set_tutorial_profile(None);
    }

    #[test]
    fn content_covers_product_and_pi_ecosystem_without_stock_promises() {
        let content = GROK_PI_TUTORIAL_TOPICS
            .iter()
            .map(|topic| topic.content)
            .collect::<Vec<_>>()
            .join("\n");

        for required in [
            "Pi agent core",
            "multi-provider",
            "models.json",
            "registerProvider",
            "/model",
            "/effort",
            "--tools",
            "/context",
            "cache graph",
            "/queue",
            "/resume",
            "/tree",
            "/tree-map",
            "/fork",
            "/clone",
            "/review-session",
            "/timeline",
            "/plan-mode",
            "Ctrl+Alt+T",
            "registerTool",
            "ctx.ui.custom",
            "/skill:",
            "AGENTS.md",
            "pi install",
            "/pi-config",
            "Ctrl+B",
            "/dashboard",
            "/workflows",
            "/export-html",
            "migrate-home",
            "~/.grok-pi",
        ] {
            assert!(content.contains(required), "missing capability: {required}");
        }

        for required_boundary in [
            "default on",
            "default off",
            "restart",
            "experimental",
            "does not install",
        ] {
            assert!(
                content.contains(required_boundary),
                "missing boundary: {required_boundary}"
            );
        }

        for forbidden in [
            "/rewind",
            "--worktree",
            "/feedback",
            "/import-claude",
            "grok inspect",
            "~/.grok/",
        ] {
            assert!(
                !content.contains(forbidden),
                "stock-only promise: {forbidden}"
            );
        }
    }
}
