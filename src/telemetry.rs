//! OTel initialization, TRACEPARENT extraction, and shutdown helpers.
//!
//! Everything here is a no-op when the `otel` cargo feature is disabled or when
//! the `OTEL_ENABLED` environment variable is absent / falsy (matching the
//! Python-side opt-in design in `mumei-agent`).

/// Initialize the OpenTelemetry tracing pipeline.
///
/// Reads `OTEL_ENABLED` and `OTEL_EXPORTER_OTLP_ENDPOINT` from the
/// environment.  When the `otel` feature is disabled or `OTEL_ENABLED` is not
/// truthy this is a no-op.
pub fn init_telemetry() {
    #[cfg(feature = "otel")]
    {
        if !otel_enabled() {
            return;
        }
        if let Err(e) = try_init_otel() {
            eprintln!(
                "mumei: OTel init failed (continuing without telemetry): {}",
                e
            );
        }
    }
}

/// Flush pending spans and shut down the tracer provider.
pub fn shutdown_telemetry() {
    #[cfg(feature = "otel")]
    {
        if !otel_enabled() {
            return;
        }
        if let Some(provider) = PROVIDER.get() {
            let _ = provider.shutdown();
        }
    }
}

#[cfg(feature = "otel")]
static PROVIDER: std::sync::OnceLock<opentelemetry_sdk::trace::SdkTracerProvider> =
    std::sync::OnceLock::new();

/// Attach the parent OTel context extracted from `TRACEPARENT` / `TRACESTATE`
/// environment variables.
///
/// Returns a guard whose `Drop` detaches the context.  Any `tracing` span
/// created while the guard is alive will be parented under the extracted
/// context (via the `tracing-opentelemetry` layer).
///
/// When the `otel` feature is disabled this returns `()` and does nothing.
#[cfg(feature = "otel")]
pub fn attach_parent_context() -> Option<opentelemetry::ContextGuard> {
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry::trace::TraceContextExt;
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use std::collections::HashMap;

    let tp = std::env::var("TRACEPARENT").ok().filter(|v| !v.is_empty());
    if tp.is_none() {
        return None;
    }

    let mut carrier: HashMap<String, String> = HashMap::new();
    if let Some(tp) = tp {
        carrier.insert("traceparent".to_string(), tp);
    }
    if let Ok(ts) = std::env::var("TRACESTATE") {
        if !ts.is_empty() {
            carrier.insert("tracestate".to_string(), ts);
        }
    }

    let propagator = TraceContextPropagator::new();
    let ctx = propagator.extract(&carrier);

    // Only attach if the extracted context contains a valid remote span.
    if ctx.span().span_context().is_valid() {
        Some(opentelemetry::Context::attach(ctx))
    } else {
        None
    }
}

#[cfg(not(feature = "otel"))]
#[allow(dead_code)]
pub fn attach_parent_context() {}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[cfg(feature = "otel")]
fn otel_enabled() -> bool {
    std::env::var("OTEL_ENABLED")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"))
        .unwrap_or(false)
}

#[cfg(feature = "otel")]
fn try_init_otel() -> Result<(), Box<dyn std::error::Error>> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::Resource;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()?;

    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter)
        .with_resource(Resource::builder().with_service_name("mumei").build())
        .build();

    opentelemetry::global::set_tracer_provider(provider.clone());
    let _ = PROVIDER.set(provider.clone());
    let tracer = provider.tracer("mumei");

    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
        .with(otel_layer)
        .try_init()
        .map_err(|e| format!("tracing subscriber init: {}", e))?;

    Ok(())
}
