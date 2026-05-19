use anyhow::Result;
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator, SdkTracerProvider};
use remerge_types::trace::TraceContext;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub mod ws_log;

pub struct TelemetryGuard {
    provider: Option<SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
        }
    }
}

pub fn init_tracing(service_name: &'static str, json: bool) -> Result<TelemetryGuard> {
    init_tracing_with_ws_log(service_name, json, None)
}

pub fn init_tracing_with_ws_log(
    service_name: &'static str,
    json: bool,
    ws_log: Option<ws_log::WsLogLayer>,
) -> Result<TelemetryGuard> {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = Resource::builder_empty()
        .with_attributes([KeyValue::new("service.name", service_name)])
        .build();

    let mut provider_builder = SdkTracerProvider::builder().with_resource(resource);

    if otlp_configured() {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .build()?;
        provider_builder = provider_builder.with_batch_exporter(exporter);
    }

    let provider = provider_builder.build();
    let tracer = provider.tracer(service_name);
    global::set_tracer_provider(provider.clone());

    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let env_filter = EnvFilter::from_default_env();

    if json {
        tracing_subscriber::registry()
            .with(ws_log)
            .with(env_filter)
            .with(telemetry_layer)
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(ws_log)
            .with(env_filter)
            .with(telemetry_layer)
            .with(tracing_subscriber::fmt::layer())
            .init();
    }

    Ok(TelemetryGuard {
        provider: Some(provider),
    })
}

pub fn new_trace_context() -> TraceContext {
    let generator = RandomIdGenerator::default();
    let trace_id = generator.new_trace_id().to_string();
    let span_id = generator.new_span_id().to_string();

    TraceContext {
        trace_id: trace_id.clone(),
        traceparent: format!("00-{trace_id}-{span_id}-01"),
    }
}

pub fn parse_trace_context(traceparent: &str) -> Option<TraceContext> {
    let normalized = traceparent.trim().to_ascii_lowercase();
    let mut parts = normalized.split('-');
    let version = parts.next()?;
    let trace_id = parts.next()?;
    let span_id = parts.next()?;
    let flags = parts.next()?;

    if parts.next().is_some()
        || version != "00"
        || trace_id.len() != 32
        || span_id.len() != 16
        || flags.len() != 2
        || trace_id == "00000000000000000000000000000000"
        || span_id == "0000000000000000"
        || !trace_id.chars().all(|c| c.is_ascii_hexdigit())
        || !span_id.chars().all(|c| c.is_ascii_hexdigit())
        || !flags.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }

    Some(TraceContext {
        trace_id: trace_id.to_string(),
        traceparent: format!("00-{trace_id}-{span_id}-{flags}"),
    })
}

pub fn set_span_parent(span: &Span, trace_context: Option<&TraceContext>) {
    let Some(trace_context) = trace_context else {
        return;
    };

    let mut carrier = std::collections::HashMap::new();
    carrier.insert(
        remerge_types::trace::TRACEPARENT_HEADER.to_string(),
        trace_context.traceparent.clone(),
    );

    let parent_context = global::get_text_map_propagator(|propagator| {
        propagator.extract(&HashMapExtractor::new(&carrier))
    });
    let _ = span.set_parent(parent_context);
}

fn otlp_configured() -> bool {
    std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some()
        || std::env::var_os("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_some()
}

struct HashMapExtractor<'a> {
    carrier: &'a std::collections::HashMap<String, String>,
}

impl<'a> HashMapExtractor<'a> {
    fn new(carrier: &'a std::collections::HashMap<String, String>) -> Self {
        Self { carrier }
    }
}

impl opentelemetry::propagation::Extractor for HashMapExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.carrier.get(key).map(String::as_str)
    }

    fn keys(&self) -> Vec<&str> {
        self.carrier.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{new_trace_context, parse_trace_context};

    #[test]
    fn generated_trace_context_is_well_formed() {
        let trace = new_trace_context();
        assert_eq!(trace.trace_id.len(), 32);
        assert!(parse_trace_context(&trace.traceparent).is_some());
    }

    #[test]
    fn invalid_traceparent_is_rejected() {
        assert!(parse_trace_context("00-bad-2222222222222222-01").is_none());
        assert!(
            parse_trace_context("00-00000000000000000000000000000000-2222222222222222-01")
                .is_none()
        );
    }
}
