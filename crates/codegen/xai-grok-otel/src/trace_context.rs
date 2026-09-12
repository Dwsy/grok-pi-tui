use opentelemetry::global;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub fn link_span_to_current(span: &tracing::Span) {
    use opentelemetry::trace::TraceContextExt;

    let current = tracing::Span::current();
    if current.is_none() {
        return;
    }
    span.add_link(current.context().span().span_context().clone());
}

pub fn current_traceparent() -> Option<String> {
    span_traceparent(&tracing::Span::current())
}

pub fn span_traceparent(span: &tracing::Span) -> Option<String> {
    if span.is_none() {
        return None;
    }

    let cx = span.context();
    let mut carrier = std::collections::HashMap::new();
    global::get_text_map_propagator(|p| {
        p.inject_context(&cx, &mut carrier);
    });

    carrier.remove("traceparent")
}

pub fn traceparent_of_span(span: &tracing::Span) -> Option<String> {
    span.in_scope(current_traceparent)
}

pub fn inject_trace_context_into_request(
    mut builder: reqwest::RequestBuilder,
) -> reqwest::RequestBuilder {
    let mut headers = HeaderMap::new();
    inject_trace_context(&mut headers);

    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }

    builder
}

pub fn trace_context_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    inject_trace_context(&mut headers);
    headers
}

pub fn inject_trace_context(headers: &mut HeaderMap) {
    let current_span = tracing::Span::current();
    let cx = if current_span.is_none() {
        opentelemetry::Context::current()
    } else {
        current_span.context()
    };

    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HeaderMapInjector(headers));
    });
}

struct HeaderMapInjector<'a>(&'a mut HeaderMap);

impl opentelemetry::propagation::Injector for HeaderMapInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        match (HeaderName::try_from(key), HeaderValue::try_from(&value)) {
            (Ok(name), Ok(val)) => {
                self.0.insert(name, val);
            }
            (Err(e), _) => {
                tracing::debug!("Invalid header name '{}': {}", key, e);
            }
            (_, Err(e)) => {
                tracing::debug!("Invalid header value for '{}': {}", key, e);
            }
        }
    }
}

pub fn span_from_meta_traceparent(
    meta: &serde_json::Map<String, serde_json::Value>,
) -> tracing::Span {
    let span = tracing::info_span!("acp_dispatch");
    if let Some(ctx) = meta
        .get("traceparent")
        .and_then(|v| v.as_str())
        .and_then(extract_context)
    {
        let _ = span.set_parent(ctx);
    }
    span
}

pub fn link_span_to_meta(span: &tracing::Span, meta: &serde_json::Value) -> bool {
    let Some(ctx) = meta
        .get("traceparent")
        .and_then(|v| v.as_str())
        .and_then(extract_context)
    else {
        return false;
    };
    span.set_parent(ctx).is_ok()
}

pub fn link_current_span_to_meta(meta: &serde_json::Value) {
    link_span_to_meta(&tracing::Span::current(), meta);
}

fn extract_context(traceparent: &str) -> Option<opentelemetry::Context> {
    use opentelemetry::trace::TraceContextExt;

    let mut carrier = std::collections::HashMap::new();
    carrier.insert("traceparent".to_string(), traceparent.to_string());

    let ctx = opentelemetry::global::get_text_map_propagator(|p| p.extract(&carrier));
    ctx.span().span_context().is_valid().then_some(ctx)
}

#[allow(clippy::disallowed_methods)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inject_trace_context_no_active_span() {
        let mut headers = HeaderMap::new();
        inject_trace_context(&mut headers);

        assert!(headers.get("traceparent").is_none());
    }

    #[test]
    fn test_header_map_injector_valid_header() {
        let mut headers = HeaderMap::new();
        {
            let mut injector = HeaderMapInjector(&mut headers);
            opentelemetry::propagation::Injector::set(
                &mut injector,
                "traceparent",
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
            );
        }

        assert_eq!(
            headers.get("traceparent").map(|v| v.to_str().unwrap()),
            Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01")
        );
    }

    #[test]
    fn test_header_map_injector_invalid_header_name() {
        let mut headers = HeaderMap::new();
        {
            let mut injector = HeaderMapInjector(&mut headers);

            opentelemetry::propagation::Injector::set(
                &mut injector,
                "invalid header",
                "value".to_string(),
            );
        }

        assert!(headers.is_empty());
    }

    #[test]
    fn test_header_map_injector_invalid_header_value() {
        let mut headers = HeaderMap::new();
        {
            let mut injector = HeaderMapInjector(&mut headers);

            opentelemetry::propagation::Injector::set(
                &mut injector,
                "traceparent",
                "invalid\x00value".to_string(),
            );
        }

        assert!(headers.is_empty());
    }

    #[test]
    fn test_extract_context_rejects_invalid_traceparent() {
        assert!(extract_context("not-a-valid-traceparent").is_none());
        assert!(extract_context("").is_none());
    }

    #[test]
    fn traceparent_of_span_captures_own_span_id_not_parent() {
        use opentelemetry::trace::TraceContextExt as _;
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::propagation::TraceContextPropagator;
        use opentelemetry_sdk::trace::SdkTracerProvider;
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        use tracing_subscriber::layer::SubscriberExt;

        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
        let provider = SdkTracerProvider::builder().build();
        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("test"))
            .with_context_activation(false);
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::Registry::default().with(otel_layer),
        );

        let parent = tracing::info_span!("startup");
        let _entered = parent.enter();
        let session_create = tracing::info_span!("startup.session_create");
        let own = session_create.context().span().span_context().clone();
        let parent_id = parent.context().span().span_context().span_id().to_string();

        let tp = traceparent_of_span(&session_create).expect("traceparent");
        let fields: Vec<&str> = tp.split('-').collect();
        assert_eq!(fields[1], own.trace_id().to_string());
        assert_eq!(fields[2], own.span_id().to_string());
        assert_ne!(fields[2], parent_id);
    }

    #[test]
    fn test_link_meta_then_inject_propagates_trace_id() {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::propagation::TraceContextPropagator;
        use opentelemetry_sdk::trace::SdkTracerProvider;
        use tracing_subscriber::layer::SubscriberExt;

        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

        let provider = SdkTracerProvider::builder().build();
        let tracer = provider.tracer("test");
        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_context_activation(false);
        let subscriber = tracing_subscriber::Registry::default().with(otel_layer);
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);

        let browser_trace_id = "0af7651916cd43dd8448eb211c80319c";
        let meta = serde_json::json!({
            "traceparent": format!("00-{browser_trace_id}-b7ad6b7169203331-01"),
        });

        let span = tracing::info_span!("test_span");
        let _entered = span.enter();
        link_current_span_to_meta(&meta);

        let client = reqwest::Client::new();
        let builder = client.get("https://cli-chat-proxy.example.com/v1/chat/completions");
        let builder = inject_trace_context_into_request(builder);
        let request = builder.build().expect("Failed to build request");

        let traceparent = request
            .headers()
            .get("traceparent")
            .expect("traceparent header missing")
            .to_str()
            .unwrap();

        assert!(
            traceparent.starts_with(&format!("00-{browser_trace_id}-")),
            "outbound traceId should match browser's. got: {traceparent}"
        );
        assert!(
            traceparent.ends_with("-01"),
            "sampled flag should be set. got: {traceparent}"
        );
    }

    #[test]
    fn test_inject_trace_context_into_request_preserves_existing_headers() {
        use opentelemetry::trace::{
            SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
        };
        use opentelemetry_sdk::propagation::TraceContextPropagator;

        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

        let trace_id = TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap();
        let span_id = SpanId::from_hex("b7ad6b7169203331").unwrap();
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        );

        let cx = opentelemetry::Context::current().with_remote_span_context(span_context);
        let _guard = cx.attach();

        let client = reqwest::Client::new();
        let builder = client
            .get("https://example.com")
            .header("x-custom-header", "custom-value")
            .header("authorization", "Bearer token123");

        let builder = inject_trace_context_into_request(builder);
        let request = builder.build().expect("Failed to build request");
        let headers = request.headers();

        assert_eq!(
            headers.get("x-custom-header").map(|v| v.to_str().unwrap()),
            Some("custom-value"),
            "Custom header should be preserved after injecting trace context"
        );
        assert_eq!(
            headers.get("authorization").map(|v| v.to_str().unwrap()),
            Some("Bearer token123"),
            "Authorization header should be preserved after injecting trace context"
        );

        let traceparent = headers
            .get("traceparent")
            .expect("traceparent header should be present with active span")
            .to_str()
            .unwrap();

        assert!(
            traceparent.starts_with("00-0af7651916cd43dd8448eb211c80319c-"),
            "traceparent should contain the correct trace_id, got: {}",
            traceparent
        );
        assert!(
            traceparent.contains("b7ad6b7169203331"),
            "traceparent should contain the correct span_id, got: {}",
            traceparent
        );
        assert!(
            traceparent.ends_with("-01"),
            "traceparent should have sampled flag set, got: {}",
            traceparent
        );
    }
}
