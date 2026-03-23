use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ── Configuration ──

/// Parsed telemetry configuration with per-signal OTLP endpoints.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub traces_endpoint: String,
    pub metrics_endpoint: String,
    pub logs_endpoint: String,
    /// Optional auth header sent with every export request, e.g.
    /// `("x-ms-ikey", "abc-123")` or `("Authorization", "Bearer xxx")`.
    pub auth_header: Option<(String, String)>,
    pub service_name: String,
}

/// Statistics returned after telemetry export.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TelemetryStats {
    pub enabled: bool,
    pub endpoint: String,
    pub traces_sent: usize,
    pub metrics_sent: usize,
    pub logs_sent: usize,
    pub errors: Vec<String>,
    pub export_time_ms: u64,
}

// ── Connection String Parsing ──

/// Parse a telemetry connection value into a `TelemetryConfig`.
///
/// Supports three formats:
/// - **Connection string** (`InstrumentationKey=xxx;IngestionEndpoint=https://...`):
///   builds per-signal endpoints from the ingestion endpoint and sets `x-ms-ikey` auth.
/// - **ARM resource ID** (starts with `/subscriptions/`): returns `None` — the caller
///   must use [`fetch_otlp_endpoints`] with a bearer token instead.
/// - **Plain OTLP URL** (`https://...`): builds per-signal endpoints, no auth header.
pub fn parse_connection_string(conn_str: &str) -> Option<TelemetryConfig> {
    let trimmed = conn_str.trim();
    if trimmed.is_empty() {
        return None;
    }

    // ARM resource ID — needs async fetch, not handled here
    if trimmed.starts_with("/subscriptions/") {
        return None;
    }

    // Key=value format (Azure App Insights connection string)
    if trimmed.contains('=') && !trimmed.starts_with("http") {
        let mut parts: HashMap<&str, &str> = HashMap::new();
        for segment in trimmed.split(';') {
            let segment = segment.trim();
            if let Some((k, v)) = segment.split_once('=') {
                parts.insert(k.trim(), v.trim());
            }
        }

        let ikey = parts.get("InstrumentationKey").map(|s| s.to_string());
        let base = parts
            .get("IngestionEndpoint")
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://dc.services.visualstudio.com".to_string());

        let ikey = ikey?;

        Some(TelemetryConfig {
            traces_endpoint: format!("{}/v1/traces", base),
            metrics_endpoint: format!("{}/v1/metrics", base),
            logs_endpoint: format!("{}/v1/logs", base),
            auth_header: Some(("x-ms-ikey".to_string(), ikey)),
            service_name: String::new(), // filled in later
        })
    } else if trimmed.starts_with("http") {
        // Plain OTLP endpoint URL
        let base = trimmed.trim_end_matches('/');
        Some(TelemetryConfig {
            traces_endpoint: format!("{}/v1/traces", base),
            metrics_endpoint: format!("{}/v1/metrics", base),
            logs_endpoint: format!("{}/v1/logs", base),
            auth_header: None,
            service_name: String::new(),
        })
    } else {
        None
    }
}

/// Parse the JSON body returned by the ARM API for an App Insights resource.
/// Returns `(traces_endpoint, metrics_endpoint, logs_endpoint)`.
pub fn parse_arm_response(json_str: &str) -> Result<(String, String, String), String> {
    let v: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("Invalid JSON from ARM API: {}", e))?;

    let props = v
        .get("properties")
        .ok_or("ARM response missing 'properties' object")?;

    let traces = props
        .get("OTLPTracesEndpoint")
        .and_then(|v| v.as_str())
        .ok_or("ARM response missing properties.OTLPTracesEndpoint — ensure api-version=2025-01-23-preview and OTLP ingestion is enabled")?;

    let metrics = props
        .get("OTLPMetricsEndpoint")
        .and_then(|v| v.as_str())
        .ok_or("ARM response missing properties.OTLPMetricsEndpoint — ensure api-version=2025-01-23-preview and OTLP ingestion is enabled")?;

    let logs = props
        .get("OTLPLogsEndpoint")
        .and_then(|v| v.as_str())
        .ok_or("ARM response missing properties.OTLPLogsEndpoint — ensure api-version=2025-01-23-preview and OTLP ingestion is enabled")?;

    Ok((
        traces.to_string(),
        metrics.to_string(),
        logs.to_string(),
    ))
}

/// Fetch OTLP endpoints from an Azure App Insights resource via ARM API.
///
/// `resource_id` is an ARM resource ID such as
/// `/subscriptions/.../providers/microsoft.insights/components/my-ai`.
/// `token` is a Bearer token for ARM API authentication.
pub async fn fetch_otlp_endpoints(
    resource_id: &str,
    token: &str,
) -> Result<TelemetryConfig, String> {
    let url = format!(
        "https://management.azure.com{}?api-version=2025-01-23-preview",
        resource_id
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| format!("ARM API request failed: {}", e))?;

    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();

    if status >= 400 {
        return Err(format!("ARM API returned HTTP {}: {}", status, body));
    }

    let (traces, metrics, logs) = parse_arm_response(&body)?;

    Ok(TelemetryConfig {
        traces_endpoint: traces,
        metrics_endpoint: metrics,
        logs_endpoint: logs,
        auth_header: Some((
            "Authorization".to_string(),
            format!("Bearer {}", token),
        )),
        service_name: String::new(),
    })
}

// ── OTLP JSON Types (subset needed for traces, metrics, logs) ──

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpKeyValue {
    pub key: String,
    pub value: OtlpAnyValue,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpAnyValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub string_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub int_value: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bool_value: Option<bool>,
}

impl OtlpAnyValue {
    pub fn string(s: impl Into<String>) -> Self {
        Self {
            string_value: Some(s.into()),
            int_value: None,
            double_value: None,
            bool_value: None,
        }
    }
    pub fn int(v: i64) -> Self {
        Self {
            string_value: None,
            int_value: Some(v),
            double_value: None,
            bool_value: None,
        }
    }
    pub fn double(v: f64) -> Self {
        Self {
            string_value: None,
            int_value: None,
            double_value: Some(v),
            bool_value: None,
        }
    }
    pub fn bool(v: bool) -> Self {
        Self {
            string_value: None,
            int_value: None,
            double_value: None,
            bool_value: Some(v),
        }
    }
}

fn kv(key: &str, val: OtlpAnyValue) -> OtlpKeyValue {
    OtlpKeyValue {
        key: key.to_string(),
        value: val,
    }
}

fn kv_str(key: &str, val: &str) -> OtlpKeyValue {
    kv(key, OtlpAnyValue::string(val))
}

// ── Traces ──

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpTraceExport {
    pub resource_spans: Vec<OtlpResourceSpans>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpResourceSpans {
    pub resource: OtlpResource,
    pub scope_spans: Vec<OtlpScopeSpans>,
}

#[derive(Debug, Serialize, Clone)]
pub struct OtlpResource {
    pub attributes: Vec<OtlpKeyValue>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpScopeSpans {
    pub scope: OtlpScope,
    pub spans: Vec<OtlpSpan>,
}

#[derive(Debug, Serialize, Clone)]
pub struct OtlpScope {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpSpan {
    pub trace_id: String,
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub name: String,
    pub kind: u8, // 1=INTERNAL, 2=SERVER, 3=CLIENT
    pub start_time_unix_nano: String,
    pub end_time_unix_nano: String,
    pub attributes: Vec<OtlpKeyValue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<OtlpSpanEvent>,
    pub status: OtlpSpanStatus,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpSpanEvent {
    pub name: String,
    pub time_unix_nano: String,
    pub attributes: Vec<OtlpKeyValue>,
}

#[derive(Debug, Serialize, Clone)]
pub struct OtlpSpanStatus {
    pub code: u8, // 0=UNSET, 1=OK, 2=ERROR
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ── Metrics ──

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpMetricExport {
    pub resource_metrics: Vec<OtlpResourceMetrics>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpResourceMetrics {
    pub resource: OtlpResource,
    pub scope_metrics: Vec<OtlpScopeMetrics>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpScopeMetrics {
    pub scope: OtlpScope,
    pub metrics: Vec<OtlpMetric>,
}

#[derive(Debug, Serialize, Clone)]
pub struct OtlpMetric {
    pub name: String,
    pub description: String,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<OtlpSum>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exponential_histogram: Option<OtlpExponentialHistogram>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpSum {
    pub data_points: Vec<OtlpNumberDataPoint>,
    pub aggregation_temporality: u8, // 1=DELTA, 2=CUMULATIVE
    pub is_monotonic: bool,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpNumberDataPoint {
    pub attributes: Vec<OtlpKeyValue>,
    pub start_time_unix_nano: String,
    pub time_unix_nano: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_int: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_double: Option<f64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpExponentialHistogram {
    pub data_points: Vec<OtlpExponentialHistogramDataPoint>,
    pub aggregation_temporality: u8,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpExponentialHistogramDataPoint {
    pub attributes: Vec<OtlpKeyValue>,
    pub start_time_unix_nano: String,
    pub time_unix_nano: String,
    pub count: u64,
    pub sum: f64,
    pub scale: i32,
    pub zero_count: u64,
    pub positive: OtlpExpHistogramBuckets,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpExpHistogramBuckets {
    pub offset: i32,
    pub bucket_counts: Vec<u64>,
}

/// Convert a set of positive f64 values into base-2 exponential histogram buckets.
/// Uses scale=0 where bucket boundaries are at 2^i: (..., 0.5, 1, 2, 4, 8, ...).
/// At scale 0, index = ceil(log2(value)); value falls in bucket [2^(i-1), 2^i).
pub fn build_exp_buckets(values: &[f64]) -> (i32, OtlpExpHistogramBuckets, u64) {
    let scale: i32 = 0;
    let mut zero_count: u64 = 0;
    let mut index_counts: std::collections::BTreeMap<i32, u64> = std::collections::BTreeMap::new();

    for &v in values {
        if v <= 0.0 {
            zero_count += 1;
            continue;
        }
        // At scale 0: index = ceil(log2(value))
        // Bucket i covers (2^(i-1), 2^i]. Index 0 covers (0.5, 1.0], index 1 covers (1.0, 2.0], etc.
        let idx = v.log2().ceil() as i32;
        *index_counts.entry(idx).or_insert(0) += 1;
    }

    if index_counts.is_empty() {
        return (
            scale,
            OtlpExpHistogramBuckets {
                offset: 0,
                bucket_counts: vec![],
            },
            zero_count,
        );
    }

    let min_idx = *index_counts.keys().next().expect("histogram bucket indices should not be empty");
    let max_idx = *index_counts.keys().next_back().expect("histogram bucket indices should not be empty");
    let mut bucket_counts = Vec::with_capacity((max_idx - min_idx + 1) as usize);
    for i in min_idx..=max_idx {
        bucket_counts.push(*index_counts.get(&i).unwrap_or(&0));
    }

    (
        scale,
        OtlpExpHistogramBuckets {
            offset: min_idx,
            bucket_counts,
        },
        zero_count,
    )
}

// ── Logs ──

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpLogExport {
    pub resource_logs: Vec<OtlpResourceLogs>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpResourceLogs {
    pub resource: OtlpResource,
    pub scope_logs: Vec<OtlpScopeLogs>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpScopeLogs {
    pub scope: OtlpScope,
    pub log_records: Vec<OtlpLogRecord>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OtlpLogRecord {
    pub time_unix_nano: String,
    pub severity_number: u8, // 9=INFO, 13=WARN, 17=ERROR
    pub severity_text: String,
    pub body: OtlpAnyValue,
    pub attributes: Vec<OtlpKeyValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
}

// ── Telemetry Collector ──

/// Accumulates OTEL spans, metrics, and logs during a test run.
pub struct TelemetryCollector {
    pub config: TelemetryConfig,
    pub trace_id: String,
    spans: Vec<OtlpSpan>,
    logs: Vec<OtlpLogRecord>,
    // Metric accumulators
    suite_outcome: Option<String>,
    suite_duration_ms: u64,
    block_counts: Vec<(String, String, String, String, String)>, // (file, block_type, outcome, block_name, group)
    block_durations: Vec<(String, String, u64, String, String)>, // (file, block_type, duration_ms, block_name, group)
    assertion_counts: Vec<(String, String, String, String)>,      // (file, outcome, block_name, group)
    file_name: String,
    start_time_ns: u64,
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn gen_span_id() -> String {
    let bytes: [u8; 8] = rand::random();
    hex::encode(&bytes)
}

fn gen_trace_id() -> String {
    let bytes: [u8; 16] = rand::random();
    hex::encode(&bytes)
}

impl TelemetryCollector {
    pub fn new(config: TelemetryConfig, file_name: &str) -> Self {
        Self {
            config,
            trace_id: gen_trace_id(),
            spans: Vec::new(),
            logs: Vec::new(),
            suite_outcome: None,
            suite_duration_ms: 0,
            block_counts: Vec::new(),
            block_durations: Vec::new(),
            assertion_counts: Vec::new(),
            file_name: file_name.to_string(),
            start_time_ns: now_nanos(),
        }
    }

    /// Record the start of the suite run (creates root span later at suite_complete).
    pub fn suite_start(&mut self, block_count: usize) {
        self.start_time_ns = now_nanos();
        self.logs.push(OtlpLogRecord {
            time_unix_nano: self.start_time_ns.to_string(),
            severity_number: 9,
            severity_text: "INFO".to_string(),
            body: OtlpAnyValue::string(format!(
                "Suite started: {} ({} blocks)",
                self.file_name, block_count
            )),
            attributes: vec![
                kv_str("file", &self.file_name),
                kv("block_count", OtlpAnyValue::int(block_count as i64)),
            ],
            trace_id: Some(self.trace_id.clone()),
            span_id: None,
        });
    }

    /// Record a completed block execution.
    pub fn block_complete(
        &mut self,
        name: &str,
        block_type: &str,
        group: Option<&str>,
        outcome: &str,
        duration_ms: u64,
        http_method: Option<&str>,
        http_url: Option<&str>,
        http_status: Option<u16>,
        http_time_ms: Option<u64>,
        assertions: &[(String, String, String, bool)], // (assertion_text, expected, actual, passed)
        extracts: &[(String, Option<String>, bool)],   // (var_name, value, success)
        error: Option<&str>,
    ) {
        let block_span_id = gen_span_id();
        let block_start_ns = now_nanos() - (duration_ms as u64 * 1_000_000);
        let block_end_ns = now_nanos();

        let mut block_attrs = vec![
            kv_str("rp.block.name", name),
            kv_str("rp.block.type", block_type),
            kv_str("rp.block.outcome", outcome),
            kv_str("rp.file", &self.file_name),
            kv("rp.block.duration_ms", OtlpAnyValue::int(duration_ms as i64)),
        ];
        if let Some(g) = group {
            block_attrs.push(kv_str("rp.block.group", g));
        }

        // Span events for assertions
        let mut events = Vec::new();
        for (text, expected, actual, passed) in assertions {
            events.push(OtlpSpanEvent {
                name: "rp.assertion".to_string(),
                time_unix_nano: block_end_ns.to_string(),
                attributes: vec![
                    kv_str("assertion", text),
                    kv_str("expected", expected),
                    kv_str("actual", actual),
                    kv("passed", OtlpAnyValue::bool(*passed)),
                ],
            });
            // Track metric
            self.assertion_counts.push((
                self.file_name.clone(),
                if *passed { "pass" } else { "fail" }.to_string(),
                name.to_string(),
                group.unwrap_or("").to_string(),
            ));
        }

        // Span events for extracts
        for (var_name, value, success) in extracts {
            events.push(OtlpSpanEvent {
                name: "rp.extract".to_string(),
                time_unix_nano: block_end_ns.to_string(),
                attributes: vec![
                    kv_str("variable", var_name),
                    kv_str("value", value.as_deref().unwrap_or("")),
                    kv("success", OtlpAnyValue::bool(*success)),
                ],
            });
        }

        // HTTP request as child span (if there was an actual request)
        if let (Some(method), Some(url)) = (http_method, http_url) {
            let http_span_id = gen_span_id();
            let http_time = http_time_ms.unwrap_or(duration_ms);
            let http_start_ns = block_end_ns - (http_time as u64 * 1_000_000);
            let mut http_attrs = vec![
                kv_str("http.method", method),
                kv_str("http.url", url),
                kv("http.response_time_ms", OtlpAnyValue::int(http_time as i64)),
            ];
            if let Some(status) = http_status {
                http_attrs.push(kv("http.status_code", OtlpAnyValue::int(status as i64)));
            }

            self.spans.push(OtlpSpan {
                trace_id: self.trace_id.clone(),
                span_id: http_span_id,
                parent_span_id: Some(block_span_id.clone()),
                name: format!("{} {}", method, url),
                kind: 3, // CLIENT
                start_time_unix_nano: http_start_ns.to_string(),
                end_time_unix_nano: block_end_ns.to_string(),
                attributes: http_attrs,
                events: Vec::new(),
                status: OtlpSpanStatus {
                    code: if http_status.map_or(false, |s| s >= 400) {
                        2
                    } else {
                        1
                    },
                    message: None,
                },
            });
        }

        // Error event and log
        if let Some(err) = error {
            events.push(OtlpSpanEvent {
                name: "exception".to_string(),
                time_unix_nano: block_end_ns.to_string(),
                attributes: vec![kv_str("exception.message", err)],
            });
            self.logs.push(OtlpLogRecord {
                time_unix_nano: block_end_ns.to_string(),
                severity_number: 17,
                severity_text: "ERROR".to_string(),
                body: OtlpAnyValue::string(format!("Block failed: {} — {}", name, err)),
                attributes: vec![
                    kv_str("file", &self.file_name),
                    kv_str("block_name", name),
                    kv_str("block_type", block_type),
                ],
                trace_id: Some(self.trace_id.clone()),
                span_id: Some(block_span_id.clone()),
            });
        }

        // Failed assertion logs
        for (text, expected, actual, passed) in assertions {
            if !passed {
                self.logs.push(OtlpLogRecord {
                    time_unix_nano: block_end_ns.to_string(),
                    severity_number: 13,
                    severity_text: "WARN".to_string(),
                    body: OtlpAnyValue::string(format!(
                        "Assertion failed in {}: {} (expected: {}, actual: {})",
                        name, text, expected, actual
                    )),
                    attributes: vec![
                        kv_str("file", &self.file_name),
                        kv_str("block_name", name),
                        kv_str("assertion", text),
                        kv_str("expected", expected),
                        kv_str("actual", actual),
                    ],
                    trace_id: Some(self.trace_id.clone()),
                    span_id: Some(block_span_id.clone()),
                });
            }
        }

        // Block completion log
        let severity = if outcome == "passed" || outcome == "skipped" {
            (9, "INFO")
        } else {
            (17, "ERROR")
        };
        self.logs.push(OtlpLogRecord {
            time_unix_nano: block_end_ns.to_string(),
            severity_number: severity.0,
            severity_text: severity.1.to_string(),
            body: OtlpAnyValue::string(format!(
                "Block complete: {} [{}] — {} ({}ms)",
                name, block_type, outcome, duration_ms
            )),
            attributes: vec![
                kv_str("file", &self.file_name),
                kv_str("block_name", name),
                kv_str("block_type", block_type),
                kv_str("outcome", outcome),
                kv("duration_ms", OtlpAnyValue::int(duration_ms as i64)),
            ],
            trace_id: Some(self.trace_id.clone()),
            span_id: Some(block_span_id.clone()),
        });

        // Block span
        let is_error = outcome == "failed" || outcome == "error";
        self.spans.push(OtlpSpan {
            trace_id: self.trace_id.clone(),
            span_id: block_span_id,
            parent_span_id: None, // will be set to root span in build_traces()
            name: format!("rp.block.execute: {}", name),
            kind: 1, // INTERNAL
            start_time_unix_nano: block_start_ns.to_string(),
            end_time_unix_nano: block_end_ns.to_string(),
            attributes: block_attrs,
            events,
            status: OtlpSpanStatus {
                code: if is_error { 2 } else { 1 },
                message: error.map(|e| e.to_string()),
            },
        });

        // Metric accumulators
        let group_str = group.unwrap_or("").to_string();
        self.block_counts.push((
            self.file_name.clone(),
            block_type.to_string(),
            outcome.to_string(),
            name.to_string(),
            group_str.clone(),
        ));
        self.block_durations.push((
            self.file_name.clone(),
            block_type.to_string(),
            duration_ms,
            name.to_string(),
            group_str,
        ));
    }

    /// Finalize the suite run and prepare for export.
    pub fn suite_complete(&mut self, passed: usize, failed: usize, skipped: usize, total_time_ms: u64) {
        let end_ns = now_nanos();
        self.suite_duration_ms = total_time_ms;

        let outcome = if failed > 0 {
            "fail"
        } else if skipped > 0 && passed > 0 {
            "partial"
        } else if passed > 0 {
            "pass"
        } else {
            "error"
        };
        self.suite_outcome = Some(outcome.to_string());

        // Root suite span
        let root_span_id = gen_span_id();
        let root_span = OtlpSpan {
            trace_id: self.trace_id.clone(),
            span_id: root_span_id.clone(),
            parent_span_id: None,
            name: format!("rp.suite.run: {}", self.file_name),
            kind: 1,
            start_time_unix_nano: self.start_time_ns.to_string(),
            end_time_unix_nano: end_ns.to_string(),
            attributes: vec![
                kv_str("rp.file", &self.file_name),
                kv_str("rp.suite.outcome", outcome),
                kv("rp.suite.passed", OtlpAnyValue::int(passed as i64)),
                kv("rp.suite.failed", OtlpAnyValue::int(failed as i64)),
                kv("rp.suite.skipped", OtlpAnyValue::int(skipped as i64)),
                kv(
                    "rp.suite.total_blocks",
                    OtlpAnyValue::int((passed + failed + skipped) as i64),
                ),
                kv(
                    "rp.suite.duration_ms",
                    OtlpAnyValue::int(total_time_ms as i64),
                ),
            ],
            events: Vec::new(),
            status: OtlpSpanStatus {
                code: if failed > 0 { 2 } else { 1 },
                message: None,
            },
        };

        // Set parent_span_id on all block spans
        for span in &mut self.spans {
            if span.parent_span_id.is_none() {
                span.parent_span_id = Some(root_span_id.clone());
            }
        }

        // Insert root span at the beginning
        self.spans.insert(0, root_span);

        // Suite complete log
        self.logs.push(OtlpLogRecord {
            time_unix_nano: end_ns.to_string(),
            severity_number: if failed > 0 { 17 } else { 9 },
            severity_text: if failed > 0 { "ERROR" } else { "INFO" }.to_string(),
            body: OtlpAnyValue::string(format!(
                "Suite complete: {} — passed: {}, failed: {}, skipped: {} ({}ms)",
                self.file_name, passed, failed, skipped, total_time_ms
            )),
            attributes: vec![
                kv_str("file", &self.file_name),
                kv_str("outcome", outcome),
                kv("passed", OtlpAnyValue::int(passed as i64)),
                kv("failed", OtlpAnyValue::int(failed as i64)),
                kv("skipped", OtlpAnyValue::int(skipped as i64)),
                kv("duration_ms", OtlpAnyValue::int(total_time_ms as i64)),
            ],
            trace_id: Some(self.trace_id.clone()),
            span_id: None,
        });
    }

    // ── Payload builders ──

    fn resource(&self) -> OtlpResource {
        let mut attrs = vec![
            kv_str("service.name", &self.config.service_name),
            kv_str("service.version", env!("CARGO_PKG_VERSION")),
            kv_str("telemetry.sdk.name", "request-pilot"),
            kv_str("telemetry.sdk.language", "rust"),
        ];
        // Propagate ikey as resource attribute when using App Insights connection strings
        if let Some((ref hdr, ref val)) = self.config.auth_header {
            if hdr == "x-ms-ikey" {
                attrs.push(kv_str("ai.ikey", val));
            }
        }
        OtlpResource { attributes: attrs }
    }

    fn scope(&self) -> OtlpScope {
        OtlpScope {
            name: "request-pilot-core".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    fn build_traces(&self) -> OtlpTraceExport {
        OtlpTraceExport {
            resource_spans: vec![OtlpResourceSpans {
                resource: self.resource(),
                scope_spans: vec![OtlpScopeSpans {
                    scope: self.scope(),
                    spans: self.spans.clone(),
                }],
            }],
        }
    }

    fn build_metrics(&self) -> OtlpMetricExport {
        let now_ns = now_nanos().to_string();
        let start_ns = self.start_time_ns.to_string();
        let mut metrics = Vec::new();

        // rp.suite.runs (counter)
        if let Some(ref outcome) = self.suite_outcome {
            metrics.push(OtlpMetric {
                name: "rp.suite.runs".to_string(),
                description: "Number of test suite runs".to_string(),
                unit: "{run}".to_string(),
                sum: Some(OtlpSum {
                    data_points: vec![OtlpNumberDataPoint {
                        attributes: vec![
                            kv_str("file", &self.file_name),
                            kv_str("outcome", outcome),
                        ],
                        start_time_unix_nano: start_ns.clone(),
                        time_unix_nano: now_ns.clone(),
                        as_int: Some(1),
                        as_double: None,
                    }],
                    aggregation_temporality: 1, // DELTA
                    is_monotonic: true,
                }),
                exponential_histogram: None,
            });
        }

        // rp.suite.duration (exponential histogram)
        if self.suite_duration_ms > 0 {
            let val = self.suite_duration_ms as f64;
            let (scale, positive, zero_count) = build_exp_buckets(&[val]);
            metrics.push(OtlpMetric {
                name: "rp.suite.duration".to_string(),
                description: "Test suite execution duration".to_string(),
                unit: "ms".to_string(),
                sum: None,
                exponential_histogram: Some(OtlpExponentialHistogram {
                    data_points: vec![OtlpExponentialHistogramDataPoint {
                        attributes: vec![kv_str("file", &self.file_name)],
                        start_time_unix_nano: start_ns.clone(),
                        time_unix_nano: now_ns.clone(),
                        count: 1,
                        sum: val,
                        scale,
                        zero_count,
                        positive,
                        min: val,
                        max: val,
                    }],
                    aggregation_temporality: 1,
                }),
            });
        }

        // rp.block.runs (counter) — aggregate by (file, block_type, outcome)
        let mut block_agg: HashMap<(String, String, String, String, String), i64> = HashMap::new();
        for (file, bt, outcome, bname, group) in &self.block_counts {
            *block_agg
                .entry((file.clone(), bt.clone(), outcome.clone(), bname.clone(), group.clone()))
                .or_insert(0) += 1;
        }
        if !block_agg.is_empty() {
            let data_points: Vec<_> = block_agg
                .iter()
                .map(|((file, bt, outcome, bname, group), count)| {
                    let mut attrs = vec![
                        kv_str("file", file),
                        kv_str("block_type", bt),
                        kv_str("outcome", outcome),
                        kv_str("block_name", bname),
                    ];
                    if !group.is_empty() {
                        attrs.push(kv_str("group", group));
                    }
                    OtlpNumberDataPoint {
                        attributes: attrs,
                        start_time_unix_nano: start_ns.clone(),
                        time_unix_nano: now_ns.clone(),
                        as_int: Some(*count),
                        as_double: None,
                    }
                })
                .collect();
            metrics.push(OtlpMetric {
                name: "rp.block.runs".to_string(),
                description: "Number of block executions".to_string(),
                unit: "{run}".to_string(),
                sum: Some(OtlpSum {
                    data_points,
                    aggregation_temporality: 1,
                    is_monotonic: true,
                }),
                exponential_histogram: None,
            });
        }

        // rp.block.duration (exponential histogram) — aggregate by (file, block_type)
        let mut dur_agg: HashMap<(String, String, String, String), Vec<u64>> = HashMap::new();
        for (file, bt, dur, bname, group) in &self.block_durations {
            dur_agg
                .entry((file.clone(), bt.clone(), bname.clone(), group.clone()))
                .or_default()
                .push(*dur);
        }
        if !dur_agg.is_empty() {
            let data_points: Vec<_> = dur_agg
                .iter()
                .map(|((file, bt, bname, group), durations)| {
                    let count = durations.len() as u64;
                    let vals: Vec<f64> = durations.iter().map(|d| *d as f64).collect();
                    let sum: f64 = vals.iter().sum();
                    let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
                    let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                    let (scale, positive, zero_count) = build_exp_buckets(&vals);
                    let mut attrs = vec![
                        kv_str("file", file),
                        kv_str("block_type", bt),
                        kv_str("block_name", bname),
                    ];
                    if !group.is_empty() {
                        attrs.push(kv_str("group", group));
                    }
                    OtlpExponentialHistogramDataPoint {
                        attributes: attrs,
                        start_time_unix_nano: start_ns.clone(),
                        time_unix_nano: now_ns.clone(),
                        count,
                        sum,
                        scale,
                        zero_count,
                        positive,
                        min,
                        max,
                    }
                })
                .collect();
            metrics.push(OtlpMetric {
                name: "rp.block.duration".to_string(),
                description: "Block execution duration".to_string(),
                unit: "ms".to_string(),
                sum: None,
                exponential_histogram: Some(OtlpExponentialHistogram {
                    data_points,
                    aggregation_temporality: 1,
                }),
            });
        }

        // rp.assertion.total (counter) — aggregate by (file, outcome)
        let mut assert_agg: HashMap<(String, String, String, String), i64> = HashMap::new();
        for (file, outcome, bname, group) in &self.assertion_counts {
            *assert_agg
                .entry((file.clone(), outcome.clone(), bname.clone(), group.clone()))
                .or_insert(0) += 1;
        }
        if !assert_agg.is_empty() {
            let data_points: Vec<_> = assert_agg
                .iter()
                .map(|((file, outcome, bname, group), count)| {
                    let mut attrs = vec![
                        kv_str("file", file),
                        kv_str("outcome", outcome),
                        kv_str("block_name", bname),
                    ];
                    if !group.is_empty() {
                        attrs.push(kv_str("group", group));
                    }
                    OtlpNumberDataPoint {
                        attributes: attrs,
                        start_time_unix_nano: start_ns.clone(),
                        time_unix_nano: now_ns.clone(),
                        as_int: Some(*count),
                        as_double: None,
                    }
                })
                .collect();
            metrics.push(OtlpMetric {
                name: "rp.assertion.total".to_string(),
                description: "Number of assertion evaluations".to_string(),
                unit: "{assertion}".to_string(),
                sum: Some(OtlpSum {
                    data_points,
                    aggregation_temporality: 1,
                    is_monotonic: true,
                }),
                exponential_histogram: None,
            });
        }

        OtlpMetricExport {
            resource_metrics: vec![OtlpResourceMetrics {
                resource: self.resource(),
                scope_metrics: vec![OtlpScopeMetrics {
                    scope: self.scope(),
                    metrics,
                }],
            }],
        }
    }

    fn build_logs(&self) -> OtlpLogExport {
        OtlpLogExport {
            resource_logs: vec![OtlpResourceLogs {
                resource: self.resource(),
                scope_logs: vec![OtlpScopeLogs {
                    scope: self.scope(),
                    log_records: self.logs.clone(),
                }],
            }],
        }
    }

    // ── Export ──

    /// Export collected telemetry to the configured OTLP endpoint.
    /// Returns stats. Never panics or propagates errors (telemetry must not break test runs).
    pub async fn export(&self) -> TelemetryStats {
        let start = Instant::now();
        let mut stats = TelemetryStats {
            enabled: true,
            endpoint: self.config.traces_endpoint.clone(),
            ..Default::default()
        };

        // Log auth info for debugging
        if let Some((ref hdr, ref val)) = self.config.auth_header {
            let token_info = if hdr == "Authorization" && val.starts_with("Bearer ") {
                let jwt = &val[7..];
                decode_jwt_audience(jwt)
                    .unwrap_or_else(|| format!("Bearer token ({}...)", &jwt[..20.min(jwt.len())]))
            } else {
                format!("{}: {}...", hdr, &val[..20.min(val.len())])
            };
            eprintln!("[telemetry] Auth: {}", token_info);
        } else {
            eprintln!("[telemetry] No auth header configured");
        }

        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                stats.errors.push(format!("HTTP client error: {}", e));
                stats.export_time_ms = start.elapsed().as_millis() as u64;
                return stats;
            }
        };

        // Build payloads
        let traces = self.build_traces();
        let metrics = self.build_metrics();
        let logs = self.build_logs();

        stats.traces_sent = traces.resource_spans.first()
            .and_then(|rs| rs.scope_spans.first())
            .map(|ss| ss.spans.len())
            .unwrap_or(0);
        stats.metrics_sent = metrics.resource_metrics.first()
            .and_then(|rm| rm.scope_metrics.first())
            .map(|sm| sm.metrics.len())
            .unwrap_or(0);
        stats.logs_sent = logs.resource_logs.first()
            .and_then(|rl| rl.scope_logs.first())
            .map(|sl| sl.log_records.len())
            .unwrap_or(0);

        // Encode to protobuf and POST each signal
        let signals: Vec<(&str, Vec<u8>, &str, usize)> = vec![
            ("Traces", crate::otlp_proto::encode_traces(&traces), &self.config.traces_endpoint, stats.traces_sent),
            ("Metrics", crate::otlp_proto::encode_metrics(&metrics), &self.config.metrics_endpoint, stats.metrics_sent),
            ("Logs", crate::otlp_proto::encode_logs(&logs), &self.config.logs_endpoint, stats.logs_sent),
        ];

        for (signal, pb, endpoint, count) in signals {
            if count == 0 {
                eprintln!("[telemetry] {}: 0 items, skipping export", signal);
                continue;
            }
            eprintln!("[telemetry] {} export: {} items, {} bytes → {}", signal, count, pb.len(), endpoint);
            match self.post_to(&client, endpoint, pb).await {
                Ok(status) => {
                    eprintln!("[telemetry] {} export: HTTP {} OK", signal, status);
                }
                Err(e) => {
                    eprintln!("[telemetry] {} export FAILED: {}", signal, e);
                    stats.errors.push(format!("{} export: {}", signal, e));
                }
            }
        }

        stats.export_time_ms = start.elapsed().as_millis() as u64;
        stats
    }

    async fn post_to(
        &self,
        client: &reqwest::Client,
        url: &str,
        body: Vec<u8>,
    ) -> Result<u16, String> {
        let mut req = client
            .post(url)
            .header("Content-Type", "application/x-protobuf");

        if let Some((ref hdr, ref val)) = self.config.auth_header {
            req = req.header(hdr, val);
        }

        let resp = req.body(body).send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            let mut msg = format!("HTTP {} from {}: {}", status, url, body);
            if status == 403 {
                if let Some((_, ref val)) = self.config.auth_header {
                    if val.starts_with("Bearer ") {
                        let aud = decode_jwt_audience(&val[7..])
                            .unwrap_or_else(|| "unknown".to_string());
                        msg.push_str(&format!(" [token audience: {}]", aud));
                    }
                }
            }
            return Err(msg);
        }
        Ok(status)
    }
}

/// Decode JWT payload (no verification) to extract the `aud` claim for debugging.
fn decode_jwt_audience(jwt: &str) -> Option<String> {
    let parts: Vec<&str> = jwt.splitn(3, '.').collect();
    if parts.len() < 2 { return None; }
    // base64url decode the payload
    let payload = parts[1];
    let padded = match payload.len() % 4 {
        2 => format!("{}==", payload),
        3 => format!("{}=", payload),
        _ => payload.to_string(),
    };
    let b64 = padded.replace('-', "+").replace('_', "/");
    let decoded = base64_decode(&b64)?;
    let json_str = String::from_utf8(decoded).ok()?;
    // Simple JSON parse for "aud" field
    let val: serde_json::Value = serde_json::from_str(&json_str).ok()?;
    val.get("aud").and_then(|a| {
        if let Some(s) = a.as_str() { Some(s.to_string()) }
        else if let Some(arr) = a.as_array() {
            Some(arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
        } else { None }
    })
}

/// Minimal base64 decoder (standard alphabet, no dependencies).
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(0),
            _ => None,
        }
    }
    let bytes = input.as_bytes();
    if bytes.len() % 4 != 0 { return None; }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        let c = val(chunk[2])?;
        let d = val(chunk[3])?;
        let n = (a as u32) << 18 | (b as u32) << 12 | (c as u32) << 6 | d as u32;
        out.push((n >> 16) as u8);
        if chunk[2] != b'=' { out.push((n >> 8) as u8); }
        if chunk[3] != b'=' { out.push(n as u8); }
    }
    Some(out)
}

// ── Hex encoding (no dependency needed) ──

mod hex {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

    pub fn encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(HEX_CHARS[(b >> 4) as usize] as char);
            s.push(HEX_CHARS[(b & 0x0f) as usize] as char);
        }
        s
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_appinsights_connection_string() {
        let cs = "InstrumentationKey=abc-123;IngestionEndpoint=https://eastus.in.applicationinsights.azure.com";
        let config = parse_connection_string(cs).unwrap();
        assert_eq!(
            config.traces_endpoint,
            "https://eastus.in.applicationinsights.azure.com/v1/traces"
        );
        assert_eq!(
            config.metrics_endpoint,
            "https://eastus.in.applicationinsights.azure.com/v1/metrics"
        );
        assert_eq!(
            config.logs_endpoint,
            "https://eastus.in.applicationinsights.azure.com/v1/logs"
        );
        assert_eq!(
            config.auth_header,
            Some(("x-ms-ikey".to_string(), "abc-123".to_string()))
        );
    }

    #[test]
    fn test_parse_connection_string_trailing_slash() {
        let cs = "InstrumentationKey=key1;IngestionEndpoint=https://example.com/";
        let config = parse_connection_string(cs).unwrap();
        assert_eq!(config.traces_endpoint, "https://example.com/v1/traces");
        assert_eq!(config.metrics_endpoint, "https://example.com/v1/metrics");
        assert_eq!(config.logs_endpoint, "https://example.com/v1/logs");
    }

    #[test]
    fn test_parse_connection_string_no_endpoint() {
        let cs = "InstrumentationKey=key1";
        let config = parse_connection_string(cs).unwrap();
        assert_eq!(
            config.traces_endpoint,
            "https://dc.services.visualstudio.com/v1/traces"
        );
        assert_eq!(
            config.auth_header,
            Some(("x-ms-ikey".to_string(), "key1".to_string()))
        );
    }

    #[test]
    fn test_parse_generic_otlp_endpoint() {
        let cs = "https://my-otel-collector:4318";
        let config = parse_connection_string(cs).unwrap();
        assert_eq!(
            config.traces_endpoint,
            "https://my-otel-collector:4318/v1/traces"
        );
        assert_eq!(
            config.metrics_endpoint,
            "https://my-otel-collector:4318/v1/metrics"
        );
        assert_eq!(
            config.logs_endpoint,
            "https://my-otel-collector:4318/v1/logs"
        );
        assert!(config.auth_header.is_none());
    }

    #[test]
    fn test_parse_empty_connection_string() {
        assert!(parse_connection_string("").is_none());
        assert!(parse_connection_string("  ").is_none());
    }

    #[test]
    fn test_parse_no_ikey_returns_none() {
        let cs = "IngestionEndpoint=https://example.com";
        assert!(parse_connection_string(cs).is_none());
    }

    #[test]
    fn test_collector_suite_lifecycle() {
        let config = TelemetryConfig {
            traces_endpoint: "https://test.example.com/v1/traces".to_string(),
            metrics_endpoint: "https://test.example.com/v1/metrics".to_string(),
            logs_endpoint: "https://test.example.com/v1/logs".to_string(),
            auth_header: Some(("x-ms-ikey".to_string(), "test-key".to_string())),
            service_name: "test-service".to_string(),
        };
        let mut collector = TelemetryCollector::new(config, "test.http");
        collector.suite_start(2);
        collector.block_complete(
            "Test A",
            "test",
            None,
            "passed",
            100,
            Some("GET"),
            Some("https://api.example.com/health"),
            Some(200),
            Some(95),
            &[("status == 200".to_string(), "200".to_string(), "200".to_string(), true)],
            &[],
            None,
        );
        collector.block_complete(
            "Test B",
            "test",
            Some("group1"),
            "failed",
            250,
            Some("POST"),
            Some("https://api.example.com/users"),
            Some(400),
            Some(240),
            &[("status == 201".to_string(), "201".to_string(), "400".to_string(), false)],
            &[],
            Some("Assertion failed"),
        );
        collector.suite_complete(1, 1, 0, 350);

        // Verify traces
        let traces = collector.build_traces();
        let spans = &traces.resource_spans[0].scope_spans[0].spans;
        assert_eq!(spans.len(), 5); // root + 2 blocks + 2 HTTP

        // Verify metrics
        let metrics = collector.build_metrics();
        let metric_list = &metrics.resource_metrics[0].scope_metrics[0].metrics;
        assert!(metric_list.len() >= 4); // suite.runs, suite.duration, block.runs, block.duration

        // Verify logs
        let logs = collector.build_logs();
        let log_records = &logs.resource_logs[0].scope_logs[0].log_records;
        assert!(log_records.len() >= 4); // start + 2 completions + suite complete

        // Verify suite outcome
        assert_eq!(collector.suite_outcome.as_deref(), Some("fail"));
    }

    #[test]
    fn test_metric_dimension_cardinality() {
        let config = TelemetryConfig {
            traces_endpoint: "https://test.example.com/v1/traces".to_string(),
            metrics_endpoint: "https://test.example.com/v1/metrics".to_string(),
            logs_endpoint: "https://test.example.com/v1/logs".to_string(),
            auth_header: Some(("x-ms-ikey".to_string(), "key".to_string())),
            service_name: "test".to_string(),
        };
        let mut collector = TelemetryCollector::new(config, "api.http");
        collector.suite_start(3);

        // 3 blocks of different types
        for (name, bt, outcome) in [("Setup", "setup", "passed"), ("Test", "test", "failed"), ("Cleanup", "teardown", "passed")] {
            collector.block_complete(name, bt, None, outcome, 50, None, None, None, None, &[], &[], None);
        }
        collector.suite_complete(2, 1, 0, 150);

        let metrics = collector.build_metrics();
        let metric_list = &metrics.resource_metrics[0].scope_metrics[0].metrics;

        // block.runs should have 3 data points (one per block_type × outcome combo)
        let block_runs = metric_list.iter().find(|m| m.name == "rp.block.runs").unwrap();
        assert_eq!(block_runs.sum.as_ref().unwrap().data_points.len(), 3);
    }

    #[test]
    fn test_parse_resource_id_returns_none() {
        let rid = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/my-rg/providers/microsoft.insights/components/my-ai";
        assert!(parse_connection_string(rid).is_none());
    }

    #[test]
    fn test_fetch_otlp_endpoints_parses_response() {
        let json = r#"{
            "properties": {
                "OTLPMetricsEndpoint": "https://managed-my-ai-dce.region.metrics.ingest.monitor.azure.com/dataCollectionRules/dcr-xxx/streams/Microsoft-OtelMetrics/otlp/v1/metrics",
                "OTLPLogsEndpoint": "https://managed-my-ai-dce.region.ingest.monitor.azure.com/dataCollectionRules/dcr-xxx/streams/Microsoft-OTLP-Logs/otlp/v1/logs",
                "OTLPTracesEndpoint": "https://managed-my-ai-dce.region.ingest.monitor.azure.com/dataCollectionRules/dcr-xxx/streams/Microsoft-OTLP-Traces/otlp/v1/traces"
            }
        }"#;
        let (traces, metrics, logs) = parse_arm_response(json).unwrap();
        assert_eq!(
            traces,
            "https://managed-my-ai-dce.region.ingest.monitor.azure.com/dataCollectionRules/dcr-xxx/streams/Microsoft-OTLP-Traces/otlp/v1/traces"
        );
        assert_eq!(
            metrics,
            "https://managed-my-ai-dce.region.metrics.ingest.monitor.azure.com/dataCollectionRules/dcr-xxx/streams/Microsoft-OtelMetrics/otlp/v1/metrics"
        );
        assert_eq!(
            logs,
            "https://managed-my-ai-dce.region.ingest.monitor.azure.com/dataCollectionRules/dcr-xxx/streams/Microsoft-OTLP-Logs/otlp/v1/logs"
        );
    }

    #[test]
    fn test_parse_arm_response_missing_properties() {
        let json = r#"{"id": "/subscriptions/..."}"#;
        let err = parse_arm_response(json).unwrap_err();
        assert!(err.contains("missing 'properties'"));
    }

    #[test]
    fn test_parse_arm_response_missing_endpoint() {
        let json = r#"{"properties": {"OTLPTracesEndpoint": "https://t"}}"#;
        let err = parse_arm_response(json).unwrap_err();
        assert!(err.contains("OTLPMetricsEndpoint"));
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex::encode(&[0xab, 0xcd, 0xef]), "abcdef");
        assert_eq!(hex::encode(&[0x00, 0xff]), "00ff");
    }

    // ── parse_arm_response edge cases ──

    #[test]
    fn test_parse_arm_response_malformed_json() {
        let err = parse_arm_response("not json at all {{{").unwrap_err();
        assert!(err.contains("Invalid JSON"));
    }

    #[test]
    fn test_parse_arm_response_missing_traces_endpoint() {
        let json = r#"{"properties": {
            "OTLPMetricsEndpoint": "https://m",
            "OTLPLogsEndpoint": "https://l"
        }}"#;
        let err = parse_arm_response(json).unwrap_err();
        assert!(err.contains("OTLPTracesEndpoint"));
    }

    #[test]
    fn test_parse_arm_response_missing_logs_endpoint() {
        let json = r#"{"properties": {
            "OTLPTracesEndpoint": "https://t",
            "OTLPMetricsEndpoint": "https://m"
        }}"#;
        let err = parse_arm_response(json).unwrap_err();
        assert!(err.contains("OTLPLogsEndpoint"));
    }

    #[test]
    fn test_parse_arm_response_null_endpoint_values() {
        let json = r#"{"properties": {
            "OTLPTracesEndpoint": null,
            "OTLPMetricsEndpoint": "https://m",
            "OTLPLogsEndpoint": "https://l"
        }}"#;
        let err = parse_arm_response(json).unwrap_err();
        assert!(err.contains("OTLPTracesEndpoint"));
    }

    #[test]
    fn test_parse_arm_response_empty_properties() {
        let json = r#"{"properties": {}}"#;
        let err = parse_arm_response(json).unwrap_err();
        assert!(err.contains("OTLPTracesEndpoint"));
    }

    // ── parse_connection_string auth_header consistency ──

    #[test]
    fn test_parse_connection_string_trailing_slash_has_auth() {
        let cs = "InstrumentationKey=key1;IngestionEndpoint=https://example.com/";
        let config = parse_connection_string(cs).unwrap();
        assert_eq!(
            config.auth_header,
            Some(("x-ms-ikey".to_string(), "key1".to_string()))
        );
    }

    #[test]
    fn test_parse_connection_string_no_endpoint_all_signals() {
        let cs = "InstrumentationKey=key1";
        let config = parse_connection_string(cs).unwrap();
        assert_eq!(
            config.metrics_endpoint,
            "https://dc.services.visualstudio.com/v1/metrics"
        );
        assert_eq!(
            config.logs_endpoint,
            "https://dc.services.visualstudio.com/v1/logs"
        );
    }

    #[test]
    fn test_parse_otlp_url_trailing_slash() {
        let cs = "https://my-collector:4318/";
        let config = parse_connection_string(cs).unwrap();
        assert_eq!(config.traces_endpoint, "https://my-collector:4318/v1/traces");
        assert_eq!(config.metrics_endpoint, "https://my-collector:4318/v1/metrics");
        assert_eq!(config.logs_endpoint, "https://my-collector:4318/v1/logs");
        assert!(config.auth_header.is_none());
    }

    // ── Collector per-signal endpoint routing ──

    #[test]
    fn test_collector_stores_distinct_per_signal_endpoints() {
        let config = TelemetryConfig {
            traces_endpoint: "https://traces.host/v1/traces".to_string(),
            metrics_endpoint: "https://metrics.host/v1/metrics".to_string(),
            logs_endpoint: "https://logs.host/v1/logs".to_string(),
            auth_header: Some(("Authorization".to_string(), "Bearer tok".to_string())),
            service_name: "svc".to_string(),
        };
        let collector = TelemetryCollector::new(config, "f.http");

        assert_eq!(collector.config.traces_endpoint, "https://traces.host/v1/traces");
        assert_eq!(collector.config.metrics_endpoint, "https://metrics.host/v1/metrics");
        assert_eq!(collector.config.logs_endpoint, "https://logs.host/v1/logs");
        assert_eq!(
            collector.config.auth_header,
            Some(("Authorization".to_string(), "Bearer tok".to_string()))
        );
    }

    #[test]
    fn test_collector_build_payloads_with_distinct_endpoints() {
        let config = TelemetryConfig {
            traces_endpoint: "https://traces.host/v1/traces".to_string(),
            metrics_endpoint: "https://metrics.host/v1/metrics".to_string(),
            logs_endpoint: "https://logs.host/v1/logs".to_string(),
            auth_header: None,
            service_name: "svc".to_string(),
        };
        let mut collector = TelemetryCollector::new(config, "t.http");
        collector.suite_start(1);
        collector.block_complete(
            "A", "test", None, "passed", 10,
            Some("GET"), Some("https://api/x"), Some(200), Some(8),
            &[], &[], None,
        );
        collector.suite_complete(1, 0, 0, 10);

        // Payloads build without error and contain data
        let t = collector.build_traces();
        assert!(!t.resource_spans[0].scope_spans[0].spans.is_empty());

        let m = collector.build_metrics();
        assert!(!m.resource_metrics[0].scope_metrics[0].metrics.is_empty());

        let l = collector.build_logs();
        assert!(!l.resource_logs[0].scope_logs[0].log_records.is_empty());

        // Config still holds the distinct endpoints for export routing
        assert_ne!(collector.config.traces_endpoint, collector.config.metrics_endpoint);
        assert_ne!(collector.config.metrics_endpoint, collector.config.logs_endpoint);
    }

    #[test]
    fn test_parse_nonsense_string_returns_none() {
        assert!(parse_connection_string("random-garbage").is_none());
    }

    #[test]
    fn test_build_exp_buckets() {
        // Scale 0: bucket boundaries at 2^i
        // Values: 100ms, 200ms, 500ms, 1000ms
        // log2(100) ≈ 6.64 → ceil = 7, log2(200) ≈ 7.64 → ceil = 8
        // log2(500) ≈ 8.97 → ceil = 9, log2(1000) ≈ 9.97 → ceil = 10
        let (scale, positive, zero_count) = build_exp_buckets(&[100.0, 200.0, 500.0, 1000.0]);
        assert_eq!(scale, 0);
        assert_eq!(zero_count, 0);
        assert_eq!(positive.offset, 7);  // min index
        assert_eq!(positive.bucket_counts, vec![1, 1, 1, 1]); // one in each bucket 7..=10
    }

    #[test]
    fn test_build_exp_buckets_with_zeros() {
        let (scale, positive, zero_count) = build_exp_buckets(&[0.0, 50.0, 0.0]);
        assert_eq!(scale, 0);
        assert_eq!(zero_count, 2);
        assert_eq!(positive.bucket_counts.iter().sum::<u64>(), 1);
    }

    #[test]
    fn test_build_exp_buckets_empty() {
        let (scale, positive, zero_count) = build_exp_buckets(&[]);
        assert_eq!(scale, 0);
        assert_eq!(zero_count, 0);
        assert!(positive.bucket_counts.is_empty());
    }

    #[test]
    fn test_build_exp_buckets_same_value() {
        // All same value → single bucket
        let (_, positive, _) = build_exp_buckets(&[256.0, 256.0, 256.0]);
        assert_eq!(positive.bucket_counts.len(), 1);
        assert_eq!(positive.bucket_counts[0], 3);
    }

    #[test]
    fn test_exp_histogram_in_metrics() {
        let config = TelemetryConfig {
            traces_endpoint: "https://test/v1/traces".to_string(),
            metrics_endpoint: "https://test/v1/metrics".to_string(),
            logs_endpoint: "https://test/v1/logs".to_string(),
            auth_header: None,
            service_name: "test".to_string(),
        };
        let mut c = TelemetryCollector::new(config, "test.http");
        c.suite_start(1);
        c.block_complete(
            "my-test", "test", Some("grp"), "passed", 150,
            Some("GET"), Some("https://example.com"), Some(200), Some(150),
            &[], &[], None,
        );
        c.suite_complete(1, 0, 0, 150);
        let m = c.build_metrics();
        let metrics = &m.resource_metrics[0].scope_metrics[0].metrics;
        // Find the duration metric
        let dur = metrics.iter().find(|m| m.name == "rp.block.duration").unwrap();
        assert!(dur.exponential_histogram.is_some());
        let hist = dur.exponential_histogram.as_ref().unwrap();
        assert_eq!(hist.aggregation_temporality, 1); // DELTA
        assert_eq!(hist.data_points.len(), 1);
        let dp = &hist.data_points[0];
        assert_eq!(dp.count, 1);
        assert_eq!(dp.sum, 150.0);
        assert_eq!(dp.scale, 0);
    }
}
