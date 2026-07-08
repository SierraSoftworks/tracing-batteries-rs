use super::{Battery, BatteryBuilder};

/// A dummy integration which does nothing, used for testing purposes.
///
/// This integration is only available when the `testing` feature is enabled,
/// and is not intended for use in production code (where other batteries will
/// be initialized instead). It is useful for testing scenarios where you need
/// a reference to a [`crate::Session`]).
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
        Box::new(Testing)
    }
}

impl Battery for Testing {}
