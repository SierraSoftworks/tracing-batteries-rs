use tracing::Instrument;
use tracing_batteries::{OpenTelemetry, OpenTelemetryProtocol, Session};

#[tokio::test]
async fn otel_setup_grpc() {
    let session = Session::new("example", "0.0.1").with_battery(
        OpenTelemetry::new("localhost:4317")
            .with_protocol(OpenTelemetryProtocol::Grpc)
            .with_header("test-header", "test-value"),
    );

    {
        let _ = tracing::info_span!("setting up opentelemetry grpc session").enter();
    }

    session.shutdown();
}