//! An end-to-end driver for the Analytics integration.
//!
//! Run an analytics server locally (by default it listens on `127.0.0.1:8085`)
//! and then run this example against it:
//!
//! ```sh
//! cargo run --example analytics --features analytics
//! PANIC=1 cargo run --example analytics --features analytics
//! ```

use tracing_batteries::{Analytics, Session};

#[derive(Debug)]
struct ExampleError {
    source: std::io::Error,
}

impl std::fmt::Display for ExampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to complete the example operation")
    }
}

impl std::error::Error for ExampleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[tokio::main]
async fn main() {
    let server =
        std::env::var("ANALYTICS_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8085".to_string());

    let session = Session::new("example", env!("CARGO_PKG_VERSION"))
        .with_context("example.context", "demo")
        .with_battery(Analytics::new(server).with_initial_page("/"));

    {
        let _page = session.record_new_page("/work");

        session.record_event(
            "example-event",
            [("plan".to_string(), "pro".to_string())].into(),
        );

        session.record_error(&ExampleError {
            source: std::io::Error::other("the underlying cause"),
        });

        if std::env::var("PANIC").is_ok() {
            panic!("intentional example panic");
        }
    }

    session.shutdown();
}
