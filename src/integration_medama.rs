use crate::prelude::*;
use crate::{Battery, BatteryBuilder, Metadata};
use radix_fmt::radix;
use rand::random;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env::consts::{ARCH, OS};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

/// A [Medama](https://oss.medama.io) integration which can be used to keep
/// track of application usage in a privacy preserving way.
///
/// <div class="warning">
///
/// This integration requires the `medama` feature to be enabled.
///
/// </div>
///
/// The Medama integration can be initialized by providing the URL of a
/// Medama server which will receive the analytics data. Telemetry will
/// be sent as if it originated on a page with the URL
/// `https://{service.name}.app/{service.version}`.
///
/// ## Example
/// ```no_run
// /// use tracing_batteries::{Session, Medama};
// ///
// /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
// ///   .with_battery(Medama::new("localhost:8000"));
// ///
// /// session.shutdown();
// /// ```
pub struct Medama {
    server: Cow<'static, str>,

    page: Option<Cow<'static, str>>,
    referrer: Option<Cow<'static, str>>,
}

impl Medama {
    /// Configures the Medama integration with the given server URL.
    ///
    /// This method is used to configure the endpoint for the Medama
    /// integration. The server URL should point to a Medama instance
    /// that is capable of receiving analytics data.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Medama};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///     .with_battery(Medama::new("localhost:8000"));
    ///
    /// session.shutdown();
    /// ```
    pub fn new<S: Into<Cow<'static, str>>>(server: S) -> Self {
        Self {
            server: server.into(),
            page: None,
            referrer: None,
        }
    }

    /// Configures the page URL which should be used for the initial analytics event.
    ///
    /// This method allows you to specify the page URL that will be sent to
    /// the Medama server as part of the startup analytics data. If not set, `/` will
    /// be used by default. Subsequent page views may be triggered by calling the
    /// [`Session::record_new_page`] method.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Medama};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///     .with_battery(Medama::new("localhost:8000")
    ///        .with_initial_page("/home"));
    ///
    /// session.shutdown();
    /// ```
    pub fn with_initial_page<S: Into<Cow<'static, str>>>(mut self, page: S) -> Self {
        self.page = Some(page.into());
        self
    }

    /// Configures the referrer URL which should be used for the analytics event.
    ///
    /// This method allows you to specify the referrer URL that will be sent to
    /// the Medama server as part of the analytics data. If not set, an empty string
    /// will be used by default.
    ///
    /// ## Example
    /// ```no_run
    /// use tracing_batteries::{Session, Medama};
    ///
    /// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    ///    .with_battery(Medama::new("localhost:8000")
    ///       .with_referrer("https://example.com"));
    ///
    /// session.shutdown();
    /// ```
    pub fn with_referrer<S: Into<Cow<'static, str>>>(mut self, referrer: S) -> Self {
        self.referrer = Some(referrer.into());
        self
    }
}

impl BatteryBuilder for Medama {
    fn setup(self, metadata: &Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        let battery = MedamaAnalyticsBattery {
            server: self.server,
            referrer: self.referrer.unwrap_or("".into()),

            metadata: metadata.clone(),

            beacon_id: RefCell::new(MedamaAnalyticsBattery::generate_beacon_id()),
            start_time: RefCell::new(chrono::Utc::now()),
            visited_pages: Mutex::new(HashSet::new()),

            is_enabled: enabled,
            outstanding_requests: Arc::new(AtomicUsize::new(0)),
            client: Arc::new(reqwest::Client::new()),
        };

        // Spawn the load beacon as a background task
        battery.send_load_beacon(&self.page.unwrap_or("/".into()));

        Box::new(battery)
    }
}

struct MedamaAnalyticsBattery {
    // Configurations from battery builder
    server: Cow<'static, str>,
    referrer: Cow<'static, str>,

    // Configurations from metadata
    metadata: Metadata,

    // Internal state tracking
    beacon_id: RefCell<String>,
    start_time: RefCell<chrono::DateTime<chrono::Utc>>,
    visited_pages: Mutex<HashSet<String>>,

    // Request management
    is_enabled: Arc<AtomicBool>,
    outstanding_requests: Arc<AtomicUsize>,
    client: Arc<reqwest::Client>,
}

impl Battery for MedamaAnalyticsBattery {
    fn record_new_page<'a>(&self, page: Cow<'static, str>) {
        self.send_unload_beacon();
        self.beacon_id.replace(Self::generate_beacon_id());
        self.send_load_beacon(&page);
    }

    fn record_error(&self, error: &dyn std::error::Error) {
        let mut data = HashMap::new();
        data.insert("error".to_string(), error.to_string());

        self.send_custom_event(data);
    }

    fn shutdown(&mut self) {
        // Spawn the unload beacon as a background task
        self.send_unload_beacon();

        // Wait for all outstanding requests to complete
        self.wait_for_outstanding_requests(Duration::from_secs(5));
    }
}

impl MedamaAnalyticsBattery {
    fn generate_beacon_id() -> String {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let uniqueness: u64 = random();

        format!("{}{}", radix(timestamp, 36), radix(uniqueness, 36))
    }

    fn generate_user_agent(service: &str, version: &str) -> String {
        let os_info = match (OS, ARCH) {
            ("macos", "x86_64") => "Macintosh; Intel Mac OS X",
            ("macos", "aarch64") => "Macintosh; Apple Mac OS X",
            ("windows", _) => "Windows NT",
            ("linux", _) => "X11; Linux",
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

    fn send_load_beacon(&self, page: &str) {
        let (is_unique, is_visited) = if let Ok(mut visited_pages) = self.visited_pages.lock() {
            let is_unique = visited_pages.is_empty();
            let is_visited = visited_pages.contains(page);
            visited_pages.insert(page.to_string());
            (is_unique, is_visited)
        } else {
            tracing::warn!("Failed to acquire lock on visited pages");
            (false, false)
        };

        self.start_time.replace(chrono::Utc::now());

        let mut data: HashMap<String, String> = self
            .metadata
            .context
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        data.insert(
            "service.name".to_string(),
            self.metadata.service.to_string(),
        );
        data.insert(
            "service.version".to_string(),
            self.metadata.version.to_string(),
        );

        let payload = MedamaLoadBeacon {
            b: self.beacon_id.borrow().clone(),
            e: "load",
            u: format!(
                "https://{}.app{}?utm_source={OS}&utm_medium={ARCH}&utm_campaign={}",
                self.metadata.service.to_lowercase(),
                page,
                self.metadata.version,
            ),
            r: self.referrer.clone(),
            p: is_unique,
            q: is_visited,
            t: iana_time_zone::get_timezone().unwrap_or_default(),
            d: data,
        };

        self.send_request("api/event/hit", payload);
    }

    fn send_unload_beacon(&self) {
        let duration = chrono::Utc::now()
            .signed_duration_since(*self.start_time.borrow())
            .num_milliseconds() as u64;

        let payload = MedamaUnloadBeacon {
            b: self.beacon_id.borrow().clone(),
            e: "unload",
            m: duration,
        };

        self.send_request("api/event/hit", payload);
    }

    fn send_custom_event(&self, data: HashMap<String, String>) {
        let payload = MedamaCustomEvent {
            b: self.beacon_id.borrow().clone(),
            e: "custom",
            g: format!("{}.app", self.metadata.service.to_lowercase()),
            d: data,
        };

        self.send_request("api/event/hit", payload);
    }

    fn send_request<P: serde::Serialize + Send + 'static>(&self, path: &str, payload: P) {
        if !self.is_enabled.load(Ordering::Relaxed) {
            return;
        }

        // Increment the outstanding requests counter
        self.outstanding_requests.fetch_add(1, Ordering::Relaxed);

        let url = format!("{}/{}", self.server, path);

        let client = self.client.clone();
        let outstanding_requests = self.outstanding_requests.clone();
        let user_agent = Self::generate_user_agent(&self.metadata.service, &self.metadata.version);
        tokio::spawn(async move {
            let result = client
                .post(&url)
                .json(&payload)
                .header("User-Agent", user_agent)
                .header(
                    "Accept-Language",
                    sys_locale::get_locale().unwrap_or_else(|| "en".to_string()),
                )
                .header("Content-Type", "text/plain")
                .send()
                .await;

            // Decrement the outstanding requests counter when done
            outstanding_requests.fetch_sub(1, Ordering::Relaxed);

            match result {
                Ok(response) => {
                    if !response.status().is_success() {
                        tracing::warn!("Medama request failed: {}", response.status());
                    }
                }
                Err(e) => {
                    // Log the error but do not crash the application
                    tracing::warn!("Error sending Medama event: {}", e);
                }
            }
        });
    }
}

#[derive(serde::Serialize)]
struct MedamaLoadBeacon {
    // The beacon ID for this event
    pub b: String,
    // The event type being sent
    pub e: &'static str,
    // The URL of the page being tracked
    pub u: String,
    // The referrer URL
    pub r: Cow<'static, str>,
    // Whether the user is unique or not
    pub p: bool,
    // Whether this is the user's first visit
    pub q: bool,
    // The user's timezone (used for location detection)
    pub t: String,
    // The data payload for the event
    pub d: HashMap<String, String>,
}

#[derive(serde::Serialize)]
struct MedamaUnloadBeacon {
    // The beacon ID for this event
    pub b: String,
    // The event type being sent
    pub e: &'static str,
    // The time spent on the page, in milliseconds
    pub m: u64,
}

#[derive(serde::Serialize)]
struct MedamaCustomEvent {
    // The beacon ID for this event
    pub b: String,
    // The event type being sent
    pub e: &'static str,
    // The group name for the event (the hostname of the app)
    pub g: String,
    // The data payload for the event
    pub d: HashMap<String, String>,
}

#[cfg(test)]
mod test {
    use crate::*;

    #[tokio::test]
    async fn medama_setup() {
        let session = Session::new("example", "0.0.1").with_battery(
            Medama::new("localhost:8000")
                .with_initial_page("/home")
                .with_referrer("https://example.com"),
        );

        {
            let _page = session.record_new_page("/test");
        }

        session.shutdown();
    }
}
