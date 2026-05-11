use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::task::JoinSet;

tokio::task_local! {
    /// File-level `# @@parallel <N>` cap propagated into spawned wave
    /// tasks. Loop blocks (and any future fan-out unit) read this to clamp
    /// their per-iteration worker count so a loop with `# @@parallel 32`
    /// inside a file capped at `# @@parallel 4` only runs 4 workers.
    /// Unset means no clamp is applied (loop uses its own limit).
    static FILE_PARALLEL_CAP: u32;
}

/// Shared cancellation flag passed into `run_suite_with_cancel`. The runner
/// polls this between blocks and at phase boundaries; setting it stops new
/// work from starting. Teardown still runs so cleanup happens even after a
/// user-initiated stop.
#[derive(Clone, Default, Debug)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    /// Reset to the not-cancelled state. Allows reusing a single token across
    /// runs.
    pub fn reset(&self) {
        self.flag.store(false, Ordering::Relaxed);
    }
}

use crate::assertions::{self, AssertionResult};
use crate::http_client;
use crate::http_parser::{TestBlock, TestSuite};
use crate::telemetry::{self, TelemetryCollector, TelemetryStats};
use crate::variables::{CapturedResponse, VariableStore};

/// Append an auto-injected request-id header (fresh UUIDv4) to `headers`, but
/// only if no header with the same name is already present (case-insensitive).
/// No-op when `block.request_id_disabled` is true or `block.request_id_header`
/// is None. Called right before dispatching the HTTP request.
fn inject_request_id_header(headers: &mut Vec<(String, String)>, block: &TestBlock) {
    if block.request_id_disabled {
        return;
    }
    let Some(name) = block.request_id_header.as_deref() else {
        return;
    };
    if headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name)) {
        return;
    }
    headers.push((name.to_string(), uuid::Uuid::new_v4().to_string()));
}

/// Capture a completed block's response into the var_store so later blocks
/// can reference it via `{{blockName.response.*}}` interpolation.
fn capture_block_response(var_store: &mut VariableStore, result: &BlockResult) {
    if result.name.is_empty() {
        return;
    }
    if let Some(resp) = &result.response {
        var_store.set_response(
            &result.name,
            CapturedResponse {
                status: resp.status,
                headers: resp.headers.clone(),
                body: resp.body.clone(),
            },
        );
    }
}

/// Lightweight progress event emitted per-block during suite execution.
#[derive(Debug, Serialize, Clone)]
pub struct BlockProgress {
    pub seq: Option<u64>,
    pub name: String,
    pub block_type: String,
    pub status: String, // "running", "passed", "failed", "skipped", "error"
    pub time_ms: u64,
    pub assertion_passed: usize,
    pub assertion_total: usize,
    pub extract_ok: usize,
    pub extract_total: usize,
    pub http_status: Option<u16>,
    pub error: Option<String>,
}

/// Progress event for a single iteration of a `# @@for` loop block.
/// Emitted before each iteration starts (status="running") and again when
/// it finishes (status mirrors the inner BlockResult's status). Carries
/// the parent block's name so consumers can route the event to the right
/// row in the UI.
#[derive(Debug, Serialize, Clone)]
pub struct IterationProgress {
    pub block_name: String,
    pub index: usize,
    pub total: usize,
    pub iter_value: String,
    pub status: String, // "running", "passed", "failed", "error"
    pub time_ms: u64,
}

/// Trait for receiving real-time block execution progress.
/// Desktop GUI implements this with Tauri events, TUI with mpsc channels.
pub trait ProgressHandler: Send + Sync {
    fn on_block_start(&self, progress: &BlockProgress);
    fn on_block_complete(&self, progress: &BlockProgress);
    /// Emitted right after each block's `BlockResult` is finalized, carrying
    /// the full result (including response body, assertion details, etc).
    /// Default impl is a no-op so consumers that only care about lightweight
    /// progress (TUI, CLI) don't need to implement it.
    fn on_block_result(&self, _result: &BlockResult) {}
    /// Emitted before AND after each iteration of a `# @@for` loop block.
    /// Default no-op so existing handlers don't need to opt in.
    fn on_iteration_progress(&self, _progress: &IterationProgress) {}
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TestRunResults {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total_time_ms: u64,
    pub block_results: Vec<BlockResult>,
    pub final_variables: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<TelemetryStats>,
}

/// Result of executing a single test block.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BlockResult {
    pub seq: Option<u64>,
    pub name: String,
    pub block_type: String,
    pub group: Option<String>,
    pub request_method: String,
    pub request_url: String,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<String>,
    pub status: String, // "passed", "failed", "skipped", "error"
    pub response: Option<http_client::HttpResponse>,
    pub assertion_results: Vec<AssertionResult>,
    pub extract_results: Vec<ExtractResult>,
    pub error: Option<String>,
    pub time_ms: u64,
    /// Per-step results for @compare blocks. Empty for normal blocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub step_results: Vec<StepResult>,
    /// Diff result for @compare blocks. Transitional single-pair field
    /// kept so old run.json history files keep deserializing and so
    /// older builds that read this field still see the primary diff.
    /// New code reads `diff_results` (which is empty when this is the
    /// only data) and falls back to wrapping `diff_result` in a
    /// single-element vec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_result: Option<assertions::DiffResult>,
    /// Multi-pair diff results — one entry per `# @@diff a b` line in
    /// source order. Empty for non-compare or non-diff blocks. The
    /// first pair is "primary": `$diff.*` assertions are evaluated
    /// against `diff_results[0].diff`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff_results: Vec<NamedDiffResult>,
    /// Per-iteration results when this block has `# @@for` set. Empty for
    /// non-loop blocks. Each entry is the BlockResult of one iteration
    /// plus the binding metadata. The outer BlockResult.status aggregates
    /// across iterations (`passed` if all pass, `failed` if any fail,
    /// `error` if loop source missing/invalid).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub iterations: Vec<IterationResult>,
}

/// Result of one iteration of a `# @@for`-looped block. Carries the bound
/// element value (truncated for display) plus the inner BlockResult so the
/// UI can render full per-iteration request/response detail (subject to the
/// storage cap policy: full bodies for failures, summary for most successes).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IterationResult {
    pub index: usize,
    pub iter_value: String,
    pub status: String, // "passed" | "failed" | "error" | "skipped"
    pub block_result: Box<BlockResult>,
    /// True when this iteration's request/response bodies have been
    /// stripped by the storage cap policy (kept assertions/status/timing
    /// only). Surface in the UI as "(body omitted)" so users can tell a
    /// trimmed iteration from one that genuinely had no body.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub body_omitted: bool,
}

/// One pair's diff outcome inside a multi-diff @compare block. Carries
/// the pair labels so callers can render `baseline ⇄ canary` headings
/// and a per-pair error so a single bad reference (e.g. step name typo)
/// doesn't kill all the other pairs.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NamedDiffResult {
    pub step_a: String,
    pub step_b: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<assertions::DiffResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// V1.3: implicit body-match check on this pair is tolerated.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_mismatch: bool,
    /// V1.3: results of per-pair `$diff.*` assertions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertion_results: Vec<AssertionResult>,
    /// V1.3: did the implicit "bodies must match" check pass?
    #[serde(default = "default_true_diff_pair")]
    pub required_match_passed: bool,
    /// V1.3: overall pair outcome (no error, required match, all asserts pass).
    #[serde(default = "default_true_diff_pair")]
    pub passed: bool,
}

fn default_true_diff_pair() -> bool {
    true
}

/// Result of executing a single step within a @compare block.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StepResult {
    pub name: String,
    pub request_method: String,
    pub request_url: String,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<String>,
    pub response: Option<http_client::HttpResponse>,
    pub assertion_results: Vec<AssertionResult>,
    pub extract_results: Vec<ExtractResult>,
    pub time_ms: u64,
    pub error: Option<String>,
    /// V1.3: true when this step is the candidate side of one or more failing diff pairs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub diff_failed: bool,
    /// V1.3: indices into BlockResult.diff_results of failing pairs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff_failed_pairs: Vec<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExtractResult {
    pub variable: String,
    pub value: Option<String>,
    pub success: bool,
}

// ── Helper functions ──

fn make_progress(
    name: &str,
    block_type: &str,
    status: &str,
    time_ms: u64,
    assertion_passed: usize,
    assertion_total: usize,
    extract_ok: usize,
    extract_total: usize,
    http_status: Option<u16>,
    error: Option<&str>,
) -> BlockProgress {
    BlockProgress {
        seq: None,
        name: name.to_string(),
        block_type: block_type.to_string(),
        status: status.to_string(),
        time_ms,
        assertion_passed,
        assertion_total,
        extract_ok,
        extract_total,
        http_status,
        error: error.map(|e| e.to_string()),
    }
}

fn emit_start(handler: &Option<Arc<dyn ProgressHandler>>, name: &str, block_type: &str) {
    if let Some(ref h) = handler {
        let progress = make_progress(name, block_type, "running", 0, 0, 0, 0, 0, None, None);
        h.on_block_start(&progress);
    }
}

fn emit_completed(handler: &Option<Arc<dyn ProgressHandler>>, result: &BlockResult) {
    if let Some(ref h) = handler {
        let assertion_passed = result.assertion_results.iter().filter(|r| r.passed).count();
        let extract_ok = result.extract_results.iter().filter(|e| e.success).count();
        let http_status = result.response.as_ref().map(|r| r.status);
        let progress = make_progress(
            &result.name,
            &result.block_type,
            &result.status,
            result.time_ms,
            assertion_passed,
            result.assertion_results.len(),
            extract_ok,
            result.extract_results.len(),
            http_status,
            result.error.as_deref(),
        );
        h.on_block_complete(&progress);
        h.on_block_result(result);
    }
}

fn emit_skipped_result(handler: &Option<Arc<dyn ProgressHandler>>, result: &BlockResult) {
    if let Some(ref h) = handler {
        let progress = make_progress(
            &result.name,
            &result.block_type,
            &result.status,
            result.time_ms,
            0,
            0,
            0,
            0,
            None,
            result.error.as_deref(),
        );
        h.on_block_complete(&progress);
        h.on_block_result(result);
    }
}

#[allow(dead_code)]
fn emit_skipped(
    handler: &Option<Arc<dyn ProgressHandler>>,
    name: &str,
    block_type: &str,
    reason: &str,
) {
    if let Some(ref h) = handler {
        let progress = make_progress(name, block_type, "skipped", 0, 0, 0, 0, 0, None, Some(reason));
        h.on_block_complete(&progress);
    }
}

/// Emit a skipped block to the handler, including a synthesized BlockResult
/// so live consumers (desktop UI) can display the skip reason immediately.
fn emit_skipped_with_result(
    handler: &Option<Arc<dyn ProgressHandler>>,
    block: &TestBlock,
    reason: &str,
) -> BlockResult {
    let result = make_skipped_result(block, reason);
    emit_skipped_result(handler, &result);
    result
}

fn make_skipped_result(block: &TestBlock, reason: &str) -> BlockResult {
    BlockResult {
        seq: None,
        name: block.name.clone(),
        block_type: block.block_type.clone(),
        group: block.group.clone(),
        request_method: block.request.method.clone(),
        request_url: block.request.url.clone(),
        request_headers: Vec::new(),
        request_body: None,
        status: "skipped".to_string(),
        response: None,
        assertion_results: Vec::new(),
        extract_results: Vec::new(),
        error: Some(reason.to_string()),
        time_ms: 0,
        step_results: Vec::new(),
        diff_result: None,
        diff_results: Vec::new(),
        iterations: Vec::new(),
    }
}

/// Execute a single block with owned values (suitable for tokio::spawn).
async fn execute_block(block: TestBlock, var_store: VariableStore, extra_headers: Vec<(String, String)>) -> BlockResult {
    let url = var_store.interpolate(&block.request.url);
    let mut headers: Vec<(String, String)> = block
        .request
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), var_store.interpolate(v)))
        .collect();
    // Append extra headers (they can override block headers if same name)
    for (k, v) in &extra_headers {
        headers.push((k.clone(), var_store.interpolate(v)));
    }
    inject_request_id_header(&mut headers, &block);
    let body = block
        .request
        .body
        .as_ref()
        .map(|b| var_store.interpolate(b));

    let start = std::time::Instant::now();
    let result =
        http_client::execute_request(&block.request.method, &url, &headers, body.as_deref()).await;
    let time_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(response) => {
            let extract_results: Vec<ExtractResult> = block
                .extracts
                .iter()
                .map(|extract| {
                    let value = assertions::resolve_response_value(
                        &extract.source_path,
                        response.status,
                        &response.headers,
                        &response.body,
                    );
                    ExtractResult {
                        variable: extract.variable_name.clone(),
                        success: value.is_some(),
                        value,
                    }
                })
                .collect();

            let assertion_results: Vec<AssertionResult> = block
                .assertions
                .iter()
                .map(|a| {
                    assertions::evaluate(a, response.status, &response.headers, &response.body)
                })
                .collect();
            let all_passed = assertion_results.iter().all(|r| r.passed);

            BlockResult {
                seq: None,
                name: block.name.clone(),
                block_type: block.block_type.clone(),
                group: block.group.clone(),
                request_method: block.request.method.clone(),
                request_url: url,
                request_headers: headers,
                request_body: body,
                status: if all_passed { "passed" } else { "failed" }.to_string(),
                response: Some(response),
                assertion_results,
                extract_results,
                error: None,
                time_ms,
                step_results: Vec::new(),
                diff_result: None,
                diff_results: Vec::new(),
                iterations: Vec::new(),
            }
        }
        Err(err) => BlockResult {
            seq: None,
            name: block.name.clone(),
            block_type: block.block_type.clone(),
            group: block.group.clone(),
            request_method: block.request.method.clone(),
            request_url: url,
            request_headers: headers,
            request_body: body,
            status: "error".to_string(),
            response: None,
            assertion_results: Vec::new(),
            extract_results: Vec::new(),
            error: Some(err),
            time_ms,
            step_results: Vec::new(),
            diff_result: None,
            diff_results: Vec::new(),
            iterations: Vec::new(),
        },
    }
}

/// Build a "loop-error" BlockResult for cases where the loop source can't be
/// turned into an iterable (missing var, not JSON, not an array, non-scalar
/// element when V1 only supports scalars/objects). The block fails outright;
/// no iterations are produced.
fn loop_error_result(
    block: &TestBlock,
    error_msg: String,
    start: std::time::Instant,
) -> BlockResult {
    BlockResult {
        seq: None,
        name: block.name.clone(),
        block_type: block.block_type.clone(),
        group: block.group.clone(),
        request_method: block.request.method.clone(),
        request_url: block.request.url.clone(),
        request_headers: Vec::new(),
        request_body: None,
        status: "error".to_string(),
        response: None,
        assertion_results: Vec::new(),
        extract_results: Vec::new(),
        error: Some(error_msg),
        time_ms: start.elapsed().as_millis() as u64,
        step_results: Vec::new(),
        diff_result: None,
        diff_results: Vec::new(),
        iterations: Vec::new(),
    }
}

/// Route a block through either the normal single-execution path or the
/// `# @@for` loop runner. This is the entry point that suite/group/teardown
/// execution paths use so loops are transparent everywhere.
async fn execute_block_or_loop(
    block: TestBlock,
    var_store: VariableStore,
    extra_headers: Vec<(String, String)>,
    handler: Option<Arc<dyn ProgressHandler>>,
    cancel: Option<CancelToken>,
) -> BlockResult {
    if block.for_loop.is_some() {
        execute_loop_block(block, var_store, extra_headers, handler, cancel).await
    } else {
        execute_block(block, var_store, extra_headers).await
    }
}

/// Execute a single iteration of a `# @@for` block. Pulled out of
/// `execute_loop_block` so both the sequential and parallel paths use the
/// same logic. Owns the progress-event emission lifecycle for one iter.
///
/// Each iteration receives a CHILD var_store derived from the loop's
/// snapshot — per-iter extracts stay local and don't pollute the outer
/// state. The inner block has its for_loop cleared so `execute_block`
/// treats it as a single request.
async fn run_one_iteration(
    block: &TestBlock,
    fl: &crate::http_parser::ForLoop,
    idx: usize,
    total: usize,
    elem: &serde_json::Value,
    parent_vars: &VariableStore,
    extra_headers: &[(String, String)],
    handler: Option<Arc<dyn ProgressHandler>>,
) -> IterationResult {
    // Bind iter_var:
    //   - JSON string -> raw string value (so `{{user_id}}` = "u1", not "\"u1\"")
    //   - everything else (object/array/number/bool/null) -> JSON-stringified
    //     so the interpolator's dotted-path navigator can walk it via
    //     `{{user.id}}` / `{{users[0].name}}`.
    let bound_value = match elem {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };

    // Truncate iter_value display so a giant object doesn't blow up
    // history rendering or progress event payloads.
    const MAX_DISPLAY: usize = 200;
    let display_value = if bound_value.chars().count() > MAX_DISPLAY {
        let trimmed: String = bound_value.chars().take(MAX_DISPLAY).collect();
        format!("{}…", trimmed)
    } else {
        bound_value.clone()
    };

    // Live-progress: iteration starting.
    if let Some(ref h) = handler {
        h.on_iteration_progress(&IterationProgress {
            block_name: block.name.clone(),
            index: idx,
            total,
            iter_value: display_value.clone(),
            status: "running".to_string(),
            time_ms: 0,
        });
    }

    let mut child = parent_vars.clone();
    child.set(&fl.iter_var, &bound_value);
    // Loop counters — interpolator's plain-name fallback resolves these.
    // Using `$index` / `$iteration` keeps them visually distinct from
    // user variables (and matches REST-Client-style built-ins).
    child.set("$index", &idx.to_string());
    child.set("$iteration", &(idx + 1).to_string());

    let mut inner_block = block.clone();
    inner_block.for_loop = None;
    let iter_start = std::time::Instant::now();
    let inner_result = execute_block(inner_block, child, extra_headers.to_vec()).await;
    let iter_ms = iter_start.elapsed().as_millis() as u64;

    let iter_status = inner_result.status.clone();

    // Live-progress: iteration finished.
    if let Some(ref h) = handler {
        h.on_iteration_progress(&IterationProgress {
            block_name: block.name.clone(),
            index: idx,
            total,
            iter_value: display_value.clone(),
            status: iter_status.clone(),
            time_ms: iter_ms,
        });
    }

    IterationResult {
        index: idx,
        iter_value: display_value,
        status: iter_status,
        block_result: Box::new(inner_result),
        body_omitted: false,
    }
}

/// Build a synthetic `IterationResult` for an iteration that never ran or
/// whose worker task panicked. Used by the parallel scheduler to fill gaps
/// (cancelled iters, JoinError) so the Vec<IterationResult> always has one
/// entry per source-array element regardless of execution path failures.
fn synthetic_skipped_iteration(
    block: &TestBlock,
    idx: usize,
    iter_value: String,
    status: &str,
    error_msg: &str,
) -> IterationResult {
    IterationResult {
        index: idx,
        iter_value: iter_value.clone(),
        status: status.to_string(),
        block_result: Box::new(BlockResult {
            seq: None,
            name: block.name.clone(),
            block_type: block.block_type.clone(),
            group: block.group.clone(),
            request_method: block.request.method.clone(),
            request_url: block.request.url.clone(),
            request_headers: Vec::new(),
            request_body: None,
            status: status.to_string(),
            response: None,
            assertion_results: Vec::new(),
            extract_results: Vec::new(),
            error: Some(error_msg.to_string()),
            time_ms: 0,
            step_results: Vec::new(),
            diff_result: None,
            diff_results: Vec::new(),
            iterations: Vec::new(),
        }),
        body_omitted: false,
    }
}

/// Execute a `# @@for iter_var in source_var` block — runs the inner block
/// once per element of the JSON array stored in `source_var`. Per-iteration
/// extracts stay LOCAL to that iteration (don't escape into the outer
/// var_store).
///
/// Concurrency: when `for_loop.parallel` is `Some(N > 1)`, iterations run
/// concurrently via a bounded worker pool of N tasks (each pulling the
/// next index from a shared atomic counter). Otherwise iterations run
/// sequentially. Either path produces identical, deterministic
/// `IterationResult` ordering (sorted by `index`).
///
/// Cancellation: the supplied `CancelToken` is checked between iterations
/// (sequential) or before each worker pulls the next index (parallel). On
/// cancel, remaining iterations are skipped with synthetic
/// `status = "skipped"` results so the UI sees the full iteration set.
async fn execute_loop_block(
    block: TestBlock,
    var_store: VariableStore,
    extra_headers: Vec<(String, String)>,
    handler: Option<Arc<dyn ProgressHandler>>,
    cancel: Option<CancelToken>,
) -> BlockResult {
    let total_start = std::time::Instant::now();
    let fl = block.for_loop.clone().expect("execute_loop_block called without for_loop");

    // 1. Look up the source variable.
    let raw = match var_store.get(&fl.source_var) {
        Some(v) => v.to_string(),
        None => {
            return loop_error_result(
                &block,
                format!(
                    "loop source variable '{}' is undefined when block '{}' runs",
                    fl.source_var, block.name
                ),
                total_start,
            )
        }
    };

    // 2. Parse as JSON.
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return loop_error_result(
                &block,
                format!(
                    "loop source '{}' is not valid JSON: {}",
                    fl.source_var, e
                ),
                total_start,
            )
        }
    };

    // 3. Require an array.
    let arr = match parsed.as_array() {
        Some(a) => a.clone(),
        None => {
            return loop_error_result(
                &block,
                format!("loop source '{}' is not a JSON array", fl.source_var),
                total_start,
            )
        }
    };

    let total = arr.len();
    let mut iterations: Vec<IterationResult> = Vec::with_capacity(total);
    let mut any_failed = false;

    // Determine concurrency. None or Some(1) => sequential. Some(N>1) =>
    // parallel via a bounded worker pool. When the wave scheduler set a
    // file-level cap via `FILE_PARALLEL_CAP`, clamp loop workers to that
    // cap too so a `# @@parallel 32` loop inside `# @@parallel 4` only
    // runs 4 workers (matches the file-level concurrency budget).
    let loop_parallel = fl.parallel.unwrap_or(1).max(1) as usize;
    let max_concurrency = match FILE_PARALLEL_CAP.try_with(|c| *c as usize) {
        Ok(cap) => loop_parallel.min(cap.max(1)),
        Err(_) => loop_parallel,
    };

    if max_concurrency <= 1 || total <= 1 {
        // ── Sequential path ─────────────────────────────────────────
        for (idx, elem) in arr.iter().enumerate() {
            // Cancellation: stop scheduling new iterations and synthesize
            // skipped results for the remainder so the UI sees the full set.
            if cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
                for skip_idx in idx..total {
                    let display = elem_display(&arr[skip_idx]);
                    iterations.push(synthetic_skipped_iteration(
                        &block, skip_idx, display, "skipped", "Cancelled by user",
                    ));
                }
                any_failed = true;
                break;
            }
            let it = run_one_iteration(
                &block, &fl, idx, total, elem,
                &var_store, &extra_headers, handler.clone(),
            ).await;
            if it.status != "passed" {
                any_failed = true;
            }
            iterations.push(it);
        }
    } else {
        // ── Parallel worker-pool path ───────────────────────────────
        // N workers each pull the next index from a shared atomic counter.
        // Cloning happens lazily inside the worker (only the workers'
        // shared state is pre-cloned, not per-iteration), so memory and
        // cost stay bounded by N rather than `total`.
        let arr = Arc::new(arr);
        let next_idx = Arc::new(AtomicUsize::new(0));
        let block_arc = Arc::new(block.clone());
        let fl_arc = Arc::new(fl.clone());
        let extra_arc = Arc::new(extra_headers.clone());
        let var_arc = Arc::new(var_store.clone());

        let workers = max_concurrency.min(total);
        let mut set: JoinSet<Vec<IterationResult>> = JoinSet::new();
        for _w in 0..workers {
            let arr = arr.clone();
            let next_idx = next_idx.clone();
            let block_arc = block_arc.clone();
            let fl_arc = fl_arc.clone();
            let var_arc = var_arc.clone();
            let extra_arc = extra_arc.clone();
            let handler_w = handler.clone();
            let cancel_w = cancel.clone();
            set.spawn(async move {
                let mut local: Vec<IterationResult> = Vec::new();
                loop {
                    if cancel_w.as_ref().is_some_and(|c| c.is_cancelled()) {
                        break;
                    }
                    let idx = next_idx.fetch_add(1, Ordering::Relaxed);
                    if idx >= arr.len() {
                        break;
                    }
                    // Clone state for THIS iteration. Same per-iter cost as
                    // the sequential path, just spread across N workers.
                    let it = run_one_iteration(
                        &block_arc, &fl_arc, idx, arr.len(), &arr[idx],
                        &var_arc, &extra_arc, handler_w.clone(),
                    ).await;
                    local.push(it);
                }
                local
            });
        }

        // Collect workers' results. JoinErrors (panics) are converted to
        // synthetic error iters below so a single bad iter doesn't crash
        // the whole loop.
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(batch) => {
                    for it in batch {
                        if it.status != "passed" {
                            any_failed = true;
                        }
                        iterations.push(it);
                    }
                }
                Err(e) => {
                    log::error!("loop worker task panicked: {}", e);
                    any_failed = true;
                }
            }
        }

        // Fill any gaps (cancelled iters or worker panics): every iter
        // index in [0, total) should have a result. Missing ones become
        // synthetic "skipped"/"error" records keyed by their index value.
        let returned: HashSet<usize> = iterations.iter().map(|it| it.index).collect();
        for idx in 0..total {
            if !returned.contains(&idx) {
                let cancelled = cancel.as_ref().is_some_and(|c| c.is_cancelled());
                let (status, msg) = if cancelled {
                    ("skipped", "Cancelled by user")
                } else {
                    ("error", "Iteration task panicked")
                };
                let display = elem_display(&arr[idx]);
                iterations.push(synthetic_skipped_iteration(
                    &block, idx, display, status, msg,
                ));
                any_failed = true;
            }
        }

        // Deterministic ordering — workers complete in arbitrary order.
        iterations.sort_by_key(|it| it.index);
    }

    let total_time_ms = total_start.elapsed().as_millis() as u64;
    let aggregate_status = if any_failed { "failed" } else { "passed" };

    BlockResult {
        seq: None,
        name: block.name.clone(),
        block_type: block.block_type.clone(),
        group: block.group.clone(),
        request_method: block.request.method.clone(),
        request_url: block.request.url.clone(),
        request_headers: Vec::new(),
        request_body: None,
        status: aggregate_status.to_string(),
        response: None,
        assertion_results: Vec::new(),
        extract_results: Vec::new(),
        error: None,
        time_ms: total_time_ms,
        step_results: Vec::new(),
        diff_result: None,
        diff_results: Vec::new(),
        iterations,
    }
}

/// Compact display string for a JSON array element — used when synthesizing
/// skipped/error IterationResults so the UI still has a meaningful iter
/// label even when the iter never executed.
fn elem_display(elem: &serde_json::Value) -> String {
    const MAX_DISPLAY: usize = 200;
    let raw = match elem {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if raw.chars().count() > MAX_DISPLAY {
        let trimmed: String = raw.chars().take(MAX_DISPLAY).collect();
        format!("{}…", trimmed)
    } else {
        raw
    }
}

/// Execute a @compare block: run steps sequentially, compute diff, evaluate comparison assertions.
async fn execute_compare_block(
    block: TestBlock,
    mut var_store: VariableStore,
    extra_headers: Vec<(String, String)>,
) -> BlockResult {
    // Fail early if the parser found validation errors (duplicate steps, bad diff refs)
    if !block.errors.is_empty() {
        let msg = block.errors.join("; ");
        return BlockResult {
            seq: None,
            name: block.name.clone(),
            block_type: block.block_type.clone(),
            group: block.group.clone(),
            request_method: String::new(),
            request_url: String::new(),
            request_headers: Vec::new(),
            request_body: None,
            status: "error".to_string(),
            response: None,
            assertion_results: Vec::new(),
            extract_results: Vec::new(),
            error: Some(msg),
            time_ms: 0,
            step_results: Vec::new(),
            diff_result: None,
            diff_results: Vec::new(),
            iterations: Vec::new(),
        };
    }

    let block_start = std::time::Instant::now();
    let mut step_results: Vec<StepResult> = Vec::new();
    let mut all_step_assertions_passed = true;
    let mut step_responses: HashMap<String, http_client::HttpResponse> = HashMap::new();

    for step in &block.steps {
        let url = var_store.interpolate(&step.request.url);
        let mut headers: Vec<(String, String)> = step
            .request
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), var_store.interpolate(v)))
            .collect();
        for (k, v) in &extra_headers {
            headers.push((k.clone(), var_store.interpolate(v)));
        }
        inject_request_id_header(&mut headers, &block);
        let body = step.request.body.as_ref().map(|b| var_store.interpolate(b));

        let start = std::time::Instant::now();
        let result =
            http_client::execute_request(&step.request.method, &url, &headers, body.as_deref())
                .await;
        let time_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(response) => {
                // Process extracts
                let extract_results: Vec<ExtractResult> = step
                    .extracts
                    .iter()
                    .map(|extract| {
                        let value = assertions::resolve_response_value(
                            &extract.source_path,
                            response.status,
                            &response.headers,
                            &response.body,
                        );
                        ExtractResult {
                            variable: extract.variable_name.clone(),
                            success: value.is_some(),
                            value,
                        }
                    })
                    .collect();

                // Merge extracted variables
                for er in &extract_results {
                    if let Some(ref v) = er.value {
                        var_store.set(&er.variable, v);
                    }
                }

                // Evaluate step assertions
                let assertion_results: Vec<AssertionResult> = step
                    .assertions
                    .iter()
                    .map(|a| {
                        assertions::evaluate(a, response.status, &response.headers, &response.body)
                    })
                    .collect();
                if !assertion_results.iter().all(|r| r.passed) {
                    all_step_assertions_passed = false;
                }

                step_responses.insert(step.name.clone(), response.clone());

                step_results.push(StepResult {
                    name: step.name.clone(),
                    request_method: step.request.method.clone(),
                    request_url: url,
                    request_headers: headers,
                    request_body: body,
                    response: Some(response),
                    assertion_results,
                    extract_results,
                    time_ms,
                    error: None,
                    diff_failed: false,
                    diff_failed_pairs: Vec::new(),
                });
            }
            Err(err) => {
                all_step_assertions_passed = false;
                step_results.push(StepResult {
                    name: step.name.clone(),
                    request_method: step.request.method.clone(),
                    request_url: url,
                    request_headers: headers,
                    request_body: body,
                    response: None,
                    assertion_results: Vec::new(),
                    extract_results: Vec::new(),
                    time_ms,
                    error: Some(err),
                    diff_failed: false,
                    diff_failed_pairs: Vec::new(),
                });
            }
        }
    }

    // V1.3: per-pair diff + assertion evaluation.
    let effective = block.effective_diffs();
    let mut diff_results: Vec<NamedDiffResult> = Vec::with_capacity(effective.len());
    let mut failing_pair_indices_per_step: HashMap<String, Vec<usize>> = HashMap::new();
    for (pair_idx, diff) in effective.iter().enumerate() {
        let missing_a = !step_responses.contains_key(&diff.step_a);
        let missing_b = !step_responses.contains_key(&diff.step_b);
        if missing_a || missing_b {
            let mut missing = Vec::new();
            if missing_a {
                missing.push(format!("'{}'", diff.step_a));
            }
            if missing_b {
                missing.push(format!("'{}'", diff.step_b));
            }
            diff_results.push(NamedDiffResult {
                step_a: diff.step_a.clone(),
                step_b: diff.step_b.clone(),
                diff: None,
                error: Some(format!(
                    "@diff step not found in responses: {}",
                    missing.join(", ")
                )),
                allow_mismatch: diff.allow_mismatch,
                assertion_results: Vec::new(),
                required_match_passed: false,
                passed: false,
            });
            continue;
        }
        let body_a = step_responses
            .get(&diff.step_a)
            .map(|r| r.body.as_str())
            .unwrap_or("");
        let body_b = step_responses
            .get(&diff.step_b)
            .map(|r| r.body.as_str())
            .unwrap_or("");
        let computed = assertions::compute_diff(body_a, body_b);
        let required_match_passed = diff.allow_mismatch || computed.match_exact;
        let pair_assertion_results: Vec<AssertionResult> = diff
            .assertions
            .iter()
            .map(|a| assertions::evaluate_with_diff(a, &computed))
            .collect();
        let all_pair_asserts_pass = pair_assertion_results.iter().all(|r| r.passed);
        let pair_passed = required_match_passed && all_pair_asserts_pass;
        if !pair_passed {
            failing_pair_indices_per_step
                .entry(diff.step_b.clone())
                .or_default()
                .push(pair_idx);
        }
        diff_results.push(NamedDiffResult {
            step_a: diff.step_a.clone(),
            step_b: diff.step_b.clone(),
            diff: Some(computed),
            error: None,
            allow_mismatch: diff.allow_mismatch,
            assertion_results: pair_assertion_results,
            required_match_passed,
            passed: pair_passed,
        });
    }

    // Attribute failing pairs onto the candidate step (`step_b`).
    for sr in step_results.iter_mut() {
        if let Some(indices) = failing_pair_indices_per_step.remove(&sr.name) {
            sr.diff_failed = !indices.is_empty();
            sr.diff_failed_pairs = indices;
        }
    }

    let diff_result: Option<assertions::DiffResult> = diff_results
        .first()
        .and_then(|d| d.diff.clone());

    // Legacy block-level `$diff.*` mirror + synthetic `$diff.match` entries
    // for non-`allow_mismatch` pairs that failed the implicit body match.
    let mut comparison_assertions: Vec<AssertionResult> = if let Some(ref diff) = diff_result {
        block
            .assertions
            .iter()
            .map(|a| assertions::evaluate_with_diff(a, diff))
            .collect()
    } else {
        Vec::new()
    };
    for pair in &diff_results {
        if pair.error.is_some() || pair.allow_mismatch {
            continue;
        }
        if let Some(d) = pair.diff.as_ref() {
            if !d.match_exact {
                comparison_assertions.push(AssertionResult {
                    assertion: format!("$diff.match {} -> {}", pair.step_a, pair.step_b),
                    passed: false,
                    actual: Some("mismatch".to_string()),
                    expected: Some("match".to_string()),
                });
            }
        }
    }

    let all_legacy_block_passed = block
        .assertions
        .iter()
        .zip(comparison_assertions.iter())
        .all(|(_, r)| r.passed);
    let all_pairs_passed = diff_results.iter().all(|d| d.passed);
    let any_diff_pair_errored = diff_results.iter().any(|d| d.error.is_some());
    let total_time_ms = block_start.elapsed().as_millis() as u64;

    let all_extracts: Vec<ExtractResult> = step_results
        .iter()
        .flat_map(|sr| sr.extract_results.iter().cloned())
        .collect();

    let first_step = step_results.first();

    let all_pairs_errored = !diff_results.is_empty()
        && diff_results.iter().all(|d| d.error.is_some());
    let block_error: Option<String> = if all_pairs_errored {
        Some(
            diff_results
                .iter()
                .filter_map(|d| d.error.as_deref())
                .collect::<Vec<_>>()
                .join("; "),
        )
    } else {
        None
    };
    // V1.3 status: error if any pair errored, failed if anything else broken, else passed.
    let status = if any_diff_pair_errored {
        "error"
    } else if all_step_assertions_passed && all_pairs_passed && all_legacy_block_passed {
        "passed"
    } else {
        "failed"
    };

    BlockResult {
        seq: None,
        name: block.name.clone(),
        block_type: block.block_type.clone(),
        group: block.group.clone(),
        request_method: first_step.map(|s| s.request_method.clone()).unwrap_or_default(),
        request_url: first_step.map(|s| s.request_url.clone()).unwrap_or_default(),
        request_headers: Vec::new(),
        request_body: None,
        status: status.to_string(),
        response: None,
        assertion_results: comparison_assertions,
        extract_results: all_extracts,
        error: block_error,
        time_ms: total_time_ms,
        step_results,
        diff_result,
        diff_results,
        iterations: Vec::new(),
    }
}

/// Check var_store for well-known telemetry variables and build a TelemetryCollector.
fn try_init_telemetry(var_store: &VariableStore) -> Option<TelemetryCollector> {
    let traces = var_store.get("telemetry_traces_endpoint").unwrap_or_default().to_string();
    let metrics = var_store.get("telemetry_metrics_endpoint").unwrap_or_default().to_string();
    let logs = var_store.get("telemetry_logs_endpoint").unwrap_or_default().to_string();

    // Need at least one endpoint
    if traces.is_empty() && metrics.is_empty() && logs.is_empty() {
        return None;
    }

    let service_name = var_store
        .get("telemetry_service")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "request-pilot".to_string());

    let auth_header = if let Some(token) = var_store.get("telemetry_token") {
        if !token.is_empty() {
            Some(("Authorization".to_string(), format!("Bearer {}", token)))
        } else {
            None
        }
    } else if let Some(api_key) = var_store.get("telemetry_api_key") {
        if !api_key.is_empty() {
            Some(("x-ms-ikey".to_string(), api_key.to_string()))
        } else {
            None
        }
    } else {
        None
    };

    let file_name = var_store.get("__telemetry_file").unwrap_or_default().to_string();
    let display = if file_name.is_empty() { service_name.clone() } else { file_name };

    let config = telemetry::TelemetryConfig {
        traces_endpoint: traces,
        metrics_endpoint: metrics,
        logs_endpoint: logs,
        auth_header,
        service_name,
    };

    Some(telemetry::TelemetryCollector::new(config, &display))
}

/// Feed a completed block result into the telemetry collector.
fn record_block_telemetry(
    telemetry: &mut Option<TelemetryCollector>,
    result: &BlockResult,
    group: Option<&str>,
) {
    let t = match telemetry.as_mut() {
        Some(t) => t,
        None => return,
    };

    // V1.3: flatten block + per-step + per-pair assertions for telemetry.
    let mut assertions: Vec<(String, String, String, bool)> = result
        .assertion_results
        .iter()
        .map(|a| {
            (
                a.assertion.clone(),
                a.expected.clone().unwrap_or_default(),
                a.actual.clone().unwrap_or_default(),
                a.passed,
            )
        })
        .collect();
    for sr in &result.step_results {
        for a in &sr.assertion_results {
            assertions.push((
                format!("[{}] {}", sr.name, a.assertion),
                a.expected.clone().unwrap_or_default(),
                a.actual.clone().unwrap_or_default(),
                a.passed,
            ));
        }
    }
    for dr in &result.diff_results {
        let label = format!("{} -> {}", dr.step_a, dr.step_b);
        for a in &dr.assertion_results {
            assertions.push((
                format!("[{}] {}", label, a.assertion),
                a.expected.clone().unwrap_or_default(),
                a.actual.clone().unwrap_or_default(),
                a.passed,
            ));
        }
    }

    let extracts: Vec<(String, Option<String>, bool)> = result
        .extract_results
        .iter()
        .map(|e| (e.variable.clone(), e.value.clone(), e.success))
        .collect();

    let http_method = if result.request_method.is_empty() {
        None
    } else {
        Some(result.request_method.as_str())
    };
    let http_url = if result.request_url.is_empty() {
        None
    } else {
        Some(result.request_url.as_str())
    };
    let http_status = result.response.as_ref().map(|r| r.status);
    let http_time = result.response.as_ref().map(|_| result.time_ms);

    t.block_complete(
        &result.name,
        &result.block_type,
        group,
        &result.status,
        result.time_ms,
        http_method,
        http_url,
        http_status,
        http_time,
        &assertions,
        &extracts,
        result.error.as_deref(),
    );
}

/// Run test blocks with group-based parallelism using topological waves.
async fn run_tests_with_groups(
    tests: &[&TestBlock],
    var_store: &mut VariableStore,
    handler: &Option<Arc<dyn ProgressHandler>>,
    extra_headers: &[(String, String)],
    cancel: Option<&CancelToken>,
    file_parallel: u32,
) -> Vec<BlockResult> {
    // Assign each block to a group (blocks without @group get unique pseudo-groups)
    let mut group_blocks: HashMap<String, Vec<(usize, TestBlock)>> = HashMap::new();
    let mut block_group: HashMap<usize, String> = HashMap::new();

    for (i, block) in tests.iter().enumerate() {
        let group_name = block
            .group
            .clone()
            .unwrap_or_else(|| format!("__standalone_{}", i));
        group_blocks
            .entry(group_name.clone())
            .or_default()
            .push((i, (*block).clone()));
        block_group.insert(i, group_name);
    }

    // Build dependency graph: group → set of group names it depends on
    let mut group_deps: HashMap<String, HashSet<String>> = HashMap::new();
    for (i, block) in tests.iter().enumerate() {
        let group_name = block_group.get(&i).expect("block index must exist in group map").clone();
        for dep in &block.depends {
            group_deps
                .entry(group_name.clone())
                .or_default()
                .insert(dep.clone());
        }
    }

    let all_group_names: HashSet<String> = group_blocks.keys().cloned().collect();
    let mut completed_groups: HashSet<String> = HashSet::new();
    let mut all_results: Vec<(usize, BlockResult)> = Vec::new();

    // Execute in topological waves
    loop {
        // ── Cancellation check at wave boundary ──
        if cancel.is_some_and(|c| c.is_cancelled()) {
            // Skip every remaining block in remaining groups (graph order
            // unspecified; all get the same reason).
            let remaining_groups: Vec<String> = all_group_names
                .difference(&completed_groups)
                .cloned()
                .collect();
            for group_name in &remaining_groups {
                if let Some(blocks) = group_blocks.remove(group_name) {
                    for (idx, block) in blocks {
                        let result = emit_skipped_with_result(handler, &block, "Cancelled by user");
                        all_results.push((idx, result));
                    }
                }
            }
            break;
        }

        // Find groups that are ready: not completed AND all deps satisfied
        let ready: Vec<String> = all_group_names
            .iter()
            .filter(|g| !completed_groups.contains(*g))
            .filter(|g| {
                group_deps
                    .get(*g)
                    .map_or(true, |deps| deps.iter().all(|d| completed_groups.contains(d)))
            })
            .cloned()
            .collect();

        if ready.is_empty() {
            // Either all done, or circular dependency — skip remaining
            let remaining: Vec<String> = all_group_names
                .difference(&completed_groups)
                .cloned()
                .collect();
            for group_name in &remaining {
                if let Some(blocks) = group_blocks.remove(group_name) {
                    for (idx, block) in blocks {
                        let result = emit_skipped_with_result(
                            handler,
                            &block,
                            "Skipped due to circular dependency",
                        );
                        all_results.push((idx, result));
                    }
                }
            }
            break;
        }

        // Collect all blocks from ready groups and spawn them with a
        // bounded scheduler. The previous implementation spawned every
        // ready block at once (`join_set.spawn` per block); on suites
        // with hundreds of blocks per wave that produced too many
        // concurrent HTTP requests for the OS / TLS stack to handle.
        // We now spawn at most `cap` tasks at a time and, each time one
        // completes, spawn the next queued block — keeping the live
        // task count <= cap regardless of wave size.
        let var_snapshot = var_store.clone();
        let mut join_set: JoinSet<(usize, BlockResult)> = JoinSet::new();
        let cap = file_parallel.max(1) as usize;
        let mut queue: std::collections::VecDeque<(usize, TestBlock)> =
            std::collections::VecDeque::new();

        for group_name in &ready {
            if let Some(blocks) = group_blocks.remove(group_name) {
                for (idx, block) in blocks {
                    // Per-block cancel check at queue time — if cancelled,
                    // synthesize a skipped result instead of dispatching the
                    // request.
                    if cancel.is_some_and(|c| c.is_cancelled()) {
                        let result =
                            emit_skipped_with_result(handler, &block, "Cancelled by user");
                        all_results.push((idx, result));
                        continue;
                    }
                    queue.push_back((idx, block));
                }
            }
        }

        let spawn_one =
            |join_set: &mut JoinSet<(usize, BlockResult)>,
             idx: usize,
             block: TestBlock,
             var_snapshot: &VariableStore,
             handler: &Option<Arc<dyn ProgressHandler>>,
             extra_headers: &[(String, String)],
             cancel: Option<&CancelToken>| {
                let vs = var_snapshot.clone();
                let h = handler.clone();
                let eh = extra_headers.to_vec();
                let cancel_for_task = cancel.cloned();
                let cap_for_task = cap as u32;
                join_set.spawn(async move {
                    FILE_PARALLEL_CAP
                        .scope(cap_for_task, async move {
                            emit_start(&h, &block.name, &block.block_type);
                            let result = if block.compare && !block.steps.is_empty() {
                                execute_compare_block(block, vs, eh).await
                            } else {
                                execute_block_or_loop(block, vs, eh, h.clone(), cancel_for_task)
                                    .await
                            };
                            emit_completed(&h, &result);
                            (idx, result)
                        })
                        .await
                });
            };

        // Prime up to `cap` tasks.
        while join_set.len() < cap {
            if let Some((idx, block)) = queue.pop_front() {
                spawn_one(
                    &mut join_set,
                    idx,
                    block,
                    &var_snapshot,
                    handler,
                    extra_headers,
                    cancel,
                );
            } else {
                break;
            }
        }

        // Drain: each time one completes, spawn the next queued block.
        while let Some(join_result) = join_set.join_next().await {
            if let Ok((idx, result)) = join_result {
                // Merge extracts back into var_store for next wave
                for er in &result.extract_results {
                    if er.success {
                        if let Some(ref v) = er.value {
                            var_store.set(&er.variable, v);
                        }
                    }
                }
                capture_block_response(var_store, &result);
                all_results.push((idx, result));
            }
            // Refill the pool when there's more work waiting.
            if let Some((idx, block)) = queue.pop_front() {
                // If a cancel arrived while we were draining, skip
                // anything still queued instead of starting new HTTP.
                if cancel.is_some_and(|c| c.is_cancelled()) {
                    let result =
                        emit_skipped_with_result(handler, &block, "Cancelled by user");
                    all_results.push((idx, result));
                    continue;
                }
                spawn_one(
                    &mut join_set,
                    idx,
                    block,
                    &var_snapshot,
                    handler,
                    extra_headers,
                    cancel,
                );
            }
        }

        // Mark wave groups as completed
        for group_name in ready {
            completed_groups.insert(group_name);
        }
    }

    // Sort by original index to maintain file order in results
    all_results.sort_by_key(|(idx, _)| *idx);
    all_results.into_iter().map(|(_, r)| r).collect()
}

/// Execute a full test suite: setup → test/request → teardown.
///
/// Pass an `Arc<dyn ProgressHandler>` to receive real-time progress events.
/// The `Arc` is required because the runner spawns parallel tasks that need
/// shared ownership of the handler.
pub async fn run_suite(
    suite: &TestSuite,
    extra_variables: &[(String, String)],
    progress: Option<Arc<dyn ProgressHandler>>,
    run_mode: Option<&str>,
) -> TestRunResults {
    run_suite_inner(suite, extra_variables, &[], progress, run_mode, None).await
}

/// Run a test suite with additional headers injected into every request.
pub async fn run_suite_with_headers(
    suite: &TestSuite,
    extra_variables: &[(String, String)],
    extra_headers: &[(String, String)],
    progress: Option<Arc<dyn ProgressHandler>>,
    run_mode: Option<&str>,
) -> TestRunResults {
    run_suite_inner(suite, extra_variables, extra_headers, progress, run_mode, None).await
}

/// Run a test suite with optional cancellation. When the token is cancelled,
/// remaining setup/test blocks are skipped (with reason "Cancelled by user");
/// teardown still runs to ensure cleanup.
pub async fn run_suite_with_cancel(
    suite: &TestSuite,
    extra_variables: &[(String, String)],
    extra_headers: &[(String, String)],
    progress: Option<Arc<dyn ProgressHandler>>,
    run_mode: Option<&str>,
    cancel: Option<CancelToken>,
) -> TestRunResults {
    run_suite_inner(suite, extra_variables, extra_headers, progress, run_mode, cancel).await
}

/// Inner implementation — emits block-progress events when handler is available.
async fn run_suite_inner(
    suite: &TestSuite,
    extra_variables: &[(String, String)],
    extra_headers: &[(String, String)],
    handler: Option<Arc<dyn ProgressHandler>>,
    run_mode: Option<&str>,
    cancel: Option<CancelToken>,
) -> TestRunResults {
    // Resolve file-level `# @@request-id` into each block's effective header
    // (blocks without their own override inherit the file-level default;
    // blocks with `request_id_disabled` stay opted-out).
    let suite_owned: TestSuite = if suite.request_id_header.is_some() {
        let mut s = suite.clone();
        if let Some(ref file_hdr) = s.request_id_header.clone() {
            for b in &mut s.blocks {
                if !b.request_id_disabled && b.request_id_header.is_none() {
                    b.request_id_header = Some(file_hdr.clone());
                }
            }
        }
        s
    } else {
        suite.clone()
    };
    let suite = &suite_owned;

    let mut var_store = VariableStore::from_pairs(&suite.variables);
    var_store.merge(extra_variables);

    // ── Initialize telemetry from well-known variables ──
    let mut telemetry: Option<TelemetryCollector> = try_init_telemetry(&var_store);

    // Partition blocks by type (preserving file order)
    let mut setups: Vec<&TestBlock> = Vec::new();
    let mut tests: Vec<&TestBlock> = Vec::new();
    let mut teardowns: Vec<&TestBlock> = Vec::new();
    for block in &suite.blocks {
        match block.block_type.as_str() {
            "setup" => setups.push(block),
            "teardown" => teardowns.push(block),
            _ => tests.push(block),
        }
    }

    // Filter blocks by run mode
    let mode_filter = |block: &&TestBlock| -> bool {
        match (&block.mode, &run_mode) {
            (None, _) => true,
            (Some(m), None) => m != "dev",
            (Some(m), Some(rm)) => m.as_str() == *rm,
        }
    };
    let setups: Vec<&TestBlock> = setups.into_iter().filter(mode_filter).collect();
    let tests: Vec<&TestBlock> = tests.into_iter().filter(mode_filter).collect();
    let teardowns: Vec<&TestBlock> = teardowns.into_iter().filter(mode_filter).collect();

    let total_blocks = setups.len() + tests.len() + teardowns.len();
    if let Some(ref mut t) = telemetry {
        t.suite_start(total_blocks);
    }

    let mut block_results: Vec<BlockResult> = Vec::new();
    let mut setup_failed = false;
    let mut cancelled = false;
    let total_start = std::time::Instant::now();

    // ── Phase 1: Setup — sequential ──
    for block in &setups {
        if cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
            cancelled = true;
            let skip_result = emit_skipped_with_result(&handler, block, "Cancelled by user");
            record_block_telemetry(&mut telemetry, &skip_result, block.group.as_deref());
            block_results.push(skip_result);
            continue;
        }
        emit_start(&handler, &block.name, &block.block_type);
        let result = if block.compare && !block.steps.is_empty() {
            execute_compare_block((*block).clone(), var_store.clone(), extra_headers.to_vec()).await
        } else {
            execute_block_or_loop((*block).clone(), var_store.clone(), extra_headers.to_vec(), handler.clone(), cancel.clone()).await
        };
        // Merge extracts into var_store
        for er in &result.extract_results {
            if er.success {
                if let Some(ref v) = er.value {
                    var_store.set(&er.variable, v);
                }
            }
        }
        capture_block_response(&mut var_store, &result);
        if result.status != "passed" {
            setup_failed = true;
        }
        emit_completed(&handler, &result);
        record_block_telemetry(&mut telemetry, &result, block.group.as_deref());
        block_results.push(result);
        if setup_failed {
            break;
        }
    }

    // ── Deferred telemetry init (endpoints may have been extracted during setup) ──
    if telemetry.is_none() {
        if let Some(t) = try_init_telemetry(&var_store) {
            telemetry = Some(t);
            if let Some(ref mut t) = telemetry {
                t.suite_start(total_blocks);
            }
            // Retroactively record completed setup blocks
            for (i, result) in block_results.iter().enumerate() {
                let group = setups.get(i).and_then(|b| b.group.as_deref());
                record_block_telemetry(&mut telemetry, result, group);
            }
        }
    }

    // ── Phase 2: Tests — parallel with dependency graph ──
    if cancelled || cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
        for block in &tests {
            let skip_result = emit_skipped_with_result(&handler, block, "Cancelled by user");
            record_block_telemetry(&mut telemetry, &skip_result, block.group.as_deref());
            block_results.push(skip_result);
        }
    } else if !setup_failed {
        let test_results = run_tests_with_groups(
            &tests,
            &mut var_store,
            &handler,
            extra_headers,
            cancel.as_ref(),
            suite
                .parallel
                .unwrap_or(crate::http_parser::DEFAULT_FILE_PARALLEL),
        )
        .await;
        for (result, block) in test_results.iter().zip(tests.iter()) {
            record_block_telemetry(&mut telemetry, result, block.group.as_deref());
        }
        block_results.extend(test_results);
    } else {
        // Skip all tests
        for block in &tests {
            let skip_result =
                emit_skipped_with_result(&handler, block, "Skipped due to setup failure");
            record_block_telemetry(&mut telemetry, &skip_result, block.group.as_deref());
            block_results.push(skip_result);
        }
    }

    // ── Phase 3: Teardown — sequential (always runs, even on cancel) ──
    for block in &teardowns {
        emit_start(&handler, &block.name, &block.block_type);
        let result = if block.compare && !block.steps.is_empty() {
            execute_compare_block((*block).clone(), var_store.clone(), extra_headers.to_vec()).await
        } else {
            // NOTE: teardown ignores `cancel` — it always runs to completion
            // so cleanup happens even after a user-initiated stop.
            execute_block_or_loop((*block).clone(), var_store.clone(), extra_headers.to_vec(), handler.clone(), None).await
        };
        for er in &result.extract_results {
            if er.success {
                if let Some(ref v) = er.value {
                    var_store.set(&er.variable, v);
                }
            }
        }
        capture_block_response(&mut var_store, &result);
        emit_completed(&handler, &result);
        record_block_telemetry(&mut telemetry, &result, block.group.as_deref());
        block_results.push(result);
    }

    let total_time_ms = total_start.elapsed().as_millis() as u64;
    let passed = block_results
        .iter()
        .filter(|r| r.status == "passed")
        .count();
    let failed = block_results
        .iter()
        .filter(|r| r.status == "failed" || r.status == "error")
        .count();
    let skipped = block_results
        .iter()
        .filter(|r| r.status == "skipped")
        .count();

    // ── Finalize and export telemetry ──
    let telemetry_stats = if let Some(mut t) = telemetry {
        t.suite_complete(passed, failed, skipped, total_time_ms);
        Some(t.export().await)
    } else {
        None
    };

    TestRunResults {
        passed,
        failed,
        skipped,
        total_time_ms,
        block_results,
        final_variables: var_store.to_map(),
        telemetry: telemetry_stats,
    }
}

/// Run only `@setup` blocks to resolve variables, returning the final variable map.
pub async fn resolve_variables_only(
    suite: &TestSuite,
    extra_variables: &[(String, String)],
    run_mode: Option<&str>,
) -> Result<HashMap<String, String>, String> {
    let mut var_store = VariableStore::from_pairs(&suite.variables);
    var_store.merge(extra_variables);

    let mode_filter = |b: &&TestBlock| -> bool {
        match (&b.mode, &run_mode) {
            (None, _) => true,
            (Some(m), None) => m != "dev",
            (Some(m), Some(rm)) => m.as_str() == *rm,
        }
    };

    let mut setup_blocks: Vec<&TestBlock> = suite
        .blocks
        .iter()
        .filter(|b| b.block_type == "setup")
        .filter(mode_filter)
        .collect();
    setup_blocks.sort_by_key(|b| suite.blocks.iter().position(|sb| std::ptr::eq(sb, *b)));

    for block in &setup_blocks {
        let url = var_store.interpolate(&block.request.url);
        let mut headers: Vec<(String, String)> = block
            .request
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), var_store.interpolate(v)))
            .collect();
        // Same request-id resolution as run_suite_inner: file-level default
        // applies unless the block opts out or overrides.
        let mut resolved_block = (*block).clone();
        if !resolved_block.request_id_disabled && resolved_block.request_id_header.is_none() {
            resolved_block.request_id_header = suite.request_id_header.clone();
        }
        inject_request_id_header(&mut headers, &resolved_block);
        let body = block
            .request
            .body
            .as_ref()
            .map(|b| var_store.interpolate(b));

        let result =
            http_client::execute_request(&block.request.method, &url, &headers, body.as_deref())
                .await;

        match result {
            Ok(response) => {
                for extract in &block.extracts {
                    let value = assertions::resolve_response_value(
                        &extract.source_path,
                        response.status,
                        &response.headers,
                        &response.body,
                    );
                    if let Some(v) = value {
                        var_store.set(&extract.variable_name, &v);
                    }
                }
                if !block.name.is_empty() {
                    var_store.set_response(
                        &block.name,
                        CapturedResponse {
                            status: response.status,
                            headers: response.headers.clone(),
                            body: response.body.clone(),
                        },
                    );
                }
            }
            Err(err) => {
                var_store.set(
                    &format!("__error_{}", block.name),
                    &format!("Setup block '{}' failed: {}", block.name, err),
                );
            }
        }
    }

    Ok(var_store.to_map())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_type_order(block_type: &str) -> u8 {
        match block_type {
            "setup" => 0,
            "test" | "request" => 1,
            "teardown" => 2,
            _ => 1,
        }
    }

    #[test]
    fn block_type_order_setup() {
        assert_eq!(block_type_order("setup"), 0);
    }

    #[test]
    fn block_type_order_test() {
        assert_eq!(block_type_order("test"), 1);
    }

    #[test]
    fn block_type_order_request() {
        assert_eq!(block_type_order("request"), 1);
    }

    #[test]
    fn block_type_order_teardown() {
        assert_eq!(block_type_order("teardown"), 2);
    }

    #[test]
    fn block_type_order_unknown_defaults_to_1() {
        assert_eq!(block_type_order("unknown"), 1);
        assert_eq!(block_type_order(""), 1);
    }

    #[test]
    fn block_type_order_sorting_correctness() {
        let mut types = vec!["teardown", "test", "setup", "request"];
        types.sort_by_key(|t| block_type_order(t));
        assert_eq!(types, vec!["setup", "test", "request", "teardown"]);
    }

    #[test]
    fn test_run_results_struct_defaults() {
        let results = TestRunResults {
            passed: 0,
            failed: 0,
            skipped: 0,
            total_time_ms: 0,
            block_results: Vec::new(),
            final_variables: HashMap::new(),
            telemetry: None,
        };
        assert_eq!(results.passed, 0);
        assert!(results.block_results.is_empty());
    }

    #[test]
    fn block_result_status_values() {
        let result = BlockResult {
            seq: None,
            name: "test".to_string(),
            block_type: "test".to_string(),
            group: None,
            request_method: "GET".to_string(),
            request_url: "http://example.com".to_string(),
            request_headers: Vec::new(),
            request_body: None,
            status: "passed".to_string(),
            response: None,
            assertion_results: Vec::new(),
            extract_results: Vec::new(),
            error: None,
            time_ms: 0,
            step_results: Vec::new(),
            diff_result: None,
            diff_results: Vec::new(),
            iterations: Vec::new(),
        };
        assert_eq!(result.status, "passed");
        assert!(result.error.is_none());
        assert!(result.seq.is_none());
    }

    #[test]
    fn extract_result_fields() {
        let er = ExtractResult {
            variable: "token".to_string(),
            value: Some("abc123".to_string()),
            success: true,
        };
        assert!(er.success);
        assert_eq!(er.value.as_deref(), Some("abc123"));
    }

    #[test]
    fn inject_request_id_header_adds_when_missing() {
        let block = TestBlock {
            block_type: "test".into(),
            name: "t".into(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: crate::http_parser::ParsedRequest {
                name: None,
                method: "GET".into(),
                url: "http://x".into(),
                headers: vec![],
                body: None,
            },
            assertions: vec![],
            extracts: vec![],
            compare: false,
            steps: vec![],
            diff: None,
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: Some("X-Request-Id".into()),
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };
        let mut headers = vec![("Content-Type".into(), "application/json".into())];
        inject_request_id_header(&mut headers, &block);
        assert_eq!(headers.len(), 2);
        let (name, value) = &headers[1];
        assert_eq!(name, "X-Request-Id");
        assert_eq!(value.len(), 36); // uuid length
    }

    #[test]
    fn inject_request_id_header_skips_when_already_present() {
        let block = TestBlock {
            block_type: "test".into(),
            name: "t".into(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: crate::http_parser::ParsedRequest {
                name: None,
                method: "GET".into(),
                url: "http://x".into(),
                headers: vec![],
                body: None,
            },
            assertions: vec![],
            extracts: vec![],
            compare: false,
            steps: vec![],
            diff: None,
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: Some("X-Request-Id".into()),
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };
        // case-insensitive match on existing header name
        let mut headers = vec![("x-request-id".into(), "user-supplied-id".into())];
        inject_request_id_header(&mut headers, &block);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].1, "user-supplied-id");
    }

    #[test]
    fn inject_request_id_header_noop_when_disabled_or_none() {
        let mut block = TestBlock {
            block_type: "test".into(),
            name: "t".into(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: crate::http_parser::ParsedRequest {
                name: None,
                method: "GET".into(),
                url: "http://x".into(),
                headers: vec![],
                body: None,
            },
            assertions: vec![],
            extracts: vec![],
            compare: false,
            steps: vec![],
            diff: None,
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };
        let mut headers: Vec<(String, String)> = vec![];
        inject_request_id_header(&mut headers, &block);
        assert!(headers.is_empty());

        block.request_id_header = Some("X-Request-Id".into());
        block.request_id_disabled = true;
        inject_request_id_header(&mut headers, &block);
        assert!(headers.is_empty());
    }

    use crate::http_parser::{CompareStep, Extract, ParsedRequest};

    fn make_block(
        block_type: &str,
        name: &str,
        extracts: Vec<Extract>,
        depends: Vec<String>,
        group: Option<&str>,
    ) -> TestBlock {
        TestBlock {
            block_type: block_type.to_string(),
            name: name.to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: group.map(|s| s.to_string()),
            depends,
            request: ParsedRequest {
                name: Some(name.to_string()),
                method: "GET".to_string(),
                url: "http://127.0.0.1:0/nonexistent".to_string(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts,
            compare: false,
            steps: Vec::new(),
            diff: None,
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        }
    }

    #[tokio::test]
    async fn test_resolve_variables_only_includes_static_vars() {
        let suite = TestSuite {
            variables: vec![
                ("host".to_string(), "example.com".to_string()),
                ("token".to_string(), "abc123".to_string()),
            ],
            blocks: Vec::new(),
            ..Default::default()
        };
        let result = resolve_variables_only(&suite, &[], None).await.unwrap();
        assert_eq!(result.get("host").unwrap(), "example.com");
        assert_eq!(result.get("token").unwrap(), "abc123");
    }

    #[tokio::test]
    async fn test_resolve_variables_only_merges_extra_variables() {
        let suite = TestSuite {
            variables: vec![("host".to_string(), "example.com".to_string())],
            blocks: Vec::new(),
            ..Default::default()
        };
        let extras = vec![("env".to_string(), "staging".to_string())];
        let result = resolve_variables_only(&suite, &extras, None).await.unwrap();
        assert_eq!(result.get("host").unwrap(), "example.com");
        assert_eq!(result.get("env").unwrap(), "staging");
    }

    #[tokio::test]
    async fn test_resolve_variables_only_skips_test_blocks() {
        // Test and teardown blocks should NOT be executed.
        // We use an unreachable address; if the function tried to call it,
        // it would store an error key. No error key ⇒ block was skipped.
        let suite = TestSuite {
            variables: vec![("base".to_string(), "val".to_string())],
            blocks: vec![
                make_block("test", "my_test", Vec::new(), vec![], None),
                make_block("teardown", "my_teardown", Vec::new(), vec![], None),
            ],
            ..Default::default()
        };
        let result = resolve_variables_only(&suite, &[], None).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result.get("base").unwrap(), "val");
        // No error keys from test/teardown blocks
        assert!(!result.keys().any(|k| k.starts_with("__error_")));
    }

    #[tokio::test]
    async fn test_resolve_variables_only_runs_setup_blocks() {
        // A setup block pointing to an unreachable address will fail,
        // but the function should continue and store an error variable.
        let suite = TestSuite {
            variables: vec![("static_var".to_string(), "hello".to_string())],
            blocks: vec![make_block(
                "setup",
                "auth_setup",
                vec![Extract {
                    variable_name: "token".to_string(),
                    source_path: "$.token".to_string(),
                }],
                vec![],
                None,
            )],
            ..Default::default()
        };
        let result = resolve_variables_only(&suite, &[], None).await.unwrap();
        assert_eq!(result.get("static_var").unwrap(), "hello");
        // Setup block should have been attempted; connection refusal ⇒ error key
        assert!(
            result.contains_key("__error_auth_setup"),
            "Expected error key for failed setup block"
        );
    }

    #[tokio::test]
    async fn test_groups_run_in_parallel_order() {
        // Two groups with no deps — both should run (order doesn't matter for correctness)
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("test", "A", vec![], vec![], Some("group1")),
                make_block("test", "B", vec![], vec![], Some("group1")),
                make_block("test", "C", vec![], vec![], Some("group2")),
            ],
            ..Default::default()
        };
        let results = run_suite_inner(&suite, &[], &[], None, None, None).await;
        assert_eq!(results.block_results.len(), 3);
        for r in &results.block_results {
            assert_ne!(r.status, "skipped");
        }
    }

    #[tokio::test]
    async fn test_depends_runs_after_dependency() {
        // group-b depends on group-a
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("test", "A1", vec![], vec![], Some("group-a")),
                make_block(
                    "test",
                    "B1",
                    vec![],
                    vec!["group-a".to_string()],
                    Some("group-b"),
                ),
            ],
            ..Default::default()
        };
        let results = run_suite_inner(&suite, &[], &[], None, None, None).await;
        assert_eq!(results.block_results.len(), 2);
        for r in &results.block_results {
            assert_ne!(r.status, "skipped");
        }
    }

    #[tokio::test]
    async fn test_standalone_blocks_run_parallel() {
        // Blocks without @group get unique pseudo-groups and run in parallel
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("test", "Solo1", vec![], vec![], None),
                make_block("test", "Solo2", vec![], vec![], None),
                make_block("test", "Solo3", vec![], vec![], None),
            ],
            ..Default::default()
        };
        let results = run_suite_inner(&suite, &[], &[], None, None, None).await;
        assert_eq!(results.block_results.len(), 3);
    }

    #[tokio::test]
    async fn test_setup_sequential_then_tests_parallel() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("setup", "Setup1", vec![], vec![], None),
                make_block("test", "T1", vec![], vec![], Some("tests")),
                make_block("test", "T2", vec![], vec![], Some("tests")),
                make_block("teardown", "Cleanup", vec![], vec![], None),
            ],
            ..Default::default()
        };
        let results = run_suite_inner(&suite, &[], &[], None, None, None).await;
        // Setup fails (unreachable addr) → tests skipped, but teardown always runs
        assert_eq!(results.block_results.len(), 4);
    }

    #[tokio::test]
    async fn test_circular_dependency_skips_blocks() {
        // group-a depends on group-b and vice versa → circular
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block(
                    "test",
                    "A",
                    vec![],
                    vec!["group-b".to_string()],
                    Some("group-a"),
                ),
                make_block(
                    "test",
                    "B",
                    vec![],
                    vec!["group-a".to_string()],
                    Some("group-b"),
                ),
            ],
            ..Default::default()
        };
        let results = run_suite_inner(&suite, &[], &[], None, None, None).await;
        assert_eq!(results.block_results.len(), 2);
        for r in &results.block_results {
            assert_eq!(r.status, "skipped");
        }
    }

    #[tokio::test]
    async fn test_teardown_always_runs_after_setup_failure() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("setup", "FailSetup", vec![], vec![], None),
                make_block("test", "T1", vec![], vec![], None),
                make_block("teardown", "Cleanup", vec![], vec![], None),
            ],
            ..Default::default()
        };
        let results = run_suite_inner(&suite, &[], &[], None, None, None).await;
        assert_eq!(results.block_results.len(), 3);
        // Setup errors (connection refused)
        assert_eq!(results.block_results[0].status, "error");
        // Test skipped
        assert_eq!(results.block_results[1].status, "skipped");
        // Teardown still runs (will error too, but it runs)
        assert_ne!(results.block_results[2].status, "skipped");
    }

    #[tokio::test]
    async fn test_results_preserve_file_order() {
        let suite = TestSuite {
            variables: vec![],
            blocks: vec![
                make_block("test", "First", vec![], vec![], Some("g1")),
                make_block("test", "Second", vec![], vec![], Some("g2")),
                make_block("test", "Third", vec![], vec![], Some("g3")),
            ],
            ..Default::default()
        };
        let results = run_suite_inner(&suite, &[], &[], None, None, None).await;
        assert_eq!(results.block_results[0].name, "First");
        assert_eq!(results.block_results[1].name, "Second");
        assert_eq!(results.block_results[2].name, "Third");
    }

    #[tokio::test]
    async fn compare_block_propagates_step_extracts() {
        use crate::variables::VariableStore;

        let block = TestBlock {
            block_type: "test".to_string(),
            name: "compare_extract_test".to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: ParsedRequest {
                name: Some("compare_extract_test".to_string()),
                method: "GET".to_string(),
                url: "http://127.0.0.1:0/nonexistent".to_string(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: true,
            steps: vec![
                CompareStep {
                    name: "step_a".to_string(),
                    request: ParsedRequest {
                        name: Some("step_a".to_string()),
                        method: "GET".to_string(),
                        url: "http://127.0.0.1:0/a".to_string(),
                        headers: Vec::new(),
                        body: None,
                    },
                    assertions: Vec::new(),
                    extracts: vec![Extract {
                        variable_name: "token_a".to_string(),
                        source_path: "$.token".to_string(),
                    }],
                },
                CompareStep {
                    name: "step_b".to_string(),
                    request: ParsedRequest {
                        name: Some("step_b".to_string()),
                        method: "GET".to_string(),
                        url: "http://127.0.0.1:0/b".to_string(),
                        headers: Vec::new(),
                        body: None,
                    },
                    assertions: Vec::new(),
                    extracts: vec![Extract {
                        variable_name: "token_b".to_string(),
                        source_path: "$.token".to_string(),
                    }],
                },
            ],
            diff: None,
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };

        let var_store = VariableStore::new();
        let result = execute_compare_block(block, var_store, vec![]).await;

        // Both steps will fail (connection refused), so extract_results will be
        // empty from the error path. But the important thing is that step_results
        // extracts are collected into the BlockResult. Let's verify the structure
        // is wired correctly by checking step_results exist.
        assert_eq!(result.step_results.len(), 2);
        assert_eq!(result.step_results[0].name, "step_a");
        assert_eq!(result.step_results[1].name, "step_b");

        // On connection error, steps produce empty extract_results, so
        // block-level extract_results should also be empty (no false positives).
        // The key fix is that when steps DO succeed, their extracts propagate.
        assert_eq!(
            result.extract_results.len(),
            result
                .step_results
                .iter()
                .map(|sr| sr.extract_results.len())
                .sum::<usize>(),
            "BlockResult.extract_results must equal the sum of all step extract_results"
        );
    }

    #[tokio::test]
    async fn compare_block_with_parser_errors_returns_error_status() {
        use crate::http_parser::{CompareStep, DiffDirective};
        use crate::variables::VariableStore;

        let block = TestBlock {
            block_type: "test".to_string(),
            name: "bad_diff_ref".to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: ParsedRequest {
                name: Some("bad_diff_ref".to_string()),
                method: String::new(),
                url: String::new(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: true,
            steps: vec![
                CompareStep {
                    name: "step_a".to_string(),
                    request: ParsedRequest {
                        name: None,
                        method: "GET".to_string(),
                        url: "http://127.0.0.1:0/a".to_string(),
                        headers: Vec::new(),
                        body: None,
                    },
                    assertions: Vec::new(),
                    extracts: Vec::new(),
                },
            ],
            diff: Some(DiffDirective {
                step_a: "step_a".to_string(),
                step_b: "nonexistent".to_string(),
                ..Default::default()
            }),
            diffs: Vec::new(),
            errors: vec!["@diff references unknown step 'nonexistent'".to_string()],
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };

        let var_store = VariableStore::new();
        let result = execute_compare_block(block, var_store, vec![]).await;

        assert_eq!(result.status, "error");
        assert!(
            result.error.as_ref().unwrap().contains("nonexistent"),
            "error should mention the invalid step name"
        );
        // No steps should have been executed
        assert!(result.step_results.is_empty());
    }

    #[tokio::test]
    async fn compare_block_missing_diff_response_returns_error() {
        use crate::http_parser::{CompareStep, DiffDirective};
        use crate::variables::VariableStore;

        // Steps exist but diff references a step whose HTTP call will fail,
        // resulting in no response entry. The runner should error, not diff empty strings.
        let block = TestBlock {
            block_type: "test".to_string(),
            name: "missing_response".to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: ParsedRequest {
                name: None,
                method: String::new(),
                url: String::new(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: true,
            steps: vec![
                CompareStep {
                    name: "step_a".to_string(),
                    request: ParsedRequest {
                        name: None,
                        method: "GET".to_string(),
                        url: "http://127.0.0.1:0/a".to_string(),
                        headers: Vec::new(),
                        body: None,
                    },
                    assertions: Vec::new(),
                    extracts: Vec::new(),
                },
                CompareStep {
                    name: "step_b".to_string(),
                    request: ParsedRequest {
                        name: None,
                        method: "GET".to_string(),
                        url: "http://127.0.0.1:0/b".to_string(),
                        headers: Vec::new(),
                        body: None,
                    },
                    assertions: Vec::new(),
                    extracts: Vec::new(),
                },
            ],
            diff: Some(DiffDirective {
                step_a: "step_a".to_string(),
                step_b: "step_b".to_string(),
                ..Default::default()
            }),
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };

        let var_store = VariableStore::new();
        let result = execute_compare_block(block, var_store, vec![]).await;

        // Both steps fail (connection refused), so their responses are NOT in
        // step_responses. The runner should detect this and return an error.
        assert_eq!(result.status, "error");
        assert!(
            result.error.as_ref().unwrap().contains("@diff step not found"),
            "expected '@diff step not found' error, got: {:?}",
            result.error
        );
    }

    #[tokio::test]
    async fn compare_block_empty_steps_returns_error() {
        use crate::http_parser::DiffDirective;
        use crate::variables::VariableStore;

        // A compare block with zero steps but a diff directive — can't diff nothing
        let block = TestBlock {
            block_type: "test".to_string(),
            name: "empty_steps".to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: ParsedRequest {
                name: None,
                method: String::new(),
                url: String::new(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: true,
            steps: vec![], // no steps
            diff: Some(DiffDirective {
                step_a: "a".to_string(),
                step_b: "b".to_string(),
                ..Default::default()
            }),
            diffs: Vec::new(),
            errors: vec![
                "@diff references unknown step 'a'".to_string(),
                "@diff references unknown step 'b'".to_string(),
            ],
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };

        let var_store = VariableStore::new();
        let result = execute_compare_block(block, var_store, vec![]).await;

        assert_eq!(result.status, "error", "empty steps with diff should be error");
        assert!(result.step_results.is_empty(), "no steps should have been executed");
        assert!(result.error.is_some(), "error message should be present");
    }

    #[tokio::test]
    async fn compare_block_diff_missing_step_response_reports_step_names() {
        use crate::http_parser::{CompareStep, DiffDirective};
        use crate::variables::VariableStore;

        // Both steps will fail (connection refused), producing no responses.
        // The diff should report which specific steps are missing.
        let block = TestBlock {
            block_type: "test".to_string(),
            name: "diff_missing_both".to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: ParsedRequest {
                name: None,
                method: String::new(),
                url: String::new(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: true,
            steps: vec![
                CompareStep {
                    name: "alpha".to_string(),
                    request: ParsedRequest {
                        name: None,
                        method: "GET".to_string(),
                        url: "http://127.0.0.1:0/alpha".to_string(),
                        headers: Vec::new(),
                        body: None,
                    },
                    assertions: Vec::new(),
                    extracts: Vec::new(),
                },
                CompareStep {
                    name: "beta".to_string(),
                    request: ParsedRequest {
                        name: None,
                        method: "GET".to_string(),
                        url: "http://127.0.0.1:0/beta".to_string(),
                        headers: Vec::new(),
                        body: None,
                    },
                    assertions: Vec::new(),
                    extracts: Vec::new(),
                },
            ],
            diff: Some(DiffDirective {
                step_a: "alpha".to_string(),
                step_b: "beta".to_string(),
                ..Default::default()
            }),
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: None,
        };

        let var_store = VariableStore::new();
        let result = execute_compare_block(block, var_store, vec![]).await;

        assert_eq!(result.status, "error");
        let err_msg = result.error.as_ref().unwrap();
        assert!(
            err_msg.contains("alpha"),
            "error should mention missing step 'alpha', got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("beta"),
            "error should mention missing step 'beta', got: {}",
            err_msg
        );
        // Steps were attempted even though they failed
        assert_eq!(result.step_results.len(), 2, "both steps should have been attempted");
        assert!(result.step_results[0].error.is_some(), "alpha step should have an error");
        assert!(result.step_results[1].error.is_some(), "beta step should have an error");
    }

    // -------------------------------------------------------------------
    // # @@for / Repeater tests
    //
    // Strategy: hit `http://127.0.0.1:0/...` so connection refused. Each
    // iteration's inner BlockResult has status = "error" but it preserves
    // the *interpolated* request_url, which is what we want to assert on
    // for substitution correctness without standing up a mock HTTP server.
    // -------------------------------------------------------------------

    fn make_loop_block(iter_var: &str, source_var: &str, url_template: &str) -> TestBlock {
        TestBlock {
            block_type: "test".to_string(),
            name: "loop_block".to_string(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: vec![],
            request: ParsedRequest {
                name: Some("loop_block".to_string()),
                method: "GET".to_string(),
                url: url_template.to_string(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: false,
            steps: Vec::new(),
            diff: None,
            diffs: Vec::new(),
            errors: Vec::new(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
            for_loop: Some(crate::http_parser::ForLoop {
                iter_var: iter_var.to_string(),
                source_var: source_var.to_string(),
                parallel: None,
            }),
        }
    }

    #[tokio::test]
    async fn loop_block_iterates_scalar_array() {
        use crate::variables::VariableStore;
        let block = make_loop_block(
            "user_id",
            "user_ids",
            "http://127.0.0.1:0/users/{{user_id}}",
        );
        let mut vs = VariableStore::new();
        vs.set("user_ids", r#"["u1","u2","u3"]"#);

        let result = execute_block_or_loop(block, vs, vec![], None, None).await;

        // 3 iterations executed in order; each substituted user_id correctly.
        assert_eq!(result.iterations.len(), 3, "all 3 elements iterated");
        assert_eq!(result.iterations[0].iter_value, "u1");
        assert_eq!(result.iterations[1].iter_value, "u2");
        assert_eq!(result.iterations[2].iter_value, "u3");
        assert_eq!(result.iterations[0].index, 0);
        assert_eq!(result.iterations[2].index, 2);
        assert!(
            result.iterations[0].block_result.request_url.ends_with("/users/u1"),
            "iter 0 should target /users/u1, got {}",
            result.iterations[0].block_result.request_url
        );
        assert!(
            result.iterations[2].block_result.request_url.ends_with("/users/u3"),
            "iter 2 should target /users/u3, got {}",
            result.iterations[2].block_result.request_url
        );
        // Connection refused on each iter -> aggregate status is "failed"
        // (not "error" — error is reserved for loop-source problems).
        assert_eq!(result.status, "failed");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn loop_block_with_object_elements_supports_dotted_interpolation() {
        use crate::variables::VariableStore;
        let block = make_loop_block(
            "user",
            "users",
            "http://127.0.0.1:0/users/{{user.id}}/posts/{{user.post}}",
        );
        let mut vs = VariableStore::new();
        vs.set("users", r#"[{"id":"u1","post":"p1"},{"id":"u2","post":"p2"}]"#);

        let result = execute_block_or_loop(block, vs, vec![], None, None).await;

        assert_eq!(result.iterations.len(), 2);
        assert!(
            result.iterations[0].block_result.request_url.ends_with("/users/u1/posts/p1"),
            "iter 0 url: {}",
            result.iterations[0].block_result.request_url
        );
        assert!(
            result.iterations[1].block_result.request_url.ends_with("/users/u2/posts/p2"),
            "iter 1 url: {}",
            result.iterations[1].block_result.request_url
        );
    }

    #[tokio::test]
    async fn loop_block_missing_source_yields_error_status() {
        use crate::variables::VariableStore;
        let block = make_loop_block(
            "x",
            "missing_var",
            "http://127.0.0.1:0/{{x}}",
        );
        let result = execute_block_or_loop(block, VariableStore::new(), vec![], None, None).await;

        assert_eq!(result.status, "error");
        assert!(result.iterations.is_empty(), "no iterations on source error");
        let err = result.error.as_deref().unwrap_or("");
        assert!(
            err.contains("missing_var") && err.contains("undefined"),
            "error should mention missing var, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn loop_block_non_array_source_yields_error_status() {
        use crate::variables::VariableStore;
        let block = make_loop_block("x", "scalar_src", "http://127.0.0.1:0/{{x}}");
        let mut vs = VariableStore::new();
        // A bare string that's NOT a JSON array.
        vs.set("scalar_src", "just-a-string");

        let result = execute_block_or_loop(block, vs, vec![], None, None).await;
        assert_eq!(result.status, "error");
        assert!(result.iterations.is_empty());
        assert!(
            result.error.as_deref().unwrap_or("").contains("not valid JSON")
                || result.error.as_deref().unwrap_or("").contains("not a JSON array"),
            "error: {:?}",
            result.error
        );

        // Object source → not-an-array branch.
        let block2 = make_loop_block("x", "obj_src", "http://127.0.0.1:0/{{x}}");
        let mut vs2 = VariableStore::new();
        vs2.set("obj_src", r#"{"k":"v"}"#);
        let result2 = execute_block_or_loop(block2, vs2, vec![], None, None).await;
        assert_eq!(result2.status, "error");
        assert!(result2.iterations.is_empty());
        assert!(
            result2.error.as_deref().unwrap_or("").contains("not a JSON array"),
            "error: {:?}",
            result2.error
        );
    }

    #[tokio::test]
    async fn loop_block_empty_array_passes_with_zero_iterations() {
        use crate::variables::VariableStore;
        let block = make_loop_block("x", "empty_src", "http://127.0.0.1:0/{{x}}");
        let mut vs = VariableStore::new();
        vs.set("empty_src", "[]");

        let result = execute_block_or_loop(block, vs, vec![], None, None).await;
        assert_eq!(result.status, "passed", "empty array is vacuously passing");
        assert!(result.iterations.is_empty());
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn loop_block_index_and_iteration_counters_bound() {
        use crate::variables::VariableStore;
        // URL template embeds both built-in counters so we can assert on them.
        let block = make_loop_block(
            "v",
            "vals",
            "http://127.0.0.1:0/idx/{{$index}}/it/{{$iteration}}/{{v}}",
        );
        let mut vs = VariableStore::new();
        vs.set("vals", r#"["a","b"]"#);
        let result = execute_block_or_loop(block, vs, vec![], None, None).await;

        assert_eq!(result.iterations.len(), 2);
        let url0 = &result.iterations[0].block_result.request_url;
        let url1 = &result.iterations[1].block_result.request_url;
        assert!(url0.contains("/idx/0/it/1/a"), "iter 0 url: {}", url0);
        assert!(url1.contains("/idx/1/it/2/b"), "iter 1 url: {}", url1);
    }

    #[tokio::test]
    async fn loop_block_per_iter_extracts_dont_escape_to_outer_store() {
        use crate::variables::VariableStore;
        // Even with extracts on the inner block, the loop runner clones
        // the var_store per iteration so per-iter extracts stay local —
        // the outer store passed in by the caller is untouched.
        let mut block = make_loop_block(
            "user_id",
            "user_ids",
            "http://127.0.0.1:0/users/{{user_id}}",
        );
        block.extracts.push(crate::http_parser::Extract {
            variable_name: "leaked".to_string(),
            source_path: "$.id".to_string(),
        });

        let mut vs = VariableStore::new();
        vs.set("user_ids", r#"["u1"]"#);
        // Sentinel: outer store before the loop.
        assert!(vs.get("leaked").is_none());

        let _result = execute_block_or_loop(block, vs.clone(), vec![], None, None).await;

        // Outer var_store (the one we pass by clone) is unaffected
        // regardless of what happened inside iterations.
        assert!(
            vs.get("leaked").is_none(),
            "per-iter extract must not escape into the outer var_store"
        );
    }

    #[tokio::test]
    async fn loop_block_truncates_oversized_iter_value_for_display() {
        use crate::variables::VariableStore;
        let block = make_loop_block("blob", "blobs", "http://127.0.0.1:0/{{blob.id}}");
        let big_value = "x".repeat(500);
        let arr = format!(r#"[{{"id":"a","payload":"{}"}}]"#, big_value);
        let mut vs = VariableStore::new();
        vs.set("blobs", &arr);

        let result = execute_block_or_loop(block, vs, vec![], None, None).await;
        assert_eq!(result.iterations.len(), 1);
        // 200-char cap (plus the ellipsis suffix) — guards UI/history rendering.
        let display_len = result.iterations[0].iter_value.chars().count();
        assert!(
            display_len <= 201,
            "iter_value should be truncated to ~200 chars, got {}",
            display_len
        );
        assert!(
            result.iterations[0].iter_value.ends_with('…'),
            "truncated iter_value should end with ellipsis"
        );
    }

    /// Test ProgressHandler that records every iteration progress event so
    /// we can assert on the precise sequence and metadata.
    struct RecordingHandler {
        events: std::sync::Mutex<Vec<IterationProgress>>,
    }
    impl ProgressHandler for RecordingHandler {
        fn on_block_start(&self, _: &BlockProgress) {}
        fn on_block_complete(&self, _: &BlockProgress) {}
        fn on_iteration_progress(&self, progress: &IterationProgress) {
            self.events.lock().unwrap().push(progress.clone());
        }
    }

    #[tokio::test]
    async fn loop_block_emits_iteration_progress_events() {
        use crate::variables::VariableStore;
        let block = make_loop_block("x", "vals", "http://127.0.0.1:0/{{x}}");
        let mut vs = VariableStore::new();
        vs.set("vals", r#"["a","b","c"]"#);

        let recorder = Arc::new(RecordingHandler {
            events: std::sync::Mutex::new(Vec::new()),
        });
        let handler: Arc<dyn ProgressHandler> = recorder.clone();

        let _ = execute_block_or_loop(block, vs, vec![], Some(handler), None).await;

        let events = recorder.events.lock().unwrap();
        // 3 iters × 2 events (running + final) = 6 progress events.
        assert_eq!(
            events.len(),
            6,
            "expected 3 running + 3 final = 6 iteration events, got {}",
            events.len()
        );
        // Sequence: (idx=0,running), (idx=0,final), (idx=1,running), (idx=1,final), ...
        assert_eq!(events[0].index, 0);
        assert_eq!(events[0].status, "running");
        assert_eq!(events[0].iter_value, "a");
        assert_eq!(events[0].total, 3);
        assert_eq!(events[0].block_name, "loop_block");

        assert_eq!(events[1].index, 0);
        // Connection refused -> inner block status is "error"; that bubbles
        // into the iteration's final-event status.
        assert!(events[1].status == "error" || events[1].status == "failed");

        assert_eq!(events[4].index, 2);
        assert_eq!(events[4].status, "running");
        assert_eq!(events[4].iter_value, "c");

        // No iteration events should fire when the loop source is invalid.
        let block_bad = make_loop_block("x", "missing", "http://127.0.0.1:0/{{x}}");
        let recorder2 = Arc::new(RecordingHandler {
            events: std::sync::Mutex::new(Vec::new()),
        });
        let h2: Arc<dyn ProgressHandler> = recorder2.clone();
        let _ = execute_block_or_loop(block_bad, VariableStore::new(), vec![], Some(h2), None).await;
        assert!(
            recorder2.events.lock().unwrap().is_empty(),
            "no iteration progress should fire when loop source is invalid"
        );
    }

    // -------------------------------------------------------------------
    // V1.2: # @@parallel — bounded-concurrency loop execution.
    //
    // Strategy: same connection-refused trick as sequential tests. We don't
    // need to verify wall-clock speedup; we verify functional correctness:
    //   - all N iterations still run to completion
    //   - results stay deterministically ordered by index
    //   - parallel(1) ≡ sequential
    //   - cancel mid-flight halts new iter pickup
    // -------------------------------------------------------------------

    fn make_parallel_loop_block(
        iter_var: &str,
        source_var: &str,
        url_template: &str,
        parallel: Option<u32>,
    ) -> TestBlock {
        let mut block = make_loop_block(iter_var, source_var, url_template);
        if let Some(ref mut fl) = block.for_loop {
            fl.parallel = parallel;
        }
        block
    }

    #[tokio::test]
    async fn parallel_loop_preserves_iteration_count_and_order() {
        use crate::variables::VariableStore;
        // 12 iters, parallel=4 → workers compete for indexes from atomic
        // counter. After sort, iteration[i].index must == i for all i, and
        // iter_value must match arr[i].
        let block = make_parallel_loop_block(
            "item",
            "items",
            "http://127.0.0.1:0/x/{{item}}",
            Some(4),
        );
        let mut vs = VariableStore::new();
        vs.set(
            "items",
            r#"["a","b","c","d","e","f","g","h","i","j","k","l"]"#,
        );

        let result = execute_block_or_loop(block, vs, vec![], None, None).await;

        assert_eq!(result.iterations.len(), 12);
        let expected = ["a","b","c","d","e","f","g","h","i","j","k","l"];
        for (i, expected_val) in expected.iter().enumerate() {
            assert_eq!(result.iterations[i].index, i, "index for slot {}", i);
            assert_eq!(
                result.iterations[i].iter_value, *expected_val,
                "iter_value for slot {}", i
            );
            assert!(
                result.iterations[i].block_result.request_url.ends_with(&format!("/x/{}", expected_val)),
                "iter {} url should end with /x/{}", i, expected_val
            );
        }
    }

    #[tokio::test]
    async fn parallel_one_is_equivalent_to_sequential() {
        use crate::variables::VariableStore;
        // parallel: Some(1) MUST take the sequential code path (parallel
        // path requires N>1 workers to be useful and would just add
        // overhead). The runner clamps to sequential — verify ordered
        // results regardless.
        let block_seq = make_parallel_loop_block(
            "x", "xs", "http://127.0.0.1:0/{{x}}", None,
        );
        let block_par1 = make_parallel_loop_block(
            "x", "xs", "http://127.0.0.1:0/{{x}}", Some(1),
        );
        let mut vs = VariableStore::new();
        vs.set("xs", r#"["one","two","three"]"#);

        let r_seq = execute_block_or_loop(block_seq, vs.clone(), vec![], None, None).await;
        let r_par = execute_block_or_loop(block_par1, vs, vec![], None, None).await;

        assert_eq!(r_seq.iterations.len(), r_par.iterations.len());
        for i in 0..r_seq.iterations.len() {
            assert_eq!(r_seq.iterations[i].index, r_par.iterations[i].index);
            assert_eq!(r_seq.iterations[i].iter_value, r_par.iterations[i].iter_value);
        }
    }

    #[tokio::test]
    async fn parallel_loop_with_more_workers_than_iters_works() {
        use crate::variables::VariableStore;
        // parallel=8 but only 3 iters: only 3 workers should spawn (we
        // clamp to total). All 3 results must come back.
        let block = make_parallel_loop_block(
            "item",
            "items",
            "http://127.0.0.1:0/{{item}}",
            Some(8),
        );
        let mut vs = VariableStore::new();
        vs.set("items", r#"["a","b","c"]"#);

        let result = execute_block_or_loop(block, vs, vec![], None, None).await;

        assert_eq!(result.iterations.len(), 3);
        assert_eq!(result.iterations[0].iter_value, "a");
        assert_eq!(result.iterations[1].iter_value, "b");
        assert_eq!(result.iterations[2].iter_value, "c");
    }

    #[tokio::test]
    async fn parallel_loop_pre_cancelled_skips_all_iterations() {
        use crate::variables::VariableStore;
        // Pre-cancelled token: workers see cancel before pulling indexes,
        // so all iters become synthetic skipped results.
        let block = make_parallel_loop_block(
            "item",
            "items",
            "http://127.0.0.1:0/{{item}}",
            Some(4),
        );
        let mut vs = VariableStore::new();
        vs.set("items", r#"["a","b","c","d","e"]"#);
        let cancel = CancelToken::new();
        cancel.cancel();

        let result = execute_block_or_loop(block, vs, vec![], None, Some(cancel)).await;

        // All 5 iters present (synthetic skipped fills the gaps).
        assert_eq!(result.iterations.len(), 5);
        for it in &result.iterations {
            assert_eq!(it.status, "skipped", "iter {} should be skipped", it.index);
            assert!(
                it.block_result.error.as_deref().map_or(false, |e| e.contains("Cancelled")),
                "iter {} should have a cancellation error", it.index
            );
        }
        assert_eq!(result.status, "failed");
    }

    #[tokio::test]
    async fn sequential_loop_pre_cancelled_skips_all_iterations() {
        use crate::variables::VariableStore;
        // Symmetry check: sequential path also synthesizes skipped results
        // when cancel fires before the first iter.
        let block = make_parallel_loop_block(
            "item",
            "items",
            "http://127.0.0.1:0/{{item}}",
            None,
        );
        let mut vs = VariableStore::new();
        vs.set("items", r#"["a","b","c"]"#);
        let cancel = CancelToken::new();
        cancel.cancel();

        let result = execute_block_or_loop(block, vs, vec![], None, Some(cancel)).await;

        assert_eq!(result.iterations.len(), 3);
        for it in &result.iterations {
            assert_eq!(it.status, "skipped");
        }
    }

    #[tokio::test]
    async fn parallel_loop_progress_events_cover_all_iterations() {
        use crate::variables::VariableStore;
        // Even though events arrive interleaved in parallel mode, every
        // iter should fire at least one running and one terminal event.
        let recorder = Arc::new(RecordingHandler {
            events: std::sync::Mutex::new(Vec::new()),
        });
        let handler: Arc<dyn ProgressHandler> = recorder.clone();
        let block = make_parallel_loop_block(
            "item",
            "items",
            "http://127.0.0.1:0/{{item}}",
            Some(3),
        );
        let mut vs = VariableStore::new();
        vs.set("items", r#"["a","b","c","d","e","f"]"#);

        let _ = execute_block_or_loop(block, vs, vec![], Some(handler), None).await;

        let events = recorder.events.lock().unwrap();
        // 6 iters × 2 events (running + terminal) = 12 events
        assert_eq!(events.len(), 12, "expected 12 progress events, got {}", events.len());
        // Each index should have exactly one running and one terminal event.
        for idx in 0..6 {
            let for_idx: Vec<_> = events.iter().filter(|e| e.index == idx).collect();
            assert_eq!(for_idx.len(), 2, "iter {} should have 2 events", idx);
            assert!(for_idx.iter().any(|e| e.status == "running"));
            assert!(for_idx.iter().any(|e| e.status != "running"));
        }
    }
}
