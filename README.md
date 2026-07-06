# Tracing, batteries included
**Easily configure tracing integrations for your Rust applications**

This library has been built to simplify the process of configuring tracing
integrations for Rust applications, handling the complexity of maintaining
all the various `tracing`, `opentelemetry` and `sentry` API changes that
happen in the Rust ecosystem.

The goal here is that you should be able to write your telemetry integration
code once, and then forget about it while this library takes care of doing
the gymnastics required to keep everything working.

## Usage
The first step here is adding the `tracing-batteries-rs` crate to your
`Cargo.toml` file:

```toml
[dependencies]
tracing-batteries = { git = "https://github.com/sierrasoftworks/tracing-batteries-rs.git" }
```

**NOTE** I'm opting to use Git here because the goal of this library is to handle all the
continuous updates to the broader `tracing` and `opentelemetry` ecosystems, using Dependabot
to do so automatically. As such, tracking the `main` branch of this repository is the best way
(for my own use cases) to handle migrations across the various tools that depend upon this library.
Your own mileage may vary, and if you have strong feelings about this, please feel free to maintain
your own fork with a lower update cadence.

Then you'll want to add the tracing initialization to your application.

```rust
use tracing_batteries::{Session, Medama, Sentry, OpenTelemetry, OpenTelemetryProtocol};

fn main() {
    let session = Session::new("my-service", env!("CARGO_PKG_VERSION"))
        .with_context("environment", "production")
        .with_battery(Medama::new("https://medama.example.com"))
        .with_battery(Sentry::new("https://username@password.ingest.sentry.io/project"))
        .with_battery(OpenTelemetry::new("https://api.honeycomb.io")
          .with_protocol(OpenTelemetryProtocol::HttpJson)
          .with_header("x-honeycomb-team", "your-access-token"));

    // Your app code goes here

    session.shutdown();
}
```

## Integrations
This library ships with some integration "batteries" which you can easily
add to your `Session` to enable telemetry emission to various backends.

### Analytics
The `Analytics` integration allows you to send telemetry data to a self-hosted
[analytics](https://github.com/SierraSoftworks/analytics) privacy preserving analytics server.
This will track application execution as page views, custom events as events, and errors as
rich exception reports (including the error's type, cause chain, and backtrace).

Unhandled panics are also captured and reported as exceptions by default, including the panic's
message, location, and a backtrace. You can disable this behaviour by calling
`.with_panic_capture(false)` on the battery.

**NOTE** You will need to ensure that the `analytics` feature is enabled, it is **NOT** enabled by default.

```rust
use tracing_batteries::{Session, Analytics};
use tracing_batteries::prelude::*;

fn main() {
    let session = Session::new("my-service", env!("CARGO_PKG_VERSION"))
        .with_battery(Analytics::new("https://analytics.example.com"));

    // Your app code goes here

    session.shutdown();
}
```

### Medama
The `Medama` integration allows you to send telemetry data to a self-hosted [Medama](https://oss.medama.io)
privacy preserving analytics server. This will track application execution as page views, and
errors as events.

**NOTE** You will need to ensure that the `medama` feature is enabled, it is **NOT** enabled by default.

```rust
use tracing_batteries::{Session, Medama};
use tracing_batteries::prelude::*;

fn main() {
    let session = Session::new("my-service", env!("CARGO_PKG_VERSION"))
        .with_battery(Medama::new("https://medama.example.com"));
    
    // Your app code goes here

    session.shutdown();
}
```

### OpenTelemetry
The `OpenTelemetry` integration allows you to send telemetry data from the `tracing` crate
to an OpenTelemetry compatible backend.

**NOTE** You will need to ensure that the `opentelemetry` feature is enabled, it is enabled by default.

```rust
use tracing_batteries::{Session, OpenTelemetry, OpenTelemetryProtocol, OpenTelemetryLevel};
use tracing_batteries::prelude::*;

fn main() {
    let session = Session::new("my-service", env!("CARGO_PKG_VERSION"))
        .with_battery(OpenTelemetry::new("https://api.honeycomb.io")
          .with_header("x-honeycomb-team", "your-access-token")
          .with_default_level(OpenTelemetryLevel::WARN));

    // tracing_batteries::prelude::info_span is re-exported from tracing to allow you to use it in your code
    info_span!("my-span").in_scope(|| {
        info!("Hello, OpenTelemetry!");
    });

    session.shutdown();
}
```

### Sentry
The `Sentry` integration allows you to send session and error information to
Sentry from within your application.

**NOTE** You will need to ensure that the `sentry` feature is enabled, it is enabled by default.

```rust
use tracing_batteries::{Session, Sentry, SentryLevel};
use tracing_batteries::prelude::*;

fn main() {
    let session = Session::new("my-service", env!("CARGO_PKG_VERSION"))
        .with_battery(Sentry::new("https://user:pass@ingest.sentry.io/project")
          .with_default_level(SentryLevel::INFO));

    // tracing_batteries::prelude::sentry is re-exported from the sentry crate to allow you to use it in your code
    sentry::capture_message("Hello, Sentry!", sentry::Level::Info);

    session.shutdown();
}
```

### Umami
The `Umami` integration allows you to send telemetry data to a self-hosted [Umami](https://umami.is/)
privacy preserving analytics server. This will track application execution as page views, and
errors as events.

**NOTE** You will need to ensure that the `umami` feature is enabled, it is **NOT** enabled by default.

```rust
use tracing_batteries::{Session, Umami};
use tracing_batteries::prelude::*;

fn main() {
    let session = Session::new("my-service", env!("CARGO_PKG_VERSION"))
        .with_battery(Umami::new("https://umami.example.com", "your-website-id"));

    // Your app code goes here

    session.shutdown();
}
```