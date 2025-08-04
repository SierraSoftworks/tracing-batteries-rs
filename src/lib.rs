use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[cfg(feature = "medama")]
mod integration_medama;
#[cfg(feature = "opentelemetry")]
mod integration_opentelemetry;
#[cfg(feature = "sentry")]
mod integration_sentry;
mod metadata;
pub mod prelude;
mod session;

pub use metadata::Metadata;
pub use session::Session;

#[cfg(feature = "medama")]
pub use integration_medama::*;
#[cfg(feature = "opentelemetry")]
pub use integration_opentelemetry::*;
#[cfg(feature = "sentry")]
pub use integration_sentry::*;

/// A trait which is implemented by integration builders, allowing them to be used with this library.
///
/// This trait should be implemented on a builder object which will be
/// used by the library to initialize a specific tracing integration.
///
/// The builder itself is responsible for registering and initializing the integration,
/// ensuring that it is ready for use. It should then return a type implementing
/// the [`Battery`] trait which will then be used to manage error reporting and
/// shutdown of the integration.
pub trait BatteryBuilder {
    /// Sets up the integration and returns a [`Battery`] which will be used to manage it.
    ///
    /// This method is called by the library to initialize the integration and should return
    /// a [`Battery`] which will be used to manage the integration's lifecycle.
    ///
    /// The `metadata` parameter contains information about the service which is being monitored,
    /// including the service name, version, and any additional context which has been provided.
    /// The `metadata.context` should usually be attached to any descriptive information about
    /// the service that is reported to the telemetry system (for example, the `Resource`,
    /// `extra` context fields, or identifying dimensions).
    fn setup(self, metadata: &Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery>;
}

/// A trait which is implemented by the initialized integration, allowing it to receive
/// notifications about errors and to be shut down when the process is exiting.
///
/// This trait should be implemented on the type which is returned by the [`BatteryBuilder::setup`] method.
pub trait Battery {
    /// Called whenever the [`Session::record_new_page`] method is called, allowing the integration
    /// to report that a new page view has started (and finish any existing page views which are
    /// currently active). Only one page view can be active at a time, so this method should
    /// finish the previous page view before starting a new one.
    fn record_new_page<'a>(&self, _page: &'a str) {}

    /// Called whenever the [`Session::record_error`] method is called, allowing the integration
    /// to report an error to the telemetry system through the appropriate mechanism.
    fn record_error(&self, _error: &dyn std::error::Error) {}

    /// Called when the process is exiting, allowing the integration to perform any necessary cleanup
    /// and shutdown operations.
    ///
    /// There is no guarantee that the application will not attempt to use the integration after this
    /// method is called, so if necessary the integration should ensure that it can handle this safely.
    fn shutdown(&mut self) {}
}

#[cfg(test)]
mod tests {
    use std::sync::{atomic::AtomicBool, Arc};

    use crate::{Battery, BatteryBuilder, Session};

    #[test]
    fn basic_setup() {
        let session = Session::new("example", "0.0.1")
            .with_context("example", "yes")
            .with_battery(ExampleBattery);

        session.shutdown();
    }

    struct ExampleBattery;

    impl BatteryBuilder for ExampleBattery {
        fn setup(self, _metadata: &crate::Metadata, _enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
            println!("ExampleBattery initialized");
            Box::new(ExampleBattery)
        }
    }

    impl Battery for ExampleBattery {
        fn shutdown(&mut self) {
            println!("ExampleBattery dropped");
        }
    }
}
