use std::backtrace::{Backtrace, BacktraceStatus};

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
        }
    }

    /// Returns the backtrace as text, only when one was actually captured.
    pub fn backtrace_text(&self) -> Option<String> {
        (self.backtrace.status() == BacktraceStatus::Captured).then(|| self.backtrace.to_string())
    }
}

impl<'a, E: std::error::Error> From<&'a E> for ErrorInfo<'a> {
    fn from(error: &'a E) -> Self {
        Self::new(error)
    }
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
    }
}
