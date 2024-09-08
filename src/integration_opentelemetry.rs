use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
};

use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{Battery, BatteryBuilder};
pub use opentelemetry_otlp::Protocol as OpenTelemetryProtocol;

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
    headers: HashMap<&'static str, Cow<'static, str>>,
    protocol: OpenTelemetryProtocol,
}

impl OpenTelemetry {
    pub fn new<S: Into<Cow<'static, str>>>(endpoint: S) -> Self {
        Self {
            endpoint: endpoint.into(),
            headers: HashMap::new(),
            protocol: OpenTelemetryProtocol::Grpc,
        }
    }

    pub fn with_header<K: Into<&'static str>, V: Into<Cow<'static, str>>>(
        mut self,
        key: K,
        value: V,
    ) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    pub fn with_protocol(mut self, protocol: OpenTelemetryProtocol) -> Self {
        self.protocol = protocol;
        self
    }
}

impl BatteryBuilder for OpenTelemetry {
    fn setup(self, metadata: &crate::Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        let mut resource_metadata = vec![
            opentelemetry::KeyValue::new("service.name", metadata.service.clone()),
            opentelemetry::KeyValue::new("service.version", metadata.version.clone()),
            opentelemetry::KeyValue::new("host.os", std::env::consts::OS),
            opentelemetry::KeyValue::new("host.architecture", std::env::consts::ARCH),
        ];

        for (key, value) in metadata.context.iter() {
            resource_metadata.push(opentelemetry::KeyValue::new(*key, value.clone()));
        }

        let pipeline_builder = match self.protocol {
            OpenTelemetryProtocol::Grpc => {
                opentelemetry_otlp::new_pipeline().tracing().with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .tonic()
                        .with_endpoint(self.endpoint)
                        .with_metadata({
                            let mut tracing_metadata = tonic::metadata::MetadataMap::new();
                            for (key, value) in self.headers {
                                tracing_metadata.insert(key, value.parse().unwrap());
                            }
                            tracing_metadata
                        }),
                )
            }
            OpenTelemetryProtocol::HttpBinary | OpenTelemetryProtocol::HttpJson => {
                opentelemetry_otlp::new_pipeline().tracing().with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .http()
                        .with_protocol(self.protocol)
                        .with_endpoint(format!("{}/v1/traces", self.endpoint))
                        .with_headers({
                            let mut tracing_headers = HashMap::new();
                            for (key, value) in self.headers {
                                tracing_headers.insert(key.to_string(), value.to_string());
                            }
                            tracing_headers
                        })
                        .with_http_client(reqwest::Client::new()),
                )
            }
        };

        let provider = pipeline_builder
            .with_trace_config(
                opentelemetry_sdk::trace::Config::default()
                    .with_resource(opentelemetry_sdk::Resource::new(resource_metadata)),
            )
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .unwrap();

        opentelemetry::global::set_tracer_provider(provider.clone());

        tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::DEBUG)
            .with(tracing_subscriber::filter::dynamic_filter_fn(
                move |_meta, _ctx| enabled.load(std::sync::atomic::Ordering::Relaxed),
            ))
            .with(tracing_opentelemetry::OpenTelemetryLayer::new(
                provider.tracer(metadata.service.clone()),
            ))
            .init();

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
