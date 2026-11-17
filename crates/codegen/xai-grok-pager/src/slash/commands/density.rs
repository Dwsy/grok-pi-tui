//! `/density`: set how much detail tool calls show.
//!
//! Bare `/density` lists the five levels with the active one marked.
//! `/density <level>` dispatches `Action::SetToolCallDensity`; labels, canonical
//! values, and the `verbose` / `minimal` aliases are all accepted.

use crate::app::actions::Action;
use crate::appearance::cache::ToolCallDensity;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct DensityCommand;

impl SlashCommand for DensityCommand {
    slash_meta! {
        name: "density",
        description: "Set tool call density, from Detailed to Compact",
        usage: "/density <level>",
        arg_placeholder: "detailed|compact-shells|compact-ungrouped|compact-grouped|compact-all-grouped",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let arg = args.trim();
        if arg.is_empty() {
            return CommandResult::Message(density_menu());
        }
        match ToolCallDensity::parse(arg) {
            Some(level) => {
                CommandResult::Action(Action::SetToolCallDensity(level.as_canonical().to_string()))
            }
            None => CommandResult::Error(format!(
                "Unknown density '{arg}'. Use detailed, compact-shells, compact-ungrouped, \
                 compact-grouped, or compact-all-grouped (current: {})",
                crate::appearance::cache::load_tool_call_density().label()
            )),
        }
    }
}

fn density_menu() -> String {
    let current = crate::appearance::cache::load_tool_call_density();
    let mut out = String::from("Tool call density\n");
    for level in ToolCallDensity::ALL {
        let marker = if level == current {
            "\u{25cf}"
        } else {
            "\u{25cb}"
        };
        out.push_str(&format!(
            "  {marker} {} \u{00b7} {} \u{2014} {}\n",
            level.label(),
            level.as_canonical(),
            level.choice_description()
        ));
    }
    out.push_str("Aliases: verbose = detailed, minimal = compact.");
    out
}
