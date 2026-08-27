//! `/plan-mode` -- toggle plan mode on/off.
//!
//! Unlike `/plan` (which only enters), `/plan-mode` toggles: if plan mode is
//! active it turns it off, otherwise it turns it on. This mirrors the platform
//! Plan shortcut (Ctrl+Alt+T on Windows, Ctrl+Shift+T elsewhere).

use crate::app::actions::{Action, PlanModeKind};
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Toggle plan mode on/off.
pub struct PlanModeCommand;

impl SlashCommand for PlanModeCommand {
    fn name(&self) -> &str {
        "plan-mode"
    }

    fn aliases(&self) -> &[&str] {
        &["toggle-plan", "plan-toggle"]
    }

    fn description(&self) -> &str {
        "Toggle plan mode on/off"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/plan-mode"
    }

    fn run(&self, ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        let in_plan = ctx.pager_state.plan_mode_active;
        let kind = if in_plan {
            PlanModeKind::Off
        } else {
            PlanModeKind::On
        };
        CommandResult::Action(Action::SetPlanMode(kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    fn make_ctx(plan_active: bool) -> (ModelState, BundleState, PagerLocalSnapshot) {
        (
            ModelState::default(),
            BundleState::default(),
            PagerLocalSnapshot {
                plan_mode_active: plan_active,
                ..PagerLocalSnapshot::default()
            },
        )
    }

    #[test]
    fn toggles_on_when_inactive() {
        let (models, bundle, pager_state) = make_ctx(false);
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: false,
            usage_command_visible: true,
            pager_state,
        };
        match PlanModeCommand.run(&mut ctx, "") {
            CommandResult::Action(Action::SetPlanMode(kind)) => {
                assert_eq!(kind, PlanModeKind::On);
            }
            other => panic!("expected SetPlanMode(On), got {other:?}"),
        }
    }

    #[test]
    fn toggles_off_when_active() {
        let (models, bundle, pager_state) = make_ctx(true);
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: false,
            usage_command_visible: true,
            pager_state,
        };
        match PlanModeCommand.run(&mut ctx, "") {
            CommandResult::Action(Action::SetPlanMode(kind)) => {
                assert_eq!(kind, PlanModeKind::Off);
            }
            other => panic!("expected SetPlanMode(Off), got {other:?}"),
        }
    }
}
