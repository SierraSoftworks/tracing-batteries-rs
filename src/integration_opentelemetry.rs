use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
};

use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::{
    trace::{Sampler, SdkTracerProvider},
    Resource,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

use crate::{Battery, BatteryBuilder};
pub use opentelemetry_otlp::Protocol as OpenTelemetryProtocol;
pub use opentelemetry_sdk::trace::Sampler as OpenTelemetrySampler;
pub use tracing::Level as OpenTelemetryLevel;

const KEY_NOT_PARSED_PLACEHOLDER: &'static str = "x-key-not-parsed-correctly";

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
    ///  .with_battery(OpenTelemetry::new("localhost:4317"));
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

    fn build_opentelemetry_provider(
        &self,
        metadata: &crate::Metadata,
    ) -> Option<SdkTracerProvider> {
        if self.endpoint.is_empty() {
            return None;
        }

        let pipeline_builder = opentelemetry_sdk::trace::TracerProviderBuilder::default()
            .with_resource(self.build_resource(metadata))
            .with_sampler(self.sampler.clone());

        let pipeline_builder = match self.get_protocol() {
            OpenTelemetryProtocol::Grpc => pipeline_builder.with_batch_exporter(
                opentelemetry_otlp::SpanExporter::builder()
                    .with_tonic()
                    .with_endpoint(self.endpoint.clone())
                    .with_metadata({
                        let mut tracing_metadata = tonic::metadata::MetadataMap::new();
                        for (key, value) in self.headers.iter() {
                            if let (key, Ok(value)) = (
                                key.parse().unwrap_or_else(|_| {
                                    tonic::metadata::MetadataKey::from_static(
                                        KEY_NOT_PARSED_PLACEHOLDER,
                                    )
                                }),
                                value.to_string().parse(),
                            ) {
                                if key.as_str() != KEY_NOT_PARSED_PLACEHOLDER {
                                    tracing_metadata.insert(key, value);
                                }
                            }
                        }
                        tracing_metadata
                    })
                    .build()
                    .ok()?,
            ),
            proto @ (OpenTelemetryProtocol::HttpBinary | OpenTelemetryProtocol::HttpJson) => {
                pipeline_builder.with_batch_exporter(
                    opentelemetry_otlp::SpanExporter::builder()
                        .with_http()
                        .with_protocol(proto)
                        .with_endpoint(format!("{}/v1/traces", self.endpoint))
                        .with_headers({
                            let mut tracing_headers = HashMap::new();
                            for (key, value) in self.headers.iter() {
                                tracing_headers.insert(key.to_string(), value.to_string());
                            }
                            tracing_headers
                        })
                        .build()
                        .ok()?,
                )
            }
        };

        Some(pipeline_builder.build())
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
            .build()
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
                "traceidratio" => {
                    Sampler::TraceIdRatioBased(get_trace_ratio())
                }
                "parentbased_always_on" => Sampler::ParentBased(
                    Box::new(Sampler::AlwaysOn),
                ),
                "parentbased_always_off" => Sampler::ParentBased(
                    Box::new(Sampler::AlwaysOff),
                ),
                "parentbased_traceidratio" => {
                    Sampler::ParentBased(Box::new(
                        Sampler::TraceIdRatioBased(get_trace_ratio()),
                    ))
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
            .with(tracing_subscriber::filter::dynamic_filter_fn(
                move |_meta, _ctx| enabled.load(std::sync::atomic::Ordering::Relaxed),
            ));

        if let Some(provider) = self.build_opentelemetry_provider(metadata) {
            let layer = Box::new(tracing_opentelemetry::OpenTelemetryLayer::new(
                provider.tracer(metadata.service.clone()),
            ));
            opentelemetry::global::set_tracer_provider(provider.clone());

            match self.force_stdout {
                Some(true) => {
                    registry
                        .with(layer)
                        .with(
                            tracing_subscriber::filter::filter_fn(|meta| meta.is_event())
                                .and_then(tracing_subscriber::fmt::layer()),
                        )
                        .init();
                }
                _ => {
                    registry.with(layer).init();
                }
            }

            Box::new(OpenTelemetryBattery {
                provider: Some(provider),
            })
        } else if !matches!(self.force_stdout, Some(false)) {
            registry
                .with(
                    tracing_subscriber::filter::filter_fn(|meta| meta.is_event())
                        .and_then(tracing_subscriber::fmt::layer()),
                )
                .init();

            Box::new(OpenTelemetryBattery { provider: None })
        } else {
            Box::new(OpenTelemetryBattery { provider: None })
        }
    }
}

struct OpenTelemetryBattery {
    provider: Option<SdkTracerProvider>,
}

impl Battery for OpenTelemetryBattery {
    fn record_error(&self, error: &dyn std::error::Error) {
        opentelemetry::trace::get_active_span(|span| span.record_error(error))
    }

    fn shutdown(&mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
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
}
