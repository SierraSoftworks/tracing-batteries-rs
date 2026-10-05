use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use pyroscope::{
    PyroscopeAgent,
    backend::{Backend, BackendImpl, BackendUninitialized, ReportBatch, ReportData, ThreadTag},
    pyroscope::{PyroscopeAgentBuilder, PyroscopeAgentRunning},
};

use crate::{Battery, BatteryBuilder, Metadata, lock_ignoring_poison};
pub use pyroscope::backend::BackendConfig as PyroscopeBackendConfig;

const SPY_NAME: &str = "pyroscope-rs";
const DEFAULT_SAMPLE_RATE: u32 = 100;

/// A profiler which can be attached to the [`Pyroscope`] integration through
/// [`Pyroscope::with_backend`], with each backend producing its own profile type.
///
/// The backends shipped with this library are each gated behind a feature flag:
/// [`PyroscopePprof`] (`pyroscope-pprof`) and [`PyroscopeJemalloc`] (`pyroscope-jemalloc`).
/// The trait is also implemented for the upstream
/// [`BackendImpl`](pyroscope::backend::BackendImpl), so that any other
/// [`pyroscope`] backend can be attached directly.
pub trait PyroscopeBackend {
    /// The frequency (in Hz) at which this backend samples, used to scale the reported profile.
    fn sample_rate(&self) -> u32 {
        DEFAULT_SAMPLE_RATE
    }

    /// Constructs the underlying [`pyroscope`] backend.
    fn build(self: Box<Self>) -> BackendImpl<BackendUninitialized>;
}

impl PyroscopeBackend for BackendImpl<BackendUninitialized> {
    fn build(self: Box<Self>) -> BackendImpl<BackendUninitialized> {
        *self
    }
}

/// A CPU profiling backend for the [`Pyroscope`] integration, which samples the stacks of the
/// process' threads using `pprof-rs`.
///
/// <div class="warning">
///
/// This backend requires the `pyroscope-pprof` feature to be enabled, and is only supported
/// on Linux and macOS (`x86_64` and `aarch64`).
///
/// </div>
///
/// ## Example
/// ```no_run
/// use tracing_batteries::{Session, Pyroscope, PyroscopePprof};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///   .with_battery(Pyroscope::new("http://localhost:4040")
///     .with_backend(PyroscopePprof::new().with_sample_rate(50)));
///
/// session.shutdown();
/// ```
#[cfg(feature = "pyroscope-pprof")]
#[derive(Debug, Clone, Copy)]
pub struct PyroscopePprof {
    sample_rate: u32,
    config: PyroscopeBackendConfig,
}

#[cfg(feature = "pyroscope-pprof")]
impl Default for PyroscopePprof {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "pyroscope-pprof")]
impl PyroscopePprof {
    /// Creates a CPU profiling backend which samples at 100Hz.
    pub fn new() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            config: PyroscopeBackendConfig::default(),
        }
    }

    /// Configures the frequency (in Hz) at which stacks are sampled.
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Configures which thread and process identifiers are attached to the sampled stacks.
    pub fn with_config(mut self, config: PyroscopeBackendConfig) -> Self {
        self.config = config;
        self
    }
}

#[cfg(feature = "pyroscope-pprof")]
impl PyroscopeBackend for PyroscopePprof {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn build(self: Box<Self>) -> BackendImpl<BackendUninitialized> {
        pyroscope::backend::pprof_backend(
            pyroscope::backend::PprofConfig {
                sample_rate: self.sample_rate,
            },
            self.config,
        )
    }
}

/// A memory profiling backend for the [`Pyroscope`] integration, which reports the heap
/// profiles gathered by jemalloc.
///
/// <div class="warning">
///
/// This backend requires the `pyroscope-jemalloc` feature to be enabled. Your application must
/// use jemalloc (`tikv-jemallocator` with its `profiling` feature) as its global allocator, with
/// profiling activated through its `malloc_conf` (for example
/// `_RJEM_MALLOC_CONF=prof:true,prof_active:true,lg_prof_sample:19`). If profiling is not
/// active, a warning is logged and this backend is skipped.
///
/// </div>
///
/// ## Example
/// ```no_run
/// use tracing_batteries::{Session, Pyroscope, PyroscopeJemalloc};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///   .with_battery(Pyroscope::new("http://localhost:4040")
///     .with_backend(PyroscopeJemalloc));
///
/// session.shutdown();
/// ```
#[cfg(feature = "pyroscope-jemalloc")]
#[derive(Debug, Clone, Copy, Default)]
pub struct PyroscopeJemalloc;

#[cfg(feature = "pyroscope-jemalloc")]
impl PyroscopeBackend for PyroscopeJemalloc {
    fn build(self: Box<Self>) -> BackendImpl<BackendUninitialized> {
        pyroscope::backend::jemalloc_backend()
    }
}

/// A [Pyroscope](https://grafana.com/oss/pyroscope/) integration which continuously profiles
/// your application and uploads the profiles to a Pyroscope server.
///
/// <div class="warning">
///
/// This integration requires the `pyroscope` feature to be enabled, along with the feature
/// for each backend you wish to use (`pyroscope-pprof`, `pyroscope-jemalloc`).
///
/// </div>
///
/// Profiles are gathered by the backends attached through [`Pyroscope::with_backend`], each of
/// which reports its own profile type. They are labelled with the session's
/// [`Metadata`](crate::Metadata): its service name, a `service_version` tag, and a tag for each
/// context value (with any characters which are not valid in a label name replaced by `_`).
///
/// ## Environment variables
///
/// The standard Pyroscope environment variables take precedence over the values provided in
/// code, matching the behaviour of the other integrations: `PYROSCOPE_SERVER_ADDRESS`,
/// `PYROSCOPE_BASIC_AUTH_USER`, `PYROSCOPE_BASIC_AUTH_PASSWORD` and `PYROSCOPE_TENANT_ID`.
///
/// ## Disabled sessions
///
/// Profiling only starts if the session is enabled when the battery is attached (so debug
/// builds need [`Metadata::with_debug_builds`](crate::Metadata::with_debug_builds)) and the
/// server address is not empty. If the session is disabled after that point, the samples which
/// are gathered are discarded instead of being uploaded.
///
/// ## Example
/// ```no_run
/// use tracing_batteries::{Session, Pyroscope, PyroscopePprof};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///   .with_context("environment", "production")
///   .with_battery(Pyroscope::new("https://profiles-prod-001.grafana.net")
///     .with_basic_auth("123456", "your-access-token")
///     .with_backend(PyroscopePprof::new()));
///
/// session.shutdown();
/// ```
pub struct Pyroscope {
    endpoint: Cow<'static, str>,
    basic_auth: Option<(String, String)>,
    tenant_id: Option<String>,
    headers: HashMap<String, String>,
    tags: HashMap<String, String>,
    upload_interval: Option<Duration>,
    backends: Vec<Box<dyn PyroscopeBackend>>,
}

impl Pyroscope {
    /// Configures the Pyroscope integration for the provided server address.
    ///
    /// The `PYROSCOPE_SERVER_ADDRESS` environment variable takes precedence over the provided
    /// address, and an empty address disables profiling.
    pub fn new<S: Into<Cow<'static, str>>>(endpoint: S) -> Self {
        Self {
            endpoint: std::env::var("PYROSCOPE_SERVER_ADDRESS")
                .map(Cow::Owned)
                .unwrap_or_else(|_| endpoint.into()),
            basic_auth: std::env::var("PYROSCOPE_BASIC_AUTH_USER")
                .ok()
                .zip(std::env::var("PYROSCOPE_BASIC_AUTH_PASSWORD").ok()),
            tenant_id: std::env::var("PYROSCOPE_TENANT_ID").ok(),
            headers: HashMap::new(),
            tags: HashMap::new(),
            upload_interval: None,
            backends: Vec::new(),
        }
    }

    /// Attaches a profiling backend to the integration.
    ///
    /// This method may be called multiple times to gather several types of profile at once,
    /// with each backend being run by its own profiling agent.
    ///
    /// ## Example
    /// ```no_run
    /// # #[cfg(all(feature = "pyroscope-pprof", feature = "pyroscope-jemalloc"))]
    /// # {
    /// use tracing_batteries::{Pyroscope, PyroscopeJemalloc, PyroscopePprof};
    ///
    /// Pyroscope::new("http://localhost:4040")
    ///   .with_backend(PyroscopePprof::new())
    ///   .with_backend(PyroscopeJemalloc);
    /// # }
    /// ```
    pub fn with_backend<B: PyroscopeBackend + 'static>(mut self, backend: B) -> Self {
        self.backends.push(Box::new(backend));
        self
    }

    /// Configures the credentials used to authenticate with the Pyroscope server (for Grafana
    /// Cloud, these are your stack's user ID and an access token).
    ///
    /// The `PYROSCOPE_BASIC_AUTH_USER` and `PYROSCOPE_BASIC_AUTH_PASSWORD` environment variables
    /// take precedence over these values when both are set.
    pub fn with_basic_auth<U: Into<String>, P: Into<String>>(
        mut self,
        username: U,
        password: P,
    ) -> Self {
        self.basic_auth
            .get_or_insert_with(|| (username.into(), password.into()));
        self
    }

    /// Configures the tenant that profiles are reported for on a multi-tenant Pyroscope server.
    ///
    /// The `PYROSCOPE_TENANT_ID` environment variable takes precedence over this value.
    pub fn with_tenant_id<S: Into<String>>(mut self, tenant_id: S) -> Self {
        self.tenant_id.get_or_insert_with(|| tenant_id.into());
        self
    }

    /// Adds a header to the requests made to the Pyroscope server.
    pub fn with_header<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Adds a tag to every profile which is reported, taking precedence over any tag derived
    /// from the session's metadata.
    pub fn with_tag<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.tags.insert(key.into(), value.into());
        self
    }

    /// Configures how often profiles are uploaded to the Pyroscope server (10 seconds by default).
    pub fn with_upload_interval(mut self, interval: Duration) -> Self {
        self.upload_interval = Some(interval);
        self
    }

    fn build_tags(&self, metadata: &Metadata) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert("service_version".to_string(), metadata.version.to_string());
        for (key, value) in &metadata.context {
            tags.insert(sanitize_label_name(key), value.to_string());
        }
        tags.extend(self.tags.clone());
        tags
    }

    fn start_agent(
        &self,
        metadata: &Metadata,
        backend: Box<dyn PyroscopeBackend>,
        tags: &HashMap<String, String>,
        enabled: Arc<AtomicBool>,
    ) -> pyroscope::Result<PyroscopeAgent<PyroscopeAgentRunning>> {
        let sample_rate = backend.sample_rate();
        let mut builder = PyroscopeAgentBuilder::new(
            self.endpoint.as_ref(),
            metadata.service.as_ref(),
            sample_rate,
            SPY_NAME,
            env!("CARGO_PKG_VERSION"),
            GuardedBackend::wrap(backend.build(), enabled)?,
        )
        .tags(
            tags.iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect(),
        )
        .http_headers(self.headers.clone());

        if let Some((username, password)) = &self.basic_auth {
            builder = builder.basic_auth(username, password);
        }

        if let Some(tenant_id) = &self.tenant_id {
            builder = builder.tenant_id(tenant_id.clone());
        }

        if let Some(interval) = self.upload_interval {
            builder = builder.upload_interval(interval);
        }

        builder.build()?.start()
    }
}

impl BatteryBuilder for Pyroscope {
    fn setup(mut self, metadata: &Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        let backends = std::mem::take(&mut self.backends);
        let mut agents = Vec::new();

        if self.endpoint.is_empty() || !enabled.load(Ordering::Relaxed) {
            tracing::debug!("Pyroscope profiling is disabled for this session.");
        } else if backends.is_empty() {
            tracing::warn!(
                "No Pyroscope backends were configured, so no profiles will be gathered."
            );
        } else {
            let tags = self.build_tags(metadata);
            for backend in backends {
                match self.start_agent(metadata, backend, &tags, enabled.clone()) {
                    Ok(agent) => agents.push(agent),
                    Err(error) => {
                        tracing::warn!(%error, "Failed to start a Pyroscope profiling backend.")
                    }
                }
            }
        }

        Box::new(PyroscopeBattery {
            agents: Mutex::new(agents),
        })
    }
}

struct PyroscopeBattery {
    agents: Mutex<Vec<PyroscopeAgent<PyroscopeAgentRunning>>>,
}

impl Battery for PyroscopeBattery {
    fn shutdown(&mut self) {
        // Stopping an agent uploads the samples gathered since its last report.
        for agent in lock_ignoring_poison(&self.agents).drain(..) {
            match agent.stop() {
                Ok(agent) => agent.shutdown(),
                Err(error) => {
                    tracing::warn!(%error, "Failed to stop a Pyroscope profiling backend.")
                }
            }
        }
    }
}

/// Replaces any character which is not permitted in a Pyroscope label name with an underscore.
fn sanitize_label_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Wraps a backend so that its samples are discarded while the session is disabled, and so that
/// a failed report is skipped. The agent's reporting thread exits on the first error a backend
/// returns, which silently ends profiling and then blocks forever when the agent is stopped.
struct GuardedBackend {
    inner: Box<dyn Backend>,
    enabled: Arc<AtomicBool>,
    profile_type: String,
}

impl GuardedBackend {
    fn wrap(
        backend: BackendImpl<BackendUninitialized>,
        enabled: Arc<AtomicBool>,
    ) -> pyroscope::Result<BackendImpl<BackendUninitialized>> {
        let inner = backend
            .backend
            .lock()?
            .take()
            .ok_or(pyroscope::PyroscopeError::BackendImpl)?;

        Ok(BackendImpl::new(Box::new(Self {
            inner,
            enabled,
            profile_type: "process_cpu".into(),
        })))
    }
}

impl Backend for GuardedBackend {
    fn initialize(&mut self) -> pyroscope::Result<()> {
        self.inner.initialize()
    }

    fn shutdown(self: Box<Self>) -> pyroscope::Result<()> {
        self.inner.shutdown()
    }

    fn report(&mut self) -> pyroscope::Result<ReportBatch> {
        // The report is always taken so that the backend's sample buffer is drained.
        match self.inner.report() {
            Ok(batch) if self.enabled.load(Ordering::Relaxed) => {
                self.profile_type.clone_from(&batch.profile_type);
                return Ok(batch);
            }
            Ok(batch) => self.profile_type = batch.profile_type,
            Err(error) => {
                tracing::warn!(%error, "Failed to gather a Pyroscope profile.")
            }
        }

        Ok(ReportBatch {
            profile_type: self.profile_type.clone(),
            data: ReportData::Reports(Vec::new()),
        })
    }

    fn add_tag(&self, tag: ThreadTag) -> pyroscope::Result<()> {
        self.inner.add_tag(tag)
    }

    fn remove_tag(&self, tag: ThreadTag) -> pyroscope::Result<()> {
        self.inner.remove_tag(tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;

    #[test]
    fn tags_are_derived_from_metadata() {
        let metadata = Session::new("example", "1.2.3")
            .with_context("deployment.environment", "production")
            .with_context("region", "eu");

        let tags = Pyroscope::new("http://localhost:4040")
            .with_tag("region", "us")
            .build_tags(&metadata);

        assert_eq!(
            tags.get("service_version").map(String::as_str),
            Some("1.2.3")
        );
        assert_eq!(
            tags.get("deployment_environment").map(String::as_str),
            Some("production"),
            "context keys should be converted into valid label names"
        );
        assert_eq!(
            tags.get("region").map(String::as_str),
            Some("us"),
            "explicit tags should take precedence over the session's context"
        );
    }
}
