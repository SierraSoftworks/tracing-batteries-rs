use crate::{Battery, BatteryBuilder, Metadata};

pub use sentry;

struct SentryBattery {
    raven: sentry::ClientInitGuard,
}

impl Battery for SentryBattery {
    fn shutdown(&self) {
        sentry::end_session_with_status(sentry::protocol::SessionStatus::Exited);
        self.raven.close(None);
    }

    fn record_error(&self, error: &dyn std::error::Error) {
        sentry::capture_error(error);
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
/// use tracing_batteries::{Session, Sentry, sentry};
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
/// use tracing_batteries::{Session, Sentry, sentry};
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
}

impl Sentry {
    /// Creates a new Sentry integration using the provided DSN or tuple of DSN and [`sentry::ClientOptions`].
    pub fn new<C: Into<sentry::ClientOptions>>(config: C) -> Self {
        Self {
            config: config.into(),
        }
    }
}

impl BatteryBuilder for Sentry {
    fn setup(self, metadata: &Metadata) -> Box<dyn Battery> {
        let mut config = self.config;
        config.release = match config.release {
            Some(release) => Some(release),
            None => Some(format!("{}@{}", metadata.service, metadata.version).into()),
        };

        let raven = sentry::init(config);

        sentry::configure_scope(|scope| {
            for (key, value) in &metadata.context {
                scope.set_extra(key, value.clone().into());
            }
        });

        sentry::start_session();

        Box::new(SentryBattery { raven })
    }
}
