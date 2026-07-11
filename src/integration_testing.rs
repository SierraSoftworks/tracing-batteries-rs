use super::{Battery, BatteryBuilder};

/// A dummy integration which does nothing, used for testing purposes.
///
/// This integration is only available when the `testing` feature is enabled,
/// and is not intended for use in production code (where other batteries will
/// be initialized instead). It is useful for testing scenarios where you need
/// a reference to a [`crate::Session`]).
///
/// Internally, this battery sets up a `tracing` subscriber which is compatible
/// with the `opentelemetry` crate, if the `opentelemetry` feature is enabled.
/// This allows you to test code that uses `tracing` and `opentelemetry` without
/// requiring a full telemetry setup (including things like trace context propagation
/// and span creation).
///
/// ## Example
/// ```
/// use tracing_batteries::{Session, Testing};
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///     .with_battery(Testing);
///
/// // Your code which requires a session goes here...
///
/// session.shutdown();
/// ```
pub struct Testing;

impl BatteryBuilder for Testing {
    fn setup(
        self,
        _metadata: &crate::Metadata,
        _enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Box<dyn Battery> {
        Box::new(TestingBattery::new())
    }
}

struct TestingBattery {
    #[cfg(feature = "opentelemetry")]
    _opentelemetry: tracing::subscriber::DefaultGuard,
}

impl TestingBattery {
    fn new() -> Self {
        #[cfg(feature = "opentelemetry")]
        let _opentelemetry = {
            use opentelemetry::trace::TracerProvider as _;
            use tracing_subscriber::layer::SubscriberExt as _;

            opentelemetry::global::set_text_map_propagator(
                opentelemetry_sdk::propagation::TraceContextPropagator::new(),
            );
            let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
                .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
                .build();
            let subscriber = tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("test")));

            tracing::subscriber::set_default(subscriber)
        };

        TestingBattery {
            #[cfg(feature = "opentelemetry")]
            _opentelemetry,
        }
    }
}

impl Battery for TestingBattery {}
