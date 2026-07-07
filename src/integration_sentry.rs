use std::sync::{Arc, atomic::AtomicBool};

use crate::{Battery, BatteryBuilder, Metadata};

use sentry;
pub use sentry::Level as SentryLevel;

/// Controls how [`Session::record_event`](crate::Session::record_event) calls are
/// reported to Sentry by the [`Sentry`] battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SentryEventMode {
    /// Record events as [breadcrumbs](https://docs.sentry.io/product/issues/issue-details/breadcrumbs/)
    /// which are attached to any subsequent Sentry events (the default).
    #[default]
    Breadcrumb,
    /// Record events as standalone Sentry events, each of which will appear as
    /// its own entry in your Sentry issue stream.
    Event,
}

struct SentryBattery {
    raven: sentry::ClientInitGuard,
    event_mode: SentryEventMode,
    enabled: Arc<AtomicBool>,
}

impl Battery for SentryBattery {
    fn record_event(&self, name: &str, properties: &std::collections::HashMap<String, String>) {
        match self.event_mode {
            SentryEventMode::Breadcrumb => {
                // Breadcrumbs bypass the `before_send` hook which enforces the enabled
                // flag for events, so the flag needs to be checked here instead.
                if !self.enabled.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }

                sentry::add_breadcrumb(sentry::Breadcrumb {
                    message: Some(name.into()),
                    level: sentry::Level::Info,
                    data: properties
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone().into()))
                        .collect(),
                    ..Default::default()
                });
            }
            SentryEventMode::Event => {
                let event = sentry::protocol::Event {
                    message: Some(name.into()),
                    level: sentry::Level::Info,
                    extra: properties
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone().into()))
                        .collect(),
                    ..Default::default()
                };

                sentry::capture_event(event);
            }
        }
    }

    fn record_error(&self, error: &crate::ErrorInfo) {
        sentry::capture_error(error.error);
    }

    fn shutdown(&mut self) {
        sentry::end_session_with_status(sentry::protocol::SessionStatus::Exited);
        self.raven.close(None);
    }
}

/// A [Sentry](https://sentry.io) integration which can be used to record
/// errors that occur within your application.
///
/// <div class="warning">
///
/// This integration requires the `sentry` feature to be enabled.
///
/// </div>
///
/// The Sentry integration can either be initialized by providing just a DSN,
/// or by providing a tuple of a DSN and [`sentry::ClientOptions`] struct.
///
/// ## Example (using DSN)
/// ```no_run
/// use tracing_batteries::{Session, Sentry, prelude::*};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///  .with_battery(Sentry::new("https://username:password@ingest.sentry.io/project"));
///
/// sentry::capture_message("Hello, Sentry!", sentry::Level::Info);
///
/// session.shutdown();
/// ```
///
/// ## Example (using DSN and ClientOptions)
/// ```no_run
/// use tracing_batteries::{Session, Sentry, prelude::*};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///   .with_battery(Sentry::new(("https://username:password@ingest.sentry.io/project", sentry::ClientOptions {
///     environment: Some("production".into()),
///     ..Default::default()
///   })));
///
/// sentry::capture_message("Hello, Sentry!", sentry::Level::Info);
///
/// session.shutdown();
/// ```
pub struct Sentry {
    config: sentry::ClientOptions,

    default_level: Option<SentryLevel>,
    event_mode: SentryEventMode,
}

impl Sentry {
    /// Creates a new Sentry integration using the provided DSN or tuple of DSN and [`sentry::ClientOptions`].
    pub fn new<C: Into<sentry::ClientOptions>>(config: C) -> Self {
        Self {
            config: config.into(),
            default_level: None,
            event_mode: SentryEventMode::default(),
        }
    }

    /// Sets the default level which controls the minimum event level that will be sent to Sentry.
    ///
    /// By default, all events will be sent to Sentry regardless of their level, however this
    /// can be changed by calling this method with a different level or by setting the `LOG_LEVEL`
    /// environment variable to the minimum level you want to send to Sentry.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::{Session, Sentry, SentryLevel, prelude::*};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///   .with_battery(Sentry::new(("https://username:password@ingest.sentry.io/project", sentry::ClientOptions {
    ///     environment: Some("production".into()),
    ///     ..Default::default()
    ///   })).with_default_level(SentryLevel::Warning));
    ///
    /// // Will not be sent to Sentry
    /// sentry::capture_message("Hello, Sentry!", SentryLevel::Info);
    ///
    /// session.shutdown();
    /// ```
    pub fn with_default_level(self, level: SentryLevel) -> Self {
        Self {
            default_level: Some(level),
            ..self
        }
    }

    /// Sets how [`Session::record_event`](crate::Session::record_event) calls are reported to Sentry.
    ///
    /// By default, events are recorded as [breadcrumbs](https://docs.sentry.io/product/issues/issue-details/breadcrumbs/)
    /// which are attached to any subsequent Sentry events, providing context without generating
    /// standalone entries in your issue stream. Use [`SentryEventMode::Event`] if you would prefer
    /// each event to be captured as its own Sentry event.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::{Session, Sentry, SentryEventMode, prelude::*};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///   .with_battery(Sentry::new("https://username:password@ingest.sentry.io/project")
    ///     .with_event_mode(SentryEventMode::Event));
    ///
    /// // Will be captured as a standalone Sentry event rather than a breadcrumb
    /// session.record_event("user_signup", [("method".to_string(), "email".to_string())].into());
    ///
    /// session.shutdown();
    /// ```
    pub fn with_event_mode(self, event_mode: SentryEventMode) -> Self {
        Self { event_mode, ..self }
    }

    fn build_level(&self) -> SentryLevel {
        match std::env::var("LOG_LEVEL")
            .map(|s| s.to_lowercase())
            .as_deref()
        {
            Ok("fatal") => SentryLevel::Fatal,
            Ok("error") => SentryLevel::Error,
            Ok("warn") => SentryLevel::Warning,
            Ok("info") => SentryLevel::Info,
            Ok("debug") => SentryLevel::Debug,
            Ok("trace") => SentryLevel::Debug,
            _ => self.default_level.unwrap_or(SentryLevel::Info),
        }
    }
}

impl BatteryBuilder for Sentry {
    fn setup(self, metadata: &Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        let level = self.build_level();
        let mut config = self.config;
        config.release = match config.release {
            Some(release) => Some(release),
            None => Some(format!("{}@{}", metadata.service, metadata.version).into()),
        };
        config.session_mode = sentry::SessionMode::Application;

        let send_enabled = enabled.clone();
        config.before_send = match config.before_send {
            Some(before_send) => Some(Arc::new(Box::new(
                move |event: sentry::protocol::Event<'static>| {
                    if event.level < level {
                        None
                    } else if send_enabled.load(std::sync::atomic::Ordering::Relaxed) {
                        before_send(event)
                    } else {
                        None
                    }
                },
            ))),
            None => Some(Arc::new(Box::new(
                move |event: sentry::protocol::Event<'static>| {
                    if event.level < level {
                        None
                    } else if send_enabled.load(std::sync::atomic::Ordering::Relaxed) {
                        Some(event)
                    } else {
                        None
                    }
                },
            ))),
        };

        let raven = sentry::init(config);

        sentry::configure_scope(|scope| {
            for (key, value) in &metadata.context {
                scope.set_extra(key, value.clone().into());
            }
        });

        sentry::start_session();

        Box::new(SentryBattery {
            raven,
            event_mode: self.event_mode,
            enabled,
        })
    }
}
