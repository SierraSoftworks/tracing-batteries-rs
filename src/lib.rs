use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{borrow::Cow, collections::HashMap};

#[cfg(feature = "opentelemetry")]
mod integration_opentelemetry;
#[cfg(feature = "sentry")]
mod integration_sentry;
pub mod prelude;

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
    /// Called whenever the [`Session::record_error`] method is called, allowing the integration
    /// to report an error to the telemetry system through the appropriate mechanism.
    fn record_error(&self, _error: &dyn std::error::Error) {}

    /// Called when the process is exiting, allowing the integration to perform any necessary cleanup
    /// and shutdown operations.
    ///
    /// There is no guarantee that the application will not attempt to use the integration after this
    /// method is called, so if necessary the integration should ensure that it can handle this safely.
    fn shutdown(&self) {}
}

/// A telemetry session which is used to manage the lifecycle of the telemetry subsystem.
///
/// The session is the primary entrypoint for this library and maintains a list of batteries
/// which have been initialized, as well as metadata about the service that is being monitored.
///
/// You can attach new batteries to the service at any time, however it is expected that these
/// are attached at the beginning of the application's lifecycle and the session is retained until
/// the application is ready to exit.
pub struct Session {
    metadata: Metadata,
    batteries: Vec<Box<dyn Battery>>,
    enabled: Arc<AtomicBool>,
}

impl Session {
    /// Starts the process of initializing a new telemetry session for the provided application.
    ///
    /// The `service` and `version` parameters should be used to identify the service which is being monitored,
    /// it is common to use the `env!("CARGO_PKG_NAME")` and `env!("CARGO_PKG_VERSION")` macros to provide this information.
    ///
    /// This method returns a [`Metadata`] object which may be modified until such time as a battery is attached to the session,
    /// at which point the session will be locked and only additional batteries may be added.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Sentry};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///   .with_context("example", "yes")
    ///   .with_battery(Sentry::new("https://yourdsn@sentry.example.com/app-id"));
    /// ```
    #[allow(clippy::new_ret_no_self)]
    pub fn new<S: Into<Cow<'static, str>>, V: Into<Cow<'static, str>>>(
        service: S,
        version: V,
    ) -> Metadata {
        Metadata {
            service: service.into(),
            version: version.into(),
            context: HashMap::new(),
        }
    }

    /// Records that an error has occurred within the application, reporting it to any registered batteries.
    ///
    /// This method may be called to explicitly report an error within the application to your
    /// telemetry services. It is most commonly used to report errors which are not otherwise
    /// captured by the telemetry system, such as errors which are caught and handled by the application's
    /// `main()` function.
    ///
    /// For ease of usage, this function returns the error which was passed to it, allowing it to be used
    /// inline with other error handling code.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Sentry};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///   .with_battery(Sentry::new("https://yourdsn@sentry.example.com"));
    ///
    /// match std::fs::read_to_string("nonexistent-file.txt") {
    ///  Ok(_) => {}
    ///  Err(e) => eprintln!("{:?}", session.record_error(&e)),
    /// }
    /// ```
    pub fn record_error<'a, E: std::error::Error>(&self, exception: &'a E) -> &'a E {
        for battery in &self.batteries {
            battery.record_error(exception);
        }

        exception
    }

    /// Shuts down the telemetry session, ensuring that all batteries are properly cleaned up.
    ///
    /// This method should be called when the application is ready to exit, ensuring that all
    /// telemetry data has been flushed and that all resources have been released. It is a
    /// blocking operation and will not return until all batteries have been shut down.
    pub fn shutdown(self) {
        for battery in self.batteries {
            battery.shutdown();
        }
    }

    /// Returns a reference to the [`AtomicBool`] which is used to control the enabled state of the telemetry session.
    ///
    /// This method is intended to be used by the hosting application to either check, or modify, whether the telemetry
    /// integrations report data to their respective services. The [`AtomicBool`](std::sync::atomic::AtomicBool)
    /// is used to ensure that the enabled state can be modified safely from multiple threads.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::Session;
    /// use std::sync::atomic::Ordering;
    ///
    /// # use std::sync::{Arc, atomic::AtomicBool};
    /// # use tracing_batteries::{Metadata, BatteryBuilder, Battery};
    /// # struct MockBattery;
    /// # impl Battery for MockBattery {}
    /// # impl BatteryBuilder for MockBattery {
    /// #    fn setup(self, _metadata: &Metadata, _enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
    /// #       Box::new(MockBattery)
    /// #    }
    /// # }
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///   .with_battery(MockBattery);
    ///
    /// let telemetry_enabled = session.enable();
    /// telemetry_enabled.store(false, Ordering::Relaxed);
    ///
    /// session.shutdown();
    /// ```
    pub fn enable(&self) -> Arc<AtomicBool> {
        self.enabled.clone()
    }
}

impl Session {
    /// Attaches a new battery to the telemetry session, integrating the requested telemetry
    /// provider into the application.
    pub fn with_battery<B: BatteryBuilder>(mut self, builder: B) -> Self {
        let battery = builder.setup(&self.metadata, self.enabled.clone());
        self.batteries.push(battery);
        self
    }
}

/// Metadata about the service which is being monitored by the telemetry system.
///
/// This struct contains information about the service which is being monitored, including the service name,
/// version, and any additional context which has been provided. The `metadata.context` will usually be
/// attached to any descriptive information about the service that is reported to the telemetry system
/// (for example, the `Resource`, `extra` context fields, or identifying dimensions).
///
/// This struct is returned by the [`Session::new`] method and may be modified until such time as a battery
/// is attached to the session, at which point the session will be locked and only additional batteries may be added.
///
/// ## Example
/// ```rust
/// use tracing_batteries::Session;
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///  .with_context("example", "yes");
pub struct Metadata {
    pub service: Cow<'static, str>,
    pub version: Cow<'static, str>,

    pub context: HashMap<&'static str, Cow<'static, str>>,
}

impl Metadata {
    /// Adds a new context field to the metadata, which will be reported to the telemetry system.
    pub fn with_context<V: Into<Cow<'static, str>>>(mut self, key: &'static str, value: V) -> Self {
        self.context.insert(key, value.into());
        self
    }

    /// Attaches a new battery to the telemetry session, integrating the requested telemetry
    /// provider into the application.
    pub fn with_battery<B: BatteryBuilder>(self, battery: B) -> Session {
        Session {
            metadata: self,
            batteries: Vec::new(),
            enabled: Arc::new(AtomicBool::new(true)),
        }
        .with_battery(battery)
    }
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
        fn shutdown(&self) {
            println!("ExampleBattery dropped");
        }
    }
}
