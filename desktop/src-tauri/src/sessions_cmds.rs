//! Tauri command surface for the Sessions feature.
//!
//! Holds a single `Mutex<SessionsState>` in Tauri-managed state, persists
//! `SessionsConfig` to `<config_dir>/request-pilot/sessions_config.json`,
//! and exposes commands for reading the config, listing files/versions/
//! sessions, loading a session, and recording a run.
//!
//! Recording is a best-effort side effect: failures log but never propagate
//! back to the test runner UI (the run already succeeded by the time we
//! record).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;

use request_pilot_core::chrono::{self, Utc};
use request_pilot_core::http_parser::TestSuite;
use request_pilot_core::sessions::{
    build_body_redact_rules, CapturePolicy, Component, FileIdentity, RecordInput, SessionRecord,
    SessionStore, SessionSummary, SessionTrigger, StatsRecord, TrackedFile, VersionEntry,
};
use request_pilot_core::sessions_config::{
    self, default_config_path, SessionsConfig,
};
use request_pilot_core::test_runner::TestRunResults;

pub struct SessionsState {
    pub config: SessionsConfig,
    pub store: Option<Arc<SessionStore>>,
}

impl SessionsState {
    pub fn from_disk() -> Self {
        let config = sessions_config::load_from(&default_config_path()).unwrap_or_default();
        let store = config
            .root
            .as_ref()
            .and_then(|r| SessionStore::open(PathBuf::from(r)).ok())
            .map(Arc::new);
        Self { config, store }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsStatus {
    pub root: Option<String>,
    pub auto_record: bool,
    pub active: bool,
}

fn lock<'a>(s: &'a State<'_, Mutex<SessionsState>>) -> Result<std::sync::MutexGuard<'a, SessionsState>, String> {
    s.lock().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sessions_get_status(state: State<'_, Mutex<SessionsState>>) -> Result<SessionsStatus, String> {
    let g = lock(&state)?;
    Ok(SessionsStatus {
        root: g.config.root.clone(),
        auto_record: g.config.auto_record,
        active: g.store.is_some(),
    })
}

#[tauri::command]
pub fn sessions_set_root(
    root: Option<String>,
    state: State<'_, Mutex<SessionsState>>,
) -> Result<SessionsStatus, String> {
    let mut g = lock(&state)?;
    match root.as_ref().map(|s| s.trim().to_string()) {
        Some(r) if !r.is_empty() => {
            let store = SessionStore::open(PathBuf::from(&r))
                .map_err(|e| format!("Failed to open sessions store at {}: {}", r, e))?;
            g.config.root = Some(r);
            g.store = Some(Arc::new(store));
        }
        _ => {
            g.config.root = None;
            g.store = None;
        }
    }
    sessions_config::save_to(&default_config_path(), &g.config)?;
    Ok(SessionsStatus {
        root: g.config.root.clone(),
        auto_record: g.config.auto_record,
        active: g.store.is_some(),
    })
}

#[tauri::command]
pub fn sessions_set_auto_record(
    enabled: bool,
    state: State<'_, Mutex<SessionsState>>,
) -> Result<SessionsStatus, String> {
    let mut g = lock(&state)?;
    g.config.auto_record = enabled;
    sessions_config::save_to(&default_config_path(), &g.config)?;
    Ok(SessionsStatus {
        root: g.config.root.clone(),
        auto_record: g.config.auto_record,
        active: g.store.is_some(),
    })
}

#[tauri::command]
pub fn sessions_list_files(
    state: State<'_, Mutex<SessionsState>>,
) -> Result<Vec<TrackedFile>, String> {
    let g = lock(&state)?;
    match &g.store {
        Some(s) => s.list_files().map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
pub fn sessions_list_versions(
    file_id: String,
    state: State<'_, Mutex<SessionsState>>,
) -> Result<Vec<VersionEntry>, String> {
    let g = lock(&state)?;
    match &g.store {
        Some(s) => s.list_versions(&file_id).map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
pub fn sessions_list_sessions(
    file_id: String,
    sha256: String,
    state: State<'_, Mutex<SessionsState>>,
) -> Result<Vec<SessionSummary>, String> {
    let g = lock(&state)?;
    match &g.store {
        Some(s) => s
            .list_sessions(&file_id, &sha256)
            .map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadedSession {
    pub source: String,
    pub record: SessionRecord,
}

#[tauri::command]
pub fn sessions_load_session(
    file_id: String,
    sha256: String,
    run_id: String,
    state: State<'_, Mutex<SessionsState>>,
) -> Result<LoadedSession, String> {
    let g = lock(&state)?;
    let store = g
        .store
        .as_ref()
        .ok_or_else(|| "No sessions root configured".to_string())?;
    let (source, record) = store
        .load_session(&file_id, &sha256, &run_id)
        .map_err(|e| e.to_string())?;
    Ok(LoadedSession { source, record })
}

/// Record a run. Called by the desktop after `run_test_suite` returns. This
/// is best-effort: any error is reported as a string but the caller treats
/// it as non-fatal (the run already produced its results).
#[tauri::command]
pub fn sessions_record_run(
    suite: TestSuite,
    file_path: Option<String>,
    alias: Option<String>,
    source_content: String,
    results: TestRunResults,
    variables: Vec<(String, String)>,
    mode: Option<String>,
    env_file: Option<String>,
    started_at_ms: i64,
    state: State<'_, Mutex<SessionsState>>,
) -> Result<Option<String>, String> {
    // `suite` carries `redact_body_rules` (file-level + per-block) which we
    // merge with global config patterns below.
    let (store, auto) = {
        let g = lock(&state)?;
        (g.store.clone(), g.config.auto_record)
    };
    if !auto {
        return Ok(None);
    }
    let Some(store) = store else {
        return Ok(None);
    };

    let identity = FileIdentity::from_path_and_alias(
        file_path.as_deref(),
        alias.as_deref(),
        "desktop-buffer",
    );
    let source_path = file_path.as_ref().map(PathBuf::from);
    let started = chrono::DateTime::<Utc>::from_timestamp_millis(started_at_ms)
        .unwrap_or_else(Utc::now);
    let finished = Utc::now();

    let (capture_policy, global_redact_paths) = {
        let g = lock(&state)?;
        (
            g.config.capture_policy.clone(),
            g.config.body_redaction_paths.clone(),
        )
    };
    let (suite_redact_rules, block_redact_rules) =
        build_body_redact_rules(&suite, &global_redact_paths);

    let input = RecordInput {
        identity,
        source_path: source_path.as_deref(),
        source_content: &source_content,
        policy: capture_policy,
        trigger: SessionTrigger::Manual,
        component: Component::Desktop,
        started_at: started,
        finished_at: finished,
        mode,
        env_file,
        env_identity: None,
        variables,
        results,
        file_id_override: None,
        redact_body_rules: suite_redact_rules,
        block_redact_body_rules: block_redact_rules,
    };

    match store.record_run(input) {
        Ok(rec) => Ok(Some(rec.run_id)),
        Err(e) => {
            log::warn!("[sessions] record_run failed: {}", e);
            Err(e.to_string())
        }
    }
}

// build_identity helper moved into core: FileIdentity::from_path_and_alias.

// ─── Phase 1.5: snapshot loading + capture presets ─────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct LoadedSnapshot {
    /// Raw `.http` source as it was when the run executed.
    pub source: String,
    /// Re-parsed test suite from `source`.
    pub suite: TestSuite,
    /// The captured run record (immutable, contains assertion + extract
    /// results, response payloads, etc.).
    pub record: SessionRecord,
}

/// Load a session as a snapshot: returns the source, a freshly-parsed
/// `TestSuite`, and the full `SessionRecord`. The desktop UI uses this to
/// reconstitute the test state at the moment the run executed (block
/// statuses, captured request/response bodies, assertion outcomes).
#[tauri::command]
pub fn sessions_load_as_snapshot(
    file_id: String,
    sha256: String,
    run_id: String,
    state: State<'_, Mutex<SessionsState>>,
) -> Result<LoadedSnapshot, String> {
    let g = lock(&state)?;
    let store = g
        .store
        .as_ref()
        .ok_or_else(|| "No sessions root configured".to_string())?;
    let (source, suite, record) = store
        .load_as_snapshot(&file_id, &sha256, &run_id)
        .map_err(|e| e.to_string())?;
    Ok(LoadedSnapshot {
        source,
        suite,
        record,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturePolicyView {
    pub policy: CapturePolicy,
    pub preset: String,
}

#[tauri::command]
pub fn sessions_get_capture_policy(
    state: State<'_, Mutex<SessionsState>>,
) -> Result<CapturePolicyView, String> {
    let g = lock(&state)?;
    Ok(CapturePolicyView {
        policy: g.config.capture_policy.clone(),
        preset: g.config.capture_preset.clone(),
    })
}

/// Switch the capture policy to a named preset (`snapshot`, `privacy_first`,
/// `full_debug`) or `custom` to apply an arbitrary `policy`. Persists.
#[tauri::command]
pub fn sessions_set_capture_preset(
    preset: String,
    policy: Option<CapturePolicy>,
    state: State<'_, Mutex<SessionsState>>,
) -> Result<CapturePolicyView, String> {
    let mut g = lock(&state)?;
    if preset == "custom" {
        if let Some(p) = policy {
            g.config.capture_policy = p;
        }
        g.config.capture_preset = "custom".to_string();
    } else {
        g.config.apply_preset(&preset);
    }
    sessions_config::save_to(&default_config_path(), &g.config)?;
    Ok(CapturePolicyView {
        policy: g.config.capture_policy.clone(),
        preset: g.config.capture_preset.clone(),
    })
}

#[tauri::command]
pub fn sessions_get_stats(
    state: State<'_, Mutex<SessionsState>>,
    file_id: String,
    sha: Option<String>,
) -> Result<Option<StatsRecord>, String> {
    let g = lock(&state)?;
    let store = g
        .store
        .as_ref()
        .ok_or_else(|| "Sessions store not configured".to_string())?;
    store
        .load_stats(&file_id, sha.as_deref())
        .map_err(|e| e.to_string())
}

/// Open a native folder picker and return the selected absolute path. Used
/// by the Sessions Settings UI to choose where to write the sessions root.
#[tauri::command]
pub async fn sessions_pick_root_dir(start_dir: Option<String>) -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Choose sessions folder");
        if let Some(s) = start_dir.as_deref() {
            if !s.trim().is_empty() {
                dialog = dialog.set_directory(s);
            }
        }
        dialog
            .pick_folder()
            .map(|p| p.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())
}
