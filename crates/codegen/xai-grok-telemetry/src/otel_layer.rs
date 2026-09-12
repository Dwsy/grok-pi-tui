use std::sync::Arc;

use tracing_subscriber::registry::LookupSpan;

pub use xai_grok_otel::config::{OtelClientInfo, OtelExporterConfig, OtelLayerConfig};

pub fn build_otel_layer<S>(
    client: OtelClientInfo,
    config: OtelLayerConfig,
) -> impl tracing_subscriber::layer::Layer<S>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    let mode = match crate::instrumentation::current_mode() {
        crate::instrumentation::InstrumentationMode::Server => {
            xai_grok_otel::provider::OtelProviderMode::Server
        }
        _ => xai_grok_otel::provider::OtelProviderMode::Local,
    };
    xai_grok_otel::provider::build_otel_layer(
        client,
        config,
        mode,
        Arc::new(crate::client::is_session_metrics_enabled),
    )
}

pub fn shutdown_otel() {
    let provider = std::thread::spawn(xai_grok_otel::provider::shutdown_provider);
    crate::external::shutdown();
    let _ = provider.join();
}

pub struct OtelGuard;

impl Drop for OtelGuard {
    fn drop(&mut self) {
        shutdown_otel();
    }
}

pub fn otel_guard() -> OtelGuard {
    OtelGuard
}
