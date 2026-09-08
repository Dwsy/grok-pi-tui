//! `/pi-models` -- open the native Pi model-management center.
//!
//! `/pi-models web` hands the same surface to the injected Pi web-config
//! extension, which serves it on a random loopback port.

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

/// Opens the Pager-owned editor for Pi's `models.json`.
pub struct PiModelsCommand;

impl SlashCommand for PiModelsCommand {
    fn name(&self) -> &str {
        "pi-models"
    }

    fn aliases(&self) -> &[&str] {
        &["model-config", "models-config"]
    }

    fn description(&self) -> &str {
        "Manage Pi providers and models with live reload"
    }

    fn usage(&self) -> &str {
        "/pi-models [web]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        Some(vec![ArgItem {
            display: "web".to_string(),
            match_text: "web browser ui".to_string(),
            insert_text: "web".to_string(),
            description: "Open the provider/model manager in a browser (random local port)"
                .to_string(),
        }])
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let args = args.trim();
        if args.is_empty() {
            return CommandResult::Action(Action::OpenPiModels);
        }
        let rest = match args.split_whitespace().next() {
            Some("web") => args["web".len()..].trim(),
            _ => return CommandResult::Error("usage: /pi-models [web]".to_string()),
        };
        // The Pi extension owns the server, the page and every config write.
        CommandResult::DirectPiCommand(if rest.is_empty() {
            "/pi-models-web".to_string()
        } else {
            format!("/pi-models-web {rest}")
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
    fn dispatches_native_model_manager() {
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
            PiModelsCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenPiModels)
        ));
    }

    #[test]
    fn registers_compatibility_aliases() {
        assert_eq!(
            PiModelsCommand.aliases(),
            &["model-config", "models-config"]
        );
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
            PiModelsCommand.run(&mut ctx, "web"),
            CommandResult::DirectPiCommand(text) if text == "/pi-models-web"
        ));
        assert!(matches!(
            PiModelsCommand.run(&mut ctx, "web --port 4321"),
            CommandResult::DirectPiCommand(text) if text == "/pi-models-web --port 4321"
        ));
        assert!(matches!(
            PiModelsCommand.run(&mut ctx, "nope"),
            CommandResult::Error(_)
        ));
    }
}
