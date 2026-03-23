mod live_capture;

use request_pilot_core::{
    azure_auth, env_file, history, http_client, http_parser, test_runner, url_trie, variables,
};

use history::HistoryStore;
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
}

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
    app: tauri::AppHandle,
) -> Result<test_runner::TestRunResults, String> {
    let run_id = request_pilot_core::uuid::Uuid::new_v4().to_string();
    let progress = Arc::new(TauriProgress(app.clone()));
    let headers = extra_headers.unwrap_or_default();
    let mut results = test_runner::run_suite_with_headers(
        &suite,
        &extra_variables,
        &headers,
        Some(progress),
        run_mode.as_deref(),
    ).await;

    // Add each executed request to history with seq numbers
    let mut s = store.lock().map_err(|e| e.to_string())?;
    for block_result in &mut results.block_results {
        if let Some(ref resp) = block_result.response {
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

#[tauri::command]
fn write_env_file(
    path: String,
    vars: std::collections::HashMap<String, String>,
) -> Result<(), String> {
    env_file::write_env_to_path(&path, &vars)
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
            eprintln!("Live capture server error: {}", e);
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
    tauri::Builder::default()
        .manage(Mutex::new(HistoryStore::new()))
        .manage(Arc::new(live_capture::LiveCaptureState::new()))
        .invoke_handler(tauri::generate_handler![
            send_request,
            parse_http_file,
            parse_test_file,
            generate_http,
            run_test_suite,
            resolve_variables,
            get_history,
            get_filtered_history,
            get_history_distinct_values,
            clear_history,
            suggest_urls,
            suggest_domain_paths,
            read_env_file,
            write_env_file,
            fetch_azure_token,
            check_azure_cli,
            start_device_code,
            poll_device_code,
            check_variables,
            start_live_capture,
            stop_live_capture,
            set_live_capture_mode,
            get_live_capture_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
