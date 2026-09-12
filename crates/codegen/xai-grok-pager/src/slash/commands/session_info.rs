//! `/session-info` -- show current session info (instant, not queued).
//!
//! Alias `/session` matches Pi's built-in session stats command name so
//! grok-pi users can type the Pi form and still hit the native Grok path
//! (`x.ai/session/info` → formatted system message).

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Show session info (session ID, cwd, model, context usage).
pub struct SessionInfoCommand;

impl SlashCommand for SessionInfoCommand {
    slash_meta! {
        name: "session-info",
        // Pi interactive uses `/session`; keep Grok canonical name as primary.
        aliases: ["session"],
        description: "Show session info",
        usage: "/session-info",
        session_scoped: true,
    }

    fn run(&self, ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        if ctx.session_id.is_none() {
            return CommandResult::Error("No active session".to_string());
        }

        CommandResult::Action(Action::ShowSessionInfo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_session_alias_is_registered() {
        assert_eq!(SessionInfoCommand.name(), "session-info");
        assert_eq!(SessionInfoCommand.aliases(), &["session"]);
    }
}
