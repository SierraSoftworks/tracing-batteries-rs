pub use tracing::{
    self, debug, debug_span, error, error_span, event,
    field::{debug, display, Empty as EmptyField},
    info, info_span, span, trace, trace_span, warn, warn_span, Event, instrument, Instrument, Span,
};

#[cfg(feature = "opentelemetry")]
pub use tracing_opentelemetry::{self, OpenTelemetrySpanExt};

#[cfg(feature = "opentelemetry")]
pub use opentelemetry::{self, trace::SpanKind as OpenTelemetrySpanKind, trace::TraceContextExt};

#[cfg(feature = "opentelemetry")]
pub use opentelemetry_sdk::propagation::TraceContextPropagator;

#[cfg(feature = "sentry")]
pub use sentry;
