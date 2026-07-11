//! Shared backtrace text processing used across the crate.
//!
//! Every backtrace this crate reports — whether captured at a
//! [`record_error`](crate::Session::record_error) call site or inside the analytics
//! panic hook — carries a prologue of frames belonging to the capture machinery and,
//! for panics, the panic runtime, plus a process-bootstrap epilogue. This module owns
//! the logic that parses the textual rendering of a backtrace and simplifies it down to
//! the frames that describe the failure, so the error and panic reporting paths share a
//! single, consistently-tested implementation.

/// Symbol prefixes whose frames are dropped while they appear contiguously at
/// the top (innermost end) of a backtrace.
const INITIAL_SKIP_PREFIXES: &[&str] = &["std::backtrace", "tracing_batteries"];

/// Symbol prefixes whose frames retain their function name but have their file
/// path (the `at <path>` line) removed to reduce noise.
const HIDE_PATH_PREFIXES: &[&str] = &["core::", "std::"];

/// The standard library symbol which marks the boundary between user code and
/// the runtime/OS bootstrap that invoked it (the bottom, outermost end).
const RUNTIME_BOUNDARY_MARKER: &str = "std::sys::backtrace::__rust_begin_short_backtrace";

/// The standard library symbol marking the top of the panic-dispatch machinery.
///
/// Frames at or above it (the panic hook closure, `panic_with_hook`, the panic handler)
/// belong to the runtime rather than the code that panicked, and only appear when the
/// backtrace was captured on the panic path. Trimming to just below this marker leaves
/// the trace beginning at `rust_begin_unwind`, matching what the standard library prints
/// for a panic.
const PANIC_DISPATCH_MARKER: &str = "std::sys::backtrace::__rust_end_short_backtrace";

/// A single frame parsed from a backtrace's textual rendering.
struct BacktraceFrame<'a> {
    index: usize,
    symbol: &'a str,
    location: Option<&'a str>,
}

/// Applies the backtrace simplification rules to the textual rendering of a
/// backtrace, using the default set of skip prefixes.
pub(crate) fn simplify_backtrace_text(raw: &str) -> String {
    simplify_backtrace_text_with_prefixes(raw, INITIAL_SKIP_PREFIXES)
}

/// As [`simplify_backtrace_text`], but with a caller-supplied set of prefixes to strip
/// from the top of the trace (used, for example, to strip `human_errors::` frames from
/// backtraces captured by that crate).
pub(crate) fn simplify_backtrace_text_with_prefixes(
    raw: &str,
    initial_skip_prefixes: &[&str],
) -> String {
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

    // Drop noise at the top (innermost end). Two rules combine: contiguous frames
    // matching the skip prefixes (the capture site and this crate's own frames), and —
    // on the panic path — every frame up to and including the panic-dispatch marker,
    // which sits above the code that actually panicked.
    let prefix_skip = frames[..end]
        .iter()
        .position(|frame| !starts_with_any(frame.symbol, initial_skip_prefixes))
        .unwrap_or(end);
    let panic_skip = frames[..end]
        .iter()
        .position(|frame| frame.symbol.contains(PANIC_DISPATCH_MARKER))
        .map_or(0, |index| index + 1);
    let start = prefix_skip.max(panic_skip);
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
    fn simplify_backtrace_text_strips_panic_dispatch_prologue() {
        // A backtrace captured inside the analytics panic hook: capture and crate frames
        // sit above the panic-dispatch machinery, which in turn sits above the code that
        // actually panicked. Everything up to `__rust_end_short_backtrace` is the runtime.
        let raw = "\
   0: std::backtrace::Backtrace::create
             at /rustc/library/std/src/backtrace.rs:1
   1: tracing_batteries::integration_analytics::install_panic_hook::{{closure}}
             at ./src/integration_analytics.rs:575
   2: std::panicking::panic_with_hook
             at /rustc/library/std/src/panicking.rs:1
   3: std::panicking::panic_handler::{{closure}}
             at /rustc/library/std/src/panicking.rs:2
   4: std::sys::backtrace::__rust_end_short_backtrace
             at /rustc/library/std/src/sys/backtrace.rs:1
   5: rust_begin_unwind
             at /rustc/library/std/src/panicking.rs:3
   6: core::panicking::panic_fmt
             at /rustc/library/core/src/panicking.rs:4
   7: my_app::do_work
             at ./src/main.rs:42
   8: std::sys::backtrace::__rust_begin_short_backtrace
             at /rustc/library/std/src/sys/backtrace.rs:2
   9: std::rt::lang_start_internal
             at /rustc/library/std/src/rt.rs:1
  10: main";

        let simplified = simplify_backtrace_text(raw);

        // The panic-dispatch prologue and the bootstrap epilogue are gone...
        assert!(!simplified.contains("install_panic_hook"));
        assert!(!simplified.contains("panic_with_hook"));
        assert!(!simplified.contains("panic_handler"));
        assert!(!simplified.contains("__rust_end_short_backtrace"));
        assert!(!simplified.contains("__rust_begin_short_backtrace"));
        assert!(simplified.contains("... skipped 5 frames ..."));
        assert!(simplified.contains("... skipped 3 frames ..."));

        // ...leaving the trace beginning at the panic site, std/core paths hidden.
        assert!(simplified.contains(" 5: rust_begin_unwind"));
        assert!(simplified.contains(" 6: core::panicking::panic_fmt"));
        assert!(!simplified.contains("at /rustc/library/core/src/panicking.rs:4"));
        assert!(simplified.contains(" 7: my_app::do_work"));
        assert!(simplified.contains("at ./src/main.rs:42"));
    }

    #[test]
    fn simplify_backtrace_text_falls_back_when_unparsable() {
        let raw = "not a recognisable backtrace";
        assert_eq!(simplify_backtrace_text(raw), raw);
    }
}
