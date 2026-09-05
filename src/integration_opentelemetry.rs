use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    logs::SdkLoggerProvider,
    metrics::{
        PeriodicReader, SdkMeterProvider, Temporality, data::ResourceMetrics,
        exporter::PushMetricExporter,
    },
    trace::{Sampler, SdkTracerProvider},
};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{Battery, BatteryBuilder};
pub use opentelemetry_otlp::Protocol as OpenTelemetryProtocol;
pub use opentelemetry_sdk::trace::Sampler as OpenTelemetrySampler;
pub use tracing::Level as OpenTelemetryLevel;

/// An [OpenTelemetry](opentelemetry) integration which leverages the [`tracing`] ecosystem
/// to emit span information to an OpenTelemetry collector.
///
/// <div class="warning">
///
/// This integration requires the `opentelemetry` feature to be enabled.
///
/// </div>
///
/// The OpenTelemetry integration is initialized by providing an endpoint for the OpenTelemetry
/// collector. The endpoint may either be a gRPC or HTTP endpoint, and additional headers may
/// be used to configure the connection (these are often used for authentication).
///
/// ## Resource attributes
///
/// The telemetry resource is primarily populated from the session's [`Metadata`](crate::Metadata)
/// (its service name, version, host information, and any context added through
/// [`Metadata::with_context`](crate::Metadata::with_context)). In addition to this, you can attach
/// custom resource attributes (or override the ones derived from the session metadata) by setting
/// the standard `OTEL_RESOURCE_ATTRIBUTES` environment variable to a comma separated list of
/// `key=value` pairs, for example:
///
/// ```bash
/// OTEL_RESOURCE_ATTRIBUTES=service.namespace=team-a,deployment.environment=production
/// ```
///
/// Attributes provided through the environment variable take precedence over those derived from
/// the session metadata, matching the behaviour of the other `OTEL_*` environment variables
/// supported by this integration.
///
/// ## Signals
///
/// Traces are always exported. Metrics and logs are opt-in and share the collector endpoint,
/// protocol, headers, and resource with the traces:
///
/// - [`with_metrics`](OpenTelemetry::with_metrics) installs a global
///   [`MeterProvider`](opentelemetry::metrics::MeterProvider), so instruments created through
///   [`opentelemetry::global::meter`] are exported over OTLP.
/// - [`with_logs`](OpenTelemetry::with_logs) exports [`tracing`] events as OTLP log records,
///   carrying the trace and span IDs of the span they were emitted within.
///
/// ## Example (gRPC)
/// ```no_run
/// use tracing_batteries::{Session, OpenTelemetry, OpenTelemetryProtocol};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///   .with_battery(OpenTelemetry::new("localhost:4317")
///     .with_protocol(OpenTelemetryProtocol::Grpc)
///     .with_header("x-api-key", "my-api-key"));
///
/// session.shutdown();
/// ```
///
/// ## Example (HTTP)
/// ```no_run
/// use tracing_batteries::{Session, OpenTelemetry, OpenTelemetryProtocol};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///   .with_battery(OpenTelemetry::new("http://localhost:4318")
///     .with_protocol(OpenTelemetryProtocol::HttpBinary)
///     .with_header("x-api-key", "my-api-key"));
///
/// session.shutdown();
/// ```
///
pub struct OpenTelemetry {
    endpoint: Cow<'static, str>,
    headers: HashMap<Cow<'static, str>, Cow<'static, str>>,
    protocol: Option<OpenTelemetryProtocol>,
    sampler: OpenTelemetrySampler,
    use_log_events: bool,
    use_metrics: bool,
    default_level: Option<OpenTelemetryLevel>,
    force_stdout: Option<bool>,
}

impl OpenTelemetry {
    /// Configures the OpenTelemetry integration for the provided collector endpoint.
    ///
    /// This method is used to configure the endpoint for the OpenTelemetry collector,
    /// the endpoint should correspond to the configured [`OpenTelemetryProtocol`] in use
    /// (e.g. `http://localhost:4318` for HTTP, or `localhost:4317` for gRPC).
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, OpenTelemetry};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///     .with_battery(OpenTelemetry::new("localhost:4317"));
    ///
    /// session.shutdown();
    /// ```
    pub fn new<S: Into<Cow<'static, str>>>(endpoint: S) -> Self {
        Self {
            endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .map(Cow::Owned)
                .unwrap_or_else(|_| endpoint.into()),
            headers: {
                let mut headers = HashMap::new();

                let env_headers = std::env::var("OTEL_EXPORTER_OTLP_HEADERS").unwrap_or_default();
                for header in env_headers.split(',') {
                    if let Some((key, value)) = header.split_once('=') {
                        headers.insert(key.to_owned().into(), value.to_owned().into());
                    }
                }

                headers
            },
            protocol: None,
            sampler: Self::build_sampler(),
            default_level: None,
            use_log_events: false,
            use_metrics: false,
            force_stdout: None,
        }
    }

    /// Adds a header to the OpenTelemetry collector connection.
    ///
    /// This method is used to add a header to the connection to the OpenTelemetry collector,
    /// it is commonly used for authenticating with cloud based collector offerings.
    ///
    /// <div class="warning">
    ///
    /// This method will ignore any headers whose keys already exist in the connection,
    /// including keys which are provided through the `OTEL_EXPORTER_OTLP_HEADERS` environment variable.
    /// You can specify headers through the environment variable by providing a comma separated list of
    /// key-value pairs (e.g. `key1=value1,key2=value2`).
    ///
    /// </div>
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::OpenTelemetry;
    ///
    /// OpenTelemetry::new("localhost:4317")
    ///   .with_header("x-api-key", "my-api-key");
    /// ```
    pub fn with_header<K: Into<Cow<'static, str>>, V: Into<Cow<'static, str>>>(
        mut self,
        key: K,
        value: V,
    ) -> Self {
        self.headers.entry(key.into()).or_insert(value.into());
        self
    }

    /// Configures the OpenTelemetry integration to use the provided protocol.
    ///
    /// This method is used to configure the protocol used to communicate with the OpenTelemetry collector,
    /// the protocol should correspond to the configured endpoint's supported protocol type. Some endpoints
    /// support multiple protocols, such as Honeycomb's HTTPS endpoint which can be used either for gRPC or
    /// HTTP/JSON.
    ///
    /// You can also configure the protocol using the `OTEL_EXPORTER_OTLP_PROTOCOL` environment variable,
    /// which can be set to `http-binary`, `http-json`, or `grpc`. If the environment variable is not set,
    /// the default protocol will be `grpc`.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::{OpenTelemetry, OpenTelemetryProtocol};
    ///
    /// OpenTelemetry::new("localhost:4317")
    ///  .with_protocol(OpenTelemetryProtocol::Grpc);
    /// ```
    pub fn with_protocol(mut self, protocol: OpenTelemetryProtocol) -> Self {
        self.protocol = Some(protocol);
        self
    }

    /// Configures the OpenTelemetry integration to use the provided sampler.
    ///
    /// This method is used to configure the sampler used by the OpenTelemetry integration,
    /// the sampler is used to determine which spans should be recorded and exported.
    ///
    /// The sampler can also be configured using the `OTEL_TRACES_SAMPLER` environment variable,
    /// which can be set to `always_on`, `always_off`, or `traceidratio` for basic sampling decisions.
    /// You can also use the `parentbased_always_on`, `parentbased_always_off`, or `parentbased_traceidratio`
    /// samplers to sample based on the parent span's sampling decision. If any other value is provided,
    /// the `always_on` sampler will be used.
    ///
    /// To configure the sampling ratio when using the `traceidratio` or `parentbased_traceidratio` samplers,
    /// you can set the `OTEL_TRACES_SAMPLER_ARG` environment variable to a floating point number between 0 and 1.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::{OpenTelemetry, OpenTelemetrySampler};
    ///
    /// OpenTelemetry::new("localhost:4317")
    ///  .with_sampler(OpenTelemetrySampler::AlwaysOn);
    /// ```
    pub fn with_sampler(mut self, sampler: OpenTelemetrySampler) -> Self {
        self.sampler = sampler;
        self
    }

    /// Configures the OpenTelemetry integration to use the provided log level.
    ///
    /// This method is used to configure the log level used by the OpenTelemetry integration,
    /// the log level is used to determine which spans should be recorded and exported.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::{OpenTelemetry, OpenTelemetryLevel};
    ///
    /// OpenTelemetry::new("localhost:4317")
    ///   .with_default_level(OpenTelemetryLevel::DEBUG);
    /// ```
    pub fn with_default_level(mut self, level: OpenTelemetryLevel) -> Self {
        self.default_level = Some(level);
        self
    }

    /// Configures the OpenTelemetry integration to force stdout logging behaviour.
    ///
    /// By default, the OpenTelemetry integration will log to stdout if an empty endpoint is provided.
    /// This method can be used to force the integration to log to stdout even if an endpoint is provided,
    /// or to disable stdout logging if an empty endpoint is provided.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::OpenTelemetry;
    ///
    /// OpenTelemetry::new("localhost:4317")
    ///  .with_stdout(true);
    /// ```
    pub fn with_stdout(self, stdout: bool) -> Self {
        Self {
            force_stdout: Some(stdout),
            ..self
        }
    }

    /// Configures the OpenTelemetry integration to export [`tracing`] events as OpenTelemetry
    /// log records.
    ///
    /// By default, [`tracing`] events are only attached to their enclosing span as span events.
    /// Enabling logs additionally exports each event (subject to the configured level) as an OTLP
    /// log record through the collector's `/v1/logs` endpoint. Records emitted within a span carry
    /// that span's trace and span IDs, so a log line can be joined back to the trace it belongs to
    /// without scanning span events, which makes failure data cheap to aggregate over long windows.
    ///
    /// Event fields become log attributes: `field = %value` records the value's `Display` form,
    /// `field = ?value` its `Debug` form, and a field holding a `&dyn std::error::Error` is recorded
    /// as an `exception.message` attribute.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::OpenTelemetry;
    ///
    /// OpenTelemetry::new("localhost:4317")
    ///  .with_logs();
    /// ```
    pub fn with_logs(mut self) -> Self {
        self.use_log_events = true;
        self
    }

    /// An alias for [`OpenTelemetry::with_logs`], retained for backwards compatibility.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::OpenTelemetry;
    ///
    /// OpenTelemetry::new("localhost:4317")
    ///  .with_log_events();
    /// ```
    pub fn with_log_events(self) -> Self {
        self.with_logs()
    }

    /// Configures the OpenTelemetry integration to export metrics.
    ///
    /// When enabled, a global [`MeterProvider`](opentelemetry::metrics::MeterProvider) is installed
    /// which periodically exports every instrument created through
    /// [`opentelemetry::global::meter`] to the collector's `/v1/metrics` endpoint, using the same
    /// protocol, headers, and resource as the traces. The export cadence can be tuned with the
    /// standard `OTEL_METRIC_EXPORT_INTERVAL` and `OTEL_METRIC_EXPORT_TIMEOUT` environment
    /// variables (in milliseconds; the interval defaults to 60s).
    ///
    /// Metrics are exported with cumulative temporality (the Prometheus-compatible default).
    /// Exports are suppressed while the session's `enabled` flag is `false` (for example in
    /// debug builds), matching the behaviour of the trace and log signals.
    ///
    /// <div class="warning">
    ///
    /// Instruments are bound to the provider which was global at the time they were created, so
    /// create them (or the [`Meter`](opentelemetry::metrics::Meter) they come from) only after the
    /// session has been constructed.
    ///
    /// </div>
    ///
    /// ## Exemplars
    ///
    /// Measurements are recorded against the active OpenTelemetry context, which the tracing
    /// layer activates whenever a [`tracing`] span is entered. The Rust SDK does not yet populate
    /// exemplars on exported data points; once it does, measurements taken within a span will
    /// automatically carry that span's trace and span IDs as exemplars with no further changes.
    ///
    /// ## Example
    /// ```rust
    /// use tracing_batteries::OpenTelemetry;
    /// use tracing_batteries::prelude::opentelemetry;
    ///
    /// let battery = OpenTelemetry::new("localhost:4317")
    ///   .with_metrics();
    ///
    /// // Once the session has been constructed, instruments can be created through the global meter:
    /// let requests = opentelemetry::global::meter("my-service")
    ///   .u64_counter("requests_total")
    ///   .build();
    /// requests.add(1, &[opentelemetry::KeyValue::new("status", "ok")]);
    /// ```
    pub fn with_metrics(mut self) -> Self {
        self.use_metrics = true;
        self
    }

    fn build_opentelemetry_providers(
        &self,
        metadata: &crate::Metadata,
        enabled: Arc<AtomicBool>,
    ) -> Option<OpenTelemetryProviders> {
        if self.endpoint.is_empty() {
            return None;
        }

        let protocol = self.get_protocol();
        let resource = self.build_resource(metadata);

        let span_exporter = match protocol {
            OpenTelemetryProtocol::Grpc => opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_tls_config(self.tonic_tls_config())
                .with_endpoint(self.endpoint.clone())
                .with_metadata(self.tonic_metadata())
                .build()
                .ok()?,
            proto @ (OpenTelemetryProtocol::HttpBinary | OpenTelemetryProtocol::HttpJson) => {
                opentelemetry_otlp::SpanExporter::builder()
                    .with_http()
                    .with_protocol(proto)
                    .with_endpoint(format!("{}/v1/traces", self.endpoint))
                    .with_headers(self.http_headers())
                    .build()
                    .ok()?
            }
        };

        let tracer_provider = opentelemetry_sdk::trace::TracerProviderBuilder::default()
            .with_resource(resource.clone())
            .with_sampler(self.sampler.clone())
            .with_batch_exporter(span_exporter)
            .build();

        let log_exporter = match protocol {
            OpenTelemetryProtocol::Grpc => opentelemetry_otlp::LogExporter::builder()
                .with_tonic()
                .with_tls_config(self.tonic_tls_config())
                .with_endpoint(self.endpoint.clone())
                .with_metadata(self.tonic_metadata())
                .build()
                .ok()?,
            proto @ (OpenTelemetryProtocol::HttpBinary | OpenTelemetryProtocol::HttpJson) => {
                opentelemetry_otlp::LogExporter::builder()
                    .with_http()
                    .with_protocol(proto)
                    .with_endpoint(format!("{}/v1/logs", self.endpoint))
                    .with_headers(self.http_headers())
                    .build()
                    .ok()?
            }
        };

        let logger_provider = opentelemetry_sdk::logs::LoggerProviderBuilder::default()
            .with_resource(resource.clone())
            .with_batch_exporter(log_exporter)
            .build();

        let meter_provider = if self.use_metrics {
            let metric_exporter = match protocol {
                OpenTelemetryProtocol::Grpc => opentelemetry_otlp::MetricExporter::builder()
                    .with_tonic()
                    .with_tls_config(self.tonic_tls_config())
                    .with_endpoint(self.endpoint.clone())
                    .with_metadata(self.tonic_metadata())
                    .with_temporality(Temporality::Cumulative)
                    .build()
                    .ok()?,
                proto @ (OpenTelemetryProtocol::HttpBinary | OpenTelemetryProtocol::HttpJson) => {
                    opentelemetry_otlp::MetricExporter::builder()
                        .with_http()
                        .with_protocol(proto)
                        .with_endpoint(format!("{}/v1/metrics", self.endpoint))
                        .with_headers(self.http_headers())
                        .with_temporality(Temporality::Cumulative)
                        .build()
                        .ok()?
                }
            };

            Some(
                SdkMeterProvider::builder()
                    .with_resource(resource)
                    .with_reader(
                        PeriodicReader::builder(GatedMetricExporter {
                            inner: metric_exporter,
                            enabled,
                        })
                        .build(),
                    )
                    .build(),
            )
        } else {
            None
        };

        Some(OpenTelemetryProviders {
            tracer_provider,
            logger_provider,
            meter_provider,
        })
    }

    fn tonic_tls_config(&self) -> tonic::transport::ClientTlsConfig {
        tonic::transport::ClientTlsConfig::new()
            .with_native_roots()
            .with_webpki_roots()
    }

    /// The configured headers as gRPC request metadata, skipping any header whose key or value
    /// cannot be represented as gRPC metadata.
    fn tonic_metadata(&self) -> tonic::metadata::MetadataMap {
        let mut tracing_metadata = tonic::metadata::MetadataMap::new();
        for (key, value) in self.headers.iter() {
            if let (Ok(key), Ok(value)) = (
                key.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>(),
                value.to_string().parse(),
            ) {
                tracing_metadata.insert(key, value);
            }
        }
        tracing_metadata
    }

    fn http_headers(&self) -> HashMap<String, String> {
        self.headers
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn get_protocol(&self) -> OpenTelemetryProtocol {
        match std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").ok().as_deref() {
            Some("http-binary") => opentelemetry_otlp::Protocol::HttpBinary,
            Some("http-json") => opentelemetry_otlp::Protocol::HttpJson,
            Some("grpc") => opentelemetry_otlp::Protocol::Grpc,
            _ => self.protocol.unwrap_or(OpenTelemetryProtocol::Grpc),
        }
    }

    fn build_resource(&self, metadata: &crate::Metadata) -> Resource {
        self.build_resource_with_env(metadata, Self::env_resource_attributes())
    }

    fn build_resource_with_env(
        &self,
        metadata: &crate::Metadata,
        env_attributes: Vec<opentelemetry::KeyValue>,
    ) -> Resource {
        let mut resource_metadata = vec![
            opentelemetry::KeyValue::new("service.version", metadata.version.clone()),
            opentelemetry::KeyValue::new("host.os", std::env::consts::OS),
            opentelemetry::KeyValue::new("host.architecture", std::env::consts::ARCH),
        ];

        for (key, value) in metadata.context.iter() {
            resource_metadata.push(opentelemetry::KeyValue::new(*key, value.clone()));
        }

        Resource::builder()
            .with_service_name(metadata.service.clone())
            .with_attributes(resource_metadata)
            // Resource attributes provided through the `OTEL_RESOURCE_ATTRIBUTES` environment
            // variable are applied last so that they can add to (or override) the session-provided
            // metadata at deploy time. This mirrors the precedence of the other `OTEL_*` environment
            // variables handled by this integration (endpoint, headers, protocol, and sampler).
            .with_attributes(env_attributes)
            .build()
    }

    /// Reads custom resource attributes from the `OTEL_RESOURCE_ATTRIBUTES` environment variable.
    ///
    /// The variable is parsed according to the OpenTelemetry specification as a comma separated list
    /// of `key=value` pairs (e.g. `service.namespace=team-a,deployment.environment=production`).
    fn env_resource_attributes() -> Vec<opentelemetry::KeyValue> {
        Self::parse_resource_attributes(
            std::env::var("OTEL_RESOURCE_ATTRIBUTES")
                .unwrap_or_default()
                .as_str(),
        )
    }

    /// Parses a comma separated list of `key=value` resource attributes.
    ///
    /// Whitespace surrounding keys and values is ignored, and malformed entries (those without an
    /// `=` separator, or with an empty key) are skipped rather than producing invalid attributes.
    fn parse_resource_attributes(raw: &str) -> Vec<opentelemetry::KeyValue> {
        raw.split_terminator(',')
            .filter_map(|entry| {
                let (key, value) = entry.split_once('=')?;
                let key = key.trim();
                if key.is_empty() {
                    return None;
                }

                Some(opentelemetry::KeyValue::new(
                    key.to_owned(),
                    value.trim().to_owned(),
                ))
            })
            .collect()
    }

    fn build_sampler() -> Sampler {
        fn get_trace_ratio() -> f64 {
            std::env::var("OTEL_TRACES_SAMPLER_ARG")
                .ok()
                .and_then(|ratio| ratio.parse().ok())
                .unwrap_or(1.0)
        }

        std::env::var("OTEL_TRACES_SAMPLER")
            .map(|s| match s.as_str() {
                "always_on" => Sampler::AlwaysOn,
                "always_off" => Sampler::AlwaysOff,
                "traceidratio" => Sampler::TraceIdRatioBased(get_trace_ratio()),
                "parentbased_always_on" => Sampler::ParentBased(Box::new(Sampler::AlwaysOn)),
                "parentbased_always_off" => Sampler::ParentBased(Box::new(Sampler::AlwaysOff)),
                "parentbased_traceidratio" => {
                    Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(get_trace_ratio())))
                }
                _ => Sampler::AlwaysOn,
            })
            .unwrap_or(Sampler::AlwaysOn)
    }

    fn build_level(&self) -> OpenTelemetryLevel {
        match std::env::var("LOG_LEVEL")
            .map(|s| s.to_lowercase())
            .as_deref()
        {
            Ok("error") => OpenTelemetryLevel::ERROR,
            Ok("warn") => OpenTelemetryLevel::WARN,
            Ok("info") => OpenTelemetryLevel::INFO,
            Ok("debug") => OpenTelemetryLevel::DEBUG,
            Ok("trace") => OpenTelemetryLevel::TRACE,
            _ => self.default_level.unwrap_or(OpenTelemetryLevel::INFO),
        }
    }

    fn build_stdout_layer<
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    >(
        &self,
    ) -> impl Layer<S> {
        tracing_subscriber::fmt::layer()
    }
}

impl BatteryBuilder for OpenTelemetry {
    fn setup(self, metadata: &crate::Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );

        let registry = tracing_subscriber::registry()
            .with(match self.build_level() {
                OpenTelemetryLevel::ERROR => tracing_subscriber::filter::LevelFilter::ERROR,
                OpenTelemetryLevel::WARN => tracing_subscriber::filter::LevelFilter::WARN,
                OpenTelemetryLevel::INFO => tracing_subscriber::filter::LevelFilter::INFO,
                OpenTelemetryLevel::DEBUG => tracing_subscriber::filter::LevelFilter::DEBUG,
                OpenTelemetryLevel::TRACE => tracing_subscriber::filter::LevelFilter::TRACE,
            })
            .with(tracing_subscriber::filter::dynamic_filter_fn({
                let enabled = enabled.clone();
                move |_meta, _ctx| enabled.load(std::sync::atomic::Ordering::Relaxed)
            }));

        if let Some(providers) = self.build_opentelemetry_providers(metadata, enabled) {
            opentelemetry::global::set_tracer_provider(providers.tracer_provider.clone());

            if let Some(meter_provider) = providers.meter_provider.as_ref() {
                opentelemetry::global::set_meter_provider(meter_provider.clone());
            }

            let tracer_layer = tracing_opentelemetry::OpenTelemetryLayer::new(
                providers.tracer_provider.tracer(metadata.service.clone()),
            );

            let registry = registry.with(tracer_layer);

            // The log bridge and the stdout writer are independent sinks: either, both, or neither
            // may be installed. Each is boxed so the registry has a single concrete type regardless
            // of the combination selected.
            let logging_layer = self.use_log_events.then(|| {
                Box::new(
                    opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                        &providers.logger_provider,
                    ),
                ) as Box<dyn Layer<_> + Send + Sync>
            });

            let stdout_layer = matches!(self.force_stdout, Some(true))
                .then(|| Box::new(self.build_stdout_layer()) as Box<dyn Layer<_> + Send + Sync>);

            registry.with(logging_layer).with(stdout_layer).init();

            Box::new(OpenTelemetryBattery {
                tracer_provider: Some(providers.tracer_provider),
                logger_provider: Some(providers.logger_provider),
                meter_provider: providers.meter_provider,
            })
        } else if !matches!(self.force_stdout, Some(false)) {
            registry.with(self.build_stdout_layer()).init();

            Box::new(OpenTelemetryBattery::default())
        } else {
            Box::new(OpenTelemetryBattery::default())
        }
    }
}

/// The SDK providers backing each exported signal.
struct OpenTelemetryProviders {
    tracer_provider: SdkTracerProvider,
    logger_provider: SdkLoggerProvider,
    meter_provider: Option<SdkMeterProvider>,
}

/// A metric exporter which honours the session's `enabled` flag by dropping export batches
/// while telemetry is disabled, mirroring the dynamic filter applied to the tracing layers.
struct GatedMetricExporter {
    inner: opentelemetry_otlp::MetricExporter,
    enabled: Arc<AtomicBool>,
}

impl PushMetricExporter for GatedMetricExporter {
    fn export(
        &self,
        metrics: &ResourceMetrics,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        let enabled = self.enabled.load(std::sync::atomic::Ordering::Relaxed);
        async move {
            if enabled {
                self.inner.export(metrics).await
            } else {
                Ok(())
            }
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn temporality(&self) -> Temporality {
        self.inner.temporality()
    }
}

#[derive(Default)]
struct OpenTelemetryBattery {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<SdkLoggerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl Battery for OpenTelemetryBattery {
    fn record_event(&self, name: &str, properties: &HashMap<String, String>) {
        tracing::event!(tracing::Level::INFO, name = %name, properties = ?properties);
    }

    fn record_error(&self, error: &crate::ErrorInfo) {
        opentelemetry::trace::get_active_span(|span| span.record_error(error.error))
    }

    fn shutdown(&mut self) {
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown_with_timeout(Duration::from_secs(2));
        }
        if let Some(provider) = self.logger_provider.take() {
            let _ = provider.shutdown_with_timeout(Duration::from_secs(2));
        }
        if let Some(provider) = self.meter_provider.take() {
            let _ = provider.shutdown_with_timeout(Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod test {
    use crate::*;

    #[tokio::test]
    async fn otel_setup() {
        let session = Session::new("example", "0.0.1").with_battery(
            OpenTelemetry::new("localhost:4317").with_header("test-header", "test-value"),
        );

        session.shutdown();
    }

    #[test]
    fn parses_resource_attributes() {
        let attributes = OpenTelemetry::parse_resource_attributes(
            "service.namespace=team-a, deployment.environment = production ,empty=,=novalue,malformed",
        );

        let parsed: std::collections::HashMap<String, String> = attributes
            .into_iter()
            .map(|kv| (kv.key.as_str().to_owned(), kv.value.to_string()))
            .collect();

        // Well formed pairs are parsed, with surrounding whitespace trimmed from keys and values.
        assert_eq!(parsed.get("service.namespace"), Some(&"team-a".to_owned()));
        assert_eq!(
            parsed.get("deployment.environment"),
            Some(&"production".to_owned())
        );
        // An explicit empty value is preserved.
        assert_eq!(parsed.get("empty"), Some(&"".to_owned()));
        // Entries with an empty key, or without an `=` separator, are ignored.
        assert_eq!(parsed.get(""), None);
        assert!(!parsed.contains_key("malformed"));
    }

    #[test]
    fn resource_merges_env_attributes() {
        let metadata = Session::new("example", "0.0.1").with_context("environment", "production");

        let resource = OpenTelemetry::new("localhost:4317").build_resource_with_env(
            &metadata,
            vec![
                opentelemetry::KeyValue::new("deployment.environment", "staging"),
                opentelemetry::KeyValue::new("service.version", "9.9.9"),
            ],
        );

        let get = |key: &'static str| resource.get(&opentelemetry::Key::from_static_str(key));

        // Session-provided metadata is present on the resource.
        assert_eq!(
            get("service.name"),
            Some(opentelemetry::Value::from("example"))
        );
        assert_eq!(
            get("environment"),
            Some(opentelemetry::Value::from("production"))
        );

        // Custom attributes provided through the environment are added to the resource.
        assert_eq!(
            get("deployment.environment"),
            Some(opentelemetry::Value::from("staging"))
        );

        // Environment attributes take precedence over the session metadata on conflicts.
        assert_eq!(
            get("service.version"),
            Some(opentelemetry::Value::from("9.9.9"))
        );
    }
}
