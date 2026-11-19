//! Small leader connection-state contract shared with the TUI.
//!
//! The Pi adapter does not run the stock Leader process or IPC client.
//! Keeping the status type separate prevents that whole runtime from being
//! compiled simply because the TUI retains its reconnect presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Initial connection is generation 0; reconnects increment generation.
    Connected { generation: u64 },
    /// Current reconnection attempt.
    Reconnecting { attempt: u32 },
    /// Terminal connection failure.
    Failed { error: String },
}
