//! What an [`AgentView`] is: the root of its session, or a child mirrored from a subagent.
//! The role is set once, by [`AgentView::insert_subagent_view`]; every child-specific read site
//! derives its answer from it here instead of keeping a second flag that could drift.

use crate::app::agent_view::AgentView;
use agent_client_protocol as acp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentRole {
    Root,
    Child(ChildLink),
}

/// How a child view relates to the session that spawned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildLink {
    /// The parent session id from the wire; the only session a message to this child may be sent on.
    parent_session_id: acp::SessionId,
    messaging: ChildMessaging,
}

impl ChildLink {
    /// A child with no `agentAddress` on the wire (replayed spawn, workflow child): nothing can be sent to it.
    pub(crate) fn unaddressable(parent_session_id: acp::SessionId) -> Self {
        Self {
            parent_session_id,
            messaging: ChildMessaging::Unaddressable,
        }
    }

    /// A Pi-backed persistent child session. Prompts are sent directly to the
    /// child's ACP session while lifecycle ownership remains with the parent.
    pub(crate) fn persistent(parent_session_id: acp::SessionId) -> Self {
        Self {
            parent_session_id,
            messaging: ChildMessaging::PersistentSession,
        }
    }

    pub(crate) fn parent_session_id(&self) -> &acp::SessionId {
        &self.parent_session_id
    }

    pub(crate) fn is_persistent(&self) -> bool {
        matches!(self.messaging, ChildMessaging::PersistentSession)
    }
}

/// Whether the pager can address the child at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChildMessaging {
    Unaddressable,
    PersistentSession,
}

/// Which chrome the view paints: its own session's, or the framed takeover a parent draws around a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewSurface {
    Root,
    ChildTakeover,
}

/// Where the composer's text would go, which decides whether it renders at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerRoute {
    RootSession,
    /// Pi persistent subagent session; root dispatch routes submission to
    /// `Effect::SendSubagentPrompt` for the child's ACP session.
    ChildSession,
    /// No route exists, so the composer gets zero rows and can't take focus.
    Hidden,
}

impl AgentView {
    /// `None` for the root of a session.
    pub(crate) fn child_link(&self) -> Option<&ChildLink> {
        match &self.role {
            AgentRole::Root => None,
            AgentRole::Child(link) => Some(link),
        }
    }

    pub(crate) fn surface(&self) -> ViewSurface {
        match &self.role {
            AgentRole::Root => ViewSurface::Root,
            AgentRole::Child(_) => ViewSurface::ChildTakeover,
        }
    }

    pub(crate) fn composer_route(&self) -> ComposerRoute {
        match &self.role {
            AgentRole::Root => ComposerRoute::RootSession,
            AgentRole::Child(ChildLink {
                messaging: ChildMessaging::Unaddressable,
                ..
            }) => ComposerRoute::Hidden,
            AgentRole::Child(ChildLink {
                messaging: ChildMessaging::PersistentSession,
                ..
            }) => ComposerRoute::ChildSession,
        }
    }
}

#[cfg(test)]
#[path = "role_tests.rs"]
mod tests;
