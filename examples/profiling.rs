use std::time::{Duration, Instant};

use tracing_batteries::{Profiling, ProfilingPprof, Session};

/// Exports CPU profiles to the collector at `OTEL_EXPORTER_OTLP_ENDPOINT` (or a local one).
fn main() {
    let session = Session::new("profiling-example", env!("CARGO_PKG_VERSION"))
        .with_debug_builds()
        .with_battery(Profiling::new("http://localhost:4317").with_backend(ProfilingPprof::new()));

    let started = Instant::now();
    let mut value = 0u64;
    while started.elapsed() < Duration::from_secs(2) {
        value = std::hint::black_box(value.wrapping_mul(31).wrapping_add(7));
    }

    session.shutdown();
}
