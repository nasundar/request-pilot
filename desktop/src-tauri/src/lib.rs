mod live_capture;
pub mod perf;
mod sessions_cmds;

use request_pilot_core::{
    azure_auth, env_config, env_file, history, http_client, http_parser, test_runner, url_trie,
    variables,
};

use history::HistoryStore;
use simplelog::ConfigBuilder;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

struct TauriProgress(tauri::AppHandle);

impl test_runner::ProgressHandler for TauriProgress {
    fn on_block_start(&self, progress: &test_runner::BlockProgress) {
        let _ = self.0.emit("block-progress", progress);
    }
    fn on_block_complete(&self, progress: &test_runner::BlockProgress) {
        let _ = self.0.emit("block-progress", progress);
    }
    fn on_block_result(&self, result: &test_runner::BlockResult) {
        // Carry the full result (response body, assertion details, extracts)
        // so the UI can populate file.results.block_results live as each
        // block finishes — enabling click-to-inspect during in-flight runs.
        let _ = self.0.emit("block-result", result);
    }
}

/// Shared cancel token for the currently running test suite. Set by
/// `request_run_abort` from the UI; polled by `run_suite_with_cancel` between
/// blocks. Reset at the start of each new run.
#[derive(Default)]
struct CurrentRunCancel(std::sync::Mutex<Option<test_runner::CancelToken>>);

/// Execute a single HTTP request and record it in history.
#[tauri::command]
async fn send_request(
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<http_client::HttpResponse, String> {
    let response =
        http_client::execute_request(&method, &url, &headers, body.as_deref()).await?;

    let mut s = store.lock().map_err(|e| e.to_string())?;
    let seq = s.next_seq();
    let entry = history::HistoryEntry {
        seq,
        id: request_pilot_core::uuid::Uuid::new_v4().to_string(),
        run_id: None,
        source: "manual".to_string(),
        file_name: None,
        group: None,
        block_name: None,
        compare_step: None,
        method,
        url,
        request_headers: headers,
        request_body: body,
        status: response.status,
        response_headers: response.headers.clone(),
        response_body: Some(response.body.clone()),
        response_time_ms: response.time_ms,
        response_size_bytes: response.size_bytes,
        timestamp: request_pilot_core::chrono::Utc::now().to_rfc3339(),
        result_status: String::new(),
    };
    s.add(entry);

    Ok(response)
}

#[tauri::command]
fn parse_http_file(content: String) -> Result<Vec<http_parser::ParsedRequest>, String> {
    Ok(http_parser::parse(&content))
}

#[tauri::command]
fn parse_test_file(content: String) -> Result<http_parser::TestSuite, String> {
    Ok(http_parser::parse_test_suite(&content))
}

#[tauri::command]
fn block_start_lines(content: String) -> Result<Vec<usize>, String> {
    Ok(http_parser::block_start_lines(&content))
}

#[tauri::command]
fn parse_duration(interval: String) -> Result<Option<u64>, String> {
    Ok(request_pilot_core::duration::parse_duration_secs(&interval))
}

#[tauri::command]
fn generate_http(suite: http_parser::TestSuite) -> Result<String, String> {
    Ok(http_parser::generate_http_content(&suite))
}

/// Execute a test suite from an .http file, streaming progress via events.
#[tauri::command]
async fn run_test_suite(
    suite: http_parser::TestSuite,
    extra_variables: Vec<(String, String)>,
    extra_headers: Option<Vec<(String, String)>>,
    run_mode: Option<String>,
    file_name: Option<String>,
    store: State<'_, Mutex<HistoryStore>>,
    cancel_state: State<'_, CurrentRunCancel>,
    app: tauri::AppHandle,
) -> Result<test_runner::TestRunResults, String> {
    let run_id = request_pilot_core::uuid::Uuid::new_v4().to_string();
    let progress = Arc::new(TauriProgress(app.clone()));
    let headers = extra_headers.unwrap_or_default();

    // Install a fresh cancel token for this run. Stop button will toggle it.
    let cancel = test_runner::CancelToken::new();
    if let Ok(mut guard) = cancel_state.0.lock() {
        *guard = Some(cancel.clone());
    }

    let mut results = test_runner::run_suite_with_cancel(
        &suite,
        &extra_variables,
        &headers,
        Some(progress),
        run_mode.as_deref(),
        Some(cancel.clone()),
    )
    .await;

    // Clear the cancel token now that this run is done.
    if let Ok(mut guard) = cancel_state.0.lock() {
        *guard = None;
    }

    // Add each executed request to history with seq numbers
    let mut s = store.lock().map_err(|e| e.to_string())?;
    for block_result in &mut results.block_results {
        // For compare blocks, record each step as a separate history entry
        if !block_result.step_results.is_empty() {
            for step in &block_result.step_results {
                if let Some(ref resp) = step.response {
                    let seq = s.next_seq();
                    let entry = history::HistoryEntry {
                        seq,
                        id: request_pilot_core::uuid::Uuid::new_v4().to_string(),
                        run_id: Some(run_id.clone()),
                        source: "test-run".to_string(),
                        file_name: file_name.clone(),
                        group: block_result.group.clone(),
                        block_name: Some(block_result.name.clone()),
                        compare_step: Some(step.name.clone()),
                        method: step.request_method.clone(),
                        url: step.request_url.clone(),
                        request_headers: step.request_headers.clone(),
                        request_body: step.request_body.clone(),
                        status: resp.status,
                        response_headers: resp.headers.clone(),
                        response_body: Some(resp.body.clone()),
                        response_time_ms: resp.time_ms,
                        response_size_bytes: resp.size_bytes,
                        timestamp: request_pilot_core::chrono::Utc::now().to_rfc3339(),
                        result_status: block_result.status.clone(),
                    };
                    s.add(entry);
                }
            }
        } else if let Some(ref resp) = block_result.response {
            let seq = s.next_seq();
            block_result.seq = Some(seq);
            let entry = history::HistoryEntry {
                seq,
                id: request_pilot_core::uuid::Uuid::new_v4().to_string(),
                run_id: Some(run_id.clone()),
                source: "test-run".to_string(),
                file_name: file_name.clone(),
                group: block_result.group.clone(),
                block_name: Some(block_result.name.clone()),
                compare_step: None,
                method: block_result.request_method.clone(),
                url: block_result.request_url.clone(),
                request_headers: block_result.request_headers.clone(),
                request_body: block_result.request_body.clone(),
                status: resp.status,
                response_headers: resp.headers.clone(),
                response_body: Some(resp.body.clone()),
                response_time_ms: resp.time_ms,
                response_size_bytes: resp.size_bytes,
                timestamp: request_pilot_core::chrono::Utc::now().to_rfc3339(),
                result_status: block_result.status.clone(),
            };
            s.add(entry);
        }
    }

    Ok(results)
}

#[tauri::command]
async fn resolve_variables(
    suite: http_parser::TestSuite,
    extra_variables: Vec<(String, String)>,
) -> Result<std::collections::HashMap<String, String>, String> {
    test_runner::resolve_variables_only(&suite, &extra_variables, None).await
}

#[tauri::command]
fn get_history(
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<Vec<history::HistoryEntry>, String> {
    Ok(store.lock().map_err(|e| e.to_string())?.entries.clone())
}

/// Return history entries with bodies stripped (request_body, response_body = None).
/// Much lighter for listing 1000s of entries in the UI.
#[tauri::command]
fn get_history_summary(
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<Vec<history::HistoryEntry>, String> {
    let s = store.lock().map_err(|e| e.to_string())?;
    Ok(s.entries.iter().map(|e| {
        let mut lite = e.clone();
        lite.request_body = None;
        lite.response_body = None;
        lite
    }).collect())
}

/// Return a single history entry's full data (with bodies) by seq number.
#[tauri::command]
fn get_history_entry(
    seq: u64,
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<Option<history::HistoryEntry>, String> {
    let s = store.lock().map_err(|e| e.to_string())?;
    Ok(s.entries.iter().find(|e| e.seq == seq).cloned())
}

/// Return filtered history entries with bodies stripped.
#[tauri::command]
fn get_filtered_history_summary(
    filter: history::HistoryFilter,
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<Vec<history::HistoryEntry>, String> {
    let s = store.lock().map_err(|e| e.to_string())?;
    Ok(s.filter(&filter).into_iter().map(|e| {
        let mut lite = e.clone();
        lite.request_body = None;
        lite.response_body = None;
        lite
    }).collect())
}

#[tauri::command]
fn get_filtered_history(
    filter: history::HistoryFilter,
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<Vec<history::HistoryEntry>, String> {
    let s = store.lock().map_err(|e| e.to_string())?;
    Ok(s.filter(&filter).into_iter().cloned().collect())
}

#[tauri::command]
fn clear_history(store: State<'_, Mutex<HistoryStore>>) -> Result<(), String> {
    store.lock().map_err(|e| e.to_string())?.clear();
    Ok(())
}

#[derive(serde::Serialize)]
struct HistoryDistinctValues {
    file_names: Vec<String>,
    groups: Vec<String>,
    block_names: Vec<String>,
}

/// Return distinct file names, groups, and block names from history.
#[tauri::command]
fn get_history_distinct_values(
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<HistoryDistinctValues, String> {
    let s = store.lock().map_err(|e| e.to_string())?;
    Ok(HistoryDistinctValues {
        file_names: s.distinct_file_names(),
        groups: s.distinct_groups(),
        block_names: s.distinct_block_names(),
    })
}

#[tauri::command]
fn suggest_urls(
    prefix: String,
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<Vec<url_trie::Suggestion>, String> {
    let s = store.lock().map_err(|e| e.to_string())?;
    Ok(s.url_trie.suggest(&prefix, 10))
}

#[tauri::command]
fn suggest_domain_paths(
    prefix: String,
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<Vec<url_trie::DomainPathSuggestion>, String> {
    let s = store.lock().map_err(|e| e.to_string())?;
    Ok(s.url_trie.suggest_domain_paths(&prefix, 5))
}

#[tauri::command]
fn read_env_file(path: String) -> Result<std::collections::HashMap<String, String>, String> {
    env_file::read_env_from_path(&path)
}

// ─── Multi-env (.env file list) commands ──────────────────────────────────

/// Remove entries whose `path` no longer exists on disk. Returns the number
/// of pruned entries. Idempotent — safe to call from `env_list` so stale test
/// artifacts (or files the user manually deleted) don't pile up indefinitely.
fn prune_missing(cfg: &mut env_config::EnvConfig) -> usize {
    let before = cfg.entries.len();
    let active_path = cfg.active_entry().map(|e| e.path.clone());
    cfg.entries.retain(|e| std::path::Path::new(&e.path).exists());
    // Recompute active_index against the new entry positions.
    cfg.active_index = active_path.and_then(|p| cfg.entries.iter().position(|e| e.path == p));
    before - cfg.entries.len()
}

/// Returns the persisted env config (list of `.env` entries + active index).
/// Auto-prunes entries whose backing files no longer exist.
#[tauri::command]
fn env_list() -> Result<env_config::EnvConfig, String> {
    let mut cfg = env_config::load()?;
    if prune_missing(&mut cfg) > 0 {
        env_config::save(&cfg)?;
    }
    Ok(cfg)
}

/// Load the `.env` at `path`, derive a name (from `# @@name` directive or
/// filename stem), add to the persisted list (dedupes), activate if first,
/// and return the new config plus the resolved vars of the now-active entry.
#[tauri::command]
fn env_add(path: String) -> Result<EnvAddResult, String> {
    let (directive_name, vars) = env_file::read_env_named_from_path(&path)?;
    let name = directive_name
        .or_else(|| filename_stem(&path))
        .unwrap_or_else(|| "env".to_string());
    let mut cfg = env_config::load()?;
    cfg.add(env_config::EnvEntry {
        path: path.clone(),
        name,
    });
    env_config::save(&cfg)?;
    let active_vars = resolve_active_vars(&cfg)?;
    Ok(EnvAddResult {
        config: cfg,
        active_vars,
        loaded_vars: vars.into_iter().collect(),
    })
}

/// Remove the entry at `index` and return the updated config + resolved vars
/// for whatever is active afterwards.
#[tauri::command]
fn env_remove(index: usize) -> Result<EnvListResult, String> {
    let mut cfg = env_config::load()?;
    cfg.remove(index);
    env_config::save(&cfg)?;
    let active_vars = resolve_active_vars(&cfg)?;
    Ok(EnvListResult { config: cfg, active_vars })
}

/// Set (or clear with `null`) the active entry and return the resolved vars.
#[tauri::command]
fn env_set_active(index: Option<usize>) -> Result<EnvListResult, String> {
    let mut cfg = env_config::load()?;
    cfg.set_active(index);
    env_config::save(&cfg)?;
    let active_vars = resolve_active_vars(&cfg)?;
    Ok(EnvListResult { config: cfg, active_vars })
}

/// Return the `(name, value)` pairs from the currently-active `.env` file, or
/// an empty vec if none is active.
#[tauri::command]
fn env_resolve_active() -> Result<Vec<(String, String)>, String> {
    let cfg = env_config::load()?;
    resolve_active_vars(&cfg)
}

/// Read and parse the `.env` file for `entries[index]` without changing the
/// active state. Used by the desktop UI to populate the body of any expanded
/// env card (active or not).
#[tauri::command]
fn env_resolve_entry(index: usize) -> Result<Vec<(String, String)>, String> {
    let cfg = env_config::load()?;
    let entry = cfg
        .entries
        .get(index)
        .ok_or_else(|| format!("env entry index {} out of range", index))?;
    let (_name, map) = env_file::read_env_named_from_path(&entry.path)?;
    let mut v: Vec<(String, String)> = map.into_iter().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(v)
}

/// Persist the supplied `vars` to `entries[index].path`, embedding the
/// entry's display name as `# @@name <name>` so it round-trips on reload.
/// Returns the (unchanged) config plus the resolved vars for whatever is
/// currently active.
#[tauri::command]
fn env_save(index: usize, vars: Vec<(String, String)>) -> Result<EnvListResult, String> {
    let cfg = env_config::load()?;
    let entry = cfg
        .entries
        .get(index)
        .ok_or_else(|| format!("env entry index {} out of range", index))?
        .clone();
    env_file::write_env_ordered_to_path(&entry.path, Some(&entry.name), &vars)?;
    let active_vars = resolve_active_vars(&cfg)?;
    Ok(EnvListResult { config: cfg, active_vars })
}

#[derive(serde::Serialize)]
struct EnvListResult {
    config: env_config::EnvConfig,
    active_vars: Vec<(String, String)>,
}

#[derive(serde::Serialize)]
struct EnvAddResult {
    config: env_config::EnvConfig,
    active_vars: Vec<(String, String)>,
    /// Raw vars parsed from the file we just added (handy for surface-level
    /// UI previews even when the added file isn't the active one).
    loaded_vars: Vec<(String, String)>,
}

fn resolve_active_vars(cfg: &env_config::EnvConfig) -> Result<Vec<(String, String)>, String> {
    let Some(entry) = cfg.active_entry() else {
        return Ok(Vec::new());
    };
    let (_name, map) = env_file::read_env_named_from_path(&entry.path)?;
    let mut v: Vec<(String, String)> = map.into_iter().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(v)
}

fn filename_stem(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

/// Atomically open a native save-file dialog and write content to the chosen path.
/// Returns the chosen path (or null if cancelled). The frontend never receives a raw
/// writable path — preventing arbitrary file write attacks.
#[tauri::command]
fn save_file_with_dialog(
    default_name: String,
    content: String,
    title: Option<String>,
    filters: Option<Vec<(String, String)>>,
) -> Result<Option<String>, String> {
    let mut dialog = rfd::FileDialog::new().set_file_name(&default_name);
    if let Some(t) = title {
        dialog = dialog.set_title(t);
    }
    if let Some(ref f) = filters {
        for (name, ext) in f {
            dialog = dialog.add_filter(name, &[ext.as_str()]);
        }
    }
    match dialog.save_file() {
        Some(path) => {
            let path_str = path.to_string_lossy().to_string();
            std::fs::write(&path, &content)
                .map_err(|e| format!("Failed to write {}: {}", path_str, e))?;
            Ok(Some(path_str))
        }
        None => Ok(None),
    }
}

/// Open a native file-picker dialog and return the chosen path (or null if
/// cancelled). Pure picker — no file I/O performed here.
#[tauri::command]
fn pick_file_with_dialog(
    title: Option<String>,
    filters: Option<Vec<(String, String)>>,
) -> Result<Option<String>, String> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(t) = title {
        dialog = dialog.set_title(t);
    }
    if let Some(ref f) = filters {
        for (name, ext) in f {
            // `ext` may be a comma-separated list ("env,*"); split for rfd.
            let exts: Vec<&str> = ext.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            if !exts.is_empty() {
                dialog = dialog.add_filter(name, &exts);
            }
        }
    }
    Ok(dialog.pick_file().map(|p| p.to_string_lossy().to_string()))
}

/// Signal the currently running test suite to abort. Setup/test blocks remaining
/// after the current in-flight one will be skipped with reason "Cancelled by
/// user"; teardown still runs to ensure cleanup.
#[tauri::command]
fn request_run_abort(cancel_state: State<'_, CurrentRunCancel>) -> Result<bool, String> {
    let guard = cancel_state.0.lock().map_err(|e| e.to_string())?;
    if let Some(token) = guard.as_ref() {
        token.cancel();
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Create a new empty `.env` file at the given path with an optional
/// `# @@name <display>` directive header. Used by the "+ New env file" UI
/// flow. Returns the canonical path written.
#[tauri::command]
fn env_create_file(path: String, display_name: Option<String>) -> Result<String, String> {
    use std::io::Write;
    let p = std::path::Path::new(&path);
    if !p.is_absolute() {
        return Err("env_create_file requires an absolute path".into());
    }
    if p.exists() {
        return Err(format!("File already exists: {}", path));
    }
    let mut content = String::new();
    if let Some(name) = display_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        content.push_str(&format!("# @@name {}\n", name));
    }
    content.push_str("# Add KEY=VALUE pairs below.\n");
    let mut f = std::fs::File::create(p).map_err(|e| format!("Failed to create {}: {}", path, e))?;
    f.write_all(content.as_bytes())
        .map_err(|e| format!("Failed to write {}: {}", path, e))?;
    Ok(path)
}

/// Write content to a known file path (for files already saved once).
/// Only allows writing to .http files to limit surface area.
#[tauri::command]
fn write_file(path: String, content: String) -> Result<(), String> {
    let canon = std::path::Path::new(&path);
    // Restrict to .http files only
    match canon.extension().and_then(|e| e.to_str()) {
        Some("http") => {}
        _ => return Err("write_file only allows .http files".into()),
    }
    // Ensure path is absolute (came from a prior save dialog)
    if !canon.is_absolute() {
        return Err("write_file requires an absolute path".into());
    }
    std::fs::write(canon, &content)
        .map_err(|e| format!("Failed to write {}: {}", path, e))
}

/// Add a history entry directly (e.g. from live capture).
#[tauri::command]
fn add_history_entry(
    entry: history::HistoryEntry,
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<u64, String> {
    let mut s = store.lock().map_err(|e| e.to_string())?;
    let seq = s.next_seq();
    let mut e = entry;
    e.seq = seq;
    s.add(e);
    Ok(seq)
}

/// Update an existing history entry by id (e.g. response body follow-up).
#[tauri::command]
fn update_history_entry(
    entry: history::HistoryEntry,
    store: State<'_, Mutex<HistoryStore>>,
) -> Result<bool, String> {
    let mut s = store.lock().map_err(|e| e.to_string())?;
    Ok(s.update(entry))
}

#[tauri::command]
fn fetch_azure_token(resource: String) -> Result<azure_auth::AzureToken, String> {
    azure_auth::fetch_token(&resource)
}

#[tauri::command]
fn check_azure_cli() -> Result<bool, String> {
    Ok(azure_auth::is_az_cli_available())
}

#[tauri::command]
async fn start_device_code(
    tenant_id: String,
    client_id: String,
    scope: String,
) -> Result<azure_auth::DeviceCodeResponse, String> {
    azure_auth::start_device_code_flow(&tenant_id, &client_id, &scope).await
}

#[tauri::command]
async fn poll_device_code(
    tenant_id: String,
    client_id: String,
    device_code: String,
) -> Result<azure_auth::TokenResponse, String> {
    match azure_auth::poll_device_code_token(&tenant_id, &client_id, &device_code).await? {
        Ok(token) => Ok(token),
        Err(status) => Err(status),
    }
}

#[tauri::command]
fn check_variables(
    suite: http_parser::TestSuite,
    env_vars: Vec<(String, String)>,
) -> Result<Vec<String>, String> {
    let mut store = variables::VariableStore::from_pairs(&suite.variables);
    store.merge(&env_vars);

    // Collect all text that will be interpolated across all blocks
    let mut all_text = String::new();
    for block in &suite.blocks {
        all_text.push_str(&block.request.url);
        all_text.push(' ');
        for (_, v) in &block.request.headers {
            all_text.push_str(v);
            all_text.push(' ');
        }
        if let Some(ref body) = block.request.body {
            all_text.push_str(body);
            all_text.push(' ');
        }
    }

    Ok(store.find_unresolved(&all_text))
}

/// Start the live capture WebSocket server.
#[tauri::command]
async fn start_live_capture(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, Arc<live_capture::LiveCaptureState>>,
) -> Result<(), String> {
    let state = state.inner().clone();
    tokio::spawn(async move {
        if let Err(e) = live_capture::start_server(state, app_handle).await {
            log::error!("Live capture server error: {}", e);
        }
    });
    Ok(())
}

/// Stop the live capture WebSocket server.
#[tauri::command]
async fn stop_live_capture(
    state: tauri::State<'_, Arc<live_capture::LiveCaptureState>>,
) -> Result<(), String> {
    live_capture::stop_server(&state).await;
    Ok(())
}

/// Set the live capture mode (off, all, filtered) and notify connected extension.
#[tauri::command]
async fn set_live_capture_mode(
    mode: String,
    state: tauri::State<'_, Arc<live_capture::LiveCaptureState>>,
) -> Result<(), String> {
    *state.mode.lock().await = mode.clone();
    let msg = serde_json::to_string(&live_capture::OutgoingMessage::SetMode { mode })
        .map_err(|e| e.to_string())?;
    let _ = state.command_tx.send(msg);
    Ok(())
}

/// Get the current live capture status.
#[tauri::command]
async fn get_live_capture_status(
    state: tauri::State<'_, Arc<live_capture::LiveCaptureState>>,
) -> Result<serde_json::Value, String> {
    let mode = state.mode.lock().await.clone();
    let running = *state.running.lock().await;
    let connected = *state.connected.lock().await;
    Ok(serde_json::json!({
        "mode": mode,
        "running": running,
        "connected": connected,
    }))
}

pub fn run() {
    // All log output goes to a file — never to the console
    let log_path = std::env::temp_dir().join("request-pilot-desktop.log");
    if let Ok(file) = std::fs::File::create(&log_path) {
        let _ = simplelog::WriteLogger::init(
            simplelog::LevelFilter::Info,
            ConfigBuilder::new().set_time_format_rfc3339().build(),
            file,
        );
    }

    // Install a panic hook so unexpected panics are logged instead of silently lost.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("[RequestPilot] PANIC: {}", info);
        default_hook(info);
    }));

    let sessions_state = sessions_cmds::SessionsState::from_disk();

    // Best-effort startup prune: if a retention policy is configured with at least
    // one cap, run `SessionStore::prune` once in the background. Failures are
    // logged but never block startup.
    if let (Some(store), Some(policy)) = (
        sessions_state.store.as_ref(),
        sessions_state.config.retention.as_ref(),
    ) {
        if policy.max_age_days.is_some()
            || policy.max_sessions_per_version.is_some()
            || policy.max_total_size_gb.is_some()
        {
            let store_clone = store.clone();
            let policy_clone = policy.clone();
            tauri::async_runtime::spawn(async move {
                match tauri::async_runtime::spawn_blocking(move || {
                    store_clone.prune(&policy_clone, false)
                })
                .await
                {
                    Ok(Ok(report)) => log::info!(
                        "startup prune removed {} sessions, freed {} bytes",
                        report.removed_count,
                        report.bytes_freed
                    ),
                    Ok(Err(e)) => log::warn!("startup prune failed: {}", e),
                    Err(e) => log::warn!("startup prune task panicked: {}", e),
                }
            });
        }
    }

    tauri::Builder::default()
        .manage(Mutex::new(HistoryStore::new()))
        .manage(Arc::new(live_capture::LiveCaptureState::new()))
        .manage(Mutex::new(sessions_state))
        .manage(CurrentRunCancel::default())
        .invoke_handler(tauri::generate_handler![
            send_request,
            parse_http_file,
            parse_test_file,
            block_start_lines,
            parse_duration,
            generate_http,
            run_test_suite,
            request_run_abort,
            resolve_variables,
            get_history,
            get_history_summary,
            get_history_entry,
            get_filtered_history,
            get_filtered_history_summary,
            get_history_distinct_values,
            clear_history,
            suggest_urls,
            suggest_domain_paths,
            read_env_file,
            env_list,
            env_add,
            env_remove,
            env_set_active,
            env_resolve_active,
            env_resolve_entry,
            env_save,
            env_create_file,
            fetch_azure_token,
            check_azure_cli,
            start_device_code,
            poll_device_code,
            check_variables,
            save_file_with_dialog,
            pick_file_with_dialog,
            write_file,
            add_history_entry,
            update_history_entry,
            start_live_capture,
            stop_live_capture,
            set_live_capture_mode,
            get_live_capture_status,
            perf::format_body,
            perf::build_json_tree,
            perf::expand_json_node,
            perf::compute_diff,
            perf::sort_and_normalize,
            sessions_cmds::sessions_get_status,
            sessions_cmds::sessions_set_root,
            sessions_cmds::sessions_set_auto_record,
            sessions_cmds::sessions_list_files,
            sessions_cmds::sessions_list_versions,
            sessions_cmds::sessions_list_sessions,
            sessions_cmds::sessions_load_session,
            sessions_cmds::sessions_load_as_snapshot,
            sessions_cmds::sessions_get_capture_policy,
            sessions_cmds::sessions_set_capture_preset,
            sessions_cmds::sessions_pick_root_dir,
            sessions_cmds::sessions_record_run,
            sessions_cmds::sessions_get_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
