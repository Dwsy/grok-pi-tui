pub mod conversation;
pub mod error;
pub mod types;

pub use self::conversation::*;
pub use self::error::{ResponseModelMetadata, Result, SamplingError};
pub use self::types::*;
// Data types already owned by `xai-grok-sampling-types`: named here so shell callers
// keep their `crate::sampling::*` paths while the sampler runtime stays optional.
pub use xai_grok_sampling_types::{
    ApiBackend, ConversationGroupId, OriginClientInfo, SamplingErrorKind,
};
// `Client` is the legacy alias for the sampler crate's `SamplingClient`. Only the Stock
// agent runtime names it: the Pi profile delegates sampling to Pi over JSONL RPC and
// never instantiates a sampler client.
#[cfg(feature = "stock-agent")]
pub use xai_grok_sampler::SamplingClient as Client;

// Re-export async-openai Responses API types under `rs` namespace
pub use async_openai::types::responses as rs;

// --------------------------------------------------------------------------- xai-grok-sampler runtime re-exports --------------------------------------------------------------------------- The streaming/retry/HTTP runtime still lives in `xai-grok-sampler`, named only by the Stock agent runtime.
// Everything a Pi-profile caller needs already lives in `xai-grok-sampling-types` and is re-exported above.
#[cfg(feature = "stock-agent")]
pub use xai_grok_sampler::{
    InferenceLatencyStats, RequestId, SamplerActor, SamplerConfig, SamplerHandle, SamplingChannel,
    SamplingClient, SamplingErrorInfo, SamplingEvent,
};

const CONVERSATION_GROUP_NAMESPACE: &str = "xai:grok-build:conversation-group:";

/// Derive the stable group shared by a root session and every descendant session.
pub(crate) fn derive_conversation_group_id(root_session_id: &str) -> ConversationGroupId {
    let namespace_input = format!("{CONVERSATION_GROUP_NAMESPACE}{root_session_id}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, namespace_input.as_bytes())
        .to_string()
        .into()
}

#[cfg(test)]
mod conversation_group_tests {
    use pretty_assertions::{assert_eq, assert_ne};

    use super::*;

    #[test]
    fn derivation_is_stable_and_frozen() {
        let first = derive_conversation_group_id("root-session-123");
        let second = derive_conversation_group_id("root-session-123");

        assert_eq!(first.as_ref(), "111fc242-925b-5a7d-826e-2974daad239f");
        assert_eq!(first, second);
    }

    #[test]
    fn different_roots_have_different_groups() {
        assert_ne!(
            derive_conversation_group_id("root-a"),
            derive_conversation_group_id("root-b")
        );
    }
}
