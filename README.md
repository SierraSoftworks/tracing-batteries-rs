# Tracing, batteries included
**Easily configure tracing integrations for your Rust applications**

This library has been built to simplify the process of configuring tracing
integrations for Rust applications, handling the complexity of maintaiing
all of the various `tracing`, `opentelemetry` and `sentry` API changes that
happen in the Rust ecosystem.

The goal here is that you should be able to write your telemetry integration
code once, and then forget about it while this library takes care of doing
the gymnastics required to keep everything working.

## Usage
The first step here is adding the `tracing-batteries-rs` crate to your
`Cargo.toml` file:

```toml
[dependencies]
tracing-batteries = {
  git = "https://github.com/sierrasoftworks/tracing-batteries-rs.git",
  features = ["sentry", "tracing-opentelemetry"]
}
```

Then you'll want to add the tracing initialization to your application.

```rust
use tracing_batteries::{Session, Sentry, OpenTelemetry, OpenTelemetryProtocol};

fn main() {
    let session = Session::new("my-service", env!("CARGO_PKG_VERSION"))
        .with_context("environment", "production")
        .with_battery(Sentry::new("https://username@password.ingest.sentry.io/project"))
        .with_battery(OpenTelemetry::new("https://api.honeycomb.io")
          .with_protocol(OpenTelemetryProtocol::HttpJson)
          .with_header("x-honeycomb-team", "your-access-token"));

    // Your app code goes here

    session.shutdown();
}
```
