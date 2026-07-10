use std::{
    backtrace::{Backtrace, BacktraceStatus},
    collections::HashMap,
};

/// Symbol prefixes whose frames are dropped while they appear contiguously at
/// the top (innermost end) of a backtrace.
const INITIAL_SKIP_PREFIXES: &[&str] = &["std::backtrace", "tracing_batteries"];

/// Symbol prefixes whose frames retain their function name but have their file
/// path (the `at <path>` line) removed to reduce noise.
const HIDE_PATH_PREFIXES: &[&str] = &["core::", "std::"];

/// The standard library symbol which marks the boundary between user code and
/// the runtime/OS bootstrap that invoked it.
const RUNTIME_BOUNDARY_MARKER: &str = "std::sys::backtrace::__rust_begin_short_backtrace";

/// A single frame parsed from a backtrace's textual rendering.
struct BacktraceFrame<'a> {
    index: usize,
    symbol: &'a str,
    location: Option<&'a str>,
}

/// Rich context about an error reported via [`Session::record_error`](crate::Session::record_error).
///
/// This struct is constructed by the [`Session`](crate::Session) when an error is reported,
/// capturing the concrete error type's name, its message, the chain of causes, and a backtrace
/// before the error is fanned out to each registered [`Battery`](crate::Battery). Integrations
/// which only need the original error (for example to hand it to an SDK which walks the cause
/// chain itself) can use the [`error`](ErrorInfo::error) field directly.
#[derive(Debug)]
pub struct ErrorInfo<'a> {
    /// The original error, for integrations that consume `&dyn Error` directly.
    pub error: &'a (dyn std::error::Error + 'a),

    /// The fully-qualified Rust type name of the error, e.g. `std::io::Error`.
    pub error_type: &'static str,

    /// The `Display` rendering of the error.
    pub message: String,

    /// `Display` renderings of the error's `source()` chain, outermost cause first.
    pub causes: Vec<String>,

    /// A backtrace captured at the `record_error` call site.
    ///
    /// This is captured with [`Backtrace::force_capture`], so it is always collected
    /// regardless of the `RUST_BACKTRACE`/`RUST_LIB_BACKTRACE` environment variables.
    /// It is only [unsupported](BacktraceStatus::Unsupported) on platforms without
    /// backtrace support.
    pub backtrace: Backtrace,

    /// Additional metadata about the error, which may be provided by the integration
    /// or the application. This is a free-form map of key/value pairs which may be
    /// used to provide additional context about the error, such as the HTTP status code
    /// of a failed request, the database query which failed, or any other relevant
    /// information which may help diagnose the issue.
    pub metadata: HashMap<&'static str, String>,
}

impl<'a> ErrorInfo<'a> {
    /// Captures the details of the provided error, including its type name, message,
    /// cause chain, and a backtrace.
    pub fn new<E: std::error::Error>(error: &'a E) -> Self {
        let mut causes = Vec::new();
        let mut source = error.source();
        while let Some(cause) = source {
            causes.push(cause.to_string());
            source = cause.source();
        }

        Self {
            error,
            error_type: std::any::type_name::<E>(),
            message: error.to_string(),
            causes,
            backtrace: Backtrace::force_capture(),
            metadata: HashMap::new(),
        }
    }

    /// Returns the backtrace as text, only when one was actually captured.
    pub fn backtrace_text(&self) -> Option<String> {
        (self.backtrace.status() == BacktraceStatus::Captured).then(|| self.backtrace.to_string())
    }

    /// Returns a simplified backtrace as text, only when one was actually captured.
    ///
    /// The simplified rendering removes noisy top/bottom runtime frames and hides
    /// source locations for `core::*`/`std::*` frames.
    pub fn simplified_backtrace(&self) -> Option<String> {
        (self.backtrace.status() == BacktraceStatus::Captured)
            .then(|| simplify_backtrace_text(&self.backtrace.to_string()))
    }

    /// Adds a new metadata field to the error info, which will be reported to the telemetry system.
    pub fn with_metadata(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.metadata.insert(key, value.into());
        self
    }

    /// Disables the backtrace for this error info, which may be useful for errors which are
    /// expected to occur frequently and for which a backtrace is not useful (for example,
    /// a "not found" error when looking up a resource by ID).
    ///
    /// Note that this may impact the ability of the telemetry system to associate different
    /// errors with the same root cause, as the backtrace is commonly used to identify the call
    /// site of the error.
    pub fn without_backtrace(mut self) -> Self {
        self.backtrace = Backtrace::disabled();
        self
    }
}

impl<'a, E: std::error::Error> From<&'a E> for ErrorInfo<'a> {
    fn from(error: &'a E) -> Self {
        Self::new(error)
    }
}

/// Applies the backtrace simplification rules to the textual rendering of a
/// backtrace.
fn simplify_backtrace_text(raw: &str) -> String {
    let frames = parse_frames(raw);
    if frames.is_empty() {
        return raw.to_string();
    }

    // Drop runtime/OS bootstrap frames at the bottom (outermost end).
    let end = frames
        .iter()
        .position(|frame| frame.symbol.contains(RUNTIME_BOUNDARY_MARKER))
        .unwrap_or(frames.len());
    let bottom_skipped = frames.len() - end;

    // Drop capture and crate-internal frames at the top (innermost end).
    let start = frames[..end]
        .iter()
        .position(|frame| !starts_with_any(frame.symbol, INITIAL_SKIP_PREFIXES))
        .unwrap_or(end);
    let top_skipped = start;

    let frames = &frames[start..end];
    if frames.is_empty() {
        return raw.to_string();
    }

    let mut output = String::new();

    if top_skipped > 0 {
        output.push_str(&format!("    ... skipped {top_skipped} frames ...\n"));
    }

    for frame in frames {
        output.push_str(&format!("{:>2}: {}\n", frame.index, frame.symbol));

        if let Some(location) = frame.location {
            if !starts_with_any(frame.symbol, HIDE_PATH_PREFIXES) {
                output.push_str(&format!("    {location}\n"));
            }
        }
    }

    if bottom_skipped > 0 {
        output.push_str(&format!("    ... skipped {bottom_skipped} frames ...\n"));
    }

    output
}

/// Parses the textual rendering of a backtrace into individual frames.
fn parse_frames(raw: &str) -> Vec<BacktraceFrame<'_>> {
    let mut frames = Vec::new();
    let mut lines = raw.lines().peekable();

    while let Some(line) = lines.next() {
        let Some((index, symbol)) = parse_frame(line.trim_start()) else {
            continue;
        };

        let location = match lines.peek() {
            Some(next) if next.trim_start().starts_with("at ") => {
                Some(lines.next().unwrap().trim_start())
            }
            _ => None,
        };

        frames.push(BacktraceFrame {
            index,
            symbol,
            location,
        });
    }

    frames
}

/// Parses a frame header line of the form `<index>: <symbol>`.
fn parse_frame(line: &str) -> Option<(usize, &str)> {
    let (index, symbol) = line.split_once(": ")?;
    Some((index.parse().ok()?, symbol))
}

fn starts_with_any(symbol: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| symbol.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct InnerError;

    impl std::fmt::Display for InnerError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "the inner failure")
        }
    }

    impl std::error::Error for InnerError {}

    #[derive(Debug)]
    struct OuterError {
        inner: InnerError,
    }

    impl std::fmt::Display for OuterError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "the outer failure")
        }
    }

    impl std::error::Error for OuterError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.inner)
        }
    }

    #[test]
    fn error_info_captures_type_message_and_causes() {
        let error = OuterError { inner: InnerError };
        let info = ErrorInfo::new(&error);

        assert!(
            info.error_type.contains("OuterError"),
            "the error type should include the concrete type name, got {}",
            info.error_type
        );
        assert_eq!(info.message, "the outer failure");
        assert_eq!(info.causes, vec!["the inner failure".to_string()]);
        assert_eq!(info.error.to_string(), "the outer failure");
        assert!(
            info.backtrace_text().is_some(),
            "a backtrace should always be captured, regardless of RUST_BACKTRACE"
        );
        assert!(
            info.simplified_backtrace().is_some(),
            "a simplified backtrace should be available when a backtrace is captured"
        );
    }

    #[test]
    fn simplify_backtrace_text_drops_noise_and_hides_std_paths() {
        let raw = "\
   0: std::backtrace_rs::backtrace::libunwind::trace
             at /rustc/library/std/src/backtrace.rs:1
   1: std::backtrace::Backtrace::create
             at /rustc/library/std/src/backtrace.rs:2
   2: tracing_batteries::error_info::ErrorInfo::new
             at ./src/error_info.rs:10
   3: my_app::do_work
             at ./src/main.rs:20
   4: core::ops::function::FnOnce::call_once
             at /rustc/library/core/src/ops/function.rs:250
   5: std::sys::backtrace::__rust_begin_short_backtrace
             at /rustc/library/std/src/sys/backtrace.rs:154
   6: std::rt::lang_start_internal
             at /rustc/library/std/src/rt.rs:175
   7: main";

        let simplified = simplify_backtrace_text(raw);

        assert!(!simplified.contains("std::backtrace"));
        assert!(!simplified.contains("tracing_batteries::"));
        assert!(simplified.contains("... skipped 3 frames ..."));
        assert!(simplified.contains(" 3: my_app::do_work"));
        assert!(simplified.contains("at ./src/main.rs:20"));
        assert!(simplified.contains("core::ops::function::FnOnce::call_once"));
        assert!(!simplified.contains("at /rustc/library/core/src/ops/function.rs:250"));
        assert!(!simplified.contains("__rust_begin_short_backtrace"));
        assert!(simplified.contains("... skipped 3 frames ..."));
    }

    #[test]
    fn simplify_backtrace_text_falls_back_when_unparsable() {
        let raw = "not a recognisable backtrace";
        assert_eq!(simplify_backtrace_text(raw), raw);
    }
}
