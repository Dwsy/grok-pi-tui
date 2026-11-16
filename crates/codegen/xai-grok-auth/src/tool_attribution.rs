//! Authentication failure callback contract for stock image/video/web tools.
//! No tool implementation dependency: the auth manager owns the callback and
//! tool HTTP clients only emit redacted metadata through it.

use std::sync::Arc;

/// Which tool endpoint returned HTTP 401.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::AsRefStr, strum::IntoStaticStr)]
pub enum ToolConsumer {
    #[strum(serialize = "ImageGen")]
    ImageGen,
    #[strum(serialize = "VideoGen.start")]
    VideoGenStart,
    #[strum(serialize = "VideoGen.poll")]
    VideoGenPoll,
    #[strum(serialize = "WebSearch")]
    WebSearch,
}

/// Attribution callback. `sent_bearer_suffix` must already be redacted to
/// the last `BEARER_SUFFIX_LEN` characters before crossing the boundary.
pub trait Auth401AttributionCallback: Send + Sync + std::fmt::Debug {
    fn record_401(&self, consumer: ToolConsumer, sent_bearer_suffix: Option<&str>);
}

pub type SharedAttributionCallback = Arc<dyn Auth401AttributionCallback>;
