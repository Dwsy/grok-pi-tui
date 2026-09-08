//! `/pi-config` -- open the native Pi resource configuration modal.
//!
//! `/pi-config web` hands the same surface to the injected Pi web-config
//! extension, which serves it on a random loopback port.

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

/// Opens the Pager-owned Pi resource manager without starting Pi's separate TUI.
pub struct PiConfigCommand;

impl SlashCommand for PiConfigCommand {
    fn name(&self) -> &str {
        "pi-config"
    }

    fn aliases(&self) -> &[&str] {
        &["pi-resources"]
    }

    fn description(&self) -> &str {
        "Manage Pi extensions, skills, prompts, and themes"
    }

    fn usage(&self) -> &str {
        "/pi-config [web]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        Some(vec![ArgItem {
            display: "web".to_string(),
            match_text: "web browser ui".to_string(),
            insert_text: "web".to_string(),
            description: "Open the resource manager in a browser (random local port)".to_string(),
        }])
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let args = args.trim();
        if args.is_empty() {
            return CommandResult::Action(Action::OpenPiConfig);
        }
        let rest = match args.split_whitespace().next() {
            Some("web") => args["web".len()..].trim(),
            _ => return CommandResult::Error("usage: /pi-config [web]".to_string()),
        };
        // The Pi extension owns the server, the page and every config write.
        CommandResult::DirectPiCommand(if rest.is_empty() {
            "/pi-config-web".to_string()
        } else {
            format!("/pi-config-web {rest}")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::ScreenMode;
    use crate::app::bundle::BundleState;

    #[test]
    fn dispatches_native_pi_config_action() {
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: ScreenMode::Fullscreen,
            billing_surface_visible: false,
            usage_command_visible: true,
            pager_state: Default::default(),
        };
        assert!(matches!(
            PiConfigCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenPiConfig)
        ));
    }

    #[test]
    fn registers_resource_alias() {
        assert_eq!(PiConfigCommand.aliases(), &["pi-resources"]);
    }

    #[test]
    fn web_arg_is_forwarded_to_the_pi_extension() {
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: ScreenMode::Fullscreen,
            billing_surface_visible: false,
            usage_command_visible: true,
            pager_state: Default::default(),
        };
        assert!(matches!(
            PiConfigCommand.run(&mut ctx, "web"),
            CommandResult::DirectPiCommand(text) if text == "/pi-config-web"
        ));
        assert!(matches!(
            PiConfigCommand.run(&mut ctx, "web --no-open"),
            CommandResult::DirectPiCommand(text) if text == "/pi-config-web --no-open"
        ));
        assert!(matches!(
            PiConfigCommand.run(&mut ctx, "nope"),
            CommandResult::Error(_)
        ));
    }
}
