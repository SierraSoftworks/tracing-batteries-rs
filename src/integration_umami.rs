use crate::{Battery, BatteryBuilder, Metadata, lock_ignoring_poison};
use crate::{Page, prelude::*};
use radix_fmt::radix;
use rand::random;
use sha2::Digest;
use std::borrow::Cow;
use std::collections::HashMap;
use std::env::consts::{ARCH, OS};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::RwLock;

/// A [Umami](https://umami.is/) integration for this library, allowing you to report application usage
/// in a privacy preserving way.
///
/// <div class="warning">
///
/// This integration requires the `umami` feature to be enabled.
///
/// </div>
///
/// The Umami integration can be initialized by providing the URL of an Umami server, as well as a "website ID"
/// which will receive the analytics data. Telemetry will be sent as if it originated on a page with the URL
/// `https://{service.name}.app`.
///
/// ## Example
/// ```no_run
/// use tracing_batteries::{Session, Umami};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///     .with_battery(Umami::new("https://umami.example.com", "your-website-id"));
///
/// session.shutdown();
/// ```
pub struct Umami {
    server: Cow<'static, str>,
    website_id: Cow<'static, str>,

    initial_page: Option<Page>,
    referrer: Cow<'static, str>,
}

impl Umami {
    /// Configures the Umami integration with the given server URL and website ID.
    ///
    /// This method is used to configur the endpoint and website ID used by the Umami integration.
    /// The `server` parameter should be the URL of the Umami server which will receive the analytics data,
    /// and the `website_id` parameter should be the ID of the website under which it will be tracked.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Umami};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///     .with_battery(Umami::new("https://umami.example.com", "your-website-id"));
    ///
    /// session.shutdown();
    /// ```
    pub fn new(
        server: impl Into<Cow<'static, str>>,
        website_id: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            server: server.into(),
            website_id: website_id.into(),

            initial_page: Some(Page::default()),
            referrer: Cow::Borrowed(""),
        }
    }

    /// Configures the initial page URL used by the Umami integration.
    ///
    /// This method can be used to set the initial page URL that will be sent with the first event. By default, this is "/".
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Umami};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///     .with_battery(Umami::new("https://umami.example.com", "your-website-id").with_initial_page("/home"));
    ///
    /// session.shutdown();
    /// ```
    pub fn with_initial_page(mut self, page: impl Into<Page>) -> Self {
        self.initial_page = Some(page.into());
        self
    }

    /// Configures the Umami integration to not report an initial page view.
    ///
    /// By default, the integration reports a page view for `/` (or the page configured
    /// via [`with_initial_page`](Umami::with_initial_page)) as soon as the session
    /// starts. Call this method if you would rather defer the first page view until you
    /// explicitly call [`Session::record_new_page`].
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Umami};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///     .with_battery(Umami::new("https://umami.example.com", "your-website-id").without_initial_page());
    ///
    /// session.shutdown();
    /// ```
    pub fn without_initial_page(mut self) -> Self {
        self.initial_page = None;
        self
    }

    /// Configures the referrer used by the Umami integration.
    ///
    /// This method can be used to set the referrer URL that will be sent with each event. By default, this is an empty string.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Umami};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///     .with_battery(Umami::new("https://umami.example.com", "your-website-id").with_referrer("https://example.com"));
    ///
    /// session.shutdown();
    /// ```
    pub fn with_referrer(mut self, referrer: impl Into<Cow<'static, str>>) -> Self {
        self.referrer = referrer.into();
        self
    }
}

impl BatteryBuilder for Umami {
    fn setup(self, metadata: &Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        let unique_user = UniqueUserTracker::new(metadata.service.clone());
        let initial_page = self.initial_page;

        let battery = UmamiBattery {
            server: self.server,
            website_id: self.website_id,

            metadata: metadata.clone(),

            page: Mutex::new(initial_page.clone().unwrap_or_default()),
            referrer: Mutex::new("".into()),

            cache: Arc::new(RwLock::new(None)),
            disabled: Arc::new(AtomicBool::new(false)),
            identity: Mutex::new(unique_user.identifier()),
            is_enabled: enabled,
            outstanding_requests: Arc::new(AtomicUsize::new(0)),
            client: Arc::new(reqwest::Client::new()),
        };

        // Report the initial page view, unless the initial page has been disabled.
        if let Some(page) = initial_page {
            battery.record_new_page(page);
        }

        Box::new(battery)
    }
}

struct UmamiBattery {
    // Configuration from battery builder
    server: Cow<'static, str>,
    website_id: Cow<'static, str>,

    // Configuration from metadata
    metadata: Metadata,

    // Internal state tracking (locked rather than RefCell so the battery is `Sync`)
    page: Mutex<Page>,
    referrer: Mutex<Cow<'static, str>>,
    cache: Arc<RwLock<Option<String>>>,
    disabled: Arc<AtomicBool>,
    identity: Mutex<String>,

    // Request management
    is_enabled: Arc<AtomicBool>,
    outstanding_requests: Arc<AtomicUsize>,
    client: Arc<reqwest::Client>,
}

impl Battery for UmamiBattery {
    fn record_new_page<'a>(&self, page: Page) {
        *lock_ignoring_poison(&self.page) = page.clone();
        *lock_ignoring_poison(&self.referrer) = page.title.clone().unwrap_or_default();

        let payload = self.build_payload();
        self.send_request(UmamiEvent::event(payload));
    }

    fn record_event(&self, name: &str, properties: &HashMap<String, String>) {
        let payload = self.build_payload().with_event(name, properties.clone());
        self.send_request(UmamiEvent::event(payload));
    }

    fn record_error(&self, error: &crate::ErrorInfo) {
        // Umami does not have a built-in concept of "errors", so we will just record them as events with the error message as the event name
        let mut metadata = HashMap::from([
            ("error_type".to_string(), error.error_type.to_string()),
            ("message".to_string(), error.message.clone()),
            ("causes".to_string(), error.causes.join(" -> ")),
        ]);
        metadata.extend(
            error
                .metadata
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone())),
        );

        let payload = self.build_payload().with_event("error", metadata);

        self.send_request(UmamiEvent::event(payload));
    }

    fn shutdown(&mut self) {
        // Wait for all outstanding requests to complete
        self.wait_for_outstanding_requests(Duration::from_secs(5));
    }
}

impl UmamiBattery {
    fn build_payload(&self) -> UmamiEventPayload {
        let page = lock_ignoring_poison(&self.page).clone();
        let referrer = lock_ignoring_poison(&self.referrer).to_string();
        UmamiEventPayload {
            website: self.website_id.to_string(),
            hostname: format!("{}.app", self.metadata.service),
            screen: "0x0".to_string(), // Screen resolution is not applicable in this context
            id: Some(lock_ignoring_poison(&self.identity).clone()),
            language: sys_locale::get_locale().unwrap_or_else(|| "en".to_string()),
            url: page.url.clone().to_string(),
            referrer,
            title: page.title.clone().unwrap_or_default().to_string(),
            tag: self.metadata.version.to_string(),
            name: None,
            data: None,
        }
    }

    fn generate_user_agent(service: &str, version: &str) -> String {
        let os_info = match (OS, ARCH) {
            ("macos", "x86_64") => "Macintosh; Intel Mac OS X",
            ("macos", "aarch64") => "Macintosh; Apple Mac OS X",
            ("windows", "x86_64") => "Windows NT; 10.0; Win64; x64",
            ("windows", "aarch64") => "Windows NT; 10.0; ARM64",
            ("linux", arch) => &format!("Linux {}", arch),
            _ => "Unknown OS",
        };

        format!("Mozilla/5.0 ({os_info}) Gecko/20100101 {service}/{version}")
    }

    fn wait_for_outstanding_requests(&self, timeout: Duration) {
        // Wait for up to 5 seconds for outstanding requests to complete
        let start_time = std::time::Instant::now();

        while self.outstanding_requests.load(Ordering::Relaxed) > 0 {
            if start_time.elapsed() >= timeout {
                tracing::warn!("Timeout waiting for outstanding requests to complete");
                break;
            }

            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn send_request<P: serde::Serialize + Send + 'static>(&self, payload: P) {
        if !self.is_enabled.load(Ordering::Relaxed) {
            return;
        }

        if self.disabled.load(Ordering::Relaxed) {
            return;
        }

        // Increment the outstanding requests counter
        self.outstanding_requests.fetch_add(1, Ordering::Relaxed);

        let url = format!("{}/api/send", self.server);

        let client = self.client.clone();
        let outstanding_requests = self.outstanding_requests.clone();
        let user_agent = Self::generate_user_agent(&self.metadata.service, &self.metadata.version);
        let cache_ref = self.cache.clone();
        let disabled_ref = self.disabled.clone();

        tokio::spawn(async move {
            let mut request = client
                .post(&url)
                .json(&payload)
                .header("User-Agent", user_agent)
                .header("Content-Type", "application/json");

            if let Some(cache) = cache_ref.as_ref().read().await.clone() {
                request = request.header("x-umai-cache", cache);
            }

            let result = request.send().await;

            match result {
                Ok(response) => {
                    if !response.status().is_success() {
                        tracing::warn!("Umami request failed: {}", response.status());
                    } else {
                        if let Ok(UmamiResponse {
                            cache, disabled, ..
                        }) = response.json().await
                        {
                            let mut cache_write = cache_ref.write().await;
                            *cache_write = Some(cache);

                            if let Some(disabled) = disabled {
                                disabled_ref.store(disabled, Ordering::Relaxed);
                            }
                        }
                    }
                }
                Err(e) => {
                    // Log the error but do not crash the application
                    tracing::warn!("Error sending Umami event: {}", e);
                }
            }

            // Decrement the outstanding requests counter when done
            outstanding_requests.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

struct UniqueUserTracker {
    service_name: Cow<'static, str>,
}

impl UniqueUserTracker {
    pub fn new<S: Into<Cow<'static, str>>>(service_name: S) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }

    pub fn identifier(&self) -> String {
        let marker_file = self.get_marker_file();

        // Check if the marker file exists
        if let Ok(contents) = std::fs::read_to_string(&marker_file) {
            // If it exists, return the existing identifier
            contents.trim().to_string()
        } else {
            // If it doesn't exist, generate a new identifier and save it to the file
            let identifier = self.generate_identifier();
            if let Err(err) = std::fs::write(&marker_file, &identifier) {
                tracing::warn!(
                    "Failed to write Umami unique user identifier to file: {}",
                    err
                );
            }
            identifier
        }
    }

    #[cfg(test)]
    pub fn reset(&self) {
        let marker_file = self.get_marker_file();
        if let Err(err) = std::fs::remove_file(marker_file) {
            tracing::warn!(
                "Failed to remove Umami unique user identifier file: {}",
                err
            );
        }
    }

    fn generate_identifier(&self) -> String {
        let id: u64 = random();
        format!("{}", radix(id, 36))
    }

    fn get_marker_file(&self) -> std::path::PathBuf {
        let mut hasher = sha2::Sha256::new();
        hasher.update(self.service_name.as_ref());
        let result = hasher.finalize();
        let file_name = hex::encode(&result[..8]); // Use the first 8 bytes for uniqueness

        std::env::temp_dir().join(format!("umami-unique-user-{}", file_name))
    }
}

#[derive(serde::Serialize)]
struct UmamiEvent {
    #[serde(rename = "type")]
    _type: UmamiEventType,

    payload: UmamiEventPayload,
}

impl UmamiEvent {
    pub fn event(payload: UmamiEventPayload) -> Self {
        Self {
            _type: UmamiEventType::Event,
            payload,
        }
    }

    pub fn _identify(payload: UmamiEventPayload) -> Self {
        Self {
            _type: UmamiEventType::_Identify,
            payload,
        }
    }

    pub fn _performance(payload: UmamiEventPayload) -> Self {
        Self {
            _type: UmamiEventType::_Performance,
            payload,
        }
    }
}

#[derive(serde::Serialize)]
enum UmamiEventType {
    #[serde(rename = "event")]
    Event,
    #[serde(rename = "identify")]
    _Identify,
    #[serde(rename = "performance")]
    _Performance,
}

#[derive(serde::Serialize)]
struct UmamiEventPayload {
    // The hostname of the page (e.g. my.app)
    hostname: String,
    // The screen resolution (e.g. "1920x1080")
    screen: String,
    // The user locale (e.g. "en-US")
    language: String,
    // The page path, including query parameters (but not the origin)
    url: String,
    // The referrer URL, if any (can be an empty string)
    referrer: String,
    // Page title
    title: String,
    // Additional tag description
    tag: String,
    // The persistent user ID
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    // The website ID
    website: String,
    // The name of the event, if this is an event (not a page view)
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    // Additional data to attach to the event
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<HashMap<String, String>>,
}

impl UmamiEventPayload {
    pub fn with_event(self, name: impl Into<String>, data: HashMap<String, String>) -> Self {
        Self {
            name: Some(name.into()),
            data: Some(data),
            ..self
        }
    }
}

#[derive(serde::Deserialize)]
struct UmamiResponse {
    cache: String,
    #[serde(default)]
    disabled: Option<bool>,
    #[serde(rename = "sessionId")]
    _session_id: String,
    #[serde(rename = "visitId")]
    _visit_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[tokio::test]
    async fn umami_setup() {
        let session = Session::new("test-service", "0.1.0").with_battery(
            Umami::new("http://localhost:8000", "test-website-id")
                .with_initial_page("/home")
                .with_referrer("https://example.com"),
        );

        {
            let _page = session.record_new_page("/test");
        }
    }

    #[test]
    fn test_unique_user_tracker() {
        let tracker1 = UniqueUserTracker::new("test-service");
        let id1 = tracker1.identifier();
        assert!(
            !id1.is_empty(),
            "UniqueUserTracker should return a non-empty identifier"
        );

        let tracker2 = UniqueUserTracker::new("test-service");
        let id2 = tracker2.identifier();
        assert_eq!(
            id1, id2,
            "UniqueUserTracker should return the same identifier on multiple calls"
        );

        let tracker3 = UniqueUserTracker::new("another-service");
        let id3 = tracker3.identifier();
        assert_ne!(
            id1, id3,
            "UniqueUserTracker should return different identifiers for different services"
        );

        tracker1.reset();
        let id1_new = tracker1.identifier();
        assert_ne!(
            id1, id1_new,
            "UniqueUserTracker should return a new identifier after reset"
        );
    }
}
