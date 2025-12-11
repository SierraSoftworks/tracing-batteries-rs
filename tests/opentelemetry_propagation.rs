use tracing_batteries::{OpenTelemetry, Session};
use tracing_batteries::prelude::*;

#[tokio::test]
async fn otel_propagation() {
    let session = Session::new("example", "0.0.1").with_battery(
        OpenTelemetry::new("localhost:4317")
            .with_header("test-header", "test-value"),
    );

    propagating_method();

    session.shutdown();
}

#[tracing::instrument]
fn propagating_method() {
    let headers = get_text_map_propagator(|p| {
        let mut carrier = std::collections::HashMap::new();
        p.inject_context(&Span::current().context(), &mut carrier);
        carrier
    });

    println!("Injected Headers in propagating_method: {:?}", headers);

    assert!(headers.contains_key("traceparent"));
}