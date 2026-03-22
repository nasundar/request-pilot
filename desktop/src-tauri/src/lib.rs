use request_pilot_core::{
    env_file, history, http_client, http_parser, test_runner, url_trie, variables,
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

#[tauri::command]
async fn run_test_suite(
    suite: http_parser::TestSuite,
    extra_variables: Vec<(String, String)>,
    store: State<'_, Mutex<HistoryStore>>,
    app: tauri::AppHandle,
) -> Result<test_runner::TestRunResults, String> {
    let run_id = request_pilot_core::uuid::Uuid::new_v4().to_string();
    let progress = Arc::new(TauriProgress(app.clone()));
    let mut results = test_runner::run_suite(&suite, &extra_variables, Some(progress)).await;

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
    test_runner::resolve_variables_only(&suite, &extra_variables).await
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

pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::new(HistoryStore::new()))
        .invoke_handler(tauri::generate_handler![
            send_request,
            parse_http_file,
            parse_test_file,
            generate_http,
            run_test_suite,
            resolve_variables,
            get_history,
            get_filtered_history,
            clear_history,
            suggest_urls,
            suggest_domain_paths,
            read_env_file,
            write_env_file,
            check_variables,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
