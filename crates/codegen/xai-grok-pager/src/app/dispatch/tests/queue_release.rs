//! Tests for sending a prompt through a parked wait, including after `/btw`.

use super::*;

use crate::app::agent::AgentState;
use crate::app::agent_view::test_fixtures::simulate_task_output_wait;
use crate::views::btw_overlay::BtwOverlayState;

fn running_turn_app() -> AppView {
    let mut app = test_app_with_agent();
    let agent = app.agents.get_mut(&AgentId(0)).unwrap();
    agent.session.state = AgentState::TurnRunning;
    agent.session.current_prompt_id = Some("p1".into());
    agent.front_message_committed = true;
    app
}

fn sent_texts(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::SendInterject { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Forces the local drip-feed send path so a mid-turn Enter enqueues locally instead of handing the send to the server.
struct LocalFollowUp {
    previous: crate::appearance::FollowUpBehavior,
}
impl LocalFollowUp {
    fn enter(app: &mut AppView, behavior: crate::appearance::FollowUpBehavior) -> Self {
        app.leader_mode = false;
        let previous = crate::appearance::cache::load_follow_up_behavior();
        crate::appearance::cache::set_follow_up_behavior(behavior);
        Self { previous }
    }
    fn queue(app: &mut AppView) -> Self {
        Self::enter(app, crate::appearance::FollowUpBehavior::Queue)
    }
    fn steer(app: &mut AppView) -> Self {
        Self::enter(app, crate::appearance::FollowUpBehavior::Steer)
    }
}
impl Drop for LocalFollowUp {
    fn drop(&mut self) {
        crate::appearance::cache::set_follow_up_behavior(self.previous);
    }
}

/// Sending while parked must go through even when the last thing the user did was `/btw` (the overlay is still open).
#[test]
fn send_while_waiting_goes_through_when_btw_overlay_is_open() {
    let mut app = running_turn_app();
    let _mode = LocalFollowUp::steer(&mut app);
    enqueue_local(&mut app, AgentId(0), "held");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        simulate_task_output_wait(agent, "task-1");
        assert!(agent.is_parked_on_sendable_wait());
        agent.btw_state = Some(BtwOverlayState::done(
            "what were we doing".into(),
            "waiting on the task".into(),
        ));
        agent.btw_focused = true;
    }

    let effects = dispatch_send_prompt(&mut app, "keep going".into());

    assert_eq!(sent_texts(&effects), vec!["keep going".to_string()]);
    assert_eq!(
        app.agents[&AgentId(0)]
            .session
            .pending_prompts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>(),
        vec!["held"]
    );
    assert!(
        app.agents[&AgentId(0)].btw_state.is_some(),
        "releasing the send must not dismiss the /btw overlay",
    );
}

/// A prompt queued *before* `/btw` (a follow-up typed while the model was thinking) must stay queued when the answer lands.
/// The send path is what releases a message typed while parked; `/btw` completion must not flush the queue.
#[test]
fn btw_response_does_not_flush_an_unrelated_queued_prompt() {
    let mut app = running_turn_app();
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        simulate_task_output_wait(agent, "task-1");
        agent.btw_state = Some(BtwOverlayState::Loading {
            question: "status?".into(),
        });
    }
    enqueue_local(&mut app, AgentId(0), "queued before btw");

    let effects = dispatch_task_result(
        TaskResult::BtwResponse {
            agent_id: AgentId(0),
            result: Ok("still waiting".into()),
            minimal_request_id: None,
        },
        &mut app,
    );

    assert!(
        sent_texts(&effects).is_empty(),
        "btw completion must not interject a pre-queued follow-up, got {effects:?}"
    );
    assert_eq!(
        app.agents[&AgentId(0)]
            .session
            .pending_prompts
            .front()
            .map(|p| p.text.as_str()),
        Some("queued before btw")
    );
    assert!(
        matches!(
            app.agents[&AgentId(0)].btw_state,
            Some(BtwOverlayState::Done { .. })
        ),
        "the overlay must still show the answer",
    );
}

#[test]
fn queue_mode_send_while_waiting_stays_queued() {
    let mut app = running_turn_app();
    let _mode = LocalFollowUp::queue(&mut app);
    enqueue_local(&mut app, AgentId(0), "queued while thinking");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        simulate_task_output_wait(agent, "task-1");
        assert!(agent.is_parked_on_sendable_wait());
    }

    let effects = dispatch_send_prompt(&mut app, "just typed".into());

    assert!(
        sent_texts(&effects).is_empty(),
        "Queue must not interject during a wait, got {effects:?}"
    );
    assert_eq!(
        app.agents[&AgentId(0)]
            .session
            .pending_prompts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>(),
        vec!["queued while thinking", "just typed"]
    );
}

/// Enter during a wait must interject the message just typed, not an earlier follow-up that was queued while the model was still thinking.
#[test]
fn send_while_waiting_releases_the_new_prompt_not_an_older_queued_row() {
    let mut app = running_turn_app();
    let _mode = LocalFollowUp::steer(&mut app);
    enqueue_local(&mut app, AgentId(0), "queued while thinking");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        simulate_task_output_wait(agent, "task-1");
        assert!(agent.is_parked_on_sendable_wait());
    }

    let effects = dispatch_send_prompt(&mut app, "just typed".into());

    assert_eq!(sent_texts(&effects), vec!["just typed".to_string()]);
    assert_eq!(
        app.agents[&AgentId(0)]
            .session
            .pending_prompts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>(),
        vec!["queued while thinking"]
    );
}

#[test]
fn queue_mode_send_while_waiting_with_empty_queue_stays_queued() {
    let mut app = running_turn_app();
    let _mode = LocalFollowUp::queue(&mut app);
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        simulate_task_output_wait(agent, "task-1");
        assert!(agent.is_parked_on_sendable_wait());
    }

    let effects = dispatch_send_prompt(&mut app, "just typed".into());

    assert!(
        sent_texts(&effects).is_empty(),
        "Queue must not wait-interject even with an empty pile, got {effects:?}"
    );
    assert_eq!(
        app.agents[&AgentId(0)]
            .session
            .pending_prompts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>(),
        vec!["just typed"]
    );
}

/// A thinking turn (running, but not parked / watching) still queues.
#[test]
fn send_while_thinking_stays_queued() {
    let mut app = running_turn_app();
    let _mode = LocalFollowUp::queue(&mut app);

    let effects = dispatch_send_prompt(&mut app, "later".into());

    assert!(
        sent_texts(&effects).is_empty(),
        "a thinking turn must not interject, got {effects:?}"
    );
    assert_eq!(app.agents[&AgentId(0)].session.pending_prompts.len(), 1);
}
