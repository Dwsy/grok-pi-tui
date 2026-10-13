//! `/compact` takes no arguments.
//! `run` returns `CommandResult::QueueCommand` so the dispatch layer enqueues it as `QueueEntryKind::Command`.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

const NO_ARGS: &str = "/compact takes no arguments.";

/// Compact the conversation history.
pub struct CompactCommand;

impl SlashCommand for CompactCommand {
    slash_meta! {
        name: "compact",
        description: "Compact conversation history",
        usage: "/compact",
        session_scoped: true,
    }

    /// Refusing here, before the send path runs, also keeps an edited queue row in place
    /// (`EditedCommandGate` pre-checks this hook).
    fn submission_refusal(&self, args: &str, _voice_owns_prompt: bool) -> Option<&'static str> {
        (!args.trim().is_empty()).then_some(NO_ARGS)
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if args.trim().is_empty() {
            CommandResult::QueueCommand("/compact".to_string())
        } else {
            CommandResult::Error(NO_ARGS.to_string())
        }
    }
}
