pub use tracing::{
    self, Event, Instrument, Span, debug, debug_span, error, error_span, event,
    field::{Empty as EmptyField, debug, display},
    info, info_span, instrument, span, trace, trace_span, warn, warn_span,
};

#[cfg(feature = "opentelemetry")]
pub use tracing_opentelemetry::{self, OpenTelemetrySpanExt};

#[cfg(feature = "opentelemetry")]
pub use opentelemetry::{
    self,
    global::{get_text_map_propagator, set_text_map_propagator},
    propagation::{
        Extractor as OpenTelemetryPropagationExtractor,
        Injector as OpenTelemetryPropagationInjector,
    },
    trace::SpanKind as OpenTelemetrySpanKind,
    trace::TraceContextExt,
};

#[cfg(feature = "opentelemetry")]
pub use opentelemetry_sdk::propagation::TraceContextPropagator;

#[cfg(feature = "sentry")]
pub use sentry;
