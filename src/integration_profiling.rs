use std::{
    borrow::Cow,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use crate::{Battery, BatteryBuilder, Metadata, OpenTelemetry, OpenTelemetryProtocol};
#[cfg(not(windows))]
use pyroscope::backend::{BackendImpl, BackendUninitialized};

const DEFAULT_SAMPLE_RATE: u32 = 100;
const DEFAULT_UPLOAD_INTERVAL: Duration = Duration::from_secs(10);

/// A profiler which can be attached to the [`Profiling`] integration through
/// [`Profiling::with_backend`].
///
/// The backends shipped with this library are each gated behind a feature flag:
/// [`ProfilingPprof`] (`profiling-pprof`) and [`ProfilingJemalloc`] (`profiling-jemalloc`).
/// Any other backend built for the [`pyroscope`] crate (which is re-exported from the
/// [`prelude`](crate::prelude)) can be attached by implementing this trait for a type which
/// constructs it.
pub trait ProfilingBackend {
    /// The frequency (in Hz) at which this backend samples, used to scale the reported profile.
    ///
    /// This must match the rate that the profiler built by [`ProfilingBackend::build`] samples
    /// at, and backends with a rate outside of 1Hz to 1MHz are skipped.
    fn sample_rate(&self) -> u32 {
        DEFAULT_SAMPLE_RATE
    }

    /// Constructs the underlying profiler, or returns [`None`] if it is not supported on the
    /// current platform. Profiling is not available on Windows, where this method does not
    /// exist.
    #[cfg(not(windows))]
    fn build(self: Box<Self>) -> Option<BackendImpl<BackendUninitialized>>;
}

/// A CPU profiling backend for the [`Profiling`] integration, which samples the stacks of the
/// process' threads using `pprof-rs`.
///
/// <div class="warning">
///
/// This backend requires the `profiling-pprof` feature to be enabled. It is supported on Linux
/// and macOS (`x86_64` and `aarch64`), and does nothing on other platforms.
///
/// </div>
///
/// ## Example
/// ```no_run
/// use tracing_batteries::{Session, Profiling, ProfilingPprof};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///   .with_battery(Profiling::new("http://localhost:4317")
///     .with_backend(ProfilingPprof::new().with_sample_rate(50)));
///
/// session.shutdown();
/// ```
#[cfg(feature = "profiling-pprof")]
#[derive(Debug, Clone, Copy)]
pub struct ProfilingPprof {
    sample_rate: u32,
}

#[cfg(feature = "profiling-pprof")]
impl Default for ProfilingPprof {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "profiling-pprof")]
impl ProfilingPprof {
    /// Creates a CPU profiling backend which samples at 100Hz.
    pub fn new() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
        }
    }

    /// Configures the frequency (in Hz) at which stacks are sampled.
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }
}

#[cfg(feature = "profiling-pprof")]
impl ProfilingBackend for ProfilingPprof {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    // This must match the targets for which the pprof-rs backend is enabled in `Cargo.toml`.
    #[cfg(all(
        any(target_os = "linux", target_os = "macos"),
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    fn build(self: Box<Self>) -> Option<BackendImpl<BackendUninitialized>> {
        Some(pyroscope::backend::pprof_backend(
            pyroscope::backend::PprofConfig {
                sample_rate: self.sample_rate,
            },
            Default::default(),
        ))
    }

    #[cfg(all(
        not(windows),
        not(all(
            any(target_os = "linux", target_os = "macos"),
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))
    ))]
    fn build(self: Box<Self>) -> Option<BackendImpl<BackendUninitialized>> {
        tracing::debug!("CPU profiling is not supported on this platform.");
        None
    }
}

/// A memory profiling backend for the [`Profiling`] integration, which reports the heap
/// profiles gathered by jemalloc.
///
/// <div class="warning">
///
/// This backend requires the `profiling-jemalloc` feature to be enabled. Your application must
/// use jemalloc (`tikv-jemallocator` with its `profiling` feature) as its global allocator, with
/// profiling activated through its `malloc_conf` (for example
/// `_RJEM_MALLOC_CONF=prof:true,prof_active:true,lg_prof_sample:19`). If profiling is not
/// active, a warning is logged and this backend is skipped. It does nothing on Windows.
///
/// </div>
///
/// ## Example
/// ```no_run
/// use tracing_batteries::{Session, Profiling, ProfilingJemalloc};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///   .with_battery(Profiling::new("http://localhost:4317")
///     .with_backend(ProfilingJemalloc));
///
/// session.shutdown();
/// ```
#[cfg(feature = "profiling-jemalloc")]
#[derive(Debug, Clone, Copy, Default)]
pub struct ProfilingJemalloc;

#[cfg(feature = "profiling-jemalloc")]
impl ProfilingBackend for ProfilingJemalloc {
    #[cfg(not(windows))]
    fn build(self: Box<Self>) -> Option<BackendImpl<BackendUninitialized>> {
        Some(pyroscope::backend::jemalloc_backend())
    }
}

/// A continuous profiling integration which exports profiles of your application to an
/// OpenTelemetry collector, using the OTLP profiles signal.
///
/// <div class="warning">
///
/// This integration requires the `profiling` feature to be enabled, along with the feature
/// for each backend you wish to use (`profiling-pprof`, `profiling-jemalloc`).
///
/// The OTLP profiles signal is still in development (`v1development`), so your collector must
/// have profiles support enabled (for the OpenTelemetry Collector, the
/// `service.profilesSupport` feature gate and a `profiles` pipeline) and must be recent enough
/// to accept the version of the protocol used by this library.
///
/// </div>
///
/// Profiles are gathered by the backends attached through [`Profiling::with_backend`] and are
/// exported periodically, as well as when the session is shut down. The collector connection is
/// configured in the same way as the [`OpenTelemetry`] integration, and honours the same
/// `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_EXPORTER_OTLP_PROTOCOL`
/// and `OTEL_RESOURCE_ATTRIBUTES` environment variables, so that profiles are sent to the same
/// collector and described by the same resource as your traces.
///
/// ## Platform support
///
/// Profiling is not available on Windows, where this integration does nothing. This allows it
/// to be configured unconditionally by applications which are built for several platforms.
///
/// ## Disabled sessions
///
/// Profiling only starts if the session is enabled when the battery is attached (so debug
/// builds need [`Metadata::with_debug_builds`]) and the endpoint is not empty. The session's
/// enabled state is then checked each time profiles are due to be exported, with the profiles
/// gathered since the previous export being discarded if it is disabled at that point.
///
/// ## Example
/// ```no_run
/// use tracing_batteries::{Session, OpenTelemetry, Profiling, ProfilingPprof};
///
/// let session = Session::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
///   .with_battery(OpenTelemetry::new("https://otlp.example.com"))
///   .with_battery(Profiling::new("https://otlp.example.com")
///     .with_backend(ProfilingPprof::new()));
///
/// session.shutdown();
/// ```
#[cfg_attr(windows, allow(dead_code))]
pub struct Profiling {
    otlp: OpenTelemetry,
    upload_interval: Duration,
    backends: Vec<Box<dyn ProfilingBackend>>,
}

impl Profiling {
    /// Configures the profiling integration for the provided collector endpoint.
    ///
    /// The endpoint should correspond to the [`OpenTelemetryProtocol`] in use (gRPC by default),
    /// and the `OTEL_EXPORTER_OTLP_ENDPOINT` environment variable takes precedence over it. An
    /// empty endpoint disables profiling.
    pub fn new<S: Into<Cow<'static, str>>>(endpoint: S) -> Self {
        Self {
            otlp: OpenTelemetry::new(endpoint),
            upload_interval: DEFAULT_UPLOAD_INTERVAL,
            backends: Vec::new(),
        }
    }

    /// Attaches a profiling backend to the integration.
    ///
    /// This method may be called multiple times to gather several types of profile at once.
    ///
    /// ## Example
    /// ```no_run
    /// # #[cfg(all(feature = "profiling-pprof", feature = "profiling-jemalloc"))]
    /// # {
    /// use tracing_batteries::{Profiling, ProfilingJemalloc, ProfilingPprof};
    ///
    /// Profiling::new("http://localhost:4317")
    ///   .with_backend(ProfilingPprof::new())
    ///   .with_backend(ProfilingJemalloc);
    /// # }
    /// ```
    pub fn with_backend<B: ProfilingBackend + 'static>(mut self, backend: B) -> Self {
        self.backends.push(Box::new(backend));
        self
    }

    /// Adds a header to the collector connection, which is commonly used for authentication.
    ///
    /// Headers whose keys were already provided, including through the
    /// `OTEL_EXPORTER_OTLP_HEADERS` environment variable, are left unchanged.
    pub fn with_header<K: Into<Cow<'static, str>>, V: Into<Cow<'static, str>>>(
        mut self,
        key: K,
        value: V,
    ) -> Self {
        self.otlp = self.otlp.with_header(key, value);
        self
    }

    /// Configures the protocol used to communicate with the collector.
    ///
    /// The `OTEL_EXPORTER_OTLP_PROTOCOL` environment variable (`grpc`, `http-binary` or
    /// `http-json`) takes precedence over this value, and gRPC is used if neither is set.
    /// Profiles are always encoded as binary protobuf, including when `http-json` is selected.
    pub fn with_protocol(mut self, protocol: OpenTelemetryProtocol) -> Self {
        self.otlp = self.otlp.with_protocol(protocol);
        self
    }

    /// Configures how often profiles are exported to the collector (10 seconds by default).
    pub fn with_upload_interval(mut self, interval: Duration) -> Self {
        self.upload_interval = interval;
        self
    }
}

impl BatteryBuilder for Profiling {
    #[cfg(not(windows))]
    fn setup(self, metadata: &Metadata, enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        export::start(self, metadata, enabled)
    }

    #[cfg(windows)]
    fn setup(self, _metadata: &Metadata, _enabled: Arc<AtomicBool>) -> Box<dyn Battery> {
        tracing::debug!("Profiling is not supported on this platform.");
        Box::new(UnsupportedBattery)
    }
}

#[cfg(windows)]
struct UnsupportedBattery;

#[cfg(windows)]
impl Battery for UnsupportedBattery {}

/// Gathers profiles from the configured backends and exports them over OTLP.
#[cfg(not(windows))]
mod export {
    use std::{
        collections::HashMap,
        hash::{BuildHasher, RandomState},
        io::Read,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc::{Receiver, RecvTimeoutError, Sender, channel},
        },
        thread::JoinHandle,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use opentelemetry_proto::tonic::{
        collector::profiles::v1development::{
            ExportProfilesServiceRequest, profiles_service_client::ProfilesServiceClient,
        },
        common::v1::{AnyValue, InstrumentationScope, KeyValue, any_value},
        profiles::v1development as otlp,
        resource::v1::Resource,
    };
    use prost::Message;
    use pyroscope::{
        backend::{BackendImpl, BackendReady, ReportData},
        encode::r#gen::google as pprof,
    };

    use super::Profiling;
    use crate::{Battery, Metadata, OpenTelemetryProtocol, lock_ignoring_poison};

    // Rates outside of this range cannot be represented by the profilers' microsecond timers.
    const SAMPLE_RATES: std::ops::RangeInclusive<u32> = 1..=1_000_000;
    const EXPORT_TIMEOUT: Duration = Duration::from_secs(10);

    impl Profiling {
        fn build_resource(&self, metadata: &Metadata) -> Resource {
            Resource {
                attributes: self
                    .otlp
                    .build_resource(metadata)
                    .iter()
                    .map(|(key, value)| KeyValue {
                        key: key.to_string(),
                        value: Some(value.clone().into()),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }
        }

        fn build_exporter(&self) -> Exporter {
            match self.otlp.get_protocol() {
                OpenTelemetryProtocol::Grpc => Exporter::Grpc {
                    endpoint: self.otlp.endpoint.to_string(),
                    tls: self.otlp.tonic_tls_config(),
                    metadata: self.otlp.tonic_metadata(),
                },
                // The JSON encoding of profiles is not yet implemented correctly upstream, so both
                // HTTP protocols use the binary encoding (which every OTLP/HTTP receiver accepts).
                _ => Exporter::Http {
                    url: format!(
                        "{}/v1development/profiles",
                        self.otlp.endpoint.trim_end_matches('/')
                    ),
                    headers: self.otlp.http_headers(),
                },
            }
        }
    }

    pub(super) fn start(
        mut profiling: Profiling,
        metadata: &Metadata,
        enabled: Arc<AtomicBool>,
    ) -> Box<dyn Battery> {
        let mut profilers = Vec::new();

        if profiling.otlp.endpoint.is_empty() || !enabled.load(Ordering::Relaxed) {
            tracing::debug!("Profiling is disabled for this session.");
        } else {
            for backend in std::mem::take(&mut profiling.backends) {
                let sample_rate = backend.sample_rate();
                if !SAMPLE_RATES.contains(&sample_rate) {
                    tracing::warn!(
                        sample_rate,
                        "Skipping a profiling backend with an unsupported sample rate."
                    );
                    continue;
                }

                match backend.build().map(BackendImpl::initialize) {
                    Some(Ok(backend)) => profilers.push(Profiler {
                        backend,
                        sample_rate,
                    }),
                    Some(Err(error)) => {
                        tracing::warn!(%error, "Failed to start a profiling backend.")
                    }
                    None => {}
                }
            }
        }

        if profilers.is_empty() {
            return Box::new(ProfilingBattery::default());
        }

        let (stop, stopped) = channel();
        let worker = Worker {
            profilers,
            exporter: profiling.build_exporter(),
            resource: profiling.build_resource(metadata),
            interval: profiling.upload_interval,
            enabled,
        };

        let thread = std::thread::Builder::new()
            .name("profiling-export".into())
            .spawn(move || worker.run(stopped))
            .map_err(|error| tracing::warn!(%error, "Failed to start the profile exporter."))
            .ok();

        Box::new(ProfilingBattery {
            worker: Mutex::new(thread.map(|thread| (stop, thread))),
        })
    }

    #[derive(Default)]
    struct ProfilingBattery {
        worker: Mutex<Option<(Sender<()>, JoinHandle<()>)>>,
    }

    impl Battery for ProfilingBattery {
        fn shutdown(&mut self) {
            // The worker exports the profiles gathered since its last report before it exits.
            if let Some((stop, thread)) = lock_ignoring_poison(&self.worker).take() {
                let _ = stop.send(());
                let _ = thread.join();
            }
        }
    }

    struct Profiler {
        backend: BackendImpl<BackendReady>,
        sample_rate: u32,
    }

    /// Periodically gathers the profiles from each backend and exports them, on its own thread so
    /// that profiling does not depend on the hosting application's async runtime.
    struct Worker {
        profilers: Vec<Profiler>,
        exporter: Exporter,
        resource: Resource,
        interval: Duration,
        enabled: Arc<AtomicBool>,
    }

    impl Worker {
        fn run(mut self, stopped: Receiver<()>) {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::warn!(%error, "Failed to start the profile exporter.");
                    return;
                }
            };

            let mut started = SystemTime::now();
            loop {
                let stopping = !matches!(
                    stopped.recv_timeout(self.interval),
                    Err(RecvTimeoutError::Timeout)
                );

                let finished = SystemTime::now();
                let profiles = self.collect(started, finished);
                started = finished;

                // Profiles are always collected so that the backends' sample buffers are drained.
                if !profiles.is_empty() && self.enabled.load(Ordering::Relaxed) {
                    let request = build_request(&profiles, self.resource.clone());
                    let export = self.exporter.export(request);
                    match runtime
                        .block_on(async { tokio::time::timeout(EXPORT_TIMEOUT, export).await })
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => tracing::warn!(error, "Failed to export profiles."),
                        Err(_) => tracing::warn!("Timed out while exporting profiles."),
                    }
                }

                if stopping {
                    break;
                }
            }

            for profiler in self.profilers {
                if let Err(error) = profiler.backend.shutdown() {
                    tracing::warn!(%error, "Failed to stop a profiling backend.");
                }
            }
        }

        fn collect(&mut self, started: SystemTime, finished: SystemTime) -> Vec<pprof::Profile> {
            let start = unix_nanos(started);
            let duration = unix_nanos(finished).saturating_sub(start);

            self.profilers
                .iter_mut()
                .filter_map(|profiler| {
                    let profile = match profiler.backend.report().map(|batch| batch.data) {
                        Ok(ReportData::Reports(reports)) => pyroscope::encode::pprof::encode(
                            &reports,
                            profiler.sample_rate,
                            start,
                            duration,
                        ),
                        Ok(ReportData::RawPprof(data)) => decode_pprof(&data, start, duration)?,
                        Err(error) => {
                            tracing::warn!(%error, "Failed to gather a profile.");
                            return None;
                        }
                    };

                    (!profile.sample.is_empty()).then_some(profile)
                })
                .collect()
        }
    }

    fn unix_nanos(time: SystemTime) -> u64 {
        time.duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or_default()
    }

    /// Decodes a (usually gzipped) pprof profile which was encoded by the backend itself.
    fn decode_pprof(data: &[u8], start: u64, duration: u64) -> Option<pprof::Profile> {
        let mut decompressed = Vec::new();
        let data = match flate2::read::GzDecoder::new(data).read_to_end(&mut decompressed) {
            Ok(_) => decompressed.as_slice(),
            Err(_) => data,
        };

        let mut profile = pprof::Profile::decode(data)
            .map_err(|error| tracing::warn!(%error, "Failed to decode a profile."))
            .ok()?;

        if profile.time_nanos == 0 {
            profile.time_nanos = start as i64;
        }
        if profile.duration_nanos == 0 {
            profile.duration_nanos = duration as i64;
        }

        Some(profile)
    }

    enum Exporter {
        Grpc {
            endpoint: String,
            tls: tonic::transport::ClientTlsConfig,
            metadata: tonic::metadata::MetadataMap,
        },
        Http {
            url: String,
            headers: HashMap<String, String>,
        },
    }

    impl Exporter {
        async fn export(&self, body: ExportProfilesServiceRequest) -> Result<(), String> {
            match self {
                Exporter::Grpc {
                    endpoint,
                    tls,
                    metadata,
                } => {
                    let endpoint = if endpoint.contains("://") {
                        endpoint.clone()
                    } else {
                        format!("http://{endpoint}")
                    };

                    let mut channel = tonic::transport::Endpoint::from_shared(endpoint.clone())
                        .map_err(|error| error.to_string())?;
                    if endpoint.starts_with("https://") {
                        channel = channel
                            .tls_config(tls.clone())
                            .map_err(|error| error.to_string())?;
                    }

                    let mut request = tonic::Request::new(body);
                    *request.metadata_mut() = metadata.clone();

                    ProfilesServiceClient::new(channel.connect_lazy())
                        .export(request)
                        .await
                        .map_err(|status| status.to_string())?;
                }
                Exporter::Http { url, headers } => {
                    let mut request = reqwest::Client::new()
                        .post(url)
                        .header("content-type", "application/x-protobuf")
                        .body(body.encode_to_vec());
                    for (key, value) in headers {
                        request = request.header(key, value);
                    }

                    request
                        .send()
                        .await
                        .and_then(|response| response.error_for_status())
                        .map_err(|error| error.to_string())?;
                }
            }

            Ok(())
        }
    }

    /// Converts a set of pprof profiles into an OTLP export request.
    fn build_request(
        profiles: &[pprof::Profile],
        resource: Resource,
    ) -> ExportProfilesServiceRequest {
        let mut dictionary = Dictionary::new();
        let profiles = profiles
            .iter()
            .flat_map(|profile| dictionary.add_profile(profile))
            .collect();

        ExportProfilesServiceRequest {
            resource_profiles: vec![otlp::ResourceProfiles {
                resource: Some(resource),
                scope_profiles: vec![otlp::ScopeProfiles {
                    scope: Some(InstrumentationScope {
                        name: env!("CARGO_PKG_NAME").into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                        ..Default::default()
                    }),
                    profiles,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            dictionary: Some(dictionary.tables),
        }
    }

    /// Builds the lookup tables shared by the profiles in an OTLP export request, translating the
    /// identifiers used by each pprof profile into indices within those tables.
    struct Dictionary {
        tables: otlp::ProfilesDictionary,
        strings: HashMap<String, i32>,
        stacks: HashMap<Vec<i32>, i32>,
        attributes: HashMap<(i32, i32, i64, i32), i32>,
        ids: RandomState,
    }

    impl Dictionary {
        fn new() -> Self {
            // The first entry of every table is required to be its zero value, which is used to
            // represent a missing reference.
            Self {
                tables: otlp::ProfilesDictionary {
                    mapping_table: vec![Default::default()],
                    location_table: vec![Default::default()],
                    function_table: vec![Default::default()],
                    link_table: vec![Default::default()],
                    string_table: vec![String::new()],
                    attribute_table: vec![Default::default()],
                    stack_table: vec![Default::default()],
                },
                strings: HashMap::from([(String::new(), 0)]),
                stacks: HashMap::new(),
                attributes: HashMap::new(),
                ids: RandomState::new(),
            }
        }

        fn string(&mut self, value: &str) -> i32 {
            if let Some(index) = self.strings.get(value) {
                return *index;
            }

            let index = self.tables.string_table.len() as i32;
            self.tables.string_table.push(value.to_owned());
            self.strings.insert(value.to_owned(), index);
            index
        }

        fn value_type(
            &mut self,
            strings: &[i32],
            value_type: &pprof::ValueType,
        ) -> otlp::ValueType {
            otlp::ValueType {
                type_strindex: lookup(strings, value_type.r#type),
                unit_strindex: lookup(strings, value_type.unit),
            }
        }

        fn attribute(&mut self, strings: &[i32], label: &pprof::Label) -> i32 {
            let key = (
                lookup(strings, label.key),
                lookup(strings, label.str),
                label.num,
                lookup(strings, label.num_unit),
            );
            if let Some(index) = self.attributes.get(&key) {
                return *index;
            }

            let value = if label.str != 0 {
                any_value::Value::StringValue(self.tables.string_table[key.1 as usize].clone())
            } else {
                any_value::Value::IntValue(label.num)
            };

            let index = self.tables.attribute_table.len() as i32;
            self.tables.attribute_table.push(otlp::KeyValueAndUnit {
                key_strindex: key.0,
                value: Some(AnyValue { value: Some(value) }),
                unit_strindex: key.3,
            });
            self.attributes.insert(key, index);
            index
        }

        /// Adds a pprof profile to the dictionary, returning an OTLP profile for each of its sample
        /// types (as an OTLP profile describes a single type of sample).
        fn add_profile(&mut self, profile: &pprof::Profile) -> Vec<otlp::Profile> {
            let strings: Vec<i32> = profile
                .string_table
                .iter()
                .map(|value| self.string(value))
                .collect();

            let mut mappings = HashMap::new();
            for mapping in &profile.mapping {
                mappings.insert(mapping.id, self.tables.mapping_table.len() as i32);
                self.tables.mapping_table.push(otlp::Mapping {
                    memory_start: mapping.memory_start,
                    memory_limit: mapping.memory_limit,
                    file_offset: mapping.file_offset,
                    filename_strindex: lookup(&strings, mapping.filename),
                    ..Default::default()
                });
            }

            let mut functions = HashMap::new();
            for function in &profile.function {
                functions.insert(function.id, self.tables.function_table.len() as i32);
                self.tables.function_table.push(otlp::Function {
                    name_strindex: lookup(&strings, function.name),
                    system_name_strindex: lookup(&strings, function.system_name),
                    filename_strindex: lookup(&strings, function.filename),
                    start_line: function.start_line,
                });
            }

            let mut locations = HashMap::new();
            for location in &profile.location {
                locations.insert(location.id, self.tables.location_table.len() as i32);
                self.tables.location_table.push(otlp::Location {
                    mapping_index: mappings
                        .get(&location.mapping_id)
                        .copied()
                        .unwrap_or_default(),
                    address: location.address,
                    lines: location
                        .line
                        .iter()
                        .map(|line| otlp::Line {
                            function_index: functions
                                .get(&line.function_id)
                                .copied()
                                .unwrap_or_default(),
                            line: line.line,
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                });
            }

            // Both formats order the locations of a stack from its leaf to its root.
            let samples: Vec<(i32, Vec<i32>, &[i64])> = profile
                .sample
                .iter()
                .map(|sample| {
                    let stack: Vec<i32> = sample
                        .location_id
                        .iter()
                        .map(|id| locations.get(id).copied().unwrap_or_default())
                        .collect();
                    let stack = match self.stacks.get(&stack) {
                        Some(index) => *index,
                        None => {
                            let index = self.tables.stack_table.len() as i32;
                            self.tables.stack_table.push(otlp::Stack {
                                location_indices: stack.clone(),
                            });
                            self.stacks.insert(stack, index);
                            index
                        }
                    };

                    let attributes = sample
                        .label
                        .iter()
                        .map(|label| self.attribute(&strings, label))
                        .collect();

                    (stack, attributes, sample.value.as_slice())
                })
                .collect();

            let period_type = profile
                .period_type
                .as_ref()
                .map(|value_type| self.value_type(&strings, value_type));

            profile
                .sample_type
                .iter()
                .enumerate()
                .map(|(index, sample_type)| otlp::Profile {
                    sample_type: Some(self.value_type(&strings, sample_type)),
                    samples: samples
                        .iter()
                        .map(|(stack, attributes, values)| otlp::Sample {
                            stack_index: *stack,
                            attribute_indices: attributes.clone(),
                            values: values.get(index).copied().into_iter().collect(),
                            ..Default::default()
                        })
                        .collect(),
                    time_unix_nano: profile.time_nanos as u64,
                    duration_nano: profile.duration_nanos as u64,
                    period_type,
                    period: profile.period,
                    profile_id: self.profile_id(),
                    ..Default::default()
                })
                .collect()
        }

        /// Generates a random 16 byte profile identifier.
        fn profile_id(&self) -> Vec<u8> {
            let id = RandomState::new();
            [self.ids.hash_one(0u8), id.hash_one(0u8)]
                .into_iter()
                .flat_map(u64::to_be_bytes)
                .collect()
        }
    }

    /// Translates a pprof string table index into its index within the dictionary's string table.
    fn lookup(strings: &[i32], index: i64) -> i32 {
        strings.get(index as usize).copied().unwrap_or_default()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn pprof_profile() -> pprof::Profile {
            // String table: 1 = cpu, 2 = nanoseconds, 3 = main, 4 = work, 5 = thread, 6 = worker
            pprof::Profile {
                string_table: ["", "cpu", "nanoseconds", "main", "work", "thread", "worker"]
                    .map(String::from)
                    .to_vec(),
                sample_type: vec![pprof::ValueType { r#type: 1, unit: 2 }],
                function: vec![
                    pprof::Function {
                        id: 1,
                        name: 3,
                        ..Default::default()
                    },
                    pprof::Function {
                        id: 2,
                        name: 4,
                        ..Default::default()
                    },
                ],
                location: [1, 2]
                    .map(|id| pprof::Location {
                        id,
                        line: vec![pprof::Line {
                            function_id: id,
                            line: 10,
                        }],
                        ..Default::default()
                    })
                    .to_vec(),
                sample: vec![pprof::Sample {
                    location_id: vec![2, 1],
                    value: vec![30],
                    label: vec![pprof::Label {
                        key: 5,
                        str: 6,
                        ..Default::default()
                    }],
                }],
                ..Default::default()
            }
        }

        #[test]
        fn gzipped_pprof_profiles_are_decoded() {
            use std::io::Write;

            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder
                .write_all(&pprof_profile().encode_to_vec())
                .expect("compress the profile");
            let compressed = encoder.finish().expect("compress the profile");

            let profile = decode_pprof(&compressed, 100, 10).expect("a decoded profile");
            assert_eq!(profile.sample, pprof_profile().sample);
            assert_eq!(
                profile.time_nanos, 100,
                "a missing start time should be filled in"
            );
        }

        #[test]
        fn pprof_profiles_are_converted_to_otlp() {
            // Two profiles ensure that identifiers are translated relative to the shared tables.
            let request = build_request(&[pprof_profile(), pprof_profile()], Resource::default());
            let dictionary = request.dictionary.expect("a dictionary");
            let profiles = &request.resource_profiles[0].scope_profiles[0].profiles;
            assert_eq!(profiles.len(), 2);

            for profile in profiles {
                let sample_type = profile.sample_type.expect("a sample type");
                assert_eq!(
                    dictionary.string_table[sample_type.type_strindex as usize],
                    "cpu"
                );

                let sample = &profile.samples[0];
                assert_eq!(sample.values, vec![30]);

                let functions: Vec<&str> = dictionary.stack_table[sample.stack_index as usize]
                    .location_indices
                    .iter()
                    .map(|location| {
                        let line = dictionary.location_table[*location as usize].lines[0];
                        let function = dictionary.function_table[line.function_index as usize];
                        dictionary.string_table[function.name_strindex as usize].as_str()
                    })
                    .collect();
                assert_eq!(
                    functions,
                    ["work", "main"],
                    "stacks should run from leaf to root"
                );

                let attribute = &dictionary.attribute_table[sample.attribute_indices[0] as usize];
                assert_eq!(
                    dictionary.string_table[attribute.key_strindex as usize],
                    "thread"
                );
            }

            assert_eq!(
                dictionary.string_table[0], "",
                "the first entry of each table must be its zero value"
            );
            assert_eq!(dictionary.stack_table[0], otlp::Stack::default());
        }
    }
}
