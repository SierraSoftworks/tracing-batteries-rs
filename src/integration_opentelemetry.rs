use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
};

use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    trace::{Config, Sampler},
    Resource,
};
use tracing::Subscriber;
use tracing_subscriber::{
    layer::SubscriberExt, registry::LookupSpan, util::SubscriberInitExt, Layer,
};

use crate::{Battery, BatteryBuilder};
pub use opentelemetry_otlp::Protocol as OpenTelemetryProtocol;
pub use opentelemetry_sdk::trace::Sampler as OpenTelemetrySampler;

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

    fn build_opentelemetry_layer<S>(
        &self,
        metadata: &crate::Metadata,
    ) -> Option<Box<dyn Layer<S> + Send + Sync + 'static>>
    where
        S: Subscriber + Send + Sync,
        for<'a> S: LookupSpan<'a>,
    {
        if self.endpoint.is_empty() {
            return None;
        }

        let pipeline_builder = match self.get_protocol() {
            OpenTelemetryProtocol::Grpc => {
                opentelemetry_otlp::new_pipeline().tracing().with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .tonic()
                        .with_endpoint(self.endpoint.clone())
                        .with_metadata({
                            let mut tracing_metadata = tonic::metadata::MetadataMap::new();
                            for (key, value) in self.headers.iter() {
                                tracing_metadata.insert(
                                    key.parse()
                                        .unwrap_or(tonic::metadata::MetadataKey::from_static("")),
                                    value
                                        .to_string()
                                        .parse()
                                        .unwrap_or(tonic::metadata::MetadataValue::from_static("")),
                                );
                            }
                            tracing_metadata
                        }),
                )
            }
            proto @ (OpenTelemetryProtocol::HttpBinary | OpenTelemetryProtocol::HttpJson) => {
                opentelemetry_otlp::new_pipeline().tracing().with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .http()
                        .with_protocol(proto)
                        .with_endpoint(format!("{}/v1/traces", self.endpoint))
                        .with_headers({
                            let mut tracing_headers = HashMap::new();
                            for (key, value) in self.headers.iter() {
                                tracing_headers.insert(key.to_string(), value.to_string());
                            }
                            tracing_headers
                        })
                        .with_http_client(reqwest::Client::new()),
                )
            }
        };

        if let Some(provider) = pipeline_builder
            .with_trace_config(self.build_trace_config(metadata))
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .ok()
        {
            opentelemetry::global::set_tracer_provider(provider.clone());

            Some(Box::new(tracing_opentelemetry::OpenTelemetryLayer::new(
                provider.tracer(metadata.service.clone()),
            )))
        } else {
            None
        }
    }

    fn get_protocol(&self) -> OpenTelemetryProtocol {
        match std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").ok().as_deref() {
            Some("http-binary") => opentelemetry_otlp::Protocol::HttpBinary,
            Some("http-json") => opentelemetry_otlp::Protocol::HttpJson,
            Some("grpc") => opentelemetry_otlp::Protocol::Grpc,
            _ => self.protocol.unwrap_or(OpenTelemetryProtocol::Grpc),
        }
    }

    fn build_trace_config(&self, metadata: &crate::Metadata) -> Config {
        opentelemetry_sdk::trace::Config::default()
            .with_resource(self.build_resource(metadata))
            .with_sampler(self.sampler.clone())
    }

    fn build_resource(&self, metadata: &crate::Metadata) -> Resource {
        let mut resource_metadata = vec![
            opentelemetry::KeyValue::new("service.name", metadata.service.clone()),
            opentelemetry::KeyValue::new("service.version", metadata.version.clone()),
            opentelemetry::KeyValue::new("host.os", std::env::consts::OS),
            opentelemetry::KeyValue::new("host.architecture", std::env::consts::ARCH),
        ];

        for (key, value) in metadata.context.iter() {
            resource_metadata.push(opentelemetry::KeyValue::new(*key, value.clone()));
        }

        Resource::new(resource_metadata)
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
                "always_on" => opentelemetry_sdk::trace::Sampler::AlwaysOn,
                "always_off" => opentelemetry_sdk::trace::Sampler::AlwaysOff,
                "traceidratio" => {
                    opentelemetry_sdk::trace::Sampler::TraceIdRatioBased(get_trace_ratio())
                }
                "parentbased_always_on" => opentelemetry_sdk::trace::Sampler::ParentBased(
                    Box::new(opentelemetry_sdk::trace::Sampler::AlwaysOn),
                ),
                "parentbased_always_off" => opentelemetry_sdk::trace::Sampler::ParentBased(
                    Box::new(opentelemetry_sdk::trace::Sampler::AlwaysOff),
                ),
                "parentbased_traceidratio" => {
                    opentelemetry_sdk::trace::Sampler::ParentBased(Box::new(
                        opentelemetry_sdk::trace::Sampler::TraceIdRatioBased(get_trace_ratio()),
                    ))
                }
                _ => opentelemetry_sdk::trace::Sampler::AlwaysOn,
            })
            .unwrap_or(opentelemetry_sdk::trace::Sampler::AlwaysOn)
    }
}

impl BatteryBuilder for OpenTelemetry {
    fn setup(self, metadata: &crate::Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );

        let registry = tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::DEBUG)
            .with(tracing_subscriber::filter::dynamic_filter_fn(
                move |_meta, _ctx| enabled.load(std::sync::atomic::Ordering::Relaxed),
            ));

        if let Some(provider) = self.build_opentelemetry_layer(metadata) {
            registry.with(provider).init();
        } else {
            registry
                .with(
                    tracing_subscriber::filter::filter_fn(|meta| meta.is_event())
                        .and_then(tracing_subscriber::fmt::layer()),
                )
                .init();
        }

        Box::new(OpenTelemetryBattery {})
    }
}

struct OpenTelemetryBattery {}

impl Battery for OpenTelemetryBattery {
    fn shutdown(&self) {
        opentelemetry::global::shutdown_tracer_provider();
    }

    fn record_error(&self, error: &dyn std::error::Error) {
        opentelemetry::trace::get_active_span(|span| span.record_error(error))
    }
}
