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

#[cfg(all(test, feature = "otel"))]
mod tests {
    use super::attach_parent_context;
    use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
    use opentelemetry_sdk::error::OTelSdkResult;
    use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;

    // W3C Trace Context example header split into its trace-id / parent-span-id.
    const TRACE_ID_HEX: &str = "0af7651916cd43dd8448eb211c80319c";
    const PARENT_SPAN_HEX: &str = "b7ad6b7169203331";

    // Serialize env-var mutation across the tests in this module: TRACEPARENT is
    // process-global and the tests run in the same binary in parallel by default.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Minimal in-memory span exporter so we can assert the parent/child edges
    /// of the emitted spans without standing up an OTLP collector.
    #[derive(Debug, Clone, Default)]
    struct CollectingExporter {
        spans: Arc<Mutex<Vec<SpanData>>>,
    }

    impl SpanExporter for CollectingExporter {
        fn export(
            &self,
            batch: Vec<SpanData>,
        ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
            self.spans.lock().unwrap().extend(batch);
            std::future::ready(Ok(()))
        }
    }

    #[test]
    fn attach_parent_context_extracts_valid_traceparent() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "TRACEPARENT",
            format!("00-{TRACE_ID_HEX}-{PARENT_SPAN_HEX}-01"),
        );

        let guard = attach_parent_context();
        assert!(guard.is_some(), "valid TRACEPARENT should attach a context");

        let ctx = opentelemetry::Context::current();
        let sc = ctx.span().span_context().clone();
        assert!(sc.is_valid(), "attached span context must be valid");
        assert!(sc.is_remote(), "attached parent must be flagged remote");
        assert_eq!(sc.trace_id().to_string(), TRACE_ID_HEX);
        assert_eq!(sc.span_id().to_string(), PARENT_SPAN_HEX);

        drop(guard);
        std::env::remove_var("TRACEPARENT");
    }

    #[test]
    fn attach_parent_context_ignores_invalid_traceparent() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("TRACEPARENT", "invalid-garbage-value");
        assert!(
            attach_parent_context().is_none(),
            "malformed TRACEPARENT must not attach a context"
        );
        std::env::remove_var("TRACEPARENT");
    }

    #[test]
    fn attach_parent_context_none_without_traceparent() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TRACEPARENT");
        assert!(
            attach_parent_context().is_none(),
            "absent TRACEPARENT must not attach a context"
        );
    }

    /// End-to-end: replay the verify pipeline's span nesting
    /// (`mumei.verify.cli` -> `mumei.z3.solve`) under an extracted TRACEPARENT
    /// and assert the exported spans form a parent/child chain rooted at the
    /// remote caller span.
    #[test]
    fn verify_pipeline_spans_are_children_of_traceparent() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "TRACEPARENT",
            format!("00-{TRACE_ID_HEX}-{PARENT_SPAN_HEX}-01"),
        );

        let exporter = CollectingExporter::default();
        let collected = exporter.spans.clone();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter)
            .build();
        let tracer = provider.tracer("mumei");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
        let subscriber = tracing_subscriber::registry().with(otel_layer);

        tracing::subscriber::with_default(subscriber, || {
            let guard = attach_parent_context();
            assert!(guard.is_some());

            let verify_span = tracing::info_span!("mumei.verify.cli", source_path = "test.mm");
            verify_span.in_scope(|| {
                let z3_span = tracing::info_span!("mumei.z3.solve");
                z3_span.in_scope(|| {});
            });

            drop(guard);
        });

        provider.force_flush().unwrap();

        let spans = collected.lock().unwrap();
        let verify = spans
            .iter()
            .find(|s| s.name == "mumei.verify.cli")
            .expect("verify span exported");
        let z3 = spans
            .iter()
            .find(|s| s.name == "mumei.z3.solve")
            .expect("z3 span exported");

        // Both spans belong to the caller's trace.
        assert_eq!(verify.span_context.trace_id().to_string(), TRACE_ID_HEX);
        assert_eq!(z3.span_context.trace_id().to_string(), TRACE_ID_HEX);

        // The root pipeline span is a child of the remote caller span.
        assert_eq!(verify.parent_span_id.to_string(), PARENT_SPAN_HEX);

        // The z3 span is a child of the verify span.
        assert_eq!(
            z3.parent_span_id.to_string(),
            verify.span_context.span_id().to_string()
        );

        drop(spans);
        std::env::remove_var("TRACEPARENT");
    }
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
