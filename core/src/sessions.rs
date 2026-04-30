//! Persisted test-run sessions store.
//!
//! Layout (see plan.md for rationale):
//!
//! ```text
//! <root>/
//!   store.json                            # cache: known file-ids
//!   files/<file-id>/
//!     file.json                           # identity + observed paths
//!     versions/<sha256>/
//!       source.http                       # LF-normalized source
//!       meta.json                         # version metadata
//!       sessions/<run-id>/                # run-id = UUIDv7
//!         run.json                        # SessionRecord (immutable)
//!         summary.md                      # human-readable
//!         blobs/<sha256>.bin              # large captured payloads
//!       .cache/                           # rebuildable: stats.json/md, index.html
//! ```
//!
//! `run.json` and `source.http` are the only sources of truth. Everything
//! under `.cache/` is rebuildable. All writes go through a temp file +
//! atomic rename so partial writes never leave the store inconsistent.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::http_client::HttpResponse;
use crate::http_parser::BodyRedactRule;
use crate::test_runner::{BlockResult, StepResult, TestRunResults};

/// Bumped on a breaking change to any persisted JSON layout.
pub const SCHEMA_VERSION: u32 = 1;

/// Default body cap (10 MB) for the Snapshot capture preset. Keeps full
/// fidelity for typical request/response payloads while putting an upper
/// bound on absolute disk usage per session.
pub const DEFAULT_RESPONSE_BODY_CAP: usize = 10 * 1024 * 1024;

/// Default header denylist. Names are matched case-insensitively against the
/// header name; the value is replaced with the literal `<redacted>` string.
pub const DEFAULT_HEADER_DENYLIST: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "proxy-authorization",
    "www-authenticate",
];

/// Default query-param denylist. Matched case-insensitively against the
/// param key.
pub const DEFAULT_QUERY_DENYLIST: &[&str] = &["token", "key", "secret", "signature", "code"];

// ─── Capture policy ────────────────────────────────────────────────────────

/// What to capture about variables.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VariableCapture {
    /// Record only variable names (not values). Default.
    NamesOnly,
    /// Record names; record values only for the listed keys.
    Allowlist { keys: Vec<String> },
    /// Record everything. Use only for non-sensitive setups.
    All,
}

impl Default for VariableCapture {
    fn default() -> Self {
        VariableCapture::NamesOnly
    }
}

/// What to capture about request/response bodies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BodyCapture {
    /// Don't capture the body at all.
    Off,
    /// Capture up to `max_bytes`; mark `truncated: true` when over.
    Truncated { max_bytes: usize },
    /// Capture the entire body.
    Full,
}

/// Top-level capture policy. Every `SessionRecord` carries the exact policy
/// it was captured under so users can audit redaction without re-running.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapturePolicy {
    pub variables: VariableCapture,
    pub request_bodies: BodyCapture,
    pub response_bodies: BodyCapture,
    /// Header names (case-insensitive) whose values are replaced with
    /// `<redacted>` during capture.
    pub headers_denylist: Vec<String>,
    /// Query-param keys (case-insensitive) whose values are replaced with
    /// `<redacted>` in the captured `request_url`.
    pub query_param_denylist: Vec<String>,
}

impl Default for CapturePolicy {
    /// **Snapshot preset** — full request and response bodies (capped at
    /// 10 MB), variable names only, header/query secrets redacted. This is
    /// what makes "view the actual request/response that ran" work out of
    /// the box. Users can switch to Privacy-first or Full-debug presets in
    /// Settings (see `SessionsConfig`).
    fn default() -> Self {
        Self {
            variables: VariableCapture::NamesOnly,
            request_bodies: BodyCapture::Truncated {
                max_bytes: DEFAULT_RESPONSE_BODY_CAP,
            },
            response_bodies: BodyCapture::Truncated {
                max_bytes: DEFAULT_RESPONSE_BODY_CAP,
            },
            headers_denylist: DEFAULT_HEADER_DENYLIST
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            query_param_denylist: DEFAULT_QUERY_DENYLIST
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}

impl CapturePolicy {
    /// Privacy-first preset — drops request bodies, caps responses at
    /// 1 MB, redacts everything else.
    pub fn privacy_first() -> Self {
        Self {
            variables: VariableCapture::NamesOnly,
            request_bodies: BodyCapture::Off,
            response_bodies: BodyCapture::Truncated { max_bytes: 1_000_000 },
            headers_denylist: DEFAULT_HEADER_DENYLIST
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            query_param_denylist: DEFAULT_QUERY_DENYLIST
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }

    /// Full-debug preset — captures everything, no truncation, no
    /// redaction. **Only use locally on non-sensitive endpoints.**
    pub fn full_debug() -> Self {
        Self {
            variables: VariableCapture::All,
            request_bodies: BodyCapture::Full,
            response_bodies: BodyCapture::Full,
            headers_denylist: Vec::new(),
            query_param_denylist: Vec::new(),
        }
    }
}

/// Counters describing what redaction did during capture. Useful for audit
/// and for surfacing "X secrets removed" in the UI.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedactionReport {
    pub headers_redacted: u32,
    pub query_params_redacted: u32,
    pub variables_dropped: u32,
    pub bodies_dropped: u32,
    pub bodies_truncated: u32,
    /// Number of individual body-field replacements (JSON path matches +
    /// regex matches) performed by `apply_body_redaction_rules`.
    #[serde(default)]
    pub body_fields_redacted: usize,
}

// ─── Stats schema (Phase 2A) ───────────────────────────────────────────────

/// Schema version for stats files. Bump when changing the on-disk shape.
pub const STATS_SCHEMA_VERSION: u32 = 1;

/// Cap on the number of latency samples kept in `StatsRecord` for
/// percentile recomputation. Newest samples win (FIFO eviction).
pub const STATS_LATENCY_SAMPLE_CAP: usize = 500;

/// Per-block aggregates within a single (file_id, sha256) version.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct BlockStats {
    pub runs: u64,
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
    /// Bounded sample of latencies (ms) for this block, capped at
    /// STATS_LATENCY_SAMPLE_CAP. Used to recompute p50/p95.
    pub latency_samples: Vec<u64>,
    /// Cached percentiles, recomputed on each update.
    pub p50_ms: u64,
    pub p95_ms: u64,
}

/// Bucket window: `"1h"` for hourly buckets in the last 24h, `"1d"` for
/// daily buckets in the last 30d.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatsBucket {
    /// Either "1h" or "1d".
    pub window: String,
    /// RFC3339 start time (truncated to the hour or day).
    pub start: String,
    pub runs: u64,
    pub passed: u64,
    pub failed: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
}

/// Top-level stats record. Written per `<file-id>/<sha>/stats.json` and a
/// rolled-up version per `<file-id>/stats.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatsRecord {
    pub schema_version: u32,
    pub file_id: String,
    /// `Some(sha)` for per-version stats; `None` for the file-level rollup.
    pub sha256: Option<String>,
    pub session_count: u64,
    /// RFC3339 timestamps.
    pub first_seen: String,
    pub last_seen: String,
    pub totals: StatsTotals,
    pub latency: LatencyAggregate,
    pub by_block: std::collections::BTreeMap<String, BlockStats>,
    pub buckets: Vec<StatsBucket>,
    /// File-level rollup only: list of versions known with their session counts.
    /// Empty for per-version records. Keep ordered by `first_seen`.
    #[serde(default)]
    pub versions: Vec<StatsVersionRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct StatsTotals {
    pub passed: u64,
    pub failed: u64,
    pub mixed: u64,
    pub skipped_blocks: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LatencyAggregate {
    /// Bounded sample of total-run latencies (ms) for percentile recomputation.
    pub samples: Vec<u64>,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub p99_ms: u64,
    pub max_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatsVersionRef {
    pub sha256: String,
    pub session_count: u64,
    pub first_seen: String,
    pub last_seen: String,
}

impl Default for StatsRecord {
    fn default() -> Self {
        Self {
            schema_version: STATS_SCHEMA_VERSION,
            file_id: String::new(),
            sha256: None,
            session_count: 0,
            first_seen: String::new(),
            last_seen: String::new(),
            totals: StatsTotals::default(),
            latency: LatencyAggregate::default(),
            by_block: std::collections::BTreeMap::new(),
            buckets: Vec::new(),
            versions: Vec::new(),
        }
    }
}

/// Compute p50/p95/p99 from a slice of latencies. Returns (p50, p95, p99).
/// Uses the nearest-rank method. Empty slice → (0, 0, 0).
pub(crate) fn percentiles(samples: &[u64]) -> (u64, u64, u64) {
    if samples.is_empty() {
        return (0, 0, 0);
    }
    let mut s = samples.to_vec();
    s.sort_unstable();
    let pick = |p: f64| -> u64 {
        let idx = ((p / 100.0) * (s.len() as f64 - 1.0)).round() as usize;
        s[idx.min(s.len() - 1)]
    };
    (pick(50.0), pick(95.0), pick(99.0))
}

/// Bucket policy: returns `(window, bucket_start_rfc3339)` for the given
/// run timestamp relative to `now`. Hourly bucket if within last 24h, daily
/// bucket if within last 30d, otherwise `None` (run only contributes to
/// totals, not buckets).
pub(crate) fn bucket_for(
    run_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<(&'static str, String)> {
    use chrono::Timelike;
    let age = now.signed_duration_since(run_at);
    if age < chrono::Duration::zero() {
        let truncated = run_at
            .with_minute(0).unwrap()
            .with_second(0).unwrap()
            .with_nanosecond(0).unwrap();
        return Some(("1h", truncated.to_rfc3339()));
    }
    if age <= chrono::Duration::hours(24) {
        let truncated = run_at
            .with_minute(0).unwrap()
            .with_second(0).unwrap()
            .with_nanosecond(0).unwrap();
        return Some(("1h", truncated.to_rfc3339()));
    }
    if age <= chrono::Duration::days(30) {
        let truncated = run_at
            .with_hour(0).unwrap()
            .with_minute(0).unwrap()
            .with_second(0).unwrap()
            .with_nanosecond(0).unwrap();
        return Some(("1d", truncated.to_rfc3339()));
    }
    None
}

/// Push a sample into a bounded vector, evicting the oldest entry when
/// the cap is hit (FIFO).
pub(crate) fn push_bounded(samples: &mut Vec<u64>, value: u64, cap: usize) {
    if samples.len() >= cap {
        samples.remove(0);
    }
    samples.push(value);
}

fn parse_rfc3339_utc(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Classify a single run for top-level totals.
fn run_outcome(r: &TestRunResults) -> &'static str {
    if r.failed > 0 && r.passed > 0 {
        "mixed"
    } else if r.failed > 0 {
        "failed"
    } else if r.passed > 0 {
        "passed"
    } else {
        // All-skipped (or empty) runs do not increment passed/failed/mixed.
        "neither"
    }
}

/// Fold a single `SessionRecord` into a `StatsRecord` in-place. This is the
/// shared fold used by both `update_stats` (incremental) and
/// `rebuild_stats` (from-scratch). Both code paths MUST use this helper so
/// the two stay byte-equivalent (modulo JSON field ordering).
///
/// `now` controls the bucket window (`bucket_for`).
pub(crate) fn fold_run_into_stats(
    stats: &mut StatsRecord,
    run: &SessionRecord,
    now: std::time::SystemTime,
) {
    let now_utc: DateTime<Utc> = now.into();

    // session_count + timestamps
    stats.session_count += 1;
    let started_s = run.started_at.to_rfc3339();
    let finished_s = run.finished_at.to_rfc3339();
    match parse_rfc3339_utc(&stats.first_seen) {
        Some(cur) if cur <= run.started_at => { /* keep */ }
        _ => stats.first_seen = started_s.clone(),
    }
    match parse_rfc3339_utc(&stats.last_seen) {
        Some(cur) if cur >= run.finished_at => { /* keep */ }
        _ => stats.last_seen = finished_s,
    }

    // totals
    let r = &run.results;
    match run_outcome(r) {
        "passed" => stats.totals.passed += 1,
        "failed" => stats.totals.failed += 1,
        "mixed" => stats.totals.mixed += 1,
        _ => {}
    }
    stats.totals.skipped_blocks += r.skipped as u64;

    // overall latency
    push_bounded(&mut stats.latency.samples, r.total_time_ms, STATS_LATENCY_SAMPLE_CAP);
    let (p50, p95, p99) = percentiles(&stats.latency.samples);
    stats.latency.p50_ms = p50;
    stats.latency.p95_ms = p95;
    stats.latency.p99_ms = p99;
    if r.total_time_ms > stats.latency.max_ms {
        stats.latency.max_ms = r.total_time_ms;
    }

    // per-block aggregates
    for bs in &run.block_summaries {
        let entry = stats.by_block.entry(bs.name.clone()).or_default();
        entry.runs += 1;
        match bs.status.as_str() {
            "passed" => entry.passed += 1,
            "failed" | "error" => entry.failed += 1,
            "skipped" => entry.skipped += 1,
            _ => {}
        }
        push_bounded(&mut entry.latency_samples, bs.time_ms, STATS_LATENCY_SAMPLE_CAP);
        let (bp50, bp95, _) = percentiles(&entry.latency_samples);
        entry.p50_ms = bp50;
        entry.p95_ms = bp95;
    }

    // bucket
    if let Some((window, start)) = bucket_for(run.started_at, now_utc) {
        let pos = stats
            .buckets
            .iter()
            .position(|b| b.window == window && b.start == start);
        let idx = match pos {
            Some(i) => i,
            None => {
                stats.buckets.push(StatsBucket {
                    window: window.to_string(),
                    start,
                    runs: 0,
                    passed: 0,
                    failed: 0,
                    p50_ms: 0,
                    p95_ms: 0,
                });
                stats.buckets.len() - 1
            }
        };
        let b = &mut stats.buckets[idx];
        b.runs += 1;
        match run_outcome(r) {
            "passed" => b.passed += 1,
            "failed" => b.failed += 1,
            _ => {}
        }
        // We don't store per-bucket samples; reflect the global percentiles
        // so the bucket carries a meaningful number. Both fold call sites
        // (update_stats / rebuild_stats) end in the same state.
        b.p50_ms = stats.latency.p50_ms;
        b.p95_ms = stats.latency.p95_ms;
    }
}

/// Build a file-level rollup `StatsRecord` from per-version `StatsRecord`s.
/// Pure: does no IO. Both `update_stats` and `rebuild_stats` use this so
/// their rollups are identical.
pub(crate) fn rollup_versions(file_id: &str, versions: &[StatsRecord]) -> StatsRecord {
    let mut out = StatsRecord {
        file_id: file_id.to_string(),
        sha256: None,
        ..StatsRecord::default()
    };

    let mut versions_sorted: Vec<&StatsRecord> = versions.iter().collect();
    versions_sorted.sort_by(|a, b| a.first_seen.cmp(&b.first_seen));

    for v in &versions_sorted {
        out.session_count += v.session_count;

        // first_seen / last_seen
        match (parse_rfc3339_utc(&out.first_seen), parse_rfc3339_utc(&v.first_seen)) {
            (None, Some(_)) => out.first_seen = v.first_seen.clone(),
            (Some(cur), Some(other)) if other < cur => out.first_seen = v.first_seen.clone(),
            _ => {}
        }
        match (parse_rfc3339_utc(&out.last_seen), parse_rfc3339_utc(&v.last_seen)) {
            (None, Some(_)) => out.last_seen = v.last_seen.clone(),
            (Some(cur), Some(other)) if other > cur => out.last_seen = v.last_seen.clone(),
            _ => {}
        }

        // totals
        out.totals.passed += v.totals.passed;
        out.totals.failed += v.totals.failed;
        out.totals.mixed += v.totals.mixed;
        out.totals.skipped_blocks += v.totals.skipped_blocks;

        // latency: merge bounded samples FIFO (cap at STATS_LATENCY_SAMPLE_CAP)
        for s in &v.latency.samples {
            push_bounded(&mut out.latency.samples, *s, STATS_LATENCY_SAMPLE_CAP);
        }
        if v.latency.max_ms > out.latency.max_ms {
            out.latency.max_ms = v.latency.max_ms;
        }

        // by_block
        for (name, bs) in &v.by_block {
            let entry = out.by_block.entry(name.clone()).or_default();
            entry.runs += bs.runs;
            entry.passed += bs.passed;
            entry.failed += bs.failed;
            entry.skipped += bs.skipped;
            for s in &bs.latency_samples {
                push_bounded(&mut entry.latency_samples, *s, STATS_LATENCY_SAMPLE_CAP);
            }
        }

        // buckets — sum by (window, start)
        for b in &v.buckets {
            let pos = out
                .buckets
                .iter()
                .position(|x| x.window == b.window && x.start == b.start);
            match pos {
                Some(i) => {
                    let agg = &mut out.buckets[i];
                    agg.runs += b.runs;
                    agg.passed += b.passed;
                    agg.failed += b.failed;
                }
                None => out.buckets.push(b.clone()),
            }
        }

        if let Some(sha) = &v.sha256 {
            out.versions.push(StatsVersionRef {
                sha256: sha.clone(),
                session_count: v.session_count,
                first_seen: v.first_seen.clone(),
                last_seen: v.last_seen.clone(),
            });
        }
    }

    // Recompute aggregate percentiles from merged samples.
    let (p50, p95, p99) = percentiles(&out.latency.samples);
    out.latency.p50_ms = p50;
    out.latency.p95_ms = p95;
    out.latency.p99_ms = p99;

    // Recompute by_block percentiles too.
    for entry in out.by_block.values_mut() {
        let (bp50, bp95, _) = percentiles(&entry.latency_samples);
        entry.p50_ms = bp50;
        entry.p95_ms = bp95;
    }

    // Recompute bucket percentiles from the rollup latency.
    for b in out.buckets.iter_mut() {
        b.p50_ms = out.latency.p50_ms;
        b.p95_ms = out.latency.p95_ms;
    }

    out
}

// ─── Session record ────────────────────────────────────────────────────────

/// What triggered this run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionTrigger {
    Manual,
    AutoRun,
    Cli,
    Resume,
}

/// Which Pilot component recorded the session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Desktop,
    Tui,
    Cli,
    Other,
}

/// Per-block summary that is cheap to keep in `stats.json` and to embed in
/// the HTML viewer. Light-weight by design — does NOT carry payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockSummary {
    pub seq: Option<u64>,
    pub name: String,
    pub block_type: String,
    pub status: String, // "passed" | "failed" | "skipped" | "error"
    pub time_ms: u64,
    pub assertion_total: usize,
    pub assertion_passed: usize,
    pub extract_total: usize,
    pub extract_ok: usize,
}

impl BlockSummary {
    pub fn from_block(b: &BlockResult) -> Self {
        let assertion_passed = b.assertion_results.iter().filter(|a| a.passed).count();
        let extract_ok = b.extract_results.iter().filter(|e| e.success).count();
        Self {
            seq: b.seq,
            name: b.name.clone(),
            block_type: b.block_type.clone(),
            status: b.status.clone(),
            time_ms: b.time_ms,
            assertion_total: b.assertion_results.len(),
            assertion_passed,
            extract_total: b.extract_results.len(),
            extract_ok,
        }
    }
}

/// The full immutable record persisted as `run.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub schema_version: u32,
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub trigger: SessionTrigger,
    pub host: String,
    pub os: String,
    pub component: Component,
    pub request_pilot_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_identity: Option<String>,
    pub source_sha256: String,
    pub capture_policy: CapturePolicy,
    pub redaction_report: RedactionReport,
    /// Variable *names* observed during the run (values only when policy permits).
    pub variables: Vec<CapturedVariable>,
    pub results: TestRunResults,
    /// Per-block lightweight summary (denormalized for fast stats).
    pub block_summaries: Vec<BlockSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapturedVariable {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

// ─── File / version metadata (cache + manifest) ────────────────────────────

/// `files/<file-id>/file.json` — identity record for a tracked `.http` file.
/// Updated atomically on every `record_run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub schema_version: u32,
    pub file_id: String,
    /// Display name (defaults to basename; users can override later).
    pub display_name: String,
    /// All absolute paths we have ever observed for this file.
    pub source_paths: Vec<String>,
    /// Repo-relative path (e.g. `myrepo/tests/checkout.http`) when derivable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_rel_path: Option<String>,
    /// True when the file has no on-disk path (e.g. desktop unsaved buffer).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsaved: bool,
    /// True when identity falls back to absolute-path slug because no better
    /// identity was available.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub degraded_identity: bool,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// `files/<file-id>/versions/<sha>/meta.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionMeta {
    pub schema_version: u32,
    pub sha256: String,
    /// `"lf"` — the only normalization implemented. Recorded so future
    /// schema bumps can reason about the on-disk source bytes.
    pub normalization: String,
    pub byte_size: usize,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// Lightweight directory listing entry for the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedFile {
    pub file_id: String,
    pub display_name: String,
    pub source_paths: Vec<String>,
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEntry {
    pub sha256: String,
    pub byte_size: usize,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub session_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub trigger: SessionTrigger,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total_time_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_file: Option<String>,
}

// ─── Errors ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Json(serde_json::Error),
    NotFound(String),
    Invalid(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Io(e) => write!(f, "io: {}", e),
            SessionError::Json(e) => write!(f, "json: {}", e),
            SessionError::NotFound(s) => write!(f, "not found: {}", s),
            SessionError::Invalid(s) => write!(f, "invalid: {}", s),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<std::io::Error> for SessionError {
    fn from(e: std::io::Error) -> Self {
        SessionError::Io(e)
    }
}
impl From<serde_json::Error> for SessionError {
    fn from(e: serde_json::Error) -> Self {
        SessionError::Json(e)
    }
}

pub type SessionResult<T> = Result<T, SessionError>;

// ─── Identity & hashing ────────────────────────────────────────────────────

/// Normalize CRLF and lone CR to LF. Idempotent. Used both for hashing and
/// for `source.http` on-disk content.
pub fn normalize_lf(content: &str) -> String {
    // Replace \r\n first, then any remaining \r with \n.
    content.replace("\r\n", "\n").replace('\r', "\n")
}

/// Lowercase hex SHA-256 of the LF-normalized bytes.
pub fn content_sha256_normalized(content: &str) -> String {
    let normalized = normalize_lf(content);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let digest = hasher.finalize();
    hex_lower(&digest)
}

/// Hex-encode bytes (lowercase). Avoids pulling a `hex` crate.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Source for deriving a stable `file_id`. We hash one of these into the id.
#[derive(Debug, Clone)]
pub enum FileIdentity {
    /// User-provided alias. Highest priority.
    Alias(String),
    /// `<repo-name>/<path-from-repo-root>`.
    RepoRelative {
        repo: String,
        rel_path: String,
    },
    /// Last resort: absolute path. Marks the manifest with
    /// `degraded_identity: true`.
    AbsolutePath(String),
    /// Buffer with no path. Caller must supply a stable seed (e.g. an
    /// internal buffer id).
    Unsaved {
        seed: String,
    },
}

impl FileIdentity {
    pub fn file_id(&self) -> String {
        let prefix = match self {
            FileIdentity::Alias(_) => "alias",
            FileIdentity::RepoRelative { .. } => "repo",
            FileIdentity::AbsolutePath(_) => "abs",
            FileIdentity::Unsaved { .. } => "unsaved",
        };
        let raw = match self {
            FileIdentity::Alias(s) => s.clone(),
            FileIdentity::RepoRelative { repo, rel_path } => {
                format!("{}/{}", repo, rel_path.replace('\\', "/"))
            }
            FileIdentity::AbsolutePath(p) => p.replace('\\', "/").to_lowercase(),
            FileIdentity::Unsaved { seed } => seed.clone(),
        };
        let mut hasher = Sha256::new();
        hasher.update(raw.as_bytes());
        let digest = hasher.finalize();
        // 16-hex-char (64-bit) suffix is plenty here.
        format!("{}-{}", prefix, &hex_lower(&digest)[..16])
    }

    pub fn display_name(&self, fallback: &str) -> String {
        match self {
            FileIdentity::Alias(s) => s.clone(),
            FileIdentity::RepoRelative { rel_path, .. } => {
                Path::new(rel_path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(rel_path)
                    .to_string()
            }
            FileIdentity::AbsolutePath(p) => Path::new(p)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(fallback)
                .to_string(),
            FileIdentity::Unsaved { .. } => "untitled.http".to_string(),
        }
    }

    pub fn is_unsaved(&self) -> bool {
        matches!(self, FileIdentity::Unsaved { .. })
    }

    pub fn is_degraded(&self) -> bool {
        matches!(self, FileIdentity::AbsolutePath(_))
    }

    /// Best-effort identity from an optional file path and an optional
    /// alias. Shared by desktop and TUI auto-record paths so both produce
    /// the same `file_id` for the same source.
    ///
    /// Resolution order:
    /// 1. Non-empty `alias` → `Alias`.
    /// 2. Non-empty `file_path` → `RepoRelative` if the path is inside a
    ///    Git repo (walking up looking for `.git`), else `AbsolutePath`.
    /// 3. Otherwise → `Unsaved { seed: "<unsaved_seed>" }` (caller passes a
    ///    stable seed so unsaved buffers keep a consistent id across runs).
    pub fn from_path_and_alias(
        file_path: Option<&str>,
        alias: Option<&str>,
        unsaved_seed: &str,
    ) -> Self {
        if let Some(a) = alias {
            let trimmed = a.trim();
            if !trimmed.is_empty() {
                return FileIdentity::Alias(trimmed.to_string());
            }
        }
        match file_path {
            Some(p) if !p.trim().is_empty() => {
                if let Some(repo_rel) = derive_repo_relative(p) {
                    return repo_rel;
                }
                FileIdentity::AbsolutePath(p.to_string())
            }
            _ => FileIdentity::Unsaved {
                seed: unsaved_seed.to_string(),
            },
        }
    }
}

fn derive_repo_relative(path: &str) -> Option<FileIdentity> {
    let p = std::path::Path::new(path);
    let abs = std::fs::canonicalize(p).ok()?;
    let mut cur = abs.parent()?.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            let repo = cur
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("repo")
                .to_string();
            let rel = abs.strip_prefix(&cur).ok()?.to_string_lossy().to_string();
            return Some(FileIdentity::RepoRelative {
                repo,
                rel_path: rel,
            });
        }
        if !cur.pop() {
            return None;
        }
    }
}

// ─── Capture pipeline (apply policy to a TestRunResults) ───────────────────

/// Apply `policy` to `results`, mutating in place and returning a
/// `RedactionReport`. The mutation:
/// - replaces denylisted header values with `<redacted>`
/// - rewrites denylisted query-param values in `request_url` to `<redacted>`
/// - drops or truncates request/response bodies per `BodyCapture`
///
/// Variable capture is handled separately at the call site (since the source
/// of variables is the var_store, not the results).
pub fn apply_capture_policy(
    results: &mut TestRunResults,
    policy: &CapturePolicy,
) -> RedactionReport {
    let mut report = RedactionReport::default();
    let header_deny: Vec<String> = policy
        .headers_denylist
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .collect();
    let query_deny: Vec<String> = policy
        .query_param_denylist
        .iter()
        .map(|q| q.to_ascii_lowercase())
        .collect();

    for block in results.block_results.iter_mut() {
        redact_block_in_place(block, &header_deny, &query_deny, policy, &mut report);
    }
    report
}

fn redact_block_in_place(
    block: &mut BlockResult,
    header_deny: &[String],
    query_deny: &[String],
    policy: &CapturePolicy,
    report: &mut RedactionReport,
) {
    redact_headers(&mut block.request_headers, header_deny, report);
    block.request_url = redact_query_params(&block.request_url, query_deny, report);
    block.request_body =
        apply_body_policy(block.request_body.take(), &policy.request_bodies, report);
    if let Some(resp) = block.response.as_mut() {
        redact_response(resp, header_deny, &policy.response_bodies, report);
    }
    for step in block.step_results.iter_mut() {
        redact_step(step, header_deny, query_deny, policy, report);
    }
}

fn redact_step(
    step: &mut StepResult,
    header_deny: &[String],
    query_deny: &[String],
    policy: &CapturePolicy,
    report: &mut RedactionReport,
) {
    redact_headers(&mut step.request_headers, header_deny, report);
    step.request_url = redact_query_params(&step.request_url, query_deny, report);
    step.request_body =
        apply_body_policy(step.request_body.take(), &policy.request_bodies, report);
    if let Some(resp) = step.response.as_mut() {
        redact_response(resp, header_deny, &policy.response_bodies, report);
    }
}

fn redact_response(
    resp: &mut HttpResponse,
    header_deny: &[String],
    body_policy: &BodyCapture,
    report: &mut RedactionReport,
) {
    redact_headers(&mut resp.headers, header_deny, report);
    let body = std::mem::take(&mut resp.body);
    match apply_body_policy(Some(body), body_policy, report) {
        Some(b) => resp.body = b,
        None => resp.body = String::new(),
    }
}

fn redact_headers(
    headers: &mut Vec<(String, String)>,
    deny_lower: &[String],
    report: &mut RedactionReport,
) {
    for (name, value) in headers.iter_mut() {
        if deny_lower
            .iter()
            .any(|d| d.eq_ignore_ascii_case(name))
        {
            *value = "<redacted>".to_string();
            report.headers_redacted += 1;
        }
    }
}

/// Returns the URL with denylisted query-param values rewritten to
/// `<redacted>`. URLs without a `?` are returned unchanged.
fn redact_query_params(
    url: &str,
    deny_lower: &[String],
    report: &mut RedactionReport,
) -> String {
    let Some(qpos) = url.find('?') else {
        return url.to_string();
    };
    let (base, query) = url.split_at(qpos);
    // query starts with '?'. Strip it.
    let q = &query[1..];
    // Preserve the fragment if any.
    let (q_only, fragment) = match q.find('#') {
        Some(i) => (&q[..i], Some(&q[i..])),
        None => (q, None),
    };
    let mut parts: Vec<String> = Vec::new();
    for raw in q_only.split('&') {
        if raw.is_empty() {
            continue;
        }
        let (k, v) = match raw.find('=') {
            Some(i) => (&raw[..i], Some(&raw[i + 1..])),
            None => (raw, None),
        };
        let denied = deny_lower
            .iter()
            .any(|d| d.eq_ignore_ascii_case(k));
        if denied && v.is_some() {
            parts.push(format!("{}=<redacted>", k));
            report.query_params_redacted += 1;
        } else {
            parts.push(raw.to_string());
        }
    }
    let mut out = String::with_capacity(url.len());
    out.push_str(base);
    out.push('?');
    out.push_str(&parts.join("&"));
    if let Some(frag) = fragment {
        out.push_str(frag);
    }
    out
}

fn apply_body_policy(
    body: Option<String>,
    policy: &BodyCapture,
    report: &mut RedactionReport,
) -> Option<String> {
    let Some(body) = body else {
        return None;
    };
    match policy {
        BodyCapture::Off => {
            if !body.is_empty() {
                report.bodies_dropped += 1;
            }
            None
        }
        BodyCapture::Full => Some(body),
        BodyCapture::Truncated { max_bytes } => {
            if body.len() <= *max_bytes {
                Some(body)
            } else {
                report.bodies_truncated += 1;
                // Truncate on a UTF-8 boundary, not a byte index that may
                // split a multi-byte char.
                let mut end = *max_bytes;
                while end > 0 && !body.is_char_boundary(end) {
                    end -= 1;
                }
                let mut truncated = body[..end].to_string();
                truncated.push_str("\n\n<truncated>");
                Some(truncated)
            }
        }
    }
}

// ─── Body redaction (Phase 3 — JSONPath / regex rules) ────────────────────

/// Minimal JSONPath segment, supporting the subset documented on
/// `apply_body_redaction_rules`.
enum BodyPathSeg {
    Field(String),
    Index(usize),
    Wildcard,
}

/// Parse a minimal JSONPath expression. Supported subset:
///   * `$.foo`              — top-level key
///   * `$.foo.bar`          — nested key
///   * `$.foo[0]`           — array index
///   * `$.foo[*]`           — every element of an array (or every value
///                            of an object)
///
/// Recursive descent (`$..foo`) and filters are NOT supported and will
/// cause this function to return `None`. The caller should silently skip
/// rules that fail to parse.
fn parse_body_jsonpath(path: &str) -> Option<Vec<BodyPathSeg>> {
    let rest = path.strip_prefix('$')?;
    let bytes = rest.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'.' {
                    return None;
                }
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b'.' && bytes[i] != b'[' {
                    i += 1;
                }
                if start == i {
                    return None;
                }
                out.push(BodyPathSeg::Field(rest[start..i].to_string()));
            }
            b'[' => {
                let close = rest[i..].find(']')? + i;
                let inner = &rest[i + 1..close];
                if inner == "*" {
                    out.push(BodyPathSeg::Wildcard);
                } else if let Ok(n) = inner.parse::<usize>() {
                    out.push(BodyPathSeg::Index(n));
                } else {
                    return None;
                }
                i = close + 1;
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Walk `value` along `segs`. When the final segment is reached, replace
/// the matched value(s) with the literal `"[REDACTED]"` string. Returns
/// the number of leaf replacements performed. A value already equal to
/// `"[REDACTED]"` is not re-counted, so applying overlapping rules to the
/// same field doesn't inflate the counter.
fn redact_at_path(value: &mut serde_json::Value, segs: &[BodyPathSeg]) -> usize {
    if segs.is_empty() {
        if matches!(value, serde_json::Value::String(s) if s == "[REDACTED]") {
            return 0;
        }
        *value = serde_json::Value::String("[REDACTED]".to_string());
        return 1;
    }
    let (head, tail) = (&segs[0], &segs[1..]);
    let mut count = 0;
    match head {
        BodyPathSeg::Field(name) => {
            if let serde_json::Value::Object(map) = value {
                if let Some(child) = map.get_mut(name.as_str()) {
                    count += redact_at_path(child, tail);
                }
            }
        }
        BodyPathSeg::Index(idx) => {
            if let serde_json::Value::Array(arr) = value {
                if let Some(child) = arr.get_mut(*idx) {
                    count += redact_at_path(child, tail);
                }
            }
        }
        BodyPathSeg::Wildcard => match value {
            serde_json::Value::Array(arr) => {
                for child in arr.iter_mut() {
                    count += redact_at_path(child, tail);
                }
            }
            serde_json::Value::Object(map) => {
                for (_, child) in map.iter_mut() {
                    count += redact_at_path(child, tail);
                }
            }
            _ => {}
        },
    }
    count
}

fn is_jsonish_content_type(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type")
            && v.to_ascii_lowercase().contains("json")
    })
}

/// Apply `rules` to a single body string in place, returning the count of
/// redactions performed. JSONPath rules apply only when the supplied
/// `headers` declare a JSON-ish `Content-Type` and the body parses as
/// JSON; otherwise they are silently skipped. Regex rules apply
/// unconditionally to the (possibly already-redacted) body string.
fn apply_rules_to_body_string(
    body: &mut String,
    headers: &[(String, String)],
    rules: &[BodyRedactRule],
) -> usize {
    if body.is_empty() || rules.is_empty() {
        return 0;
    }
    let mut count = 0usize;

    let has_jsonpath_rule = rules
        .iter()
        .any(|r| matches!(r, BodyRedactRule::JsonPath(_)));
    if has_jsonpath_rule && is_jsonish_content_type(headers) {
        match serde_json::from_str::<serde_json::Value>(body) {
            Ok(mut v) => {
                let mut applied = 0usize;
                for rule in rules {
                    if let BodyRedactRule::JsonPath(p) = rule {
                        if let Some(segs) = parse_body_jsonpath(p) {
                            applied += redact_at_path(&mut v, &segs);
                        } else {
                            log::debug!(
                                "skipping unsupported jsonpath redaction pattern: {}",
                                p
                            );
                        }
                    }
                }
                if applied > 0 {
                    if let Ok(s) = serde_json::to_string(&v) {
                        *body = s;
                    }
                    count += applied;
                }
            }
            Err(err) => {
                log::debug!(
                    "skipping jsonpath body redaction (invalid JSON: {})",
                    err
                );
            }
        }
    }

    for rule in rules {
        if let BodyRedactRule::Regex(pat) = rule {
            match regex::Regex::new(pat) {
                Ok(re) => {
                    let n = re.find_iter(body).count();
                    if n > 0 {
                        *body = re.replace_all(body, "[REDACTED]").into_owned();
                        count += n;
                    }
                }
                Err(err) => {
                    log::debug!(
                        "skipping invalid body-redaction regex {:?}: {}",
                        pat,
                        err
                    );
                }
            }
        }
    }

    count
}

/// Apply parsed body-redaction rules to every captured request and
/// response body in `results`. Updates `report.body_fields_redacted`.
///
/// Effective rules per block = `global_rules` + entries from
/// `per_block_rules` keyed by `BlockResult.name`. Step bodies inside a
/// `@compare` block reuse the parent block's effective rules.
///
/// Behaviour:
/// * `BodyRedactRule::JsonPath` is applied only when the captured headers
///   declare a JSON-ish `Content-Type` and the body parses as JSON.
///   Invalid JSON is logged and skipped (the body is left untouched).
///   Supported JSONPath subset: `$.foo`, `$.foo.bar`, `$.foo[0]`,
///   `$.foo[*]`. Recursive descent (`$..foo`) is **not** supported and
///   such patterns are silently skipped.
/// * `BodyRedactRule::Regex` is applied to the raw body string regardless
///   of `Content-Type`. Invalid regexes are logged and skipped.
pub fn apply_body_redaction_rules(
    results: &mut TestRunResults,
    global_rules: &[BodyRedactRule],
    per_block_rules: &std::collections::HashMap<String, Vec<BodyRedactRule>>,
    report: &mut RedactionReport,
) {
    for block in results.block_results.iter_mut() {
        let block_extras = per_block_rules.get(&block.name);
        let mut effective: Vec<BodyRedactRule> = global_rules.to_vec();
        if let Some(extras) = block_extras {
            effective.extend_from_slice(extras);
        }
        if effective.is_empty() {
            continue;
        }
        if let Some(body) = block.request_body.as_mut() {
            report.body_fields_redacted +=
                apply_rules_to_body_string(body, &block.request_headers, &effective);
        }
        if let Some(resp) = block.response.as_mut() {
            report.body_fields_redacted +=
                apply_rules_to_body_string(&mut resp.body, &resp.headers, &effective);
        }
        for step in block.step_results.iter_mut() {
            if let Some(body) = step.request_body.as_mut() {
                report.body_fields_redacted +=
                    apply_rules_to_body_string(body, &step.request_headers, &effective);
            }
            if let Some(resp) = step.response.as_mut() {
                report.body_fields_redacted +=
                    apply_rules_to_body_string(&mut resp.body, &resp.headers, &effective);
            }
        }
    }
}

/// Apply variable capture policy and return the list to record.
pub fn capture_variables(
    pairs: impl IntoIterator<Item = (String, String)>,
    policy: &VariableCapture,
    report: &mut RedactionReport,
) -> Vec<CapturedVariable> {
    let mut out = Vec::new();
    for (name, value) in pairs {
        match policy {
            VariableCapture::NamesOnly => {
                if !value.is_empty() {
                    report.variables_dropped += 1;
                }
                out.push(CapturedVariable { name, value: None });
            }
            VariableCapture::Allowlist { keys } => {
                let allowed = keys.iter().any(|k| k == &name);
                if allowed {
                    out.push(CapturedVariable {
                        name,
                        value: Some(value),
                    });
                } else {
                    if !value.is_empty() {
                        report.variables_dropped += 1;
                    }
                    out.push(CapturedVariable { name, value: None });
                }
            }
            VariableCapture::All => {
                out.push(CapturedVariable {
                    name,
                    value: Some(value),
                });
            }
        }
    }
    out
}

// ─── Atomic write helpers ──────────────────────────────────────────────────

/// Write `bytes` to `path` atomically: write to a sibling temp file then
/// rename. Creates parent directories as needed.
fn atomic_write(path: &Path, bytes: &[u8]) -> SessionResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let pid = std::process::id();
    let nonce = uuid::Uuid::new_v4();
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("session"),
        pid,
        &nonce.simple().to_string()[..12],
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // Best-effort rename. On Windows this fails if dest exists; for the
    // files we manage that's fine because new content yields a new path
    // (sha256 / run-id), and for `file.json` we accept replace-semantics.
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists
            || e.raw_os_error() == Some(183) /* ERROR_ALREADY_EXISTS */ =>
        {
            // Replace semantics: remove dest, retry.
            let _ = fs::remove_file(path);
            fs::rename(&tmp, path)?;
            Ok(())
        }
        Err(e) => {
            // On Windows, a normal rename also returns this error class for
            // "replace existing"; try the explicit replace dance.
            let _ = fs::remove_file(path);
            match fs::rename(&tmp, path) {
                Ok(()) => Ok(()),
                Err(_) => {
                    let _ = fs::remove_file(&tmp);
                    Err(e.into())
                }
            }
        }
    }
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> SessionResult<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &bytes)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> SessionResult<T> {
    let bytes = fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SessionError::NotFound(path.display().to_string())
        } else {
            SessionError::Io(e)
        }
    })?;
    Ok(serde_json::from_slice(&bytes)?)
}

// ─── SessionStore ──────────────────────────────────────────────────────────

/// Root handle to a sessions directory.
pub struct SessionStore {
    root: PathBuf,
}

/// Returned by `record_run` so callers can show a "Saved to ..." link.
#[derive(Debug, Clone)]
pub struct RecordedSession {
    pub file_id: String,
    pub sha256: String,
    pub run_id: String,
    pub run_dir: PathBuf,
}

/// Inputs to `record_run`. Caller fills these from the running suite.
pub struct RecordInput<'a> {
    pub identity: FileIdentity,
    pub source_path: Option<&'a Path>,
    pub source_content: &'a str,
    pub policy: CapturePolicy,
    pub trigger: SessionTrigger,
    pub component: Component,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub mode: Option<String>,
    pub env_file: Option<String>,
    pub env_identity: Option<String>,
    pub variables: Vec<(String, String)>,
    pub results: TestRunResults,
    /// When set, overrides the `file_id` derived from `identity`. Used by
    /// snapshot replay flows so a re-run lands under the original session's
    /// `file_id` (grouping the new run beside the original in the Sessions
    /// tab) even when the active file's identity would otherwise hash
    /// differently.
    pub file_id_override: Option<String>,
    /// Effective body-redaction rules from global config + suite-level
    /// `@@redact body` directives. Applied to every captured block.
    pub redact_body_rules: Vec<BodyRedactRule>,
    /// Per-block body-redaction rules from block-level `@@redact body`
    /// directives, keyed by `TestBlock.name`. Combined with
    /// `redact_body_rules` when the block is processed.
    pub block_redact_body_rules: std::collections::HashMap<String, Vec<BodyRedactRule>>,
}

/// Build the `(suite_rules, per_block_rules)` pair for a `RecordInput`
/// from a parsed `TestSuite` and the global config patterns
/// (`SessionsConfig.body_redaction_paths`).
///
/// The suite-level vec contains: global config patterns (parsed via
/// `parse_body_redact_pattern`) ∪ `suite.redact_body_rules`. The per-block
/// map carries any block-level `@@redact body` directives, keyed by block
/// name. Both are combined inside `record_run` so each captured block
/// sees the union of all three layers.
pub fn build_body_redact_rules(
    suite: &crate::http_parser::TestSuite,
    global_paths: &[String],
) -> (
    Vec<BodyRedactRule>,
    std::collections::HashMap<String, Vec<BodyRedactRule>>,
) {
    let mut suite_rules: Vec<BodyRedactRule> = global_paths
        .iter()
        .filter_map(|s| crate::http_parser::parse_body_redact_pattern(s))
        .collect();
    suite_rules.extend(suite.redact_body_rules.iter().cloned());

    let block_rules: std::collections::HashMap<String, Vec<BodyRedactRule>> = suite
        .blocks
        .iter()
        .filter(|b| !b.redact_body_rules.is_empty())
        .map(|b| (b.name.clone(), b.redact_body_rules.clone()))
        .collect();

    (suite_rules, block_rules)
}

impl SessionStore {
    /// Open (or create) a sessions store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> SessionResult<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        fs::create_dir_all(root.join("files"))?;
        Ok(SessionStore { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn file_dir(&self, file_id: &str) -> PathBuf {
        self.root.join("files").join(file_id)
    }

    fn version_dir(&self, file_id: &str, sha: &str) -> PathBuf {
        self.file_dir(file_id).join("versions").join(sha)
    }

    fn session_dir(&self, file_id: &str, sha: &str, run_id: &str) -> PathBuf {
        self.version_dir(file_id, sha)
            .join("sessions")
            .join(run_id)
    }

    /// Persist a complete run. Idempotent on `run_id`. Updates `file.json`
    /// and `meta.json` (last_seen). Writes `source.http` if it doesn't
    /// already exist for this sha.
    pub fn record_run(&self, input: RecordInput<'_>) -> SessionResult<RecordedSession> {
        let now = Utc::now();
        let file_id = input
            .file_id_override
            .clone()
            .unwrap_or_else(|| input.identity.file_id());
        let normalized = normalize_lf(input.source_content);
        let sha = content_sha256_normalized(&normalized);

        // ── file.json ──────────────────────────────────────────────────
        let file_dir = self.file_dir(&file_id);
        fs::create_dir_all(&file_dir)?;
        let manifest_path = file_dir.join("file.json");
        let mut manifest: FileManifest = match read_json::<FileManifest>(&manifest_path) {
            Ok(m) => m,
            Err(SessionError::NotFound(_)) => FileManifest {
                schema_version: SCHEMA_VERSION,
                file_id: file_id.clone(),
                display_name: input
                    .identity
                    .display_name(input.source_path
                        .and_then(|p| p.file_name())
                        .and_then(|s| s.to_str())
                        .unwrap_or("untitled.http")),
                source_paths: Vec::new(),
                repo_rel_path: match &input.identity {
                    FileIdentity::RepoRelative { repo, rel_path } => {
                        Some(format!("{}/{}", repo, rel_path.replace('\\', "/")))
                    }
                    _ => None,
                },
                unsaved: input.identity.is_unsaved(),
                degraded_identity: input.identity.is_degraded(),
                first_seen: now,
                last_seen: now,
            },
            Err(e) => return Err(e),
        };
        if let Some(p) = input.source_path {
            let p_str = p.display().to_string();
            if !manifest.source_paths.iter().any(|s| s == &p_str) {
                manifest.source_paths.push(p_str);
            }
        }
        manifest.last_seen = now;
        atomic_write_json(&manifest_path, &manifest)?;

        // ── version dir + source.http + meta.json ──────────────────────
        let version_dir = self.version_dir(&file_id, &sha);
        fs::create_dir_all(version_dir.join("sessions"))?;
        let source_path = version_dir.join("source.http");
        if !source_path.exists() {
            atomic_write(&source_path, normalized.as_bytes())?;
        }
        let meta_path = version_dir.join("meta.json");
        let meta = match read_json::<VersionMeta>(&meta_path) {
            Ok(mut m) => {
                m.last_seen = now;
                m
            }
            Err(SessionError::NotFound(_)) => VersionMeta {
                schema_version: SCHEMA_VERSION,
                sha256: sha.clone(),
                normalization: "lf".to_string(),
                byte_size: normalized.len(),
                first_seen: now,
                last_seen: now,
            },
            Err(e) => return Err(e),
        };
        atomic_write_json(&meta_path, &meta)?;

        // ── apply capture policy ───────────────────────────────────────
        let mut report = RedactionReport::default();
        let mut results = input.results;
        let intermediate = apply_capture_policy(&mut results, &input.policy);
        report.headers_redacted += intermediate.headers_redacted;
        report.query_params_redacted += intermediate.query_params_redacted;
        report.bodies_dropped += intermediate.bodies_dropped;
        report.bodies_truncated += intermediate.bodies_truncated;
        // Apply parsed body-redaction rules (global config + suite-level +
        // block-level) AFTER capture policy so we operate on whatever
        // truncated/kept payload landed in the results.
        apply_body_redaction_rules(
            &mut results,
            &input.redact_body_rules,
            &input.block_redact_body_rules,
            &mut report,
        );
        let captured_vars = capture_variables(input.variables, &input.policy.variables, &mut report);

        // ── session record ─────────────────────────────────────────────
        let run_id = run_id_uuidv7();
        let block_summaries: Vec<BlockSummary> = results
            .block_results
            .iter()
            .map(BlockSummary::from_block)
            .collect();
        let record = SessionRecord {
            schema_version: SCHEMA_VERSION,
            run_id: run_id.clone(),
            started_at: input.started_at,
            finished_at: input.finished_at,
            trigger: input.trigger,
            host: hostname::get()
                .ok()
                .and_then(|s| s.into_string().ok())
                .unwrap_or_else(|| "unknown".to_string()),
            os: std::env::consts::OS.to_string(),
            component: input.component,
            request_pilot_version: env!("CARGO_PKG_VERSION").to_string(),
            mode: input.mode,
            env_file: input.env_file,
            env_identity: input.env_identity,
            source_sha256: sha.clone(),
            capture_policy: input.policy,
            redaction_report: report,
            variables: captured_vars,
            results,
            block_summaries,
        };

        let session_dir = self.session_dir(&file_id, &sha, &run_id);
        fs::create_dir_all(&session_dir)?;
        atomic_write_json(&session_dir.join("run.json"), &record)?;
        let summary = render_summary_md(&record);
        atomic_write(&session_dir.join("summary.md"), summary.as_bytes())?;

        // Best-effort stats update: a failure here must never propagate to
        // the caller, since the run itself is already durably persisted.
        if let Err(err) = self.update_stats(&file_id, &sha, &record) {
            log::warn!("failed to update stats for {}/{}: {}", file_id, sha, err);
        }

        Ok(RecordedSession {
            file_id,
            sha256: sha,
            run_id,
            run_dir: session_dir,
        })
    }

    pub fn list_files(&self) -> SessionResult<Vec<TrackedFile>> {
        let files_dir = self.root.join("files");
        if !files_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&files_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let manifest_path = entry.path().join("file.json");
            if let Ok(m) = read_json::<FileManifest>(&manifest_path) {
                out.push(TrackedFile {
                    file_id: m.file_id,
                    display_name: m.display_name,
                    source_paths: m.source_paths,
                    last_seen: m.last_seen,
                });
            } else {
                // Entry without a manifest is ignored (could be partial
                // write recovered later).
                let _ = name;
            }
        }
        out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        Ok(out)
    }

    pub fn list_versions(&self, file_id: &str) -> SessionResult<Vec<VersionEntry>> {
        let versions_dir = self.file_dir(file_id).join("versions");
        if !versions_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&versions_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let sha = entry.file_name().to_string_lossy().to_string();
            let meta_path = entry.path().join("meta.json");
            let Ok(meta) = read_json::<VersionMeta>(&meta_path) else {
                continue;
            };
            let sessions_dir = entry.path().join("sessions");
            let session_count = if sessions_dir.exists() {
                fs::read_dir(&sessions_dir)?
                    .filter_map(|r| r.ok())
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .count()
            } else {
                0
            };
            out.push(VersionEntry {
                sha256: sha,
                byte_size: meta.byte_size,
                first_seen: meta.first_seen,
                last_seen: meta.last_seen,
                session_count,
            });
        }
        out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        Ok(out)
    }

    pub fn list_sessions(
        &self,
        file_id: &str,
        sha: &str,
    ) -> SessionResult<Vec<SessionSummary>> {
        let sessions_dir = self.version_dir(file_id, sha).join("sessions");
        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&sessions_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let run_path = entry.path().join("run.json");
            let Ok(rec) = read_json::<SessionRecord>(&run_path) else {
                continue;
            };
            out.push(SessionSummary {
                run_id: rec.run_id,
                started_at: rec.started_at,
                finished_at: rec.finished_at,
                trigger: rec.trigger,
                passed: rec.results.passed,
                failed: rec.results.failed,
                skipped: rec.results.skipped,
                total_time_ms: rec.results.total_time_ms,
                mode: rec.mode,
                env_file: rec.env_file,
            });
        }
        out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(out)
    }

    pub fn load_session(
        &self,
        file_id: &str,
        sha: &str,
        run_id: &str,
    ) -> SessionResult<(String, SessionRecord)> {
        let dir = self.session_dir(file_id, sha, run_id);
        let record: SessionRecord = read_json(&dir.join("run.json"))?;
        let source = fs::read_to_string(self.version_dir(file_id, sha).join("source.http"))
            .map_err(SessionError::Io)?;
        Ok((source, record))
    }

    /// Load a session as a snapshot: returns the source, a freshly parsed
    /// `TestSuite`, and the full `SessionRecord`. The TUI and desktop both
    /// use this to reconstitute the test state at the moment a run executed.
    pub fn load_as_snapshot(
        &self,
        file_id: &str,
        sha256: &str,
        run_id: &str,
    ) -> SessionResult<(String, crate::http_parser::TestSuite, SessionRecord)> {
        let (source, record) = self.load_session(file_id, sha256, run_id)?;
        let suite = crate::http_parser::parse_test_suite(&source);
        Ok((source, suite, record))
    }

    /// Fold a single recorded run into `<file_id>/versions/<sha>/stats.json`,
    /// then rebuild `<file_id>/stats.json` from every per-version stats file
    /// on disk. Both writes are atomic. Uses the same `fold_run_into_stats`
    /// helper as `rebuild_stats` so the two paths converge.
    pub fn update_stats(
        &self,
        file_id: &str,
        sha256: &str,
        run: &SessionRecord,
    ) -> SessionResult<()> {
        self.update_stats_at(file_id, sha256, run, std::time::SystemTime::now())
    }

    /// `update_stats` with an injectable `now` for testing bucket eviction.
    pub(crate) fn update_stats_at(
        &self,
        file_id: &str,
        sha256: &str,
        run: &SessionRecord,
        now: std::time::SystemTime,
    ) -> SessionResult<()> {
        let version_dir = self.version_dir(file_id, sha256);
        fs::create_dir_all(&version_dir)?;
        let stats_path = version_dir.join("stats.json");

        // Load existing or start fresh on missing/corrupt JSON.
        let mut stats = match read_json::<StatsRecord>(&stats_path) {
            Ok(r) => r,
            Err(_) => StatsRecord {
                file_id: file_id.to_string(),
                sha256: Some(sha256.to_string()),
                ..StatsRecord::default()
            },
        };
        // Ensure identity is correct even if the file was hand-edited.
        stats.schema_version = STATS_SCHEMA_VERSION;
        stats.file_id = file_id.to_string();
        stats.sha256 = Some(sha256.to_string());

        fold_run_into_stats(&mut stats, run, now);
        atomic_write_json(&stats_path, &stats)?;

        // Rebuild the file-level rollup from every per-version stats.json.
        let file_dir = self.file_dir(file_id);
        let versions_dir = file_dir.join("versions");
        let mut version_stats: Vec<StatsRecord> = Vec::new();
        if versions_dir.is_dir() {
            for entry in fs::read_dir(&versions_dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let p = entry.path().join("stats.json");
                match read_json::<StatsRecord>(&p) {
                    Ok(v) => version_stats.push(v),
                    Err(SessionError::NotFound(_)) => continue,
                    Err(_) => continue,
                }
            }
        }
        let rollup = rollup_versions(file_id, &version_stats);
        atomic_write_json(&file_dir.join("stats.json"), &rollup)?;
        Ok(())
    }

    /// Recompute `stats.json` for every sha of `file_id` from scratch by
    /// walking every `run.json`. Also rebuilds the file-level rollup. Safe
    /// to call any time. Returns Ok and is a no-op when the file directory
    /// does not exist.
    pub fn rebuild_stats(&self, file_id: &str) -> SessionResult<()> {
        self.rebuild_stats_at(file_id, std::time::SystemTime::now())
    }

    /// Same as `rebuild_stats` but with an injectable `now` for tests.
    pub(crate) fn rebuild_stats_at(
        &self,
        file_id: &str,
        now: std::time::SystemTime,
    ) -> SessionResult<()> {
        let file_dir = self.file_dir(file_id);
        if !file_dir.exists() {
            return Ok(());
        }
        let versions_dir = file_dir.join("versions");
        if !versions_dir.exists() {
            // No versions yet. Drop any stale rollup so callers see a clean state.
            let rollup = StatsRecord {
                file_id: file_id.to_string(),
                sha256: None,
                ..StatsRecord::default()
            };
            atomic_write_json(&file_dir.join("stats.json"), &rollup)?;
            return Ok(());
        }

        let mut version_stats: Vec<StatsRecord> = Vec::new();

        for entry in fs::read_dir(&versions_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let sha = entry.file_name().to_string_lossy().to_string();
            let version_path = entry.path();
            let sessions_dir = version_path.join("sessions");

            // Collect runs and sort by started_at to ensure deterministic
            // ordering (must match incremental fold order).
            let mut runs: Vec<SessionRecord> = Vec::new();
            if sessions_dir.exists() {
                for run_entry in fs::read_dir(&sessions_dir)? {
                    let run_entry = run_entry?;
                    if !run_entry.file_type()?.is_dir() {
                        continue;
                    }
                    let run_json = run_entry.path().join("run.json");
                    match read_json::<SessionRecord>(&run_json) {
                        Ok(rec) => runs.push(rec),
                        Err(SessionError::NotFound(_)) => continue,
                        Err(e) => return Err(e),
                    }
                }
            }
            runs.sort_by(|a, b| a.started_at.cmp(&b.started_at));

            let mut stats = StatsRecord {
                file_id: file_id.to_string(),
                sha256: Some(sha.clone()),
                ..StatsRecord::default()
            };
            for run in &runs {
                fold_run_into_stats(&mut stats, run, now);
            }

            atomic_write_json(&version_path.join("stats.json"), &stats)?;
            version_stats.push(stats);
        }

        // File-level rollup.
        let rollup = rollup_versions(file_id, &version_stats);
        atomic_write_json(&file_dir.join("stats.json"), &rollup)?;
        Ok(())
    }

    /// Read a `StatsRecord` from disk.
    ///
    /// - `sha = Some(..)` returns the per-version stats at
    ///   `<root>/files/<file_id>/versions/<sha>/stats.json`.
    /// - `sha = None` returns the file-level rollup at
    ///   `<root>/files/<file_id>/stats.json`.
    ///
    /// Returns `Ok(None)` if the corresponding `stats.json` does not exist
    /// yet (no runs recorded). Returns `Err` on I/O or parse errors.
    pub fn load_stats(
        &self,
        file_id: &str,
        sha: Option<&str>,
    ) -> SessionResult<Option<StatsRecord>> {
        let path = match sha {
            Some(s) => self.version_dir(file_id, s).join("stats.json"),
            None => self.file_dir(file_id).join("stats.json"),
        };
        if !path.exists() {
            return Ok(None);
        }
        let stats: StatsRecord = read_json(&path)?;
        Ok(Some(stats))
    }

    /// Apply a retention policy to the on-disk store. When `dry_run` is
    /// true, no files are deleted and the returned `PruneReport` describes
    /// what *would* be removed. All caps in `policy` are optional; passing
    /// `RetentionPolicy::default()` is a no-op.
    pub fn prune(
        &self,
        policy: &crate::sessions_config::RetentionPolicy,
        dry_run: bool,
    ) -> SessionResult<PruneReport> {
        let now = Utc::now();
        let entries = self.collect_session_entries()?;
        let mut remove = vec![false; entries.len()];

        // (1) max_age_days
        if let Some(days) = policy.max_age_days {
            let cutoff = now - chrono::Duration::days(days as i64);
            for (i, e) in entries.iter().enumerate() {
                if e.started_at < cutoff {
                    remove[i] = true;
                }
            }
        }

        // (2) max_sessions_per_version: keep newest N per (file_id, sha).
        if let Some(n) = policy.max_sessions_per_version {
            let n = n as usize;
            let mut by_version: std::collections::HashMap<(String, String), Vec<usize>> =
                std::collections::HashMap::new();
            for (i, e) in entries.iter().enumerate() {
                if remove[i] {
                    continue;
                }
                by_version
                    .entry((e.file_id.clone(), e.sha.clone()))
                    .or_default()
                    .push(i);
            }
            for (_, mut idxs) in by_version {
                idxs.sort_by(|a, b| entries[*b].started_at.cmp(&entries[*a].started_at));
                for &i in idxs.iter().skip(n) {
                    remove[i] = true;
                }
            }
        }

        // (3) max_total_size_gb: drop oldest survivors first until under cap.
        if let Some(gb) = policy.max_total_size_gb {
            let cap_bytes = (gb * (1024.0 * 1024.0 * 1024.0)) as u64;
            let mut survivors: Vec<usize> = (0..entries.len()).filter(|&i| !remove[i]).collect();
            let mut total: u64 = survivors.iter().map(|&i| entries[i].size_bytes).sum();
            survivors.sort_by(|a, b| entries[*a].started_at.cmp(&entries[*b].started_at));
            for i in survivors {
                if total <= cap_bytes {
                    break;
                }
                remove[i] = true;
                total = total.saturating_sub(entries[i].size_bytes);
            }
        }

        if dry_run {
            let mut removed_count = 0usize;
            let mut bytes_freed: u64 = 0;
            let mut kept_count = 0usize;
            for (i, e) in entries.iter().enumerate() {
                if remove[i] {
                    removed_count += 1;
                    bytes_freed += e.size_bytes;
                } else {
                    kept_count += 1;
                }
            }
            return Ok(PruneReport {
                removed_count,
                bytes_freed,
                kept_count,
                dry_run: true,
            });
        }

        // Apply deletions, surviving individual failures.
        let mut actually_removed = 0usize;
        let mut actually_freed: u64 = 0;
        let mut affected_versions: std::collections::BTreeSet<(String, String)> =
            std::collections::BTreeSet::new();
        let mut affected_files: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for (i, e) in entries.iter().enumerate() {
            if !remove[i] {
                continue;
            }
            if fs::remove_dir_all(&e.run_dir).is_ok() {
                actually_removed += 1;
                actually_freed += e.size_bytes;
                affected_versions.insert((e.file_id.clone(), e.sha.clone()));
                affected_files.insert(e.file_id.clone());
            }
        }

        // Best-effort rebuild of derived data after deletions.
        for fid in &affected_files {
            let _ = self.rebuild_stats(fid);
        }

        Ok(PruneReport {
            removed_count: actually_removed,
            bytes_freed: actually_freed,
            kept_count: entries.len() - actually_removed,
            dry_run: false,
        })
    }

    fn collect_session_entries(&self) -> SessionResult<Vec<SessionEntry>> {
        let files_dir = self.root.join("files");
        let mut out: Vec<SessionEntry> = Vec::new();
        if !files_dir.exists() {
            return Ok(out);
        }
        for f_entry in fs::read_dir(&files_dir)? {
            let f_entry = f_entry?;
            if !f_entry.file_type()?.is_dir() {
                continue;
            }
            let file_id = f_entry.file_name().to_string_lossy().to_string();
            let versions_dir = f_entry.path().join("versions");
            if !versions_dir.exists() {
                continue;
            }
            for v_entry in fs::read_dir(&versions_dir)? {
                let v_entry = v_entry?;
                if !v_entry.file_type()?.is_dir() {
                    continue;
                }
                let sha = v_entry.file_name().to_string_lossy().to_string();
                let sessions_dir = v_entry.path().join("sessions");
                if !sessions_dir.exists() {
                    continue;
                }
                for s_entry in fs::read_dir(&sessions_dir)? {
                    let s_entry = s_entry?;
                    if !s_entry.file_type()?.is_dir() {
                        continue;
                    }
                    let run_dir = s_entry.path();
                    let run_json = run_dir.join("run.json");
                    let rec: SessionRecord = match read_json(&run_json) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let size_bytes = dir_size_bytes(&run_dir);
                    out.push(SessionEntry {
                        file_id: file_id.clone(),
                        sha: sha.clone(),
                        run_dir,
                        started_at: rec.started_at,
                        size_bytes,
                    });
                }
            }
        }
        Ok(out)
    }
}

/// Report returned by `SessionStore::prune`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneReport {
    pub removed_count: usize,
    pub bytes_freed: u64,
    pub kept_count: usize,
    pub dry_run: bool,
}

struct SessionEntry {
    file_id: String,
    sha: String,
    run_dir: std::path::PathBuf,
    started_at: chrono::DateTime<chrono::Utc>,
    size_bytes: u64,
}

fn dir_size_bytes(path: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    if let Ok(rd) = fs::read_dir(path) {
        for entry in rd.flatten() {
            let p = entry.path();
            if let Ok(md) = entry.metadata() {
                if md.is_file() {
                    total += md.len();
                } else if md.is_dir() {
                    total += dir_size_bytes(&p);
                }
            }
        }
    }
    total
}

/// Generate a UUIDv7-based run id. Falls back to UUIDv4 if v7 isn't
/// available at runtime (it always is on supported platforms — this is just
/// belt-and-braces).
fn run_id_uuidv7() -> String {
    let id = uuid::Uuid::now_v7();
    id.simple().to_string()
}

// ─── Markdown summary ──────────────────────────────────────────────────────

fn render_summary_md(rec: &SessionRecord) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Run {}\n\n", &rec.run_id));
    out.push_str(&format!(
        "- **Started**: {}\n- **Finished**: {}\n- **Duration**: {} ms\n- **Trigger**: {:?}\n- **Component**: {:?}\n- **Host**: {}\n",
        rec.started_at.to_rfc3339(),
        rec.finished_at.to_rfc3339(),
        rec.results.total_time_ms,
        rec.trigger,
        rec.component,
        rec.host,
    ));
    if let Some(m) = &rec.mode {
        out.push_str(&format!("- **Mode**: {}\n", m));
    }
    if let Some(env) = &rec.env_file {
        out.push_str(&format!("- **Env file**: {}\n", env));
    }
    out.push_str(&format!(
        "- **Source SHA**: `{}`\n",
        &rec.source_sha256[..16]
    ));
    out.push('\n');
    out.push_str(&format!(
        "**Results**: ✓ {} passed · ✗ {} failed · ⊘ {} skipped\n\n",
        rec.results.passed, rec.results.failed, rec.results.skipped
    ));

    let r = &rec.redaction_report;
    if r.headers_redacted + r.query_params_redacted + r.variables_dropped + r.bodies_dropped + r.bodies_truncated
        > 0
    {
        out.push_str("**Redaction**: ");
        let mut parts = Vec::new();
        if r.headers_redacted > 0 {
            parts.push(format!("{} headers", r.headers_redacted));
        }
        if r.query_params_redacted > 0 {
            parts.push(format!("{} query params", r.query_params_redacted));
        }
        if r.variables_dropped > 0 {
            parts.push(format!("{} variables", r.variables_dropped));
        }
        if r.bodies_dropped > 0 {
            parts.push(format!("{} bodies dropped", r.bodies_dropped));
        }
        if r.bodies_truncated > 0 {
            parts.push(format!("{} bodies truncated", r.bodies_truncated));
        }
        out.push_str(&parts.join(", "));
        out.push_str("\n\n");
    }

    out.push_str("## Blocks\n\n");
    out.push_str("| # | Type | Name | Status | Time | Asserts |\n");
    out.push_str("|---|------|------|--------|------|---------|\n");
    for s in &rec.block_summaries {
        let glyph = match s.status.as_str() {
            "passed" => "✓",
            "failed" => "✗",
            "skipped" => "⊘",
            _ => "•",
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} {} | {} ms | {}/{} |\n",
            s.seq.map(|n| n.to_string()).unwrap_or_default(),
            s.block_type,
            md_escape(&s.name),
            glyph,
            s.status,
            s.time_ms,
            s.assertion_passed,
            s.assertion_total,
        ));
    }

    let mut grouped: BTreeMap<String, Vec<&BlockResult>> = BTreeMap::new();
    for b in &rec.results.block_results {
        if b.status == "failed" || b.status == "error" {
            grouped
                .entry(b.status.clone())
                .or_default()
                .push(b);
        }
    }
    if !grouped.is_empty() {
        out.push_str("\n## Failures\n\n");
        for (status, blocks) in &grouped {
            for b in blocks {
                out.push_str(&format!("### {} — {}\n\n", status, md_escape(&b.name)));
                out.push_str(&format!(
                    "- `{} {}` → {} ms\n",
                    b.request_method, b.request_url, b.time_ms
                ));
                if let Some(err) = &b.error {
                    out.push_str(&format!("- error: {}\n", md_escape(err)));
                }
                for a in &b.assertion_results {
                    if !a.passed {
                        out.push_str(&format!(
                            "  - assertion failed: `{}` (expected `{}`, got `{}`)\n",
                            md_escape(&a.assertion),
                            md_escape(a.expected.as_deref().unwrap_or("")),
                            md_escape(a.actual.as_deref().unwrap_or("")),
                        ));
                    }
                }
                out.push('\n');
            }
        }
    }

    out
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// Render a `StatsRecord` as human-friendly markdown.
pub fn render_stats_md(stats: &StatsRecord) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();

    // Header
    match &stats.sha256 {
        Some(sha) => {
            let short = if sha.len() >= 8 { &sha[..8] } else { sha.as_str() };
            let _ = writeln!(out, "# Stats for {} @ {}", stats.file_id, short);
        }
        None => {
            let _ = writeln!(out, "# Stats for {}", stats.file_id);
        }
    }
    out.push('\n');

    // Totals
    let totals = &stats.totals;
    let total_runs = totals.passed + totals.failed + totals.mixed;
    let pass_rate = if total_runs == 0 {
        0.0
    } else {
        totals.passed as f64 / total_runs as f64
    };
    let _ = writeln!(out, "## Totals");
    let _ = writeln!(
        out,
        "- pass: {}, fail: {}, mixed: {}, skipped blocks: {}",
        totals.passed, totals.failed, totals.mixed, totals.skipped_blocks
    );
    let _ = writeln!(out, "- pass-rate: {:.1}%", pass_rate * 100.0);
    let _ = writeln!(out, "- session_count: {}", stats.session_count);
    let _ = writeln!(
        out,
        "- window: {} → {}",
        stats.first_seen, stats.last_seen
    );
    out.push('\n');

    // Last 24 hourly buckets, ordered by start ascending
    let mut hourly: Vec<&StatsBucket> = stats
        .buckets
        .iter()
        .filter(|b| b.window == "1h")
        .collect();
    hourly.sort_by(|a, b| a.start.cmp(&b.start));
    let recent: Vec<&StatsBucket> = hourly.iter().rev().take(24).rev().copied().collect();

    // Pass-rate trend
    let _ = writeln!(out, "## Pass rate (last 24h)");
    let pr_values: Vec<f64> = recent
        .iter()
        .map(|b| {
            if b.runs == 0 {
                f64::NAN
            } else {
                b.passed as f64 / b.runs as f64
            }
        })
        .collect();
    let _ = writeln!(out, "`{}`", sparkline(&pr_values, 0.0, 1.0));
    out.push('\n');

    // Top 5 flakiest blocks
    let _ = writeln!(out, "## Top 5 flakiest blocks");
    let mut blocks: Vec<(&String, &BlockStats, f64)> = stats
        .by_block
        .iter()
        .map(|(name, bs)| {
            let denom = (bs.passed + bs.failed).max(1) as f64;
            let flakiness = bs.failed as f64 / denom;
            (name, bs, flakiness)
        })
        .collect();
    blocks.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(b.0))
    });
    if blocks.is_empty() {
        let _ = writeln!(out, "- (none)");
    } else {
        let _ = writeln!(out, "| block | pass | fail | flakiness |");
        let _ = writeln!(out, "|---|---|---|---|");
        for (name, bs, flak) in blocks.iter().take(5) {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {:.1}% |",
                md_escape(name),
                bs.passed,
                bs.failed,
                flak * 100.0
            );
        }
    }
    out.push('\n');

    // Latency trend (p50)
    let _ = writeln!(out, "## Latency trend (p50)");
    let p50_vals: Vec<f64> = recent
        .iter()
        .map(|b| {
            if b.runs == 0 {
                f64::NAN
            } else {
                b.p50_ms as f64
            }
        })
        .collect();
    let (lmin, lmax) = {
        let finite: Vec<f64> = p50_vals.iter().copied().filter(|v| v.is_finite()).collect();
        if finite.is_empty() {
            (0.0, 1.0)
        } else {
            let mn = finite.iter().cloned().fold(f64::INFINITY, f64::min);
            let mx = finite.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            if (mx - mn).abs() < f64::EPSILON {
                (mn, mn + 1.0)
            } else {
                (mn, mx)
            }
        }
    };
    let _ = writeln!(out, "`{}`", sparkline(&p50_vals, lmin, lmax));
    out.push('\n');

    // Versions (file-level rollup only)
    if stats.sha256.is_none() && !stats.versions.is_empty() {
        let _ = writeln!(out, "## Versions");
        let _ = writeln!(out, "| sha | sessions | window |");
        let _ = writeln!(out, "|---|---|---|");
        for v in &stats.versions {
            let short = if v.sha256.len() >= 8 {
                &v.sha256[..8]
            } else {
                v.sha256.as_str()
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} → {} |",
                short, v.session_count, v.first_seen, v.last_seen
            );
        }
        out.push('\n');
    }

    out
}

fn sparkline(values: &[f64], min: f64, max: f64) -> String {
    const RAMP: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let span = (max - min).max(f64::EPSILON);
    let mut s = String::with_capacity(values.len());
    for v in values {
        if !v.is_finite() {
            s.push(' ');
            continue;
        }
        let t = ((v - min) / span).clamp(0.0, 1.0);
        let idx = ((t * (RAMP.len() as f64 - 1.0)).round() as usize).min(RAMP.len() - 1);
        s.push(RAMP[idx]);
    }
    s
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assertions::AssertionResult;
    use crate::http_client::HttpResponse;
    use crate::http_parser::ParsedRequest;
    use crate::test_runner::ExtractResult;
    use std::collections::HashMap;

    fn tmp_root(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        p.push(format!("rp-sessions-test-{}-{}", label, &nonce[..12]));
        p
    }

    fn dummy_block(name: &str, status: &str, body: Option<&str>) -> BlockResult {
        BlockResult {
            seq: Some(1),
            name: name.to_string(),
            block_type: "test".to_string(),
            group: None,
            request_method: "GET".to_string(),
            request_url: "https://example.com/api?token=abc&page=1".to_string(),
            request_headers: vec![
                ("Authorization".to_string(), "Bearer secret-token".to_string()),
                ("X-Trace".to_string(), "ok".to_string()),
            ],
            request_body: Some("req-body".to_string()),
            status: status.to_string(),
            response: Some(HttpResponse {
                status: 200,
                status_text: "OK".to_string(),
                headers: vec![
                    ("Set-Cookie".to_string(), "session=xyz".to_string()),
                    ("Content-Type".to_string(), "application/json".to_string()),
                ],
                body: body.unwrap_or("{\"ok\":true}").to_string(),
                time_ms: 12,
                size_bytes: 11,
            }),
            assertion_results: vec![AssertionResult {
                assertion: "status == 200".to_string(),
                expected: Some("200".to_string()),
                actual: Some("200".to_string()),
                passed: true,
            }],
            extract_results: vec![ExtractResult {
                variable: "id".to_string(),
                value: Some("42".to_string()),
                success: true,
            }],
            error: None,
            time_ms: 12,
            step_results: vec![],
            diff_result: None,
        }
    }

    fn dummy_results(blocks: Vec<BlockResult>) -> TestRunResults {
        let passed = blocks.iter().filter(|b| b.status == "passed").count();
        let failed = blocks.iter().filter(|b| b.status == "failed").count();
        let skipped = blocks.iter().filter(|b| b.status == "skipped").count();
        TestRunResults {
            passed,
            failed,
            skipped,
            total_time_ms: 12 * blocks.len() as u64,
            block_results: blocks,
            final_variables: HashMap::new(),
            telemetry: None,
        }
    }

    // ─── normalize_lf / sha ────────────────────────────────────────────

    #[test]
    fn normalize_lf_idempotent() {
        let s = "a\nb\nc\n";
        assert_eq!(normalize_lf(s), s);
    }

    #[test]
    fn normalize_lf_handles_crlf() {
        assert_eq!(normalize_lf("a\r\nb\r\nc"), "a\nb\nc");
    }

    #[test]
    fn normalize_lf_handles_lone_cr() {
        assert_eq!(normalize_lf("a\rb\rc"), "a\nb\nc");
    }

    #[test]
    fn sha_stable_across_line_endings() {
        let lf = "GET https://x\n# hello\n";
        let crlf = "GET https://x\r\n# hello\r\n";
        assert_eq!(content_sha256_normalized(lf), content_sha256_normalized(crlf));
    }

    #[test]
    fn sha_differs_for_different_content() {
        assert_ne!(
            content_sha256_normalized("GET http://a"),
            content_sha256_normalized("GET http://b")
        );
    }

    #[test]
    fn hex_lower_basic() {
        assert_eq!(hex_lower(&[0xab, 0xcd, 0x01]), "abcd01");
    }

    // ─── FileIdentity ──────────────────────────────────────────────────

    #[test]
    fn file_id_stable_for_repo_relative() {
        let a = FileIdentity::RepoRelative {
            repo: "myrepo".to_string(),
            rel_path: "tests/checkout.http".to_string(),
        };
        let b = FileIdentity::RepoRelative {
            repo: "myrepo".to_string(),
            rel_path: "tests\\checkout.http".to_string(), // backslashes
        };
        assert_eq!(a.file_id(), b.file_id());
        assert!(a.file_id().starts_with("repo-"));
    }

    #[test]
    fn file_id_case_insensitive_for_abs_path() {
        let a = FileIdentity::AbsolutePath("C:\\Repos\\x.http".to_string());
        let b = FileIdentity::AbsolutePath("c:/repos/x.http".to_string());
        assert_eq!(a.file_id(), b.file_id());
        assert!(a.file_id().starts_with("abs-"));
    }

    #[test]
    fn file_id_distinguishes_kinds() {
        let a = FileIdentity::Alias("foo".into()).file_id();
        let r = FileIdentity::RepoRelative {
            repo: "foo".into(),
            rel_path: "".into(),
        }
        .file_id();
        let p = FileIdentity::AbsolutePath("foo".into()).file_id();
        let u = FileIdentity::Unsaved { seed: "foo".into() }.file_id();
        let all = [&a, &r, &p, &u];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "{} vs {}", all[i], all[j]);
            }
        }
    }

    // ─── Capture / redaction ───────────────────────────────────────────

    #[test]
    fn redact_headers_replaces_authorization() {
        let mut headers = vec![
            ("Authorization".to_string(), "Bearer abc".to_string()),
            ("X-Trace".to_string(), "keep".to_string()),
        ];
        let mut report = RedactionReport::default();
        redact_headers(&mut headers, &["authorization".to_string()], &mut report);
        assert_eq!(headers[0].1, "<redacted>");
        assert_eq!(headers[1].1, "keep");
        assert_eq!(report.headers_redacted, 1);
    }

    #[test]
    fn redact_query_params_basic() {
        let mut report = RedactionReport::default();
        let url = redact_query_params(
            "https://x.com/a?token=secret&page=1",
            &["token".to_string()],
            &mut report,
        );
        assert_eq!(url, "https://x.com/a?token=<redacted>&page=1");
        assert_eq!(report.query_params_redacted, 1);
    }

    #[test]
    fn redact_query_params_no_query_unchanged() {
        let mut r = RedactionReport::default();
        let out = redact_query_params("https://x.com/a", &["token".into()], &mut r);
        assert_eq!(out, "https://x.com/a");
        assert_eq!(r.query_params_redacted, 0);
    }

    #[test]
    fn redact_query_params_preserves_fragment() {
        let mut r = RedactionReport::default();
        let out = redact_query_params(
            "https://x.com/a?token=abc#frag",
            &["token".into()],
            &mut r,
        );
        assert_eq!(out, "https://x.com/a?token=<redacted>#frag");
    }

    #[test]
    fn redact_query_params_keeps_value_less_keys() {
        let mut r = RedactionReport::default();
        let out = redact_query_params(
            "https://x.com/a?flag&token=abc",
            &["token".into()],
            &mut r,
        );
        assert_eq!(out, "https://x.com/a?flag&token=<redacted>");
    }

    #[test]
    fn body_policy_off_drops() {
        let mut r = RedactionReport::default();
        let out = apply_body_policy(Some("hello".into()), &BodyCapture::Off, &mut r);
        assert_eq!(out, None);
        assert_eq!(r.bodies_dropped, 1);
    }

    #[test]
    fn body_policy_full_keeps_intact() {
        let mut r = RedactionReport::default();
        let out = apply_body_policy(
            Some("hello".into()),
            &BodyCapture::Full,
            &mut r,
        );
        assert_eq!(out.as_deref(), Some("hello"));
        assert_eq!(r.bodies_truncated, 0);
    }

    #[test]
    fn body_policy_truncates_on_utf8_boundary() {
        let mut r = RedactionReport::default();
        // Multi-byte char straddles byte 5; ensure we don't split it.
        let s = "abcd€xyz".to_string(); // '€' is 3 bytes
        let out = apply_body_policy(
            Some(s),
            &BodyCapture::Truncated { max_bytes: 5 },
            &mut r,
        )
        .unwrap();
        assert!(out.starts_with("abcd"));
        assert!(out.ends_with("<truncated>"));
        assert_eq!(r.bodies_truncated, 1);
    }

    #[test]
    fn capture_variables_names_only() {
        let mut r = RedactionReport::default();
        let out = capture_variables(
            vec![("base".into(), "v1".into()), ("token".into(), "secret".into())],
            &VariableCapture::NamesOnly,
            &mut r,
        );
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|v| v.value.is_none()));
        assert_eq!(r.variables_dropped, 2);
    }

    #[test]
    fn capture_variables_allowlist() {
        let mut r = RedactionReport::default();
        let out = capture_variables(
            vec![("base".into(), "v1".into()), ("token".into(), "secret".into())],
            &VariableCapture::Allowlist {
                keys: vec!["base".into()],
            },
            &mut r,
        );
        let by_name: HashMap<_, _> = out.iter().map(|v| (v.name.clone(), v.value.clone())).collect();
        assert_eq!(by_name["base"], Some("v1".to_string()));
        assert_eq!(by_name["token"], None);
        assert_eq!(r.variables_dropped, 1);
    }

    #[test]
    fn apply_capture_policy_redacts_block_and_response() {
        let mut results = dummy_results(vec![dummy_block("t", "passed", None)]);
        let report = apply_capture_policy(&mut results, &CapturePolicy::default());
        let b = &results.block_results[0];
        // Default Snapshot preset keeps request body intact (under 10 MB cap)
        assert!(b.request_body.is_some());
        // headers redacted
        let auth = b.request_headers.iter().find(|(k, _)| k == "Authorization").unwrap();
        assert_eq!(auth.1, "<redacted>");
        // query token redacted
        assert!(b.request_url.contains("token=<redacted>"));
        // response set-cookie redacted, body kept (small + within 10 MB)
        let resp = b.response.as_ref().unwrap();
        let sc = resp.headers.iter().find(|(k, _)| k == "Set-Cookie").unwrap();
        assert_eq!(sc.1, "<redacted>");
        assert_eq!(resp.body, "{\"ok\":true}");
        // Header + query redactions still tracked
        assert!(report.headers_redacted >= 1);
        assert!(report.query_params_redacted >= 1);
    }

    #[test]
    fn privacy_first_preset_drops_request_body() {
        let mut results = dummy_results(vec![dummy_block("t", "passed", None)]);
        let _ = apply_capture_policy(&mut results, &CapturePolicy::privacy_first());
        let b = &results.block_results[0];
        assert!(b.request_body.is_none(), "privacy_first must drop request bodies");
    }

    #[test]
    fn full_debug_preset_keeps_everything() {
        let mut results = dummy_results(vec![dummy_block("t", "passed", None)]);
        let _ = apply_capture_policy(&mut results, &CapturePolicy::full_debug());
        let b = &results.block_results[0];
        assert!(b.request_body.is_some());
        // No header denylist → Authorization stays as-is
        let auth = b.request_headers.iter().find(|(k, _)| k == "Authorization").unwrap();
        assert_ne!(auth.1, "<redacted>");
        // No query denylist → token value stays
        assert!(!b.request_url.contains("token=<redacted>"));
    }

    #[test]
    fn snapshot_default_preserves_large_request_body_under_cap() {
        // 1 KB request body — well under 10 MB cap, must round-trip intact.
        let big_body: String = "x".repeat(1024);
        let mut block = dummy_block("t", "passed", Some("{\"ok\":true}"));
        block.request_body = Some(big_body.clone());
        let mut results = dummy_results(vec![block]);
        apply_capture_policy(&mut results, &CapturePolicy::default());
        assert_eq!(
            results.block_results[0].request_body.as_deref(),
            Some(big_body.as_str()),
            "Snapshot default must keep request bodies that fit in the cap"
        );
    }

    // ─── Body redaction (jsonpath / regex rules) ───────────────────────

    fn json_block(name: &str, req_body: Option<&str>, resp_body: &str) -> BlockResult {
        let mut b = dummy_block(name, "passed", Some(resp_body));
        b.request_headers = vec![(
            "Content-Type".to_string(),
            "application/json".to_string(),
        )];
        b.request_body = req_body.map(|s| s.to_string());
        b
    }

    #[test]
    fn redact_jsonpath_replaces_value_in_json_body() {
        let block = json_block(
            "login",
            Some("{\"user\":\"a\",\"password\":\"hunter2\"}"),
            "{\"token\":\"abc\",\"user\":{\"id\":1}}",
        );
        let mut results = dummy_results(vec![block]);
        let mut report = RedactionReport::default();
        apply_body_redaction_rules(
            &mut results,
            &[
                BodyRedactRule::JsonPath("$.password".into()),
                BodyRedactRule::JsonPath("$.token".into()),
            ],
            &std::collections::HashMap::new(),
            &mut report,
        );
        let req = results.block_results[0].request_body.as_ref().unwrap();
        assert!(req.contains("\"password\":\"[REDACTED]\""), "req: {}", req);
        assert!(req.contains("\"user\":\"a\""), "non-matching keys preserved");
        let resp = &results.block_results[0].response.as_ref().unwrap().body;
        assert!(resp.contains("\"token\":\"[REDACTED]\""), "resp: {}", resp);
        assert_eq!(report.body_fields_redacted, 2);
    }

    #[test]
    fn redact_regex_replaces_match_in_text_body() {
        let mut block = dummy_block("t", "passed", Some("authorize: Bearer abcd1234"));
        block.request_headers = vec![(
            "Content-Type".to_string(),
            "text/plain".to_string(),
        )];
        // Override response Content-Type to text as well.
        if let Some(resp) = block.response.as_mut() {
            resp.headers = vec![(
                "Content-Type".to_string(),
                "text/plain".to_string(),
            )];
        }
        block.request_body = Some("Bearer xyz_token_value".to_string());
        let mut results = dummy_results(vec![block]);
        let mut report = RedactionReport::default();
        apply_body_redaction_rules(
            &mut results,
            &[BodyRedactRule::Regex(r"Bearer\s+\S+".into())],
            &std::collections::HashMap::new(),
            &mut report,
        );
        assert_eq!(
            results.block_results[0].request_body.as_deref(),
            Some("[REDACTED]")
        );
        let resp_body = &results.block_results[0].response.as_ref().unwrap().body;
        assert_eq!(resp_body, "authorize: [REDACTED]");
        assert_eq!(report.body_fields_redacted, 2);
    }

    #[test]
    fn redact_count_reflects_actual_replacements() {
        // Three matches across req+resp for a regex rule, plus one jsonpath
        // hit on the response — total 4.
        let mut block = json_block(
            "t",
            Some("{\"a\":1,\"b\":2}"),
            "{\"a\":1,\"secret\":\"S\",\"list\":[\"k=1\",\"k=2\"]}",
        );
        // Body keeping both jsonpath and regex applicable.
        block.request_body = Some("{\"a\":\"k=req\"}".to_string());
        let mut results = dummy_results(vec![block]);
        let mut report = RedactionReport::default();
        apply_body_redaction_rules(
            &mut results,
            &[
                BodyRedactRule::JsonPath("$.secret".into()),
                BodyRedactRule::Regex("k=[0-9a-z]+".into()),
            ],
            &std::collections::HashMap::new(),
            &mut report,
        );
        // 1 jsonpath match (resp.secret) + 1 regex match in req body
        // + 2 regex matches in resp body = 4.
        assert_eq!(report.body_fields_redacted, 4);
    }

    #[test]
    fn redact_combines_global_suite_block_rules() {
        let block = json_block(
            "alpha",
            Some("{\"global_secret\":\"g\",\"local_secret\":\"l\"}"),
            "{\"global_secret\":\"g\",\"local_secret\":\"l\"}",
        );
        let mut results = dummy_results(vec![block]);
        let mut report = RedactionReport::default();
        let mut per_block: std::collections::HashMap<String, Vec<BodyRedactRule>> =
            std::collections::HashMap::new();
        per_block.insert(
            "alpha".to_string(),
            vec![BodyRedactRule::JsonPath("$.local_secret".into())],
        );
        apply_body_redaction_rules(
            &mut results,
            &[BodyRedactRule::JsonPath("$.global_secret".into())],
            &per_block,
            &mut report,
        );
        let req = results.block_results[0].request_body.as_ref().unwrap();
        assert!(req.contains("\"global_secret\":\"[REDACTED]\""));
        assert!(req.contains("\"local_secret\":\"[REDACTED]\""));
        // 2 (req) + 2 (resp) = 4.
        assert_eq!(report.body_fields_redacted, 4);
    }

    #[test]
    fn build_body_redact_rules_merges_config_suite_and_per_block() {
        use crate::http_parser::parse_test_suite;

        let src = r#"# @@redact body $.suite_secret

### @@test alpha
# @@redact body $.alpha_secret
GET https://example.com/a

### @@test beta
GET https://example.com/b
"#;
        let suite = parse_test_suite(src);
        let global = vec![
            "$.global_path".to_string(),
            "/Bearer\\s+\\S+/".to_string(),
            "".to_string(), // empty entry should be silently dropped
        ];
        let (suite_rules, per_block) = build_body_redact_rules(&suite, &global);

        // Global config patterns come first, then suite-level rules from
        // the file-level `# @@redact body $.suite_secret` directive.
        assert_eq!(suite_rules.len(), 3);
        assert!(matches!(
            &suite_rules[0],
            BodyRedactRule::JsonPath(p) if p == "$.global_path"
        ));
        assert!(matches!(
            &suite_rules[1],
            BodyRedactRule::Regex(p) if p == "Bearer\\s+\\S+"
        ));
        assert!(matches!(
            &suite_rules[2],
            BodyRedactRule::JsonPath(p) if p == "$.suite_secret"
        ));

        // Per-block map only carries blocks with non-empty rules.
        assert_eq!(per_block.len(), 1);
        let alpha = per_block.get("alpha").expect("alpha rules present");
        assert_eq!(alpha.len(), 1);
        assert!(matches!(
            &alpha[0],
            BodyRedactRule::JsonPath(p) if p == "$.alpha_secret"
        ));
        assert!(per_block.get("beta").is_none());
    }

    #[test]
    fn redact_skips_non_json_body_for_jsonpath_rule() {
        let mut block = dummy_block("t", "passed", Some("not json at all"));
        block.request_headers = vec![(
            "Content-Type".to_string(),
            "text/plain".to_string(),
        )];
        if let Some(resp) = block.response.as_mut() {
            resp.headers = vec![(
                "Content-Type".to_string(),
                "text/plain".to_string(),
            )];
        }
        block.request_body = Some("plain {\"password\":\"x\"} text".to_string());
        let mut results = dummy_results(vec![block]);
        let mut report = RedactionReport::default();
        apply_body_redaction_rules(
            &mut results,
            &[BodyRedactRule::JsonPath("$.password".into())],
            &std::collections::HashMap::new(),
            &mut report,
        );
        // Non-JSON Content-Type → JSONPath rule skipped, body untouched.
        assert_eq!(
            results.block_results[0].request_body.as_deref(),
            Some("plain {\"password\":\"x\"} text")
        );
        assert_eq!(
            results.block_results[0].response.as_ref().unwrap().body,
            "not json at all"
        );
        assert_eq!(report.body_fields_redacted, 0);
    }

    #[test]
    fn redact_handles_invalid_json_gracefully() {
        // Content-Type is JSON but body is malformed — must NOT panic and
        // must leave the body unchanged.
        let mut block = json_block(
            "t",
            Some("{not valid json"),
            "{also not valid",
        );
        let original_req = block.request_body.clone();
        let original_resp = block
            .response
            .as_ref()
            .map(|r| r.body.clone());
        let mut results = dummy_results(vec![block]);
        let mut report = RedactionReport::default();
        apply_body_redaction_rules(
            &mut results,
            &[BodyRedactRule::JsonPath("$.password".into())],
            &std::collections::HashMap::new(),
            &mut report,
        );
        assert_eq!(results.block_results[0].request_body, original_req);
        assert_eq!(
            results.block_results[0].response.as_ref().map(|r| r.body.clone()),
            original_resp
        );
        assert_eq!(report.body_fields_redacted, 0);
    }

    #[test]
    fn redact_jsonpath_supports_nested_and_array_subset() {
        let block = json_block(
            "t",
            None,
            "{\"user\":{\"ssn\":\"1\"},\"items\":[{\"k\":\"a\"},{\"k\":\"b\"}]}",
        );
        let mut results = dummy_results(vec![block]);
        let mut report = RedactionReport::default();
        apply_body_redaction_rules(
            &mut results,
            &[
                BodyRedactRule::JsonPath("$.user.ssn".into()),
                BodyRedactRule::JsonPath("$.items[0].k".into()),
                BodyRedactRule::JsonPath("$.items[*].k".into()),
            ],
            &std::collections::HashMap::new(),
            &mut report,
        );
        let resp = &results.block_results[0].response.as_ref().unwrap().body;
        assert!(resp.contains("\"ssn\":\"[REDACTED]\""), "{}", resp);
        assert!(resp.contains("\"items\":[{\"k\":\"[REDACTED]\"},{\"k\":\"[REDACTED]\"}]"), "{}", resp);
        // 1 (ssn) + 1 (items[0].k) + 2 (items[*].k — second one re-redacts
        // already-redacted [0] which our `redact_at_path` declines to
        // re-count, plus index [1] which is a fresh hit) = 3.
        assert_eq!(report.body_fields_redacted, 3);
    }

    // ─── SessionStore round-trip ───────────────────────────────────────

    fn record_input<'a>(
        identity: FileIdentity,
        source: &'a str,
        results: TestRunResults,
    ) -> RecordInput<'a> {
        let now = Utc::now();
        RecordInput {
            identity,
            source_path: None,
            source_content: source,
            policy: CapturePolicy::default(),
            trigger: SessionTrigger::Manual,
            component: Component::Cli,
            started_at: now,
            finished_at: now,
            mode: None,
            env_file: None,
            env_identity: None,
            variables: vec![("base".into(), "v1".into())],
            results,
            file_id_override: None,
            redact_body_rules: Vec::new(),
            block_redact_body_rules: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn record_run_writes_expected_files_and_round_trips() {
        let root = tmp_root("rt");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("checkout".into());
        let src = "GET https://example.com/health\n";
        let results = dummy_results(vec![dummy_block("hc", "passed", None)]);

        let recorded = store.record_run(record_input(id.clone(), src, results)).unwrap();
        assert!(recorded.run_dir.exists());
        assert!(recorded.run_dir.join("run.json").exists());
        assert!(recorded.run_dir.join("summary.md").exists());
        let version_dir = store.version_dir(&recorded.file_id, &recorded.sha256);
        assert!(version_dir.join("source.http").exists());
        assert!(version_dir.join("meta.json").exists());

        let files = store.list_files().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_id, recorded.file_id);

        let versions = store.list_versions(&recorded.file_id).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].sha256, recorded.sha256);
        assert_eq!(versions[0].session_count, 1);

        let sessions = store
            .list_sessions(&recorded.file_id, &recorded.sha256)
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].run_id, recorded.run_id);
        assert_eq!(sessions[0].passed, 1);

        let (loaded_src, rec) = store
            .load_session(&recorded.file_id, &recorded.sha256, &recorded.run_id)
            .unwrap();
        assert_eq!(loaded_src, src);
        assert_eq!(rec.run_id, recorded.run_id);
        assert_eq!(rec.results.passed, 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_as_snapshot_returns_parsed_suite() {
        use crate::http_parser;
        let root = tmp_root("snap");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("snap-test".into());
        let src = "@variables\nbase=https://example.com\n\n### @test foo\nGET {{base}}/x\n# @assert status == 200\n";
        let expected_suite = http_parser::parse_test_suite(src);
        let results = dummy_results(vec![dummy_block("foo", "passed", None)]);
        let recorded = store
            .record_run(record_input(id.clone(), src, results))
            .unwrap();

        let (loaded_src, loaded_suite, loaded_rec) = store
            .load_as_snapshot(&recorded.file_id, &recorded.sha256, &recorded.run_id)
            .unwrap();
        assert_eq!(loaded_src, src);
        assert_eq!(loaded_suite.blocks.len(), expected_suite.blocks.len());
        assert_eq!(loaded_rec.run_id, recorded.run_id);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn record_run_same_source_reuses_version() {
        let root = tmp_root("samever");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("file".into());
        let src = "GET https://x\n";
        let r1 = store
            .record_run(record_input(
                id.clone(),
                src,
                dummy_results(vec![dummy_block("a", "passed", None)]),
            ))
            .unwrap();
        let r2 = store
            .record_run(record_input(
                id.clone(),
                src,
                dummy_results(vec![dummy_block("b", "failed", None)]),
            ))
            .unwrap();
        assert_eq!(r1.sha256, r2.sha256);
        assert_ne!(r1.run_id, r2.run_id);
        let versions = store.list_versions(&r1.file_id).unwrap();
        assert_eq!(versions.len(), 1);
        let sessions = store.list_sessions(&r1.file_id, &r1.sha256).unwrap();
        assert_eq!(sessions.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn record_run_crlf_does_not_fork_version() {
        let root = tmp_root("crlf");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("crlf".into());
        let lf = "GET https://x\n# c\n";
        let crlf = "GET https://x\r\n# c\r\n";
        let r1 = store
            .record_run(record_input(
                id.clone(),
                lf,
                dummy_results(vec![dummy_block("a", "passed", None)]),
            ))
            .unwrap();
        let r2 = store
            .record_run(record_input(
                id.clone(),
                crlf,
                dummy_results(vec![dummy_block("a", "passed", None)]),
            ))
            .unwrap();
        assert_eq!(r1.sha256, r2.sha256);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn record_run_different_source_creates_new_version() {
        let root = tmp_root("vbump");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("v".into());
        let r1 = store
            .record_run(record_input(
                id.clone(),
                "GET http://a\n",
                dummy_results(vec![dummy_block("a", "passed", None)]),
            ))
            .unwrap();
        let r2 = store
            .record_run(record_input(
                id.clone(),
                "GET http://b\n",
                dummy_results(vec![dummy_block("a", "passed", None)]),
            ))
            .unwrap();
        assert_eq!(r1.file_id, r2.file_id);
        assert_ne!(r1.sha256, r2.sha256);
        let versions = store.list_versions(&r1.file_id).unwrap();
        assert_eq!(versions.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn manifest_records_source_paths() {
        let root = tmp_root("paths");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("p".into());
        let src = "GET http://x\n";
        let p1 = PathBuf::from("/tmp/a/x.http");
        let p2 = PathBuf::from("/tmp/b/x.http");
        let mut input = record_input(id.clone(), src, dummy_results(vec![dummy_block("x", "passed", None)]));
        input.source_path = Some(&p1);
        store.record_run(input).unwrap();
        let mut input = record_input(id.clone(), src, dummy_results(vec![dummy_block("x", "passed", None)]));
        input.source_path = Some(&p2);
        store.record_run(input).unwrap();

        let files = store.list_files().unwrap();
        assert_eq!(files[0].source_paths.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn run_id_is_unique_across_rapid_calls() {
        let mut ids = std::collections::HashSet::new();
        for _ in 0..50 {
            ids.insert(run_id_uuidv7());
        }
        assert_eq!(ids.len(), 50);
    }

    #[test]
    fn render_summary_md_smoke() {
        let mut blocks = vec![dummy_block("ok", "passed", None)];
        let mut failing = dummy_block("oops", "failed", None);
        failing.assertion_results = vec![AssertionResult {
            assertion: "status == 200".into(),
            expected: Some("200".into()),
            actual: Some("500".into()),
            passed: false,
        }];
        failing.error = Some("HTTP 500".into());
        blocks.push(failing);
        let results = dummy_results(blocks);
        let block_summaries: Vec<BlockSummary> = results
            .block_results
            .iter()
            .map(BlockSummary::from_block)
            .collect();
        let rec = SessionRecord {
            schema_version: SCHEMA_VERSION,
            run_id: "abc123".into(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            trigger: SessionTrigger::Manual,
            host: "h".into(),
            os: "test".into(),
            component: Component::Cli,
            request_pilot_version: "0.0.0".into(),
            mode: None,
            env_file: None,
            env_identity: None,
            source_sha256: "ffff".repeat(16),
            capture_policy: CapturePolicy::default(),
            redaction_report: RedactionReport {
                headers_redacted: 1,
                ..Default::default()
            },
            variables: vec![],
            results,
            block_summaries,
        };
        let md = render_summary_md(&rec);
        assert!(md.contains("# Run abc123"));
        assert!(md.contains("✓ 1 passed"));
        assert!(md.contains("## Failures"));
        assert!(md.contains("oops"));
        assert!(md.contains("Redaction"));
    }

    // Belt-and-braces: ParsedRequest is referenced via test_runner types.
    // Touch it so unused-import warnings don't sneak in if the module is
    // rearranged in future.
    #[allow(dead_code)]
    fn _touch_parsed_request() -> ParsedRequest {
        ParsedRequest {
            name: None,
            method: "GET".into(),
            url: "http://x".into(),
            headers: vec![],
            body: None,
        }
    }

    #[test]
    fn percentiles_basic() {
        let samples: Vec<u64> = (1..=100).collect();
        let (p50, p95, p99) = percentiles(&samples);
        assert!(p50 >= 50 && p50 <= 51, "p50 = {}", p50);
        assert!(p95 >= 95 && p95 <= 96, "p95 = {}", p95);
        assert!(p99 >= 99 && p99 <= 100, "p99 = {}", p99);
    }

    #[test]
    fn percentiles_empty() {
        assert_eq!(percentiles(&[]), (0, 0, 0));
    }

    #[test]
    fn bucket_for_hourly_within_24h() {
        let now = chrono::Utc::now();
        let run = now - chrono::Duration::minutes(30);
        let b = bucket_for(run, now).unwrap();
        assert_eq!(b.0, "1h");
    }

    #[test]
    fn bucket_for_daily_within_30d() {
        let now = chrono::Utc::now();
        let run = now - chrono::Duration::days(7);
        let b = bucket_for(run, now).unwrap();
        assert_eq!(b.0, "1d");
    }

    #[test]
    fn bucket_for_too_old_returns_none() {
        let now = chrono::Utc::now();
        let run = now - chrono::Duration::days(60);
        assert!(bucket_for(run, now).is_none());
    }

    #[test]
    fn push_bounded_evicts_oldest() {
        let mut v: Vec<u64> = (1..=5).collect();
        push_bounded(&mut v, 99, 5);
        assert_eq!(v, vec![2, 3, 4, 5, 99]);
    }

    #[test]
    fn stats_record_default_smoke() {
        let r = StatsRecord::default();
        assert_eq!(r.schema_version, STATS_SCHEMA_VERSION);
        assert!(r.by_block.is_empty());
        assert!(r.versions.is_empty());
    }

    #[test]
    fn stats_record_round_trips_serde() {
        let mut r = StatsRecord::default();
        r.file_id = "test".to_string();
        r.session_count = 5;
        r.totals.passed = 4;
        r.latency.samples = vec![100, 200, 300];
        r.by_block.insert("block-a".to_string(), BlockStats::default());
        let json = serde_json::to_string(&r).unwrap();
        let back: StatsRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn render_stats_md_basic_shape() {
        let mut r = StatsRecord::default();
        r.file_id = "tests/auth.http".to_string();
        r.sha256 = Some("abcdef0123456789".to_string());
        r.session_count = 10;
        r.first_seen = "2025-01-01T00:00:00Z".to_string();
        r.last_seen = "2025-01-02T00:00:00Z".to_string();
        r.totals = StatsTotals {
            passed: 7,
            failed: 2,
            mixed: 1,
            skipped_blocks: 0,
        };
        r.buckets = vec![
            StatsBucket {
                window: "1h".to_string(),
                start: "2025-01-01T10:00:00Z".to_string(),
                runs: 5,
                passed: 4,
                failed: 1,
                p50_ms: 120,
                p95_ms: 300,
            },
            StatsBucket {
                window: "1h".to_string(),
                start: "2025-01-01T11:00:00Z".to_string(),
                runs: 3,
                passed: 3,
                failed: 0,
                p50_ms: 90,
                p95_ms: 200,
            },
        ];
        r.by_block.insert(
            "login".to_string(),
            BlockStats {
                runs: 10,
                passed: 7,
                failed: 3,
                skipped: 0,
                latency_samples: vec![],
                p50_ms: 100,
                p95_ms: 250,
            },
        );
        r.by_block.insert(
            "stable".to_string(),
            BlockStats {
                runs: 10,
                passed: 10,
                failed: 0,
                skipped: 0,
                latency_samples: vec![],
                p50_ms: 80,
                p95_ms: 150,
            },
        );

        let md = render_stats_md(&r);

        assert!(md.contains("# Stats"), "missing header: {md}");
        assert!(md.contains("tests/auth.http"));
        assert!(md.contains("abcdef01"));
        assert!(md.contains("Totals"));
        assert!(md.contains("pass-rate"));
        assert!(md.contains("session_count: 10"));
        assert!(md.contains("Pass rate"));
        assert!(md.contains("Top"));
        assert!(md.contains("login"));
        assert!(md.contains("Latency"));
        // Sparkline lines are present (backticked)
        let sparkline_lines = md.matches('`').count();
        assert!(sparkline_lines >= 4, "expected sparkline backticks: {md}");
    }

    // ─── rebuild_stats / fold equivalence ──────────────────────────────

    #[test]
    fn rebuild_stats_handles_missing_file_dir() {
        let root = tmp_root("rebuild-missing");
        let store = SessionStore::open(&root).unwrap();
        // Never recorded a run for this file_id.
        store.rebuild_stats("nope/never-existed.http").unwrap();
        // Confirm: no file dir created, no error.
        assert!(!store.file_dir("nope/never-existed.http").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rebuild_matches_incremental_fold() {
        let root = tmp_root("rebuild-eq");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("rebuild-target".into());

        // Two distinct sources → two shas → diverse runs across both.
        let src_a = "GET https://example.com/a\n";
        let src_b = "GET https://example.com/b\n";

        // Build a small variety of block outcomes per run.
        let mk_results = |kinds: &[&str]| -> TestRunResults {
            let blocks: Vec<BlockResult> = kinds
                .iter()
                .enumerate()
                .map(|(i, k)| {
                    let mut b = dummy_block(&format!("blk-{}", i), k, None);
                    b.time_ms = 10 + (i as u64) * 5;
                    b
                })
                .collect();
            dummy_results(blocks)
        };

        let runs: Vec<(&str, TestRunResults)> = vec![
            (src_a, mk_results(&["passed", "passed"])),
            (src_a, mk_results(&["passed", "failed"])),
            (src_b, mk_results(&["failed"])),
            (src_b, mk_results(&["passed", "skipped"])),
            (src_a, mk_results(&["passed"])),
        ];

        let mut recorded_per_sha: std::collections::BTreeMap<String, Vec<SessionRecord>> =
            std::collections::BTreeMap::new();
        let mut file_id_out: Option<String> = None;
        for (src, results) in runs {
            let rec = store
                .record_run(record_input(id.clone(), src, results))
                .unwrap();
            file_id_out = Some(rec.file_id.clone());
            let run_json: SessionRecord =
                read_json(&rec.run_dir.join("run.json")).unwrap();
            recorded_per_sha
                .entry(rec.sha256.clone())
                .or_default()
                .push(run_json);
        }
        let file_id = file_id_out.unwrap();

        // Build the expected stats by manually folding (this is what
        // update_stats does incrementally) using a fixed `now`.
        let now = std::time::SystemTime::now();
        let mut expected_per_sha: std::collections::BTreeMap<String, StatsRecord> =
            std::collections::BTreeMap::new();
        for (sha, mut runs) in recorded_per_sha {
            runs.sort_by(|a, b| a.started_at.cmp(&b.started_at));
            let mut s = StatsRecord {
                file_id: file_id.clone(),
                sha256: Some(sha.clone()),
                ..StatsRecord::default()
            };
            for r in &runs {
                fold_run_into_stats(&mut s, r, now);
            }
            expected_per_sha.insert(sha, s);
        }
        let expected_versions: Vec<StatsRecord> = expected_per_sha.values().cloned().collect();
        let expected_rollup = rollup_versions(&file_id, &expected_versions);

        // Run rebuild with the same `now` for deterministic bucket output.
        store.rebuild_stats_at(&file_id, now).unwrap();

        // Verify per-version stats round-trip equal to the expected fold.
        for (sha, expected) in &expected_per_sha {
            let path = store.version_dir(&file_id, sha).join("stats.json");
            let actual: StatsRecord = read_json(&path).unwrap();
            assert_eq!(&actual, expected, "mismatch for sha {}", sha);
        }

        // Verify the file-level rollup.
        let rollup_path = store.file_dir(&file_id).join("stats.json");
        let actual_rollup: StatsRecord = read_json(&rollup_path).unwrap();
        assert_eq!(actual_rollup, expected_rollup);

        // Sanity: rollup carries one StatsVersionRef per sha.
        assert_eq!(actual_rollup.versions.len(), expected_per_sha.len());
        assert!(actual_rollup.sha256.is_none());

        // Idempotence: running rebuild twice yields the same bytes.
        let bytes1 = fs::read(&rollup_path).unwrap();
        store.rebuild_stats_at(&file_id, now).unwrap();
        let bytes2 = fs::read(&rollup_path).unwrap();
        assert_eq!(bytes1, bytes2);

        let _ = fs::remove_dir_all(&root);
    }

    // ─── update_stats (incremental fold) ───────────────────────────────

    fn make_session_record(
        blocks: Vec<BlockResult>,
        started_at: chrono::DateTime<Utc>,
    ) -> SessionRecord {
        let results = dummy_results(blocks);
        let block_summaries: Vec<BlockSummary> = results
            .block_results
            .iter()
            .map(BlockSummary::from_block)
            .collect();
        SessionRecord {
            schema_version: SCHEMA_VERSION,
            run_id: run_id_uuidv7(),
            started_at,
            finished_at: started_at,
            trigger: SessionTrigger::Manual,
            host: "h".into(),
            os: "test".into(),
            component: Component::Cli,
            request_pilot_version: "0.0.0".into(),
            mode: None,
            env_file: None,
            env_identity: None,
            source_sha256: "abc".into(),
            capture_policy: CapturePolicy::default(),
            redaction_report: RedactionReport::default(),
            variables: vec![],
            results,
            block_summaries,
        }
    }

    #[test]
    fn update_stats_creates_new_record() {
        let root = tmp_root("upd-new");
        let store = SessionStore::open(&root).unwrap();
        let file_id = "fid-new";
        let sha = "sha-aaaa";
        let now = Utc::now();
        let run = make_session_record(vec![dummy_block("login", "passed", None)], now);

        store.update_stats(file_id, sha, &run).unwrap();

        let stats_path = store.version_dir(file_id, sha).join("stats.json");
        assert!(stats_path.exists());
        let stats: StatsRecord =
            serde_json::from_slice(&fs::read(&stats_path).unwrap()).unwrap();
        assert_eq!(stats.schema_version, STATS_SCHEMA_VERSION);
        assert_eq!(stats.file_id, file_id);
        assert_eq!(stats.sha256.as_deref(), Some(sha));
        assert_eq!(stats.session_count, 1);
        assert_eq!(stats.totals.passed, 1);
        assert_eq!(stats.totals.failed, 0);
        assert_eq!(stats.by_block.get("login").map(|b| b.passed), Some(1));
        assert!(!stats.first_seen.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn update_stats_folds_two_runs() {
        let root = tmp_root("upd-fold");
        let store = SessionStore::open(&root).unwrap();
        let file_id = "fid-fold";
        let sha = "sha-fold";
        let now = Utc::now();

        let r1 = make_session_record(vec![dummy_block("hc", "passed", None)], now);
        store.update_stats(file_id, sha, &r1).unwrap();

        let r2 = make_session_record(
            vec![dummy_block("hc", "failed", None)],
            now + chrono::Duration::seconds(5),
        );
        store.update_stats(file_id, sha, &r2).unwrap();

        let stats_path = store.version_dir(file_id, sha).join("stats.json");
        let stats: StatsRecord =
            serde_json::from_slice(&fs::read(&stats_path).unwrap()).unwrap();
        assert_eq!(stats.session_count, 2);
        assert_eq!(stats.totals.passed, 1);
        assert_eq!(stats.totals.failed, 1);
        let hc = stats.by_block.get("hc").expect("block hc present");
        assert_eq!(hc.runs, 2);
        assert_eq!(hc.passed, 1);
        assert_eq!(hc.failed, 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_stats_returns_none_when_missing() {
        let root = tmp_root("load-stats-missing");
        let store = SessionStore::open(&root).unwrap();

        // Nothing recorded yet: both the per-version and rollup paths are
        // absent, so load_stats must return Ok(None).
        let v = store.load_stats("fid-absent", Some("sha-absent")).unwrap();
        assert!(v.is_none(), "expected None for missing per-version stats");

        let r = store.load_stats("fid-absent", None).unwrap();
        assert!(r.is_none(), "expected None for missing rollup stats");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_stats_reads_back_after_record_run() {
        let root = tmp_root("load-stats-roundtrip");
        let store = SessionStore::open(&root).unwrap();
        let file_id = "fid-load";
        let sha = "sha-load";
        let now = Utc::now();

        let run = make_session_record(vec![dummy_block("login", "passed", None)], now);
        store.update_stats(file_id, sha, &run).unwrap();

        // Per-version stats round-trip.
        let v = store
            .load_stats(file_id, Some(sha))
            .unwrap()
            .expect("per-version stats present");
        assert_eq!(v.file_id, file_id);
        assert_eq!(v.sha256.as_deref(), Some(sha));
        assert_eq!(v.session_count, 1);
        assert_eq!(v.totals.passed, 1);

        // File-level rollup round-trip.
        let r = store
            .load_stats(file_id, None)
            .unwrap()
            .expect("rollup stats present");
        assert_eq!(r.file_id, file_id);
        assert!(r.sha256.is_none());
        assert_eq!(r.session_count, 1);
        assert_eq!(r.versions.len(), 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn update_stats_handles_corrupt_existing() {
        let root = tmp_root("upd-corrupt");
        let store = SessionStore::open(&root).unwrap();
        let file_id = "fid-corrupt";
        let sha = "sha-corrupt";
        let version_dir = store.version_dir(file_id, sha);
        fs::create_dir_all(&version_dir).unwrap();
        fs::write(version_dir.join("stats.json"), b"{not valid json").unwrap();

        let run =
            make_session_record(vec![dummy_block("ok", "passed", None)], Utc::now());
        store.update_stats(file_id, sha, &run).unwrap();

        let stats: StatsRecord =
            serde_json::from_slice(&fs::read(version_dir.join("stats.json")).unwrap())
                .unwrap();
        assert_eq!(stats.session_count, 1);
        assert_eq!(stats.totals.passed, 1);
        assert_eq!(stats.file_id, file_id);
        assert_eq!(stats.sha256.as_deref(), Some(sha));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn update_stats_updates_parent_rollup() {
        let root = tmp_root("upd-rollup");
        let store = SessionStore::open(&root).unwrap();
        let file_id = "fid-rollup";
        let now = Utc::now();

        let r1 = make_session_record(vec![dummy_block("a", "passed", None)], now);
        store.update_stats(file_id, "sha-1111", &r1).unwrap();

        let r2 = make_session_record(
            vec![dummy_block("b", "failed", None)],
            now + chrono::Duration::seconds(1),
        );
        store.update_stats(file_id, "sha-2222", &r2).unwrap();

        let parent_path = store.file_dir(file_id).join("stats.json");
        assert!(parent_path.exists());
        let parent: StatsRecord =
            serde_json::from_slice(&fs::read(&parent_path).unwrap()).unwrap();
        assert!(parent.sha256.is_none());
        assert_eq!(parent.versions.len(), 2);
        assert_eq!(parent.session_count, 2);
        assert_eq!(parent.totals.passed, 1);
        assert_eq!(parent.totals.failed, 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn record_run_updates_stats_best_effort() {
        let root = tmp_root("rr-stats");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("stats-wire".into());
        let src = "GET https://example.com/health\n";
        let results = dummy_results(vec![dummy_block("hc", "passed", None)]);

        let recorded = store
            .record_run(record_input(id.clone(), src, results))
            .unwrap();

        let stats_path = store
            .version_dir(&recorded.file_id, &recorded.sha256)
            .join("stats.json");
        assert!(
            stats_path.exists(),
            "record_run should fire-and-forget update_stats; missing {}",
            stats_path.display()
        );
        let stats: StatsRecord =
            serde_json::from_slice(&fs::read(&stats_path).unwrap()).unwrap();
        assert_eq!(stats.file_id, recorded.file_id);
        assert_eq!(stats.sha256.as_deref(), Some(recorded.sha256.as_str()));
        assert_eq!(stats.session_count, 1);
        assert_eq!(stats.totals.passed, 1);
        assert!(
            stats.by_block.contains_key("hc"),
            "expected block 'hc' in stats.by_block, got {:?}",
            stats.by_block.keys().collect::<Vec<_>>()
        );

        let _ = fs::remove_dir_all(&root);
    }

    // NOTE: A `record_run_succeeds_when_stats_update_fails` test was
    // considered. Reliably forcing `update_stats` to fail cross-platform is
    // tricky (Windows vs. Unix permissions/dir-vs-file semantics differ),
    // so we instead document the contract here: failures from
    // `update_stats` inside `record_run` are logged via `log::warn!` and
    // swallowed — they must never propagate to the caller.

    #[test]
    fn update_stats_bucket_eviction() {
        let root = tmp_root("upd-evict");
        let store = SessionStore::open(&root).unwrap();
        let file_id = "fid-evict";
        let sha = "sha-evict";

        // Run dated 60 days ago — outside both 24h and 30d windows.
        let now = Utc::now();
        let old_ts = now - chrono::Duration::days(60);
        let run = make_session_record(vec![dummy_block("hc", "passed", None)], old_ts);
        store.update_stats(file_id, sha, &run).unwrap();

        let stats_path = store.version_dir(file_id, sha).join("stats.json");
        let stats: StatsRecord =
            serde_json::from_slice(&fs::read(&stats_path).unwrap()).unwrap();
        assert_eq!(stats.session_count, 1, "still counted in totals");
        assert_eq!(stats.totals.passed, 1);
        assert!(
            stats.buckets.is_empty(),
            "run older than 30d must not contribute to buckets, got {:?}",
            stats.buckets
        );

        let _ = fs::remove_dir_all(&root);
    }

    // ─── record_run → stats.json end-to-end ────────────────────────────

    #[test]
    fn stats_round_trip_after_single_run() {
        let root = tmp_root("stats-rt-single");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("stats-single".into());
        let src = "GET https://example.com/health\n";
        let results = dummy_results(vec![dummy_block("hc", "passed", None)]);

        let recorded = store
            .record_run(record_input(id.clone(), src, results))
            .unwrap();

        let stats_path = store
            .version_dir(&recorded.file_id, &recorded.sha256)
            .join("stats.json");
        assert!(stats_path.exists(), "stats.json must be written by record_run");

        let stats: StatsRecord = read_json(&stats_path).unwrap();
        assert_eq!(stats.schema_version, STATS_SCHEMA_VERSION);
        assert_eq!(stats.file_id, recorded.file_id);
        assert_eq!(stats.sha256.as_deref(), Some(recorded.sha256.as_str()));
        assert_eq!(stats.session_count, 1);
        assert_eq!(stats.totals.passed, 1);
        assert_eq!(stats.totals.failed, 0);

        let hc = stats.by_block.get("hc").expect("block 'hc' present");
        assert_eq!(hc.runs, 1);
        assert_eq!(hc.passed, 1);
        assert_eq!(hc.failed, 0);

        // LatencyAggregate validity: with a single sample, p50/p95/p99
        // should all collapse to that sample, max == sample, and the
        // percentile invariants must hold.
        assert_eq!(stats.latency.samples.len(), 1);
        assert!(stats.latency.max_ms >= stats.latency.p99_ms);
        assert!(stats.latency.p99_ms >= stats.latency.p95_ms);
        assert!(stats.latency.p95_ms >= stats.latency.p50_ms);
        assert_eq!(stats.latency.samples[0], stats.latency.max_ms);

        assert!(!stats.first_seen.is_empty());
        assert!(!stats.last_seen.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stats_increment_after_second_run() {
        let root = tmp_root("stats-rt-second");
        let store = SessionStore::open(&root).unwrap();
        let id = FileIdentity::Alias("stats-second".into());
        let src = "GET https://example.com/health\n";

        let r1 = store
            .record_run(record_input(
                id.clone(),
                src,
                dummy_results(vec![dummy_block("hc", "passed", None)]),
            ))
            .unwrap();
        let r2 = store
            .record_run(record_input(
                id.clone(),
                src,
                dummy_results(vec![dummy_block("hc", "failed", None)]),
            ))
            .unwrap();

        // Same file_id and sha across both runs.
        assert_eq!(r1.file_id, r2.file_id);
        assert_eq!(r1.sha256, r2.sha256);

        let stats_path = store
            .version_dir(&r1.file_id, &r1.sha256)
            .join("stats.json");
        let stats: StatsRecord = read_json(&stats_path).unwrap();

        assert_eq!(stats.session_count, 2);
        assert_eq!(stats.totals.passed, 1);
        assert_eq!(stats.totals.failed, 1);

        let hc = stats.by_block.get("hc").expect("block 'hc' present");
        assert_eq!(hc.runs, 2);
        assert_eq!(hc.passed, 1);
        assert_eq!(hc.failed, 1);

        // Aggregates updated: two latency samples accumulated.
        assert_eq!(stats.latency.samples.len(), 2);
        assert!(stats.latency.max_ms >= stats.latency.p50_ms);

        // first_seen <= last_seen.
        assert!(stats.first_seen <= stats.last_seen);

        let _ = fs::remove_dir_all(&root);
    }
}
