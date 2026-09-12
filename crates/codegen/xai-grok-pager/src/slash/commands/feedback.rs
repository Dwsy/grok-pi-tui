//! `/feedback`: open feedback review or ask the model to help with a report.

use agent_client_protocol as acp;

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// User text after `/feedback `, or `None` when this is not an inline feedback prompt.
#[must_use]
pub(crate) fn inline_feedback_user_text(text: &str) -> Option<&str> {
    let user_text = text.strip_prefix("/feedback ")?.trim();
    if user_text.is_empty() {
        None
    } else {
        Some(user_text)
    }
}

/// Skill text the model sees after `/feedback <text>` has already saved a local draft.
#[must_use]
pub(crate) fn feedback_skill_instruction(user_text: &str, draft_id: &str) -> String {
    format!(
        "The user invoked `/feedback` with this report:\n\n{user_text}\n\n\
         A feedback draft has been created (id: {draft_id}). Call `send_feedback` with that `draft_id` to taxonomize it. Do not create a second draft. Do not claim it was sent."
    )
}

/// `/feedback <text>` becomes a model turn. Bare `/feedback` opens the feedback modal in the full
/// TUI; in minimal mode it routes to the dispatcher's visible refusal (minimal has no modal renderer,
/// so no state is ever set).
pub struct FeedbackCommand;

impl SlashCommand for FeedbackCommand {
    slash_meta! {
        name: "feedback",
        description: "Send feedback about the current session",
        usage: "/feedback [text]",
        takes_args: true,
        arg_placeholder: "[feedback text]",
    }

    fn submission_refusal(
        &self,
        args: &str,
        is_minimal: bool,
        voice_owns_prompt: bool,
    ) -> Option<&'static str> {
        if !args.trim().is_empty() {
            None
        } else if is_minimal {
            Some(
                "Use `/feedback <text>` in minimal mode, or run without --minimal to open the feedback form.",
            )
        } else if voice_owns_prompt {
            Some("Stop voice input before opening the feedback form")
        } else {
            None
        }
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        // Prompt dispatch removes live image elements before passing these arguments.
        let user_text = args.trim();
        let result = if user_text.is_empty() {
            CommandResult::Action(Action::OpenFeedbackModal(Default::default()))
        } else {
            // Dispatch saves the draft and rewrites this block with the real id before inject.
            let instruction = feedback_skill_instruction(user_text, "pending");
            CommandResult::InjectSkill {
                display_text: format!("/feedback {user_text}"),
                prompt_blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(instruction))],
                display_as_skill: true,
                scheduled_task_preview: None,
            }
        };
        let action = match &result {
            CommandResult::Action(Action::OpenFeedbackModal(_)) => "open_empty",
            CommandResult::InjectSkill { .. } => "inject_skill",
            _ => "other",
        };
        crate::unified_log::info(
            "feedback.command",
            ctx.session_id.map(|s| s.0.as_ref()),
            Some(serde_json::json!({
                "screen_mode": ctx.screen_mode.meta_label(),
                "arg_chars": user_text.chars().count(),
                "action": action,
            })),
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;

    fn make_ctx(models: &ModelState) -> CommandExecCtx<'_> {
        let bundle = Box::leak(Box::new(crate::app::bundle::BundleState::default()));
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        }
    }

    /// The whitespace case matters: the composer keeps a trailing space while the user is still typing the command.
    #[test]
    fn bare_and_whitespace_open_the_empty_modal() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = FeedbackCommand;

        for args in ["", "   ", "\t"] {
            match cmd.run(&mut ctx, args) {
                CommandResult::Action(Action::OpenFeedbackModal(open)) => {
                    assert_eq!(open.text, None, "{args:?} should open empty");
                    assert!(
                        open.images.is_empty(),
                        "images attach at dispatch, not here"
                    );
                }
                other => panic!("{args:?} should open the modal, got {other:?}"),
            }
        }
    }

    #[test]
    fn inline_text_injects_feedback_skill() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);

        let result = FeedbackCommand.run(&mut ctx, "  todo is chopped  ");
        assert!(
            !matches!(&result, CommandResult::Action(Action::SendFeedback { .. })),
            "inline text must not send feedback immediately"
        );
        match result {
            CommandResult::InjectSkill {
                display_text,
                prompt_blocks,
                display_as_skill,
                scheduled_task_preview,
            } => {
                assert_eq!(display_text, "/feedback todo is chopped");
                assert!(display_as_skill);
                assert!(scheduled_task_preview.is_none());
                let [acp::ContentBlock::Text(prompt)] = &prompt_blocks[..] else {
                    panic!("expected one text prompt block, got {prompt_blocks:?}");
                };
                assert!(prompt.text.contains("todo is chopped"));
                assert!(!prompt.text.contains("todo is chopped\""));
                assert!(prompt.text.contains("send_feedback"));
                assert!(prompt.text.contains("draft_id"));
                assert!(prompt.text.contains("pending"));
                assert!(!prompt.text.contains("Do not draft"));
            }
            other => panic!("inline text must inject a skill, got {other:?}"),
        }
    }
}
