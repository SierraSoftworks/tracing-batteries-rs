use tracing_batteries::{OpenTelemetry, OpenTelemetryProtocol, Session};

#[tokio::test]
async fn otel_setup_http_json() {
    let session = Session::new("example", "0.0.1").with_battery(
        OpenTelemetry::new("http://localhost:4318")
            .with_protocol(OpenTelemetryProtocol::HttpJson)
            .with_header("test-header", "test-value"),
    );

    {
        let _ = tracing::info_span!("setting up opentelemetry http-json session").enter();
    }

    session.shutdown();
}