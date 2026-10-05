use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use tracing_batteries::{Pyroscope, PyroscopePprof, Session};

/// Starts a server which accepts profile uploads, reporting the request line and headers of
/// each one it receives.
fn profile_server() -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the profile server");
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = channel();

    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(n) if n > 0 => request.extend_from_slice(&buffer[..n]),
                    _ => break,
                }
            }

            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            let _ = tx.send(String::from_utf8_lossy(&request).to_lowercase());
        }
    });

    (endpoint, rx)
}

fn battery(endpoint: String) -> Pyroscope {
    Pyroscope::new(endpoint)
        .with_tenant_id("example-tenant")
        .with_header("x-example", "yes")
        .with_upload_interval(Duration::from_secs(1))
        .with_backend(PyroscopePprof::new())
}

#[test]
fn profiles_are_uploaded() {
    let (endpoint, uploads) = profile_server();

    let session = Session::new("example", "0.0.1")
        .with_debug_builds()
        .with_battery(battery(endpoint));

    // Shutting the session down uploads the profile gathered so far.
    session.shutdown();

    let upload = uploads
        .recv_timeout(Duration::from_secs(15))
        .expect("a profile should be uploaded on shutdown");
    assert!(
        upload.starts_with("post /push.v1.pusherservice/push"),
        "unexpected request: {upload}"
    );
    assert!(upload.contains("x-scope-orgid: example-tenant"));
    assert!(upload.contains("x-example: yes"));
}

#[test]
fn disabled_sessions_are_not_profiled() {
    let (endpoint, uploads) = profile_server();

    let mut metadata = Session::new("example", "0.0.1");
    metadata.enabled_by_default = false;
    metadata.with_battery(battery(endpoint)).shutdown();

    assert!(
        uploads.recv_timeout(Duration::from_millis(500)).is_err(),
        "a disabled session should not upload profiles"
    );
}
