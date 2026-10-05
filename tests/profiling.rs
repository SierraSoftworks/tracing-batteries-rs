use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use opentelemetry_proto::tonic::collector::profiles::v1development::ExportProfilesServiceRequest;
use prost::Message;
use tracing_batteries::{OpenTelemetryProtocol, Profiling, ProfilingPprof, Session};

/// Starts a collector which accepts OTLP/HTTP exports, reporting the head and body of each
/// request it receives.
fn collector() -> (String, Receiver<(String, Vec<u8>)>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the collector");
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = channel();

    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = Vec::new();
            let mut buffer = [0u8; 8192];
            let (head, body) = loop {
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|length| length.parse::<usize>().ok())
                        .unwrap_or_default();
                    if request.len() >= end + 4 + length {
                        break (head, request[end + 4..].to_vec());
                    }
                }

                match stream.read(&mut buffer) {
                    Ok(n) if n > 0 => request.extend_from_slice(&buffer[..n]),
                    _ => break (String::new(), Vec::new()),
                }
            };

            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            let _ = tx.send((head, body));
        }
    });

    (endpoint, rx)
}

fn battery(endpoint: String) -> Profiling {
    battery_with(endpoint, ProfilingPprof::new())
}

fn battery_with(endpoint: String, backend: ProfilingPprof) -> Profiling {
    Profiling::new(endpoint)
        .with_protocol(OpenTelemetryProtocol::HttpBinary)
        .with_header("x-example", "yes")
        .with_backend(backend)
}

/// Keeps the CPU busy for long enough that the profiler is certain to have sampled it.
fn burn_cpu() {
    let started = Instant::now();
    let mut value = 0u64;
    while started.elapsed() < Duration::from_millis(500) {
        value = std::hint::black_box(value.wrapping_mul(31).wrapping_add(7));
    }
}

#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn profiles_are_exported() {
    let (endpoint, exports) = collector();

    let session = Session::new("example", "0.0.1")
        .with_debug_builds()
        .with_battery(battery(endpoint));

    burn_cpu();

    // Shutting the session down exports the profile gathered so far.
    session.shutdown();

    let (head, body) = exports
        .recv_timeout(Duration::from_secs(15))
        .expect("a profile should be exported on shutdown");
    assert!(
        head.starts_with("post /v1development/profiles "),
        "unexpected request: {head}"
    );
    assert!(head.contains("x-example: yes"));

    let request = ExportProfilesServiceRequest::decode(body.as_slice())
        .expect("the export should be an OTLP profiles request");
    let resource_profiles = &request.resource_profiles[0];

    let resource = resource_profiles.resource.as_ref().expect("a resource");
    assert!(
        resource
            .attributes
            .iter()
            .any(|kv| kv.key == "service.name"),
        "the session's metadata should describe the profile"
    );

    let profile = &resource_profiles.scope_profiles[0].profiles[0];
    assert!(
        !profile.samples.is_empty(),
        "the profile should have samples"
    );
    assert!(
        !request
            .dictionary
            .expect("a dictionary")
            .function_table
            .is_empty(),
        "the sampled stacks should be symbolized"
    );
}

#[test]
fn disabled_sessions_are_not_profiled() {
    let (endpoint, exports) = collector();

    let mut metadata = Session::new("example", "0.0.1");
    metadata.enabled_by_default = false;
    let session = metadata.with_battery(battery(endpoint));

    burn_cpu();
    session.shutdown();

    assert!(
        exports.recv_timeout(Duration::from_millis(500)).is_err(),
        "a disabled session should not export profiles"
    );
}

#[test]
fn unsupported_sample_rates_are_skipped() {
    for sample_rate in [0, 2_000_000] {
        let (endpoint, exports) = collector();

        let session = Session::new("example", "0.0.1")
            .with_debug_builds()
            .with_battery(battery_with(
                endpoint,
                ProfilingPprof::new().with_sample_rate(sample_rate),
            ));

        burn_cpu();
        session.shutdown();

        assert!(
            exports.recv_timeout(Duration::from_millis(500)).is_err(),
            "a sample rate of {sample_rate}Hz should not be profiled"
        );
    }
}
