use crate::{Battery, BatteryBuilder, ErrorInfo, Metadata, Page};
use radix_fmt::radix;
use rand::random;
use sha2::Digest;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::env::consts::{ARCH, OS};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

// Client-side caps mirroring the analytics server's ingest limits. The server truncates
// oversized fields itself, but its HTTP layer rejects request bodies larger than 16KB
// outright, so bounding the large fields client-side is what keeps events deliverable.
const MAX_MESSAGE: usize = 1000;
const MAX_STACK: usize = 8 * 1024;
const MAX_METADATA_KEY: usize = 256;
const MAX_METADATA_ENTRIES: usize = 32;
const MAX_METADATA_VALUE: usize = 1024;

/// An [Analytics](https://github.com/SierraSoftworks/analytics) integration which reports
/// application usage and errors to a self-hosted analytics server in a privacy preserving way.
///
/// <div class="warning">
///
/// This integration requires the `analytics` feature to be enabled.
///
/// </div>
///
/// The Analytics integration is initialized by providing the URL of an analytics server
/// which will receive the telemetry. Application execution is tracked as page views on a
/// virtual website with the hostname `{service.name}.app` (configurable via
/// [`with_hostname`](Analytics::with_hostname)), custom events are reported through
/// [`Session::record_event`](crate::Session::record_event), and errors reported through
/// [`Session::record_error`](crate::Session::record_error) become exception reports with
/// the error's type, message, cause chain, and a backtrace.
///
/// Every event of one application run carries the same session id (generated afresh per
/// battery, held only in memory), so the analytics server can assemble the run's page
/// views, events, and exceptions into a single session trace without making separate
/// runs correlatable.
///
/// Unhandled panics are also captured and reported as exceptions by default; call
/// [`with_panic_capture(false)`](Analytics::with_panic_capture) to disable this. The panic
/// hook chains any previously installed hook, and a panicking process may spend up to
/// approximately three seconds delivering the report before continuing to unwind or abort.
///
/// Requests carry a `{service.name}/{service.version}` User-Agent so the server classifies
/// them as an application rather than a browser. Note that the server silently discards
/// traffic whose User-Agent looks like a bot, so service names containing `bot`, `crawler`,
/// or `spider` will not be recorded.
///
/// ## Example
/// ```no_run
/// use tracing_batteries::{Session, Analytics};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///     .with_battery(Analytics::new("https://analytics.example.com"));
///
/// session.shutdown();
/// ```
pub struct Analytics {
    server: Cow<'static, str>,

    page: Option<Page>,
    referrer: Option<Cow<'static, str>>,
    hostname: Option<Cow<'static, str>>,
    panic_capture: bool,
}

impl Analytics {
    /// Configures the Analytics integration with the given server URL.
    ///
    /// The server URL should point to an analytics server instance which is capable
    /// of receiving telemetry, without any trailing path (the integration posts to
    /// `{server}/track/hit` and `{server}/track/exception`).
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Analytics};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///     .with_battery(Analytics::new("https://analytics.example.com"));
    ///
    /// session.shutdown();
    /// ```
    pub fn new<S: Into<Cow<'static, str>>>(server: S) -> Self {
        Self {
            server: server.into(),
            page: None,
            referrer: None,
            hostname: None,
            panic_capture: true,
        }
    }

    /// Configures the page URL which should be used for the initial analytics event.
    ///
    /// This method allows you to specify the page path that will be reported when the
    /// session starts. If not set, `/` will be used by default. Subsequent page views
    /// may be triggered by calling the [`Session::record_new_page`](crate::Session::record_new_page) method.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Analytics};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///     .with_battery(Analytics::new("https://analytics.example.com")
    ///        .with_initial_page("/home"));
    ///
    /// session.shutdown();
    /// ```
    pub fn with_initial_page<S: Into<Page>>(mut self, page: S) -> Self {
        self.page = Some(page.into());
        self
    }

    /// Configures the referrer URL which should be sent with the initial page view.
    ///
    /// If not set, no referrer is reported.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Analytics};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///    .with_battery(Analytics::new("https://analytics.example.com")
    ///       .with_referrer("https://example.com"));
    ///
    /// session.shutdown();
    /// ```
    pub fn with_referrer<S: Into<Cow<'static, str>>>(mut self, referrer: S) -> Self {
        self.referrer = Some(referrer.into());
        self
    }

    /// Configures the hostname under which this application's telemetry is recorded.
    ///
    /// The analytics server identifies telemetry sources by the hostname of the page URL,
    /// which defaults to `{service.name}.app` (lowercased). Use this method if you want
    /// your telemetry to be attributed to a different hostname.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Analytics};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///    .with_battery(Analytics::new("https://analytics.example.com")
    ///       .with_hostname("myapp.example.com"));
    ///
    /// session.shutdown();
    /// ```
    pub fn with_hostname<S: Into<Cow<'static, str>>>(mut self, hostname: S) -> Self {
        self.hostname = Some(hostname.into());
        self
    }

    /// Configures whether unhandled panics are captured and reported as exceptions.
    ///
    /// Panic capture is **enabled by default**. When enabled, the integration installs a
    /// panic hook (chaining any previously installed hook) which reports panics to the
    /// analytics server as unhandled exceptions, including the panic message, location,
    /// and a backtrace. Delivery is bounded to approximately three seconds, after which
    /// the process continues to unwind or abort as usual.
    ///
    /// Note that the captured backtrace includes source file paths from the build
    /// environment; pass `false` here if that is not something you wish to report.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Analytics};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///    .with_battery(Analytics::new("https://analytics.example.com")
    ///       .with_panic_capture(false));
    ///
    /// session.shutdown();
    /// ```
    pub fn with_panic_capture(mut self, enabled: bool) -> Self {
        self.panic_capture = enabled;
        self
    }
}

impl BatteryBuilder for Analytics {
    fn setup(self, metadata: &Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        let server = self.server.trim_end_matches('/');
        let host = self
            .hostname
            .map(|hostname| hostname.to_string())
            .unwrap_or_else(|| format!("{}.app", metadata.service.to_lowercase()));

        let mut context: BTreeMap<String, String> = metadata
            .context
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        context.insert("service.name".to_string(), metadata.service.to_string());
        context.insert("service.version".to_string(), metadata.version.to_string());

        let initial_path = normalize_path(&self.page.unwrap_or_default().url);

        let core = Arc::new(AnalyticsCore {
            hit_endpoint: format!("{server}/track/hit"),
            exception_endpoint: format!("{server}/track/exception"),
            host,
            user_agent: generate_user_agent(&metadata.service, &metadata.version),
            language: sys_locale::get_locale().unwrap_or_else(|| "en".to_string()),
            timezone: iana_time_zone::get_timezone().ok(),
            version: metadata.version.to_string(),
            session_id: generate_beacon_id(),
            context,
            page: Mutex::new(PageState {
                beacon: generate_beacon_id(),
                path: initial_path,
                start: chrono::Utc::now(),
            }),
            is_enabled: enabled,
            hook_armed: AtomicBool::new(self.panic_capture),
            panic_reporting: AtomicBool::new(false),
            outstanding_requests: AtomicUsize::new(0),
            client: reqwest::Client::new(),
        });

        if self.panic_capture {
            install_panic_hook(core.clone());
        }

        let battery = AnalyticsBattery {
            core,
            tracker: UniqueVisitTracker::new(metadata.service.clone()),
            referrer: self
                .referrer
                .map(|referrer| referrer.to_string())
                .filter(|referrer| !referrer.is_empty()),
        };

        battery.send_load_beacon();

        Box::new(battery)
    }
}

/// The state describing the currently active page view.
struct PageState {
    beacon: String,
    path: String,
    start: chrono::DateTime<chrono::Utc>,
}

/// State shared between the battery and the panic hook, which requires it to be
/// `Sync` and `'static` (hence the `Mutex` rather than a `RefCell`).
struct AnalyticsCore {
    hit_endpoint: String,
    exception_endpoint: String,
    host: String,
    user_agent: String,
    language: String,
    timezone: Option<String>,
    version: String,
    /// The per-run session id linking every event this battery reports into one
    /// session trace on the server (page views rotate their beacon id, but the
    /// session id is fixed for the lifetime of the battery — mirroring the
    /// browser tracker, where it is fixed for the lifetime of the page's JS
    /// context). It exists only in memory, so runs remain uncorrelatable.
    session_id: String,
    context: BTreeMap<String, String>,

    page: Mutex<PageState>,

    is_enabled: Arc<AtomicBool>,
    hook_armed: AtomicBool,
    panic_reporting: AtomicBool,
    outstanding_requests: AtomicUsize,
    client: reqwest::Client,
}

impl AnalyticsCore {
    /// Builds the full page URL under which telemetry is recorded. The version rides
    /// along as a UTM campaign tag, which is the only queryable version dimension the
    /// server offers for page views (the `v` field only exists on exceptions).
    fn page_url(&self, path: &str) -> String {
        let separator = if path.contains('?') { '&' } else { '?' };
        format!(
            "https://{}{}{}utm_campaign={}",
            self.host, path, separator, self.version
        )
    }

    /// Returns the current page's beacon ID and path.
    fn current_page(&self) -> (String, String) {
        match self.page.lock() {
            Ok(page) => (page.beacon.clone(), page.path.clone()),
            Err(_) => {
                tracing::warn!("Failed to acquire lock on analytics page state");
                (generate_beacon_id(), "/".to_string())
            }
        }
    }

    fn send_request<P: serde::Serialize + Send + 'static>(
        self: &Arc<Self>,
        url: String,
        payload: P,
    ) {
        if !self.is_enabled.load(Ordering::Relaxed) {
            return;
        }

        // Increment the outstanding requests counter, allowing shutdown to drain in-flight sends
        self.outstanding_requests.fetch_add(1, Ordering::Relaxed);

        let core = self.clone();
        let send_future = async move {
            let result = core
                .client
                .post(&url)
                .json(&payload)
                .header("User-Agent", core.user_agent.as_str())
                .header("Accept-Language", core.language.as_str())
                .send()
                .await;

            core.outstanding_requests.fetch_sub(1, Ordering::Relaxed);

            match result {
                Ok(response) if !response.status().is_success() => {
                    tracing::warn!("Analytics request failed: {}", response.status());
                }
                Ok(_) => {}
                Err(e) => {
                    // Log the error but do not crash the application
                    tracing::warn!("Error sending analytics event: {}", e);
                }
            }
        };

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(send_future);
            }
            Err(_) => {
                // No ambient Tokio runtime: fall back to a short-lived background thread
                // with its own runtime rather than panicking.
                let core = self.clone();
                std::thread::spawn(move || {
                    match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => runtime.block_on(send_future),
                        Err(e) => {
                            core.outstanding_requests.fetch_sub(1, Ordering::Relaxed);
                            tracing::warn!("Failed to build analytics runtime: {}", e);
                        }
                    }
                });
            }
        }
    }

    fn wait_for_outstanding_requests(&self, timeout: Duration) {
        let start_time = std::time::Instant::now();

        while self.outstanding_requests.load(Ordering::Relaxed) > 0 {
            if start_time.elapsed() >= timeout {
                tracing::warn!("Timeout waiting for outstanding requests to complete");
                break;
            }

            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Reports a panic to the analytics server as an unhandled exception.
    ///
    /// This runs inside the process's panic hook, so it must never panic itself, never
    /// block on a lock, and cannot rely on the application's Tokio runtime surviving
    /// long enough to deliver the request.
    fn report_panic(&self, info: &std::panic::PanicHookInfo<'_>) {
        if !self.hook_armed.load(Ordering::Relaxed) || !self.is_enabled.load(Ordering::Relaxed) {
            return;
        }

        // Latch against re-entry: a panic anywhere in the reporting path (including on
        // the sender thread, whose panics also run this process-global hook) must never
        // trigger another report. The latch is only released on a clean completion, so
        // a reporting-path panic disables further reports rather than risking a storm.
        if self.panic_reporting.swap(true, Ordering::SeqCst) {
            return;
        }

        let message = if let Some(message) = info.payload().downcast_ref::<&str>() {
            message.to_string()
        } else if let Some(message) = info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "panic with a non-string payload".to_string()
        };

        let location = info.location().map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        });

        // Capture on the panicking thread (before handing off to the sender thread) so
        // the trace shows the panic site rather than the sender's stack.
        let backtrace = std::backtrace::Backtrace::force_capture();

        // try_lock, never lock: the panic may have fired while a battery method held
        // this mutex, and blocking here would deadlock the dying process.
        let (beacon, path) = match self.page.try_lock() {
            Ok(page) => (Some(page.beacon.clone()), page.path.clone()),
            Err(_) => (None, "/".to_string()),
        };

        let mut metadata = self.context.clone();
        if let Some(location) = &location {
            metadata.insert(
                "panic.location".to_string(),
                truncate_chars(location, MAX_METADATA_VALUE),
            );
        }
        let thread = std::thread::current();
        metadata.insert(
            "panic.thread".to_string(),
            thread.name().unwrap_or("<unnamed>").to_string(),
        );

        let stack = match &location {
            Some(location) => format!("panicked at {location}\n\n{backtrace}"),
            None => backtrace.to_string(),
        };

        let payload = ExceptionPayload {
            u: self.page_url(&path),
            b: beacon,
            i: Some(self.session_id.clone()),
            ty: "panic".to_string(),
            m: truncate_chars(&message, MAX_MESSAGE),
            s: Some(truncate_chars(&stack, MAX_STACK)),
            h: false,
            v: Some(self.version.clone()),
            // A backtrace captured inside a panic hook tops out in the panic runtime's
            // own frames, which would collapse every panic into a single server-side
            // group; grouping by panic site (file:line) is what we actually want.
            fp: info
                .location()
                .map(|location| format!("panic@{}:{}", location.file(), location.line())),
            d: Some(metadata),
        };

        // Deliver from a fresh thread with its own single-threaded runtime and its own
        // client: the application's runtime (and the shared client's connection pool,
        // whose background tasks live on it) may already be tearing down, and a blocking
        // client would panic if this hook fired on a runtime worker thread. The join is
        // bounded by the client's connect/total timeouts, keeping worst-case panic
        // latency to a few seconds.
        let endpoint = self.exception_endpoint.clone();
        let user_agent = self.user_agent.clone();
        if let Ok(handle) = std::thread::Builder::new()
            .name("analytics-panic".to_string())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };

                // The client must be built and the request constructed inside the
                // runtime context: reqwest sets up its timeout timer when the request
                // future is created, not when it is first polled.
                runtime.block_on(async move {
                    let Ok(client) = reqwest::Client::builder()
                        .timeout(Duration::from_secs(2))
                        .connect_timeout(Duration::from_secs(1))
                        .build()
                    else {
                        return;
                    };

                    let _ = client
                        .post(&endpoint)
                        .json(&payload)
                        .header("User-Agent", user_agent)
                        .send()
                        .await;
                });
            })
        {
            let _ = handle.join();
        }

        self.panic_reporting.store(false, Ordering::SeqCst);
    }
}

/// Installs a panic hook which reports panics through the provided core, chaining
/// the previously installed hook.
///
/// The hook is never uninstalled (other hooks may have chained on top of it since, so
/// removing it from the middle of the chain is not safe); instead, shutdown disarms it
/// via [`AnalyticsCore::hook_armed`], turning it into a passthrough to the previous hook.
fn install_panic_hook(core: Arc<AnalyticsCore>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A panic inside a panic hook aborts the process; make sure that cannot happen.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            core.report_panic(info);
        }));

        previous(info);
    }));
}

struct AnalyticsBattery {
    core: Arc<AnalyticsCore>,
    tracker: UniqueVisitTracker,
    referrer: Option<String>,
}

impl Battery for AnalyticsBattery {
    fn record_new_page<'a>(&self, page: Page) {
        self.send_unload_beacon();

        if let Ok(mut state) = self.core.page.lock() {
            state.beacon = generate_beacon_id();
            state.path = normalize_path(&page.url);
            state.start = chrono::Utc::now();
        } else {
            tracing::warn!("Failed to acquire lock on analytics page state");
        }

        self.send_load_beacon();
    }

    fn record_event(&self, name: &str, properties: &HashMap<String, String>) {
        let (beacon, path) = self.core.current_page();

        let payload = HitPayload {
            b: beacon,
            i: Some(self.core.session_id.clone()),
            e: "custom",
            u: self.core.page_url(&path),
            r: None,
            q: false,
            p: false,
            t: self.core.timezone.clone(),
            m: None,
            n: Some(truncate_chars(name, MAX_METADATA_KEY)),
            d: (!properties.is_empty()).then(|| cap_metadata(properties)),
        };

        self.core
            .send_request(self.core.hit_endpoint.clone(), payload);
    }

    fn record_error(&self, error: &ErrorInfo) {
        let (beacon, path) = self.core.current_page();

        let payload = ExceptionPayload {
            u: self.core.page_url(&path),
            b: Some(beacon),
            i: Some(self.core.session_id.clone()),
            ty: truncate_chars(error.error_type, MAX_MESSAGE),
            m: truncate_chars(&error.message, MAX_MESSAGE),
            s: compose_stack(error),
            h: true,
            v: Some(self.core.version.clone()),
            fp: None,
            d: Some(self.core.context.clone()),
        };

        self.core
            .send_request(self.core.exception_endpoint.clone(), payload);
    }

    fn shutdown(&mut self) {
        self.send_unload_beacon();

        // Disarm the panic hook so it no longer reports after the session has ended
        self.core.hook_armed.store(false, Ordering::Relaxed);

        // Wait for all outstanding requests to complete
        self.core
            .wait_for_outstanding_requests(Duration::from_secs(5));
    }
}

impl AnalyticsBattery {
    fn send_load_beacon(&self) {
        let (beacon, path) = self.core.current_page();
        let (unique_visit, unique_page) = self.tracker.record_page_visit(&path);

        let payload = HitPayload {
            b: beacon,
            i: Some(self.core.session_id.clone()),
            e: "load",
            u: self.core.page_url(&path),
            r: self.referrer.clone(),
            q: unique_visit,
            p: unique_page,
            t: self.core.timezone.clone(),
            m: None,
            n: None,
            d: Some(self.core.context.clone()),
        };

        self.core
            .send_request(self.core.hit_endpoint.clone(), payload);
    }

    fn send_unload_beacon(&self) {
        let (beacon, path, start) = match self.core.page.lock() {
            Ok(page) => (page.beacon.clone(), page.path.clone(), page.start),
            Err(_) => {
                tracing::warn!("Failed to acquire lock on analytics page state");
                return;
            }
        };

        let duration = chrono::Utc::now()
            .signed_duration_since(start)
            .num_milliseconds();

        let payload = HitPayload {
            b: beacon,
            i: Some(self.core.session_id.clone()),
            e: "unload",
            u: self.core.page_url(&path),
            r: None,
            q: false,
            p: false,
            t: self.core.timezone.clone(),
            m: Some(duration.max(0)),
            n: None,
            d: None,
        };

        self.core
            .send_request(self.core.hit_endpoint.clone(), payload);
    }
}

/// Composes the exception "stack" text from an [`ErrorInfo`]: the cause chain first
/// (so the server's top-of-stack fingerprinting sees error-specific lines rather than
/// `record_error` plumbing frames), followed by the backtrace when one was captured.
fn compose_stack(error: &ErrorInfo) -> Option<String> {
    let mut stack = String::new();
    for cause in &error.causes {
        stack.push_str("caused by: ");
        stack.push_str(cause);
        stack.push('\n');
    }

    if let Some(backtrace) = error.backtrace_text() {
        if !stack.is_empty() {
            stack.push('\n');
        }
        stack.push_str(&backtrace);
    }

    (!stack.is_empty()).then(|| truncate_chars(&stack, MAX_STACK))
}

/// Caps custom event metadata to the server's ingest limits, keeping the retained
/// subset deterministic by sorting keys first (as the server itself does).
fn cap_metadata(properties: &HashMap<String, String>) -> BTreeMap<String, String> {
    let sorted: BTreeMap<String, String> = properties
        .iter()
        .map(|(key, value)| {
            (
                truncate_chars(key, MAX_METADATA_KEY),
                truncate_chars(value, MAX_METADATA_VALUE),
            )
        })
        .collect();

    sorted.into_iter().take(MAX_METADATA_ENTRIES).collect()
}

/// Truncates a string to at most `max_bytes` bytes without splitting a character.
fn truncate_chars(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }

    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }

    value[..end].to_string()
}

/// Normalizes a page path so it always begins with a `/`.
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        "/".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// Generates a short random id (base36 timestamp + random suffix), used for both
/// per-page-view beacon ids and the per-run session id — the same shape the browser
/// tracker produces.
fn generate_beacon_id() -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let uniqueness: u64 = random();

    format!("{}{}", radix(timestamp, 36), radix(uniqueness, 36))
}

/// Generates a product-token User-Agent (e.g. `myapp/1.2.3 (Windows NT 10.0; x86_64)`)
/// which the analytics server classifies as an application client, with the OS derived
/// from the parenthesized platform comment.
fn generate_user_agent(service: &str, version: &str) -> String {
    let os_info = match OS {
        "windows" => "Windows NT 10.0",
        "macos" => "Macintosh",
        "ios" => "iOS",
        "android" => "Android",
        "linux" => "Linux",
        other => other,
    };

    format!("{service}/{version} ({os_info}; {ARCH})")
}

/// Tracks which pages have been visited today, backing the analytics server's
/// `unique_visit` (first visit to the service today) and `unique_page` (first view
/// of a given page today) flags, which the server trusts as sent.
///
/// State is kept in a single temp-directory file per service: the first line holds the
/// UTC date, and each following line a truncated hash of a visited page path — no
/// identifiers and no raw paths are ever written to disk.
struct UniqueVisitTracker {
    service_name: Cow<'static, str>,
}

impl UniqueVisitTracker {
    pub fn new<S: Into<Cow<'static, str>>>(service_name: S) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }

    /// Records a visit to the given page, returning whether this is the first visit to
    /// the service today and whether this is the first view of this page today.
    pub fn record_page_visit(&self, page_path: &str) -> (bool, bool) {
        let today = chrono::Utc::now().date_naive().to_string();
        let page_hash = Self::page_hash(page_path);
        let state_file = self.get_state_file();

        let mut unique_visit = true;
        let mut unique_page = true;
        let mut page_hashes: Vec<String> = Vec::new();

        if let Ok(contents) = std::fs::read_to_string(&state_file) {
            let mut lines = contents.lines();
            if lines.next() == Some(today.as_str()) {
                unique_visit = false;
                page_hashes = lines
                    .filter(|line| !line.is_empty())
                    .map(|line| line.to_string())
                    .collect();
                unique_page = !page_hashes.contains(&page_hash);
            }
        }

        if unique_page {
            page_hashes.push(page_hash);
        }

        let mut contents = today;
        for hash in &page_hashes {
            contents.push('\n');
            contents.push_str(hash);
        }

        if let Err(err) = std::fs::write(&state_file, contents) {
            tracing::warn!("Failed to record analytics visit state: {}", err);
        }

        (unique_visit, unique_page)
    }

    #[cfg(test)]
    pub fn reset(&self) {
        let state_file = self.get_state_file();
        _ = std::fs::remove_file(state_file);
    }

    fn page_hash(page_path: &str) -> String {
        let mut hasher = sha2::Sha256::new();
        hasher.update(page_path.as_bytes());
        let result = hasher.finalize();
        hex::encode(&result[..8])
    }

    fn get_state_file(&self) -> std::path::PathBuf {
        // Generates a truncated sha256 hash of the service name
        let mut hasher = sha2::Sha256::new();
        hasher.update(self.service_name.as_bytes());
        let result = hasher.finalize();
        let file_name = hex::encode(&result[..8]);

        std::env::temp_dir().join(format!("analytics-daily-{file_name}"))
    }
}

/// The wire format for `POST {server}/track/hit`, mirroring the analytics server's
/// `TrackEvent` DTO. Field names are the single-letter wire keys.
#[derive(serde::Serialize)]
struct HitPayload {
    // The beacon ID linking the events of a single page view
    b: String,
    // The session ID linking every event of this application run
    #[serde(skip_serializing_if = "Option::is_none")]
    i: Option<String>,
    // The event kind: "load", "unload" or "custom"
    e: &'static str,
    // The full URL of the page being tracked (required on every kind)
    u: String,
    // The referrer URL
    #[serde(skip_serializing_if = "Option::is_none")]
    r: Option<String>,
    // Whether this is the first visit to the service today
    q: bool,
    // Whether this is the first view of this page today
    p: bool,
    // The IANA timezone (used for country detection)
    #[serde(skip_serializing_if = "Option::is_none")]
    t: Option<String>,
    // The time spent on the page in milliseconds (sent on unload)
    #[serde(skip_serializing_if = "Option::is_none")]
    m: Option<i64>,
    // The custom event name (sent on custom events)
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<String>,
    // Additional metadata to attach to the event
    #[serde(skip_serializing_if = "Option::is_none")]
    d: Option<BTreeMap<String, String>>,
}

/// The wire format for `POST {server}/track/exception`, mirroring the analytics
/// server's `ExceptionReport` DTO. Field names are the single-letter wire keys.
#[derive(serde::Serialize)]
struct ExceptionPayload {
    // The URL of the page the exception occurred on (attributes it to a source)
    u: String,
    // The beacon ID linking the exception to a page view
    #[serde(skip_serializing_if = "Option::is_none")]
    b: Option<String>,
    // The session ID linking the exception to this application run's session
    // trace (same `i` key as on hits; `s` is taken by the stack here)
    #[serde(skip_serializing_if = "Option::is_none")]
    i: Option<String>,
    // The exception type name
    ty: String,
    // The exception message
    m: String,
    // The stack trace text
    #[serde(skip_serializing_if = "Option::is_none")]
    s: Option<String>,
    // Whether the exception was handled (true) or unhandled (false)
    h: bool,
    // The reporting application's version
    #[serde(skip_serializing_if = "Option::is_none")]
    v: Option<String>,
    // An optional grouping fingerprint override
    #[serde(skip_serializing_if = "Option::is_none")]
    fp: Option<String>,
    // Additional metadata to attach to the exception
    #[serde(skip_serializing_if = "Option::is_none")]
    d: Option<BTreeMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[tokio::test]
    async fn analytics_setup() {
        let session = Session::new("example", "0.0.1").with_battery(
            Analytics::new("http://127.0.0.1:9")
                .with_initial_page("/home")
                .with_referrer("https://example.com")
                .with_panic_capture(false),
        );

        {
            let _page = session.record_new_page("/test");
            session.record_event(
                "example-event",
                [("key".to_string(), "value".to_string())].into(),
            );
            session.record_error(&std::io::Error::other("example error"));
        }

        session.shutdown();
    }

    #[test]
    fn test_unique_visit_tracker() {
        let tracker = UniqueVisitTracker::new("test_analytics_service");
        let tracker2 = UniqueVisitTracker::new("test_analytics_service");
        let tracker3 = UniqueVisitTracker::new("another_analytics_service");
        tracker.reset();
        tracker3.reset();

        assert_eq!(
            tracker.record_page_visit("/"),
            (true, true),
            "the first visit of the day should be unique for both the site and the page"
        );
        assert_eq!(
            tracker.record_page_visit("/"),
            (false, false),
            "a repeated page visit should not be unique"
        );
        assert_eq!(
            tracker.record_page_visit("/other"),
            (false, true),
            "a new page on the same day should only be page-unique"
        );
        assert_eq!(
            tracker2.record_page_visit("/other"),
            (false, false),
            "the state should propagate across trackers for the same service"
        );
        assert_eq!(
            tracker3.record_page_visit("/"),
            (true, true),
            "the state should not propagate across different services"
        );
    }

    #[test]
    fn hit_payload_wire_format() {
        let payload = HitPayload {
            b: "beacon123".to_string(),
            i: Some("session123".to_string()),
            e: "load",
            u: "https://example.app/home?utm_campaign=1.0.0".to_string(),
            r: None,
            q: true,
            p: false,
            t: Some("Europe/Vienna".to_string()),
            m: None,
            n: None,
            d: None,
        };

        let value = serde_json::to_value(&payload).expect("payload should serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "b": "beacon123",
                "i": "session123",
                "e": "load",
                "u": "https://example.app/home?utm_campaign=1.0.0",
                "q": true,
                "p": false,
                "t": "Europe/Vienna",
            })
        );

        let payload = HitPayload {
            b: "beacon123".to_string(),
            i: Some("session123".to_string()),
            e: "custom",
            u: "https://example.app/".to_string(),
            r: None,
            q: false,
            p: false,
            t: None,
            m: None,
            n: Some("signup".to_string()),
            d: Some([("plan".to_string(), "pro".to_string())].into()),
        };

        let value = serde_json::to_value(&payload).expect("payload should serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "b": "beacon123",
                "i": "session123",
                "e": "custom",
                "u": "https://example.app/",
                "q": false,
                "p": false,
                "n": "signup",
                "d": { "plan": "pro" },
            })
        );
    }

    #[test]
    fn exception_payload_wire_format() {
        let payload = ExceptionPayload {
            u: "https://example.app/".to_string(),
            b: Some("beacon123".to_string()),
            i: Some("session123".to_string()),
            ty: "std::io::Error".to_string(),
            m: "file not found".to_string(),
            s: None,
            h: true,
            v: Some("1.0.0".to_string()),
            fp: None,
            d: None,
        };

        let value = serde_json::to_value(&payload).expect("payload should serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "u": "https://example.app/",
                "b": "beacon123",
                "i": "session123",
                "ty": "std::io::Error",
                "m": "file not found",
                "h": true,
                "v": "1.0.0",
            })
        );
    }

    #[test]
    fn truncate_chars_respects_char_boundaries() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel");

        // 'é' is two bytes in UTF-8; truncating mid-character must back off cleanly.
        assert_eq!(truncate_chars("ééé", 3), "é");
        assert_eq!(truncate_chars("ééé", 4), "éé");
    }

    #[test]
    fn normalize_path_adds_leading_slash() {
        assert_eq!(normalize_path("/home"), "/home");
        assert_eq!(normalize_path("home"), "/home");
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("  "), "/");
    }

    /// Serializes the panic-hook tests: the hook is process-global, so running them
    /// concurrently would let one test's panic fire the other test's armed hook.
    static PANIC_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn panic_hook_reports_exception() {
        let _guard = PANIC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // A real TCP listener: connections queue on the backlog, so the request can be
        // accepted and inspected after the panic hook's bounded delivery has completed.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let address = listener.local_addr().expect("listener address");

        let session = Session::new("test_panic_reporting", "0.0.1")
            .with_battery(Analytics::new(format!("http://{address}")));

        let result = std::thread::spawn(|| panic!("intentional reported panic")).join();
        assert!(
            result.is_err(),
            "the panicking thread should report an error"
        );

        listener
            .set_nonblocking(true)
            .expect("listener should support non-blocking accepts");

        // The setup load beacon also targets this listener, so scan the queued
        // connections for the exception report rather than assuming an order.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut exception_request = None;
        while exception_request.is_none() && std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    let request = read_request(stream);
                    if request.contains("POST /track/exception") {
                        exception_request = Some(request);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => panic!("failed to accept test connection: {e}"),
            }
        }

        let request = exception_request.expect("the panic hook should deliver an exception");
        assert!(
            request.contains("intentional reported panic"),
            "the exception should carry the panic message: {request}"
        );
        assert!(
            request.contains("\"ty\":\"panic\""),
            "the exception should be typed as a panic: {request}"
        );
        assert!(
            request.contains("\"h\":false"),
            "a panic should be reported as unhandled: {request}"
        );

        session.shutdown();
        let _ = std::panic::take_hook();
    }

    /// Reads whatever the client wrote to the socket (the sender has already timed out
    /// and closed by the time this runs, so reads terminate quickly).
    fn read_request(mut stream: std::net::TcpStream) -> String {
        use std::io::Read;

        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .expect("read timeout should be configurable");

        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => request.extend_from_slice(&buffer[..count]),
                Err(_) => break,
            }
        }

        String::from_utf8_lossy(&request).into_owned()
    }

    #[test]
    fn panic_hook_chains_previous_hook() {
        let _guard = PANIC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let marker = Arc::new(AtomicBool::new(false));
        let marker_ref = marker.clone();
        std::panic::set_hook(Box::new(move |_| {
            marker_ref.store(true, Ordering::Relaxed);
        }));

        let session = Session::new("test_panic_service", "0.0.1")
            .with_battery(Analytics::new("http://127.0.0.1:9"));

        // Disable the session so the hook short-circuits before performing any network
        // delivery, keeping this test fast and offline.
        session.enable().store(false, Ordering::Relaxed);

        let result = std::thread::spawn(|| panic!("intentional test panic")).join();
        assert!(
            result.is_err(),
            "the panicking thread should report an error"
        );
        assert!(
            marker.load(Ordering::Relaxed),
            "the previously installed panic hook should still be invoked"
        );

        session.shutdown();

        // Restore the default hook so later tests aren't affected by the marker hook.
        let _ = std::panic::take_hook();
    }
}
