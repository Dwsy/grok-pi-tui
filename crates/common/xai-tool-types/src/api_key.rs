//! Minimal key-provider interface shared by authentication and tool HTTP clients.
//! The trait lives outside the concrete tool implementation crate to avoid a
//! backwards Login → Tools dependency.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Resolves the current API key for outbound tool HTTP requests.
pub trait ApiKeyProvider: Send + Sync + 'static {
    /// Synchronous cached read without refresh.
    fn current_api_key(&self) -> Option<String>;

    /// Per-request resolve; auth providers can override to refresh credentials.
    fn current_api_key_async(&self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(std::future::ready(self.current_api_key()))
    }
}

pub type SharedApiKeyProvider = Arc<dyn ApiKeyProvider>;
