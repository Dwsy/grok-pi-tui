//! 401 attribution: callback hook + shared helpers for tool HTTP clients.

use xai_grok_auth::bearer_suffix;
pub use xai_grok_auth::tool_attribution::{
    Auth401AttributionCallback, SharedAttributionCallback, ToolConsumer,
};

pub use xai_grok_auth::bearer_fragment::BEARER_SUFFIX_LEN;

/// Record a 401 if a callback is wired, truncating to the tail first so only
/// the fragment is ever materialized.
pub(crate) fn emit_401(
    callback: Option<&SharedAttributionCallback>,
    consumer: ToolConsumer,
    sent_bearer: Option<&str>,
) {
    if let Some(cb) = callback {
        let suffix = sent_bearer.map(|s| bearer_suffix(s).to_string());
        cb.record_401(consumer, suffix.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_consumer_as_str_stable_identifiers() {
        assert_eq!(ToolConsumer::ImageGen.as_ref(), "ImageGen");
        assert_eq!(ToolConsumer::VideoGenStart.as_ref(), "VideoGen.start");
        assert_eq!(ToolConsumer::VideoGenPoll.as_ref(), "VideoGen.poll");
        assert_eq!(ToolConsumer::WebSearch.as_ref(), "WebSearch");
    }
}
