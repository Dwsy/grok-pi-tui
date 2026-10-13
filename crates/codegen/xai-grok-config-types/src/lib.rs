#![deny(clippy::indexing_slicing)]

mod file_acceleration;
pub use file_acceleration::*;
mod flags;
pub use flags::*;
mod registry;
pub use registry::*;
mod memory;
pub use memory::*;
mod mcp;
pub use mcp::*;
mod permission;
pub use permission::*;
mod auth_provider;
pub use auth_provider::*;
mod privacy;
pub use privacy::{PrivacyMode, SharedPrivacyMode};
mod sandbox;
pub use sandbox::*;

pub use xai_grok_config::{
    CampaignOverride, ConsentGate, ContextualHintsRemote, DisplayRefreshSettings,
    DoomLoopRecoverySettings, GoalRoleModel, LongReasoningReminderSettings, RemoteRequestEncoding,
    RemoteSettings, WorktreeAutoGcSettings, WorktreeKindMaxAge, deserialize_tolerant,
};
