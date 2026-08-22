//! `/eval-display` -- switch Eval v2 between effects-first and legacy rendering.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct EvalDisplayCommand;

fn parse_mode(args: &str) -> Result<&'static str, String> {
    match args.trim().to_ascii_lowercase().as_str() {
        "" | "toggle" => Ok("toggle"),
        "effects" => Ok("effects"),
        "legacy" => Ok("legacy"),
        _ => Err("Usage: /eval-display [effects|legacy]".to_string()),
    }
}

impl SlashCommand for EvalDisplayCommand {
    fn name(&self) -> &str {
        "eval-display"
    }
    fn description(&self) -> &str {
        "Toggle Eval v2 effects-first vs legacy source rendering"
    }
    fn usage(&self) -> &str {
        "/eval-display [effects|legacy]"
    }
    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        match parse_mode(args) {
            Ok(mode) => CommandResult::Action(Action::SetPiEvalV2DisplayMode(mode.to_string())),
            Err(message) => CommandResult::Error(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_mode;
    #[test]
    fn parse_mode_accepts_toggle_and_canonical_values() {
        assert_eq!(parse_mode("").unwrap(), "toggle");
        assert_eq!(parse_mode("toggle").unwrap(), "toggle");
        assert_eq!(parse_mode("effects").unwrap(), "effects");
        assert_eq!(parse_mode("legacy").unwrap(), "legacy");
        assert!(parse_mode("source").is_err());
    }
}
