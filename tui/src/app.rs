use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::Arc;
use request_pilot_core::http_parser::{TestSuite, parse_test_suite, generate_http_content};
use request_pilot_core::test_runner::{BlockProgress, ProgressHandler, TestRunResults, BlockResult};
use request_pilot_core::history::{HistoryStore, HistoryEntry};
use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

// === Input mode system ===

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Input { prompt: String, purpose: InputPurpose, buffer: String },
    Confirm { prompt: String, purpose: ConfirmPurpose },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPurpose {
    OpenFile,
    LoadEnv,
    SaveEnvAs,
    NewFileName,
    AddVarName,
    AddVarValue { name: String },
    EditVarValue { name: String },
    ExportHistory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmPurpose {
    CloseFile { file_idx: usize },
    ClearHistory,
    DeleteVar { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseTab {
    Body,
    Headers,
    Assertions,
}

/// Which top-level mode the TUI is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Files,
    History,
}

/// Which pane has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    FileTree,
    CodeView,
    Response,
    Variables,
    HistoryList,
    FilterInput,
}

/// Sidebar sub-tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Files,
    Variables,
}

/// Messages sent from the test runner to the event loop.
pub enum RunnerMessage {
    BlockStart(BlockProgress),
    BlockComplete(BlockProgress),
    SuiteComplete {
        file_idx: usize,
        results: TestRunResults,
    },
}

/// A loaded .http file with parsed suite and optional results.
pub struct LoadedFile {
    pub path: Option<PathBuf>,
    pub name: String,
    pub content: String,
    pub suite: TestSuite,
    pub results: Option<TestRunResults>,
    pub expanded: bool,
    pub group_expanded: HashMap<String, bool>,
}

/// Tree node for navigating the file/group/block hierarchy.
#[derive(Debug, Clone)]
pub enum TreeNode {
    File { file_idx: usize },
    Group { file_idx: usize, group_name: String },
    Block { file_idx: usize, block_idx: usize },
}

/// Progress handler that sends events over an mpsc channel.
pub struct TuiProgress {
    tx: mpsc::UnboundedSender<RunnerMessage>,
}

impl TuiProgress {
    pub fn new(tx: mpsc::UnboundedSender<RunnerMessage>) -> Self {
        Self { tx }
    }
}

impl ProgressHandler for TuiProgress {
    fn on_block_start(&self, progress: &BlockProgress) {
        let _ = self.tx.send(RunnerMessage::BlockStart(progress.clone()));
    }

    fn on_block_complete(&self, progress: &BlockProgress) {
        let _ = self.tx.send(RunnerMessage::BlockComplete(progress.clone()));
    }
}

pub struct App {
    pub mode: Mode,
    pub focus: Focus,
    pub sidebar_tab: SidebarTab,
    pub loaded_files: Vec<LoadedFile>,
    pub env_vars: HashMap<String, String>,
    pub env_path: Option<PathBuf>,
    pub history: HistoryStore,
    pub active_file_idx: Option<usize>,
    pub active_block_idx: Option<usize>,
    pub tree_nodes: Vec<TreeNode>,
    pub tree_cursor: usize,
    pub should_quit: bool,
    pub status_message: Option<(String, std::time::Instant)>,
    pub run_queued: bool,
    pub run_single_queued: Option<(usize, usize)>,
    pub run_group_queued: Option<(usize, Vec<usize>)>,
    pub is_running: bool,
    pub show_help: bool,

    // New state
    pub input_mode: InputMode,
    pub response_tab: ResponseTab,
    pub history_detail_idx: Option<usize>,
    pub vars_cursor: usize,

    // Scroll state
    pub code_scroll: u16,
    pub response_scroll: u16,
    pub vars_scroll: u16,

    // Progress tracking
    pub progress_current: usize,
    pub progress_total: usize,
    pub spinner_tick: usize,

    // History mode state
    pub history_cursor: usize,
    pub history_scroll: u16,
    pub filter_text: String,
    pub filtered_history_len: usize,

    // Channel receiver (set up in run())
    runner_rx: Option<mpsc::UnboundedReceiver<RunnerMessage>>,
    runner_tx: mpsc::UnboundedSender<RunnerMessage>,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            mode: Mode::Files,
            focus: Focus::FileTree,
            sidebar_tab: SidebarTab::Files,
            loaded_files: Vec::new(),
            env_vars: HashMap::new(),
            env_path: None,
            history: HistoryStore::new(),
            active_file_idx: None,
            active_block_idx: None,
            tree_nodes: Vec::new(),
            tree_cursor: 0,
            should_quit: false,
            status_message: None,
            run_queued: false,
            run_single_queued: None,
            run_group_queued: None,
            is_running: false,
            show_help: false,
            input_mode: InputMode::Normal,
            response_tab: ResponseTab::Body,
            history_detail_idx: None,
            vars_cursor: 0,
            code_scroll: 0,
            response_scroll: 0,
            vars_scroll: 0,
            progress_current: 0,
            progress_total: 0,
            spinner_tick: 0,
            history_cursor: 0,
            history_scroll: 0,
            filter_text: String::new(),
            filtered_history_len: 0,
            runner_rx: Some(rx),
            runner_tx: tx,
        }
    }

    pub fn load_env(&mut self, path: &Path) -> color_eyre::Result<()> {
        let path_str = path.to_string_lossy().to_string();
        let vars = request_pilot_core::env_file::read_env_from_path(&path_str)
            .map_err(|e| color_eyre::eyre::eyre!(e))?;
        for (k, v) in vars {
            self.env_vars.insert(k, v);
        }
        self.env_path = Some(path.to_path_buf());
        self.set_status(format!("Loaded env: {}", path.display()));
        Ok(())
    }

    pub fn load_file(&mut self, path: &Path) -> color_eyre::Result<()> {
        let content = std::fs::read_to_string(path)?;
        let suite = parse_test_suite(&content);
        let name = path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        for (k, v) in &suite.variables {
            if !v.is_empty() && !v.starts_with("your-") {
                self.env_vars.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }

        self.loaded_files.push(LoadedFile {
            path: Some(path.to_path_buf()),
            name,
            content,
            suite,
            results: None,
            expanded: true,
            group_expanded: HashMap::new(),
        });

        self.rebuild_tree();
        if self.active_file_idx.is_none() {
            self.active_file_idx = Some(0);
            self.active_block_idx = Some(0);
        }
        self.set_status(format!("Loaded: {}", path.display()));
        Ok(())
    }

    pub fn queue_run_all(&mut self) {
        if !self.is_running {
            self.run_queued = true;
        }
    }

    pub fn queue_run_single(&mut self, file_idx: usize, block_idx: usize) {
        if !self.is_running {
            self.run_single_queued = Some((file_idx, block_idx));
        }
    }

    pub fn queue_run_group(&mut self, file_idx: usize, block_indices: Vec<usize>) {
        if !self.is_running && !block_indices.is_empty() {
            self.run_group_queued = Some((file_idx, block_indices));
        }
    }

    pub fn set_status(&mut self, msg: String) {
        self.status_message = Some((msg, std::time::Instant::now()));
    }

    /// Reset scroll positions when switching blocks.
    pub fn select_block(&mut self, file_idx: usize, block_idx: usize) {
        self.active_file_idx = Some(file_idx);
        self.active_block_idx = Some(block_idx);
        self.code_scroll = 0;
        self.response_scroll = 0;
    }

    /// Record block results into history.
    fn record_history(&mut self, results: &TestRunResults) {
        let run_id = request_pilot_core::uuid::Uuid::new_v4().to_string();
        for br in &results.block_results {
            if let Some(ref resp) = br.response {
                let seq = self.history.next_seq();
                let entry = HistoryEntry {
                    seq,
                    id: request_pilot_core::uuid::Uuid::new_v4().to_string(),
                    run_id: Some(run_id.clone()),
                    source: "tui".to_string(),
                    block_name: Some(br.name.clone()),
                    method: br.request_method.clone(),
                    url: br.request_url.clone(),
                    request_headers: br.request_headers.clone(),
                    request_body: br.request_body.clone(),
                    status: resp.status,
                    response_headers: resp.headers.clone(),
                    response_body: Some(resp.body.clone()),
                    response_time_ms: resp.time_ms,
                    response_size_bytes: resp.size_bytes,
                    timestamp: request_pilot_core::chrono::Utc::now()
                        .to_rfc3339(),
                };
                self.history.add(entry);
            }
        }
    }

    /// Get spinner character based on tick.
    pub fn spinner_char(&self) -> &'static str {
        const FRAMES: &[&str] = &["\u{2800}", "\u{2801}", "\u{2803}", "\u{2807}", "\u{280f}", "\u{281f}", "\u{283f}", "\u{287f}"];
        FRAMES[self.spinner_tick % FRAMES.len()]
    }

    /// Update filtered history length for cursor bounds.
    fn update_filtered_history_len(&mut self) {
        use request_pilot_core::history::HistoryFilter;
        let filter = if self.filter_text.is_empty() {
            HistoryFilter::default()
        } else {
            HistoryFilter {
                url_contains: Some(self.filter_text.clone()),
                ..Default::default()
            }
        };
        self.filtered_history_len = self.history.filter(&filter).len();
    }

    /// Rebuild the flat tree node list from loaded files.
    pub fn rebuild_tree(&mut self) {
        self.tree_nodes.clear();
        for (fi, file) in self.loaded_files.iter().enumerate() {
            self.tree_nodes.push(TreeNode::File { file_idx: fi });
            if !file.expanded { continue; }

            let mut seen_groups: Vec<String> = Vec::new();
            for (bi, block) in file.suite.blocks.iter().enumerate() {
                if let Some(ref grp) = block.group {
                    if !seen_groups.contains(grp) {
                        seen_groups.push(grp.clone());
                        self.tree_nodes.push(TreeNode::Group {
                            file_idx: fi,
                            group_name: grp.clone(),
                        });
                    }
                    let expanded = file.group_expanded.get(grp).copied().unwrap_or(true);
                    if expanded {
                        self.tree_nodes.push(TreeNode::Block { file_idx: fi, block_idx: bi });
                    }
                } else {
                    self.tree_nodes.push(TreeNode::Block { file_idx: fi, block_idx: bi });
                }
            }
        }
    }

    // --- New interactive methods ---

    pub fn new_file(&mut self) {
        let template = "### New Request\n\n# @name Example Request\n# @type test\nGET https://httpbin.org/get\nContent-Type: application/json\n";
        let suite = parse_test_suite(template);
        self.loaded_files.push(LoadedFile {
            path: None,
            name: "untitled.http".to_string(),
            content: template.to_string(),
            suite,
            results: None,
            expanded: true,
            group_expanded: HashMap::new(),
        });
        self.rebuild_tree();
        self.active_file_idx = Some(self.loaded_files.len() - 1);
        self.active_block_idx = if self.loaded_files.last().map_or(false, |f| !f.suite.blocks.is_empty()) {
            Some(0)
        } else {
            None
        };
        self.set_status("Created new file".to_string());
    }

    pub fn save_current_file(&mut self) {
        if let Some(fi) = self.active_file_idx {
            if let Some(file) = self.loaded_files.get(fi) {
                if let Some(ref path) = file.path {
                    let content = generate_http_content(&file.suite);
                    let display = path.display().to_string();
                    match std::fs::write(path, &content) {
                        Ok(_) => {
                            self.loaded_files[fi].content = content;
                            self.set_status(format!("Saved: {}", display));
                        }
                        Err(e) => self.set_status(format!("Save failed: {}", e)),
                    }
                } else {
                    self.set_status("No file path (new file)".to_string());
                }
            }
        }
    }

    pub fn close_file(&mut self, file_idx: usize) {
        if file_idx < self.loaded_files.len() {
            self.loaded_files.remove(file_idx);
            self.rebuild_tree();
            if self.loaded_files.is_empty() {
                self.active_file_idx = None;
                self.active_block_idx = None;
                self.tree_cursor = 0;
            } else {
                if let Some(afi) = self.active_file_idx {
                    if afi >= self.loaded_files.len() {
                        self.active_file_idx = Some(self.loaded_files.len() - 1);
                    } else if afi > file_idx {
                        self.active_file_idx = Some(afi - 1);
                    }
                }
                self.tree_cursor = self.tree_cursor.min(self.tree_nodes.len().saturating_sub(1));
            }
            self.set_status("File closed".to_string());
        }
    }

    pub fn reload_current_file(&mut self) {
        if let Some(fi) = self.active_file_idx {
            let path = self.loaded_files.get(fi).and_then(|f| f.path.clone());
            if let Some(path) = path {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let suite = parse_test_suite(&content);
                        let file = &mut self.loaded_files[fi];
                        file.content = content;
                        file.suite = suite;
                        file.results = None;
                        self.rebuild_tree();
                        self.set_status(format!("Reloaded: {}", path.display()));
                    }
                    Err(e) => self.set_status(format!("Reload failed: {}", e)),
                }
            } else {
                self.set_status("No file path to reload".to_string());
            }
        }
    }

    pub fn toggle_block_disabled(&mut self) {
        if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
            if let Some(file) = self.loaded_files.get_mut(fi) {
                if let Some(block) = file.suite.blocks.get_mut(bi) {
                    block.disabled = !block.disabled;
                    let state = if block.disabled { "disabled" } else { "enabled" };
                    let name = block.name.clone();
                    self.set_status(format!("{}: {}", name, state));
                }
            }
        }
    }

    pub fn save_env(&mut self) {
        if let Some(ref path) = self.env_path {
            let path_str = path.to_string_lossy().to_string();
            match request_pilot_core::env_file::write_env_to_path(&path_str, &self.env_vars) {
                Ok(_) => self.set_status(format!("Saved env: {}", path.display())),
                Err(e) => self.set_status(format!("Save env failed: {}", e)),
            }
        } else {
            self.set_status("No .env path set".to_string());
        }
    }

    pub fn add_variable(&mut self, name: String, value: String) {
        self.env_vars.insert(name.clone(), value);
        self.set_status(format!("Added variable: {}", name));
    }

    pub fn delete_variable(&mut self, name: &str) {
        if self.env_vars.remove(name).is_some() {
            self.set_status(format!("Deleted variable: {}", name));
            let count = self.env_vars.len();
            if self.vars_cursor >= count && count > 0 {
                self.vars_cursor = count - 1;
            }
        }
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history_cursor = 0;
        self.set_status("History cleared".to_string());
    }

    pub fn export_history(&mut self, path_str: String) {
        use request_pilot_core::history::HistoryFilter;
        let entries = self.history.filter(&HistoryFilter::default());
        match serde_json::to_string_pretty(&entries) {
            Ok(json) => {
                match std::fs::write(&path_str, json) {
                    Ok(_) => self.set_status(format!("Exported history to: {}", path_str)),
                    Err(e) => self.set_status(format!("Export failed: {}", e)),
                }
            }
            Err(e) => self.set_status(format!("Serialization failed: {}", e)),
        }
    }

    pub fn get_sorted_var_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.env_vars.keys().cloned().collect();
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        names
    }

    pub fn submit_input(&mut self, purpose: InputPurpose, value: String) {
        match purpose {
            InputPurpose::OpenFile => {
                let path = PathBuf::from(&value);
                if let Err(e) = self.load_file(&path) {
                    self.set_status(format!("Failed to open: {}", e));
                }
            }
            InputPurpose::LoadEnv => {
                let path = PathBuf::from(&value);
                if let Err(e) = self.load_env(&path) {
                    self.set_status(format!("Failed to load env: {}", e));
                }
            }
            InputPurpose::SaveEnvAs => {
                self.env_path = Some(PathBuf::from(&value));
                self.save_env();
            }
            InputPurpose::NewFileName => {
                self.new_file();
            }
            InputPurpose::AddVarName => {
                if !value.is_empty() {
                    self.input_mode = InputMode::Input {
                        prompt: format!("Value for '{}': ", value),
                        purpose: InputPurpose::AddVarValue { name: value },
                        buffer: String::new(),
                    };
                    return;
                }
            }
            InputPurpose::AddVarValue { name } => {
                self.add_variable(name, value);
            }
            InputPurpose::EditVarValue { name } => {
                if self.env_vars.contains_key(&name) {
                    self.env_vars.insert(name.clone(), value);
                    self.set_status(format!("Updated variable: {}", name));
                }
            }
            InputPurpose::ExportHistory => {
                self.export_history(value);
            }
        }
    }

    pub fn confirm_action(&mut self, purpose: ConfirmPurpose) {
        match purpose {
            ConfirmPurpose::CloseFile { file_idx } => {
                self.close_file(file_idx);
            }
            ConfirmPurpose::ClearHistory => {
                self.clear_history();
            }
            ConfirmPurpose::DeleteVar { name } => {
                self.delete_variable(&name);
            }
        }
    }

    // --- Run helpers ---

    fn spawn_suite_run(&mut self, fi: usize) {
        self.is_running = true;
        self.progress_current = 0;
        self.progress_total = self.loaded_files[fi].suite.blocks.len();
        self.spinner_tick = 0;

        let suite = self.loaded_files[fi].suite.clone();
        let extra_vars: Vec<(String, String)> = self.env_vars.iter()
            .map(|(k, v)| (k.clone(), v.clone())).collect();
        let tx = self.runner_tx.clone();
        let handler = Arc::new(TuiProgress::new(tx.clone()));

        tokio::spawn(async move {
            let results = request_pilot_core::test_runner::run_suite(
                &suite, &extra_vars, Some(handler), None,
            ).await;
            let _ = tx.send(RunnerMessage::SuiteComplete { file_idx: fi, results });
        });
        self.set_status("Running all tests...".to_string());
    }

    fn spawn_single_block_run(&mut self, fi: usize, bi: usize) {
        let suite_data = self.loaded_files.get(fi).and_then(|file| {
            file.suite.blocks.get(bi).map(|block| {
                let single_suite = TestSuite {
                    variables: file.suite.variables.clone(),
                    blocks: vec![block.clone()],
                    ..Default::default()
                };
                let total_blocks = file.suite.blocks.len();
                (single_suite, block.name.clone(), total_blocks)
            })
        });

        if let Some((single_suite, block_name, total_blocks)) = suite_data {
            self.is_running = true;
            self.progress_current = 0;
            self.progress_total = 1;
            self.spinner_tick = 0;
            self.set_status(format!("Running: {}...", block_name));

            let extra_vars: Vec<(String, String)> = self.env_vars.iter()
                .map(|(k, v)| (k.clone(), v.clone())).collect();
            let tx = self.runner_tx.clone();
            let handler = Arc::new(TuiProgress::new(tx.clone()));

            tokio::spawn(async move {
                let single_results = request_pilot_core::test_runner::run_suite(
                    &single_suite, &extra_vars, Some(handler), None,
                ).await;

                let mut full_results = TestRunResults {
                    passed: single_results.passed,
                    failed: single_results.failed,
                    skipped: single_results.skipped,
                    total_time_ms: single_results.total_time_ms,
                    block_results: Vec::new(),
                    final_variables: single_results.final_variables,
                    telemetry: None,
                };
                for _ in 0..total_blocks {
                    full_results.block_results.push(BlockResult {
                        seq: None,
                        name: String::new(),
                        block_type: String::new(),
                        request_method: String::new(),
                        request_url: String::new(),
                        request_headers: Vec::new(),
                        request_body: None,
                        status: "pending".to_string(),
                        response: None,
                        assertion_results: Vec::new(),
                        extract_results: Vec::new(),
                        error: None,
                        time_ms: 0,
                    });
                }
                if let Some(single_br) = single_results.block_results.into_iter().next() {
                    if bi < full_results.block_results.len() {
                        full_results.block_results[bi] = single_br;
                    }
                }
                let _ = tx.send(RunnerMessage::SuiteComplete {
                    file_idx: fi, results: full_results,
                });
            });
        }
    }

    fn spawn_group_run(&mut self, fi: usize, block_indices: Vec<usize>) {
        let suite_data = self.loaded_files.get(fi).map(|file| {
            let blocks: Vec<_> = block_indices.iter()
                .filter_map(|&bi| file.suite.blocks.get(bi).cloned())
                .collect();
            let group_suite = TestSuite {
                variables: file.suite.variables.clone(),
                blocks,
                ..Default::default()
            };
            let total_blocks = file.suite.blocks.len();
            (group_suite, total_blocks)
        });

        if let Some((group_suite, total_blocks)) = suite_data {
            if group_suite.blocks.is_empty() { return; }
            self.is_running = true;
            self.progress_current = 0;
            self.progress_total = group_suite.blocks.len();
            self.spinner_tick = 0;
            self.set_status("Running group...".to_string());

            let extra_vars: Vec<(String, String)> = self.env_vars.iter()
                .map(|(k, v)| (k.clone(), v.clone())).collect();
            let tx = self.runner_tx.clone();
            let handler = Arc::new(TuiProgress::new(tx.clone()));
            let indices = block_indices;

            tokio::spawn(async move {
                let group_results = request_pilot_core::test_runner::run_suite(
                    &group_suite, &extra_vars, Some(handler), None,
                ).await;

                let passed = group_results.passed;
                let failed = group_results.failed;
                let skipped = group_results.skipped;
                let total_time_ms = group_results.total_time_ms;
                let final_variables = group_results.final_variables;
                let result_blocks = group_results.block_results;

                let mut block_results: Vec<BlockResult> = (0..total_blocks).map(|_| BlockResult {
                    seq: None,
                    name: String::new(),
                    block_type: String::new(),
                    request_method: String::new(),
                    request_url: String::new(),
                    request_headers: Vec::new(),
                    request_body: None,
                    status: "pending".to_string(),
                    response: None,
                    assertion_results: Vec::new(),
                    extract_results: Vec::new(),
                    error: None,
                    time_ms: 0,
                }).collect();

                for (br, &orig_idx) in result_blocks.into_iter().zip(indices.iter()) {
                    if orig_idx < block_results.len() {
                        block_results[orig_idx] = br;
                    }
                }

                let full_results = TestRunResults {
                    passed, failed, skipped, total_time_ms, block_results, final_variables,
                    telemetry: None,
                };
                let _ = tx.send(RunnerMessage::SuiteComplete {
                    file_idx: fi, results: full_results,
                });
            });
        }
    }

    /// Main event loop.
    pub async fn run(&mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        let mut rx = self.runner_rx.take().expect("runner_rx already taken");

        loop {
            self.update_filtered_history_len();

            terminal.draw(|frame| ui::draw(frame, self))?;

            // Drain progress messages
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    RunnerMessage::BlockStart(progress) => {
                        self.set_status(format!(
                            "{} Running: {}...",
                            self.spinner_char(),
                            progress.name
                        ));
                    }
                    RunnerMessage::BlockComplete(progress) => {
                        self.progress_current += 1;
                        let icon = match progress.status.as_str() {
                            "passed" => "\u{2713}",
                            "failed" | "error" => "\u{2717}",
                            "skipped" => "\u{2298}",
                            _ => "\u{00b7}",
                        };
                        self.set_status(format!(
                            "{} {} ({}/{})",
                            icon, progress.name, self.progress_current, self.progress_total
                        ));
                    }
                    RunnerMessage::SuiteComplete { file_idx, results } => {
                        self.record_history(&results);
                        if let Some(file) = self.loaded_files.get_mut(file_idx) {
                            file.results = Some(results);
                        }
                        self.is_running = false;
                        self.progress_current = 0;
                        self.progress_total = 0;

                        if let Some(file) = self.loaded_files.get(file_idx) {
                            if let Some(ref r) = file.results {
                                self.set_status(format!(
                                    "\u{2713} {} passed  \u{2717} {} failed  \u{2298} {} skipped  \u{00b7} {}ms",
                                    r.passed, r.failed, r.skipped, r.total_time_ms
                                ));
                            }
                        }
                    }
                }
            }

            // Check for queued full run
            if self.run_queued && !self.is_running {
                self.run_queued = false;
                if let Some(fi) = self.active_file_idx {
                    self.spawn_suite_run(fi);
                }
            }

            // Check for queued single-block run
            if let Some((fi, bi)) = self.run_single_queued.take() {
                if !self.is_running {
                    self.spawn_single_block_run(fi, bi);
                }
            }

            // Check for queued group run
            if let Some((fi, indices)) = self.run_group_queued.take() {
                if !self.is_running {
                    self.spawn_group_run(fi, indices);
                }
            }

            // Update spinner tick
            if self.is_running {
                self.spinner_tick = self.spinner_tick.wrapping_add(1);
            }

            // Poll events with 100ms timeout
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    events::handle_key(self, key);
                }
            }

            // Expire status messages after 5s
            if let Some((_, instant)) = &self.status_message {
                if instant.elapsed() > std::time::Duration::from_secs(5) {
                    self.status_message = None;
                }
            }

            if self.should_quit { break; }
        }
        Ok(())
    }
}

use crate::events;
use crate::ui;
