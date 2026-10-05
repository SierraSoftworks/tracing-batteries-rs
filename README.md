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

The `LOG_LEVEL` environment variable (`error`, `warn`, `info`, `debug` or `trace`), or
`.with_default_level(...)` when it is unset, controls how much is written to stdout. It does not
reduce what is exported over OTLP, which always includes `INFO` spans and log events, so
`LOG_LEVEL=warn` quietens your console without losing your traces. Setting it to `debug` or
`trace` makes the OTLP export more verbose as well.

The OpenTelemetry resource is populated from your `Session`'s metadata (its service name,
version, host information, and any `.with_context(...)` values). You can attach additional
custom resource attributes, or override the ones derived from the session metadata, by setting
the standard [`OTEL_RESOURCE_ATTRIBUTES`](https://opentelemetry.io/docs/specs/otel/resource/sdk/#specifying-resource-information-via-an-environment-variable)
environment variable to a comma separated list of `key=value` pairs:

```bash
OTEL_RESOURCE_ATTRIBUTES=service.namespace=team-a,deployment.environment=production
```

Attributes provided through the environment variable take precedence over those derived from the
session metadata, consistent with the other `OTEL_*` environment variables (such as
`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_EXPORTER_OTLP_PROTOCOL`, and
`OTEL_TRACES_SAMPLER`) that this integration understands.

#### Metrics and logs
Traces are always exported. Metrics and logs are opt-in, and share the endpoint, protocol,
headers, and resource configured for traces:

```rust
use tracing_batteries::{Session, OpenTelemetry};
use tracing_batteries::prelude::*;

fn main() {
    let session = Session::new("my-service", env!("CARGO_PKG_VERSION"))
        .with_battery(OpenTelemetry::new("https://api.honeycomb.io")
          .with_metrics()
          .with_logs());

    // Instruments must be created after the session, through the global meter.
    let requests = opentelemetry::global::meter("my-service")
        .u64_counter("requests_total")
        .build();

    info_span!("request").in_scope(|| {
        requests.add(1, &[opentelemetry::KeyValue::new("status", "ok")]);
        // Exported as an OTLP log record carrying the enclosing span's trace and span IDs.
        warn!(attempt = 2, "Retrying the request");
    });

    session.shutdown();
}
```

- `with_metrics()` installs a global `MeterProvider` exporting cumulative metrics on the
  `OTEL_METRIC_EXPORT_INTERVAL` cadence (default 60s). Measurements are recorded against the
  active span context, so exemplars will attach automatically once the Rust SDK emits them.
- `with_logs()` exports `tracing` events as OTLP log records (at `INFO` and above, or more
  verbose when `LOG_LEVEL` asks for it), each carrying the trace and span IDs of the span it was emitted within.
  Event fields become log attributes, and an error-typed field becomes `exception.message`.
- Both signals honour the session's `enabled` flag, so they are suppressed in debug builds
  unless `.with_debug_builds()` is used.

When this integration writes to stdout, it colourizes the output with ANSI escape codes only if
stdout is a terminal and the [`NO_COLOR`](https://no-color.org) environment variable is unset
(setting it to any value disables colour), so captured logs — `docker logs`, a systemd journal, or
a CI run — stay free of escape codes. Call `.with_ansi(true)` or `.with_ansi(false)` on the battery
to make that choice explicitly instead.

### Profiling
The `Profiling` integration continuously profiles your application and exports the profiles to an
OpenTelemetry collector using the OTLP profiles signal. It connects to the collector in the same
way as the `OpenTelemetry` integration and honours the same `OTEL_EXPORTER_OTLP_ENDPOINT`,
`OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_EXPORTER_OTLP_PROTOCOL` and `OTEL_RESOURCE_ATTRIBUTES`
environment variables, so profiles are sent to the same collector and described by the same
resource as your traces.

**NOTE** You will need to ensure that the `profiling` feature is enabled, along with the feature
for each backend you want to use. These are **NOT** enabled by default.

| Backend             | Feature              | Profile                                                             |
| ------------------- | -------------------- | ------------------------------------------------------------------- |
| `ProfilingPprof`    | `profiling-pprof`    | CPU (Linux and macOS)                                               |
| `ProfilingJemalloc` | `profiling-jemalloc` | Memory (requires jemalloc as your allocator, with profiling active) |

```rust
use tracing_batteries::{Session, OpenTelemetry, Profiling, ProfilingPprof};

fn main() {
    let session = Session::new("my-service", env!("CARGO_PKG_VERSION"))
        .with_battery(OpenTelemetry::new("https://otlp.example.com"))
        .with_battery(Profiling::new("https://otlp.example.com")
          .with_backend(ProfilingPprof::new().with_sample_rate(100)));

    // Your app code goes here

    session.shutdown();
}
```

- The OTLP profiles signal is still in development, so your collector needs profiles support
  enabled. For the OpenTelemetry Collector, that is the `service.profilesSupport` feature gate
  and a `profiles` pipeline which includes the `otlp` receiver.
- Profiling is not available on Windows, where the integration and its backends do nothing, so
  it can be configured unconditionally by applications built for several platforms.
- Profiling only starts if the session is enabled when the battery is attached (so debug builds
  need `.with_debug_builds()`), and each export is skipped if the session is disabled when it
  is due.
- Any backend built for the [`pyroscope`](https://github.com/grafana/pyroscope-rs) crate can be
  attached by implementing the `ProfilingBackend` trait.

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