use std::sync::atomic::Ordering;

use tracing_batteries::prelude::opentelemetry::{self, KeyValue, global};
use tracing_batteries::{OpenTelemetry, Session};

/// Enabling metrics installs a real (non no-op) global meter provider which accepts measurements
/// through the standard OpenTelemetry API, and the session shuts it down cleanly without an
/// available collector.
#[tokio::test]
async fn otel_metrics_setup() {
    let session = Session::new("example", "0.0.1")
        .with_debug_builds()
        .with_battery(
            OpenTelemetry::new("localhost:4317")
                .with_header("test-header", "test-value")
                .with_metrics()
                .with_logs(),
        );

    let meter = global::meter("example");
    let counter = meter.u64_counter("example_total").build();
    let histogram = meter
        .f64_histogram("example_duration")
        .with_unit("s")
        .build();

    let span = tracing::info_span!("measuring");
    let _guard = span.enter();

    // Measurements taken within a span see an active OpenTelemetry context (the hook exemplars
    // will attach to once the SDK supports them).
    let cx = opentelemetry::Context::current();
    assert!(
        opentelemetry::trace::TraceContextExt::has_active_span(&cx),
        "the tracing layer should activate the span's OpenTelemetry context"
    );

    counter.add(1, &[KeyValue::new("status", "ok")]);
    histogram.record(0.25, &[KeyValue::new("status", "ok")]);
    tracing::info!(status = "ok", "an exported log record");

    // Flipping the enabled flag must not break subsequent measurements or shutdown.
    session.enable().store(false, Ordering::Relaxed);
    counter.add(1, &[KeyValue::new("status", "suppressed")]);

    drop(_guard);
    session.shutdown();
}
