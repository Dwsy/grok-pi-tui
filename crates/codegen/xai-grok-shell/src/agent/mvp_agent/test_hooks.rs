//! Environment-driven fault injection for end-to-end tests of client-side recovery paths.
//! Every hook is inert unless its variable is set; the cost when unset is one env read at the hook site.

use agent_client_protocol as acp;

/// `GROK_TEST_PROMPT_BLACKHOLE=1` parks `session/prompt` forever after the dispatch lock is taken.
pub(crate) const PROMPT_BLACKHOLE_ENV: &str = "GROK_TEST_PROMPT_BLACKHOLE";

/// Reproduce a shell whose prompt intake never reaches the session actor. The caller holds the session's
/// dispatch lock, so a follow-up `session/cancel` parks behind it too; that is the wedge shape the client's
/// acknowledgment watch covers and why its cancel send is bounded.
pub(crate) async fn park_forever_if_blackholed(session_id: &acp::SessionId) {
    let blackholed = std::env::var(PROMPT_BLACKHOLE_ENV).is_ok_and(|value| value.trim() == "1");
    if !blackholed {
        return;
    }
    tracing::warn!(
        env = PROMPT_BLACKHOLE_ENV,
        session_id = %session_id.0,
        "test hook: parking session/prompt forever"
    );
    std::future::pending::<()>().await;
}
