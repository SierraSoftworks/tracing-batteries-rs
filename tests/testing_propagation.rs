#![cfg(all(feature = "testing", feature = "opentelemetry"))]

use std::collections::HashMap;

use tracing_batteries::prelude::*;
use tracing_batteries::{Session, Testing};

/// A carrier which implements the OpenTelemetry [`Injector`](OpenTelemetryPropagationInjector)
/// and [`Extractor`](OpenTelemetryPropagationExtractor) traits over a [`HashMap`], allowing us
/// to round-trip trace context through text headers exactly as a real transport would.
#[derive(Default)]
struct HashMapCarrier(HashMap<String, String>);

impl OpenTelemetryPropagationInjector for HashMapCarrier {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }
}

impl OpenTelemetryPropagationExtractor for HashMapCarrier {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|v| v.as_str())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

/// The `Testing` battery should install a text map propagator and an OpenTelemetry tracing
/// layer, so injecting the current context inside an instrumented function must produce a
/// W3C `traceparent` header.
#[test]
fn testing_injects_trace_context() {
    let session = Session::new("example", "0.0.1").with_battery(Testing);

    let carrier = inject_current_context();

    assert!(
        carrier.0.contains_key("traceparent"),
        "expected a traceparent header to be injected, got: {:?}",
        carrier.0
    );

    session.shutdown();
}

/// Trace context should survive a full inject/extract round-trip: a span created on the
/// "client" side and injected into a carrier must be extractable on the "server" side and
/// belong to the same distributed trace.
#[test]
fn testing_propagates_trace_across_boundary() {
    let session = Session::new("example", "0.0.1").with_battery(Testing);

    let (carrier, client_trace_id) = client_side();
    assert!(
        client_trace_id.is_some(),
        "the client span should have a valid trace context"
    );
    assert!(
        carrier.0.contains_key("traceparent"),
        "the client should have injected a traceparent header, got: {:?}",
        carrier.0
    );

    let server_trace_id = server_side(&carrier);

    assert_eq!(
        client_trace_id, server_trace_id,
        "the trace id should be preserved across the propagation boundary"
    );

    session.shutdown();
}

/// In-process propagation should also work: a child span created within a parent span must
/// share the parent's trace id.
#[test]
fn testing_propagates_trace_across_nested_spans() {
    let session = Session::new("example", "0.0.1").with_battery(Testing);

    let (parent_trace_id, child_trace_id) = nested_spans();

    assert!(
        parent_trace_id.is_some(),
        "the parent span should have a valid trace context"
    );
    assert_eq!(
        parent_trace_id, child_trace_id,
        "a nested span should inherit the trace id of its parent"
    );

    session.shutdown();
}

#[tracing::instrument]
fn inject_current_context() -> HashMapCarrier {
    get_text_map_propagator(|propagator| {
        let mut carrier = HashMapCarrier::default();
        propagator.inject_context(&Span::current().context(), &mut carrier);
        carrier
    })
}

#[tracing::instrument]
fn client_side() -> (HashMapCarrier, Option<opentelemetry::trace::TraceId>) {
    let context = Span::current().context();
    let trace_id = trace_id_of(&context);

    let carrier = get_text_map_propagator(|propagator| {
        let mut carrier = HashMapCarrier::default();
        propagator.inject_context(&context, &mut carrier);
        carrier
    });

    (carrier, trace_id)
}

fn server_side(carrier: &HashMapCarrier) -> Option<opentelemetry::trace::TraceId> {
    let parent_context = get_text_map_propagator(|propagator| propagator.extract(carrier));

    let span = tracing::info_span!("server_handler");
    let _ = span.set_parent(parent_context);

    trace_id_of(&span.context())
}

#[tracing::instrument]
fn nested_spans() -> (
    Option<opentelemetry::trace::TraceId>,
    Option<opentelemetry::trace::TraceId>,
) {
    let parent_trace_id = trace_id_of(&Span::current().context());

    let child = tracing::info_span!("child");
    let child_trace_id = child.in_scope(|| trace_id_of(&Span::current().context()));

    (parent_trace_id, child_trace_id)
}

/// Extracts the trace id from an OpenTelemetry [`Context`](opentelemetry::Context), returning
/// [`None`] when the context does not carry a valid span.
fn trace_id_of(context: &opentelemetry::Context) -> Option<opentelemetry::trace::TraceId> {
    let span_context = context.span().span_context().clone();
    if span_context.is_valid() {
        Some(span_context.trace_id())
    } else {
        None
    }
}
