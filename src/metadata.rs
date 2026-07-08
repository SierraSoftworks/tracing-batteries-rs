use crate::{BatteryBuilder, Session};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

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
#[derive(Clone)]
pub struct Metadata {
    pub service: Cow<'static, str>,
    pub version: Cow<'static, str>,

    pub context: HashMap<&'static str, Cow<'static, str>>,

    pub enabled_by_default: bool,
}

impl Metadata {
    /// Adds a new context field to the metadata, which will be reported to the telemetry system.
    pub fn with_context<V: Into<Cow<'static, str>>>(mut self, key: &'static str, value: V) -> Self {
        self.context.insert(key, value.into());
        self
    }

    /// Enables emission of telemetry events when running under debug builds.
    ///
    /// This method can be used to override the default behavior of the telemetry system,
    /// which is to disable telemetry events when running under debug builds. This is useful for
    /// development and testing, as it allows developers to see telemetry events without having to
    /// build a release version of the application.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Sentry};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///    .with_debug_builds()
    ///    .with_battery(Sentry::new("https://username:password@ingest.sentry.io/project"));
    ///
    /// // Your code goes here...
    ///
    /// session.shutdown();
    /// ```
    pub fn with_debug_builds(mut self) -> Self {
        #[cfg(debug_assertions)]
        {
            self.enabled_by_default = true;
        }
        self
    }

    /// Attaches a new battery to the telemetry session, integrating the requested telemetry
    /// provider into the application.
    pub fn with_battery<B: BatteryBuilder>(self, battery: B) -> Session {
        let enabled = AtomicBool::new(self.enabled_by_default);

        Session {
            metadata: self,
            batteries: Vec::new(),
            page_stack: Mutex::new(Vec::new()),
            enabled: Arc::new(enabled),
        }
        .with_battery(battery)
    }
}
