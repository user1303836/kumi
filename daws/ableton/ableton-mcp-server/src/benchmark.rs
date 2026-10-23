//! Source benchmark gates for the native protocol and maximum-size PCM analyzer.
//!
//! Report names and budgets match the TypeScript runner. Allocation columns measure total
//! tracked Rust heap bytes: Rust has no separate V8 heap/external/ArrayBuffer categories.
//! This conservative native measurement includes actual injected allocations, not estimates.
//! Run optimized `ableton-mcp-benchmark` for performance gates; debug builds are not timed claims.
use crate::analysis::{
    analyze_pcm, PcmAnalysisInput, MAX_ANALYSIS_CHANNELS, MAX_ANALYSIS_SAMPLES, MAX_TIME_FREQUENCY_BANDS, MAX_TIME_FREQUENCY_FRAMES,
    MAX_WAVEFORM_BINS,
};
use crate::host::{McpHost, McpHostOptions};
use crate::live::UnavailableLiveAdapter;
use crate::mcp_protocol::LEGACY_PROTOCOL_VERSION;
use kumi_common::js::json::stringify;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    f64::consts::PI,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    rc::Rc,
    task::{Context, Poll},
    time::Instant,
};
use tokio::io::AsyncWrite;

pub mod memory;
pub const ANALYSIS_MEASUREMENTS: usize = 3;
const PING_SAMPLES: usize = 256;
const BATCH_SIZE: usize = 128;
const MAX_CHANNEL_ANALYSIS_SAMPLES: usize = 96_000;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkMeasurement {
    pub name: String,
    pub value: f64,
    pub unit: String,
    pub budget: f64,
    pub passed: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub measurements: Vec<BenchmarkMeasurement>,
    pub passed: bool,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkBudgets {
    pub ping_p95_milliseconds: f64,
    pub ping_requests_per_second: f64,
    pub batch_p95_milliseconds: f64,
    pub response_loss_percent: f64,
    pub cancellation_p95_milliseconds: f64,
    pub recovery_milliseconds: f64,
    pub analysis_p95_milliseconds: f64,
    pub analysis_array_buffer_delta_bytes: f64,
    pub analysis_output_bytes: f64,
    pub isolated_peak_rss_bytes: f64,
    pub isolated_peak_heap_used_bytes: f64,
    pub isolated_peak_external_bytes: f64,
    pub isolated_peak_array_buffers_bytes: f64,
    pub isolated_latency_p95_milliseconds: f64,
    pub max_channel_analysis_p95_milliseconds: f64,
    pub waveform_time_frequency_p95_milliseconds: f64,
    pub waveform_time_frequency_output_bytes: f64,
    pub resume_milliseconds: f64,
}
pub const BENCHMARK_BUDGETS: BenchmarkBudgets = BenchmarkBudgets {
    ping_p95_milliseconds: 5.,
    ping_requests_per_second: 5000.,
    batch_p95_milliseconds: 100.,
    response_loss_percent: 0.,
    cancellation_p95_milliseconds: 5.,
    recovery_milliseconds: 100.,
    analysis_p95_milliseconds: 2000.,
    analysis_array_buffer_delta_bytes: 140_000_000.,
    analysis_output_bytes: 2_000_000.,
    isolated_peak_rss_bytes: 512_000_000.,
    isolated_peak_heap_used_bytes: 256_000_000.,
    isolated_peak_external_bytes: 256_000_000.,
    isolated_peak_array_buffers_bytes: 256_000_000.,
    isolated_latency_p95_milliseconds: 2000.,
    max_channel_analysis_p95_milliseconds: 250.,
    waveform_time_frequency_p95_milliseconds: 250.,
    waveform_time_frequency_output_bytes: 2_000_000.,
    resume_milliseconds: 100.,
};
#[derive(Debug, Clone, Default)]
pub struct IsolatedAnalysisOptions {
    pub slow_milliseconds: Option<f64>,
    pub allocate: bool,
    pub latency_budget_milliseconds: Option<f64>,
    pub array_buffers_budget_bytes: Option<f64>,
    /// Native sibling worker by default; override for an explicitly selected build in tests.
    pub worker_path: Option<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolatedAnalysisResources {
    #[serde(default)]
    pub elapsed_milliseconds: f64,
    pub latency_measurements: Vec<f64>,
    pub peak_rss_bytes: f64,
    pub peak_heap_used_bytes: f64,
    pub peak_external_bytes: f64,
    pub peak_array_buffers_bytes: f64,
    pub output_bytes: f64,
}
pub fn percentile(values: &[f64], fraction: f64) -> Result<f64, String> {
    if values.is_empty() {
        return Err("cannot calculate a percentile from no measurements".into());
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((sorted.len() as f64 * fraction).ceil() - 1.).min((sorted.len() - 1) as f64);
    Ok(if index >= 0. && index.is_finite() { sorted[index as usize] } else { 0. })
}
pub fn measure(name: &str, value: f64, unit: &str, budget: f64, minimum: bool) -> BenchmarkMeasurement {
    BenchmarkMeasurement {
        name: name.into(),
        value,
        unit: unit.into(),
        budget,
        passed: if minimum { value >= budget } else { value <= budget },
    }
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|v| v != 0. && !v.is_nan()),
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}
fn elapsed(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.
}
fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":LEGACY_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"benchmark","version":"1"}}})
}
fn initialized() -> Value {
    json!({"jsonrpc":"2.0","method":"notifications/initialized"})
}
fn ping(id: usize) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"ping"})
}
fn fresh_host() -> Result<McpHost, String> {
    let host = McpHost::new(Rc::new(UnavailableLiveAdapter), McpHostOptions::default()).map_err(|e| e.to_string())?;
    host.handle(&initialize()).map_err(|e| e.to_string())?;
    host.handle(&initialized()).map_err(|e| e.to_string())?;
    Ok(host)
}
pub fn audio_fixture() -> Vec<f64> {
    (0..MAX_ANALYSIS_SAMPLES).map(|i| (0.5 * ((2. * PI * 440. * i as f64) / 48_000.).sin()) as f32 as f64).collect()
}
pub fn measure_maximum_input_analysis() -> Result<Vec<BenchmarkMeasurement>, String> {
    measure_maximum_input_analysis_with(|input| analyze_pcm(input).map(|v| serde_json::to_value(v).unwrap()).map_err(|e| e.to_string()))
}
pub fn measure_maximum_input_analysis_with(
    mut analyzer: impl FnMut(&PcmAnalysisInput<'_>) -> Result<Value, String>,
) -> Result<Vec<BenchmarkMeasurement>, String> {
    let samples = audio_fixture();
    let input = PcmAnalysisInput { samples: &samples, sample_rate: 48_000., channels: None, channel_layout: None, frame_size: None };
    analyzer(&input)?;
    let mut times = vec![];
    let mut maximum_delta = 0;
    let mut maximum_output = 0;
    for _ in 0..ANALYSIS_MEASUREMENTS {
        let before = memory::allocation_bytes()?.0;
        let started = Instant::now();
        let result = analyzer(&input)?;
        times.push(elapsed(started));
        let after = memory::allocation_bytes()?.0;
        maximum_delta = maximum_delta.max(after.saturating_sub(before));
        maximum_output = maximum_output.max(stringify(&result).len());
        if result["sampleCount"].as_f64() != Some(MAX_ANALYSIS_SAMPLES as f64) || truthy(&result["safety"]["projectMutated"]) {
            return Err("analysis result was incomplete or unsafe".into());
        }
    }
    Ok(vec![
        measure("pcm_analysis_p95_latency", percentile(&times, 0.95)?, "ms", BENCHMARK_BUDGETS.analysis_p95_milliseconds, false),
        measure(
            "pcm_analysis_array_buffer_delta",
            maximum_delta as f64,
            "bytes",
            BENCHMARK_BUDGETS.analysis_array_buffer_delta_bytes,
            false,
        ),
        measure("pcm_analysis_output_bytes", maximum_output as f64, "bytes", BENCHMARK_BUDGETS.analysis_output_bytes, false),
    ])
}
pub fn isolated_measurements(
    resources: &IsolatedAnalysisResources,
    options: &IsolatedAnalysisOptions,
) -> Result<Vec<BenchmarkMeasurement>, String> {
    Ok(vec![
        measure(
            "pcm_isolated_latency_p95",
            percentile(&resources.latency_measurements, 0.95)?,
            "ms",
            options.latency_budget_milliseconds.unwrap_or(BENCHMARK_BUDGETS.isolated_latency_p95_milliseconds),
            false,
        ),
        measure("pcm_isolated_peak_rss", resources.peak_rss_bytes, "bytes", BENCHMARK_BUDGETS.isolated_peak_rss_bytes, false),
        measure(
            "pcm_isolated_peak_heap_used",
            resources.peak_heap_used_bytes,
            "bytes",
            BENCHMARK_BUDGETS.isolated_peak_heap_used_bytes,
            false,
        ),
        measure(
            "pcm_isolated_peak_external",
            resources.peak_external_bytes,
            "bytes",
            BENCHMARK_BUDGETS.isolated_peak_external_bytes,
            false,
        ),
        measure(
            "pcm_isolated_peak_array_buffers",
            resources.peak_array_buffers_bytes,
            "bytes",
            options.array_buffers_budget_bytes.unwrap_or(BENCHMARK_BUDGETS.isolated_peak_array_buffers_bytes),
            false,
        ),
        measure("pcm_isolated_output_bytes", resources.output_bytes, "bytes", BENCHMARK_BUDGETS.analysis_output_bytes, false),
    ])
}
fn worker_path() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = current.parent().ok_or("analysis worker directory is unavailable")?;
    let filename = if cfg!(windows) { "ableton-mcp-analysis-worker.exe" } else { "ableton-mcp-analysis-worker" };
    let sibling = parent.join(filename);
    Ok(if parent.file_name().is_some_and(|v| v == "deps") && !sibling.is_file() {
        parent.parent().unwrap_or(parent).join(filename)
    } else {
        sibling
    })
}
pub async fn measure_isolated_maximum_input_analysis(options: &IsolatedAnalysisOptions) -> Result<Vec<BenchmarkMeasurement>, String> {
    let path = options.worker_path.clone().map(Ok).unwrap_or_else(worker_path)?;
    let mut command = tokio::process::Command::new(path);
    command.arg("--benchmark").stdin(Stdio::null()).kill_on_drop(true);
    if let Some(slow) = options.slow_milliseconds {
        command.arg(format!("--slow={}", kumi_common::js::number::to_string(slow)));
    }
    if options.allocate {
        command.arg("--allocate");
    }
    let started = Instant::now();
    let output = command.output().await.map_err(|e| e.to_string())?;
    let elapsed_milliseconds = elapsed(started);
    if !output.status.success() {
        return Err(format!(
            "isolated analysis worker failed ({}): {}",
            output.status.code().unwrap_or(1),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let mut resources: IsolatedAnalysisResources =
        serde_json::from_slice(&output.stdout).map_err(|_| "isolated analysis worker returned invalid resource evidence")?;
    resources.elapsed_milliseconds = elapsed_milliseconds;
    isolated_measurements(&resources, options)
}
struct CollectedOutput(Rc<RefCell<Vec<u8>>>);
impl AsyncWrite for CollectedOutput {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<std::io::Result<usize>> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
async fn run_wire_payload(payload: String) -> Result<Vec<Value>, String> {
    let bytes = Rc::new(RefCell::new(Vec::new()));
    // PassThrough in the source collects chunks until serve completes, independently of an
    // output EOF. A notifier can still hold the writer after all input has been processed.
    crate::serve::serve(
        std::io::Cursor::new(payload.into_bytes()),
        CollectedOutput(bytes.clone()),
        tokio::io::sink(),
        None,
        McpHostOptions::default(),
    )
    .await
    .map_err(|e| e.to_string())?;
    let bytes = bytes.borrow().clone();
    String::from_utf8(bytes)
        .map_err(|e| e.to_string())?
        .lines()
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::from_str(v).map_err(|e| e.to_string()))
        .collect()
}
async fn run_wire(records: &[Value]) -> Result<Vec<Value>, String> {
    run_wire_payload(format!("{}\n", records.iter().map(stringify).collect::<Vec<_>>().join("\n"))).await
}
fn response_ids(records: &[Value]) -> BTreeSet<u64> {
    records.iter().filter_map(|r| r["id"].as_u64()).collect()
}
pub async fn measure_protocol() -> Result<Vec<BenchmarkMeasurement>, String> {
    let mut measurements = vec![];
    let host = fresh_host()?;
    for id in 2..34 {
        host.handle(&ping(id)).map_err(|e| e.to_string())?;
    }
    let mut times = vec![];
    for id in 34..34 + PING_SAMPLES {
        let started = Instant::now();
        let result = host.handle(&ping(id)).map_err(|e| e.to_string())?;
        times.push(elapsed(started));
        if result.as_ref().is_none_or(|r| r.get("result").is_none()) {
            return Err("ping did not produce a result".into());
        }
    }
    measurements.push(measure("rpc_ping_p95_latency", percentile(&times, 0.95)?, "ms", BENCHMARK_BUDGETS.ping_p95_milliseconds, false));
    let started = Instant::now();
    for id in 1000..1000 + PING_SAMPLES {
        host.handle(&ping(id)).map_err(|e| e.to_string())?;
    }
    measurements.push(measure(
        "rpc_ping_throughput",
        PING_SAMPLES as f64 * 1000. / elapsed(started).max(f64::EPSILON),
        "requests/s",
        BENCHMARK_BUDGETS.ping_requests_per_second,
        true,
    ));
    let mut times = vec![];
    for sample in 0..5 {
        let mut requests = vec![initialize(), initialized()];
        requests.extend((0..BATCH_SIZE).map(|i| ping(10_000 + i + sample * BATCH_SIZE + 1)));
        let expected = response_ids(&requests);
        let started = Instant::now();
        let results = run_wire(&requests).await?;
        times.push(elapsed(started));
        if response_ids(&results) != expected {
            return Err("batch response loss".into());
        }
    }
    measurements.push(measure(
        "ndjson_batch_p95_latency",
        percentile(&times, 0.95)?,
        "ms",
        BENCHMARK_BUDGETS.batch_p95_milliseconds,
        false,
    ));
    measurements.push(measure("ndjson_response_loss", 0., "percent", BENCHMARK_BUDGETS.response_loss_percent, false));
    let cancellation = fresh_host()?;
    let mut times = vec![];
    for index in 0..PING_SAMPLES {
        let started = Instant::now();
        let result = cancellation
            .handle(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":index+1}}))
            .map_err(|e| e.to_string())?;
        times.push(elapsed(started));
        if result.is_some() {
            return Err("cancellation notification produced a response".into());
        }
    }
    measurements.push(measure(
        "cancellation_p95_latency",
        percentile(&times, 0.95)?,
        "ms",
        BENCHMARK_BUDGETS.cancellation_p95_milliseconds,
        false,
    ));
    let started = Instant::now();
    let recovered =
        run_wire_payload(format!("not-json\n{}\n{}\n{}\n", stringify(&initialize()), stringify(&initialized()), stringify(&ping(2))))
            .await?;
    let duration = elapsed(started);
    if recovered.len() != 3 || recovered[0]["error"]["code"] != -32700 || !response_ids(&recovered).contains(&2) {
        return Err("malformed-stream recovery did not preserve the following request".into());
    }
    measurements.push(measure("malformed_stream_recovery_latency", duration, "ms", BENCHMARK_BUDGETS.recovery_milliseconds, false));
    let started = Instant::now();
    let resumed = run_wire(&[initialize(), initialized(), ping(2)]).await?;
    let duration = elapsed(started);
    if resumed.len() != 2 || response_ids(&resumed) != BTreeSet::from([1, 2]) {
        return Err("restart-and-resume did not complete initialization and retry".into());
    }
    measurements.push(measure("restart_resume_latency", duration, "ms", BENCHMARK_BUDGETS.resume_milliseconds, false));
    Ok(measurements)
}
pub fn measure_maximum_channel_analysis() -> Result<Vec<BenchmarkMeasurement>, String> {
    let samples: Vec<f64> = (0..MAX_CHANNEL_ANALYSIS_SAMPLES)
        .map(|i| (0.25 * ((2. * PI * 220. * (i / MAX_ANALYSIS_CHANNELS) as f64) / 48_000.).sin()) as f32 as f64)
        .collect();
    let input = PcmAnalysisInput {
        samples: &samples,
        sample_rate: 48_000.,
        channels: Some(MAX_ANALYSIS_CHANNELS as f64),
        channel_layout: None,
        frame_size: None,
    };
    analyze_pcm(&input).map_err(|e| e.to_string())?;
    let mut times = vec![];
    let mut maximum_output = 0;
    for _ in 0..5 {
        let started = Instant::now();
        let result = analyze_pcm(&input).map_err(|e| e.to_string())?;
        times.push(elapsed(started));
        if result.channels_detail.len() != MAX_ANALYSIS_CHANNELS
            || result.safety.project_mutated
            || result.waveform.bins.len() > MAX_WAVEFORM_BINS
            || result.time_frequency.frames.len() > MAX_TIME_FREQUENCY_FRAMES
            || result.time_frequency.band_count > MAX_TIME_FREQUENCY_BANDS
        {
            return Err("maximum-channel analysis was incomplete, unsafe, or unbounded".into());
        }
        maximum_output = maximum_output.max(stringify(&json!({"waveform":result.waveform,"timeFrequency":result.time_frequency})).len());
    }
    Ok(vec![
        measure(
            "pcm_max_channel_analysis_p95_latency",
            percentile(&times, 0.95)?,
            "ms",
            BENCHMARK_BUDGETS.max_channel_analysis_p95_milliseconds,
            false,
        ),
        measure(
            "pcm_waveform_time_frequency_p95_latency",
            percentile(&times, 0.95)?,
            "ms",
            BENCHMARK_BUDGETS.waveform_time_frequency_p95_milliseconds,
            false,
        ),
        measure(
            "pcm_waveform_time_frequency_output_bytes",
            maximum_output as f64,
            "bytes",
            BENCHMARK_BUDGETS.waveform_time_frequency_output_bytes,
            false,
        ),
    ])
}
pub async fn run_benchmarks() -> Result<BenchmarkReport, String> {
    run_benchmarks_with_worker(None).await
}
pub async fn run_benchmarks_with_worker(worker: Option<&Path>) -> Result<BenchmarkReport, String> {
    let mut measurements = measure_protocol().await?;
    measurements.extend(measure_maximum_input_analysis()?);
    measurements.extend(
        measure_isolated_maximum_input_analysis(&IsolatedAnalysisOptions { worker_path: worker.map(Path::to_owned), ..Default::default() })
            .await?,
    );
    measurements.extend(measure_maximum_channel_analysis()?);
    let passed = measurements.iter().all(|m| m.passed);
    Ok(BenchmarkReport { measurements, passed })
}
