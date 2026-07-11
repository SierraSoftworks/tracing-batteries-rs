use std::{
    backtrace::{Backtrace, BacktraceStatus},
    collections::HashMap,
};

/// Symbol prefixes stripped from the top of a `human_errors`-captured backtrace, in
/// addition to the crate's default skip prefixes applied by [`crate::backtraces`].
#[cfg(feature = "human_errors")]
const HUMAN_ERRORS_INITIAL_SKIP_PREFIXES: &[&str] = &["std::backtrace::", "human_errors::"];

#[cfg(feature = "human_errors")]
const HUMAN_ERRORS_BACKTRACE_METADATA_KEY: &str = "human_errors.backtrace";

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
        #[cfg(feature = "human_errors")]
        if let Some(native) = self.metadata.get(HUMAN_ERRORS_BACKTRACE_METADATA_KEY) {
            return Some(native.clone());
        }

        (self.backtrace.status() == BacktraceStatus::Captured)
            .then(|| crate::backtraces::simplify_backtrace_text(&self.backtrace.to_string()))
    }

    /// Captures an [`ErrorInfo`] from a [`human_errors::Error`] while preserving
    /// the error's native cause chain and captured backtraces.
    #[cfg(feature = "human_errors")]
    pub fn from_human_error(error: &'a human_errors::Error) -> Self {
        use std::error::Error as _;

        let mut causes = Vec::new();
        let mut source = error.source();
        while let Some(cause) = source {
            if let Some(human_error) = cause.downcast_ref::<human_errors::Error>() {
                causes.push(human_error.description());
            } else {
                causes.push(cause.to_string());
            }
            source = cause.source();
        }

        let mut metadata = HashMap::new();
        if let Some(backtrace) = collect_human_error_backtraces(error) {
            metadata.insert(HUMAN_ERRORS_BACKTRACE_METADATA_KEY, backtrace);
        }

        Self {
            error,
            error_type: std::any::type_name::<human_errors::Error>(),
            message: error.description(),
            causes,
            // The human-errors backtrace is attached in metadata and exposed through
            // `simplified_backtrace()`, so this field remains disabled here.
            backtrace: Backtrace::disabled(),
            metadata,
        }
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

#[cfg(feature = "human_errors")]
fn collect_human_error_backtraces(error: &human_errors::Error) -> Option<String> {
    use std::error::Error as _;

    let mut backtraces: Vec<(String, String)> = Vec::new();

    if let Some(backtrace) = captured_backtrace(error.backtrace()) {
        backtraces.push((
            error.description(),
            crate::backtraces::simplify_backtrace_text_with_prefixes(
                &backtrace.to_string(),
                HUMAN_ERRORS_INITIAL_SKIP_PREFIXES,
            ),
        ));
    }

    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(human_error) = cause.downcast_ref::<human_errors::Error>() {
            if let Some(backtrace) = captured_backtrace(human_error.backtrace()) {
                backtraces.push((
                    human_error.description(),
                    crate::backtraces::simplify_backtrace_text_with_prefixes(
                        &backtrace.to_string(),
                        HUMAN_ERRORS_INITIAL_SKIP_PREFIXES,
                    ),
                ));
            }
        }

        source = cause.source();
    }

    if backtraces.is_empty() {
        return None;
    }

    Some(
        backtraces
            .into_iter()
            .map(|(description, backtrace)| format!("Backtrace ({description}):\n{backtrace}"))
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

#[cfg(feature = "human_errors")]
fn captured_backtrace(backtrace: Option<&Backtrace>) -> Option<&Backtrace> {
    backtrace.filter(|backtrace| backtrace.status() == BacktraceStatus::Captured)
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

    #[cfg(feature = "human_errors")]
    #[test]
    fn from_human_error_preserves_native_message_and_causes() {
        let error = human_errors::wrap_system(
            human_errors::system("inner failure", &["check inner systems"]),
            "outer failure",
            &["check outer systems"],
        );

        let info = ErrorInfo::from_human_error(&error);

        assert!(info.error_type.contains("human_errors"));
        assert_eq!(info.message, "outer failure");
        assert_eq!(info.causes, vec!["inner failure".to_string()]);

        if error
            .backtrace()
            .is_some_and(|backtrace| backtrace.status() == BacktraceStatus::Captured)
        {
            assert!(
                info.simplified_backtrace().is_some(),
                "native human-errors backtraces should be surfaced through simplified_backtrace"
            );
        } else {
            assert!(
                info.simplified_backtrace().is_none(),
                "simplified backtrace should be absent when human-errors did not capture one"
            );
        }
    }
}
