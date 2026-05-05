use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use request_pilot_core::http_parser::{TestSuite, TestBlock, ParsedRequest, parse_test_suite, generate_http_content};
use request_pilot_core::test_runner::{BlockProgress, ProgressHandler, TestRunResults, BlockResult};
use request_pilot_core::history::{HistoryStore, HistoryEntry};
use request_pilot_core::sessions::{
    Component, FileIdentity, RecordInput, RecordedSession, SessionStore, SessionTrigger,
};
use request_pilot_core::sessions_config::{self, default_config_path, SessionsConfig};
use request_pilot_core::chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;
use request_pilot_core::azure_auth::AzureToken;
use request_pilot_core::telemetry::TelemetryStats;
use crate::components::diff_viewer::DiffViewerData;

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
    AddVarName,
    AddVarValue { name: String },
    EditVarValue { name: String },
    ExportHistory,
    SaveFile,
    // Assertion CRUD
    AssertLeft,
    AssertOperator { left: String },
    AssertRight { left: String, operator: String },
    EditAssertLeft { index: usize },
    EditAssertOperator { index: usize, left: String },
    EditAssertRight { index: usize, left: String, operator: String },
    // Extract CRUD
    ExtractVarName,
    ExtractSourcePath { variable_name: String },
    EditExtractVarName { index: usize },
    EditExtractSourcePath { index: usize, variable_name: String },
    // Metadata editing (inspector)
    EditBlockName,
    EditBlockDescription,
    EditBlockGroup,
    EditBlockDepends,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmPurpose {
    CloseFile { file_idx: usize },
    ClearHistory,
    DeleteVar { name: String },
    DeleteAssertion { index: usize },
    DeleteExtract { index: usize },
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
    Code,
    Logs,
    Sessions,
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
    Builder,
}

/// Sidebar sub-tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Files,
    Variables,
}

// --- History grouping/filter types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryGroupBy {
    Flat,
    Domain,
    Status,
    Source,
    FileGroupTest,
}

impl HistoryGroupBy {
    pub fn label(&self) -> &'static str {
        match self {
            HistoryGroupBy::Flat => "Flat",
            HistoryGroupBy::Domain => "Domain",
            HistoryGroupBy::Status => "Status",
            HistoryGroupBy::Source => "Source",
            HistoryGroupBy::FileGroupTest => "File/Group",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            HistoryGroupBy::Flat => HistoryGroupBy::Domain,
            HistoryGroupBy::Domain => HistoryGroupBy::Status,
            HistoryGroupBy::Status => HistoryGroupBy::Source,
            HistoryGroupBy::Source => HistoryGroupBy::FileGroupTest,
            HistoryGroupBy::FileGroupTest => HistoryGroupBy::Flat,
        }
    }
}

#[derive(Debug, Clone)]
pub enum HistoryPopup {
    MethodFilter { cursor: usize },
    StatusFilter { cursor: usize },
}

// --- Builder types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuilderFocus {
    Method,
    Url,
    Params,
    Headers,
    Body,
    Assertions,
    Extracts,
}

// --- Logs types ---

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFilter {
    All,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AzureAuthState {
    Off,
    Authenticated,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderEditField {
    Key,
    Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HeaderEditMode {
    Browse,
    Adding { field: HeaderEditField, key_buf: String, val_buf: String },
    Editing { index: usize, field: HeaderEditField, key_buf: String, val_buf: String },
}

/// Context captured at run-spawn time for best-effort auto-record into the
/// Sessions store. Carries everything that can't be recovered later from
/// `App` state alone (e.g. the exact start timestamp and the variables that
/// were merged into the run).
#[derive(Debug, Clone)]
pub struct AutoRecordContext {
    pub started_at: DateTime<Utc>,
    pub run_mode: Option<String>,
    pub env_file: Option<String>,
    pub variables: Vec<(String, String)>,
    /// When `Some`, overrides the `file_id` derived from the active file's
    /// path. Set by snapshot-replay flows so the new run is recorded under
    /// the original session's `file_id` (grouping with the original in the
    /// Sessions tab) rather than a new id derived from the buffer.
    pub file_id_override: Option<String>,
}

/// Best-effort auto-record helper. Returns `Ok(Some(_))` when a session was
/// written, `Ok(None)` when auto-record is disabled or no root is configured,
/// and `Err(_)` only on actual store errors. Callers should treat any error
/// as non-fatal — the run has already produced its results.
///
/// Reused by every TUI run-completion path so a `run.json` lands under the
/// configured `sessions.root` whenever the user has opted in.
pub fn auto_record_run(
    config: &SessionsConfig,
    file_path: Option<&Path>,
    source_content: &str,
    results: &TestRunResults,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    run_mode: Option<&str>,
    env_file: Option<&str>,
    variables: Vec<(String, String)>,
    file_id_override: Option<&str>,
) -> Result<Option<RecordedSession>, String> {
    if !config.auto_record {
        return Ok(None);
    }
    let Some(root) = config.root.as_ref() else {
        return Ok(None);
    };
    let store = SessionStore::open(PathBuf::from(root))
        .map_err(|e| format!("open sessions store at {}: {}", root, e))?;
    let identity = FileIdentity::from_path_and_alias(
        file_path.and_then(|p| p.to_str()),
        None,
        "tui-buffer",
    );
    let parsed_suite = parse_test_suite(source_content);
    let (suite_redact_rules, block_redact_rules) =
        request_pilot_core::sessions::build_body_redact_rules(
            &parsed_suite,
            &config.body_redaction_paths,
        );
    let input = RecordInput {
        identity,
        source_path: file_path,
        source_content,
        policy: config.capture_policy.clone(),
        trigger: SessionTrigger::Manual,
        component: Component::Tui,
        started_at,
        finished_at,
        mode: run_mode.map(|s| s.to_string()),
        env_file: env_file.map(|s| s.to_string()),
        env_identity: None,
        variables,
        results: results.clone(),
        file_id_override: file_id_override.map(|s| s.to_string()),
        redact_body_rules: suite_redact_rules,
        block_redact_body_rules: block_redact_rules,
    };
    store
        .record_run(input)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Messages sent from the test runner to the event loop.
pub enum RunnerMessage {
    BlockStart(BlockProgress),
    BlockComplete(BlockProgress),
    SuiteComplete {
        file_idx: usize,
        file_id: u64,
        results: TestRunResults,
        record_ctx: Option<AutoRecordContext>,
    },
    AzureAuthResult(Result<request_pilot_core::azure_auth::AzureToken, String>),
    AzureCliCheck(bool),
    LiveCaptureRequest(crate::live_capture::CapturedRequest),
    LiveCaptureStatus { connected: bool },
}

/// Metadata describing a session snapshot the user has loaded into the
/// Files tab. Mirrors the desktop app's `snapshot` field on a loaded file.
#[derive(Debug, Clone)]
pub struct SnapshotMeta {
    pub file_id: String,
    pub sha256: String,
    pub run_id: String,
    pub recorded_at: DateTime<Utc>,
}

/// Which subset of a snapshot the user is about to replay. Drives the
/// confirmation modal text and the spawn path selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayKind {
    /// Re-run every block in the snapshot's suite.
    All,
    /// Re-run a single block, identified by name.
    Block(String),
}

/// State for the "Replay this snapshot?" confirmation modal. Captured at
/// the moment the user pressed `r` so the modal can show stable values
/// even if the cursor moves while the modal is open.
#[derive(Debug, Clone)]
pub struct ReplayConfirm {
    pub kind: ReplayKind,
    pub file_idx: usize,
    pub file_name: String,
    pub mode: Option<String>,
    pub env_file: Option<String>,
    pub first_host: Option<String>,
    pub snapshot_meta: SnapshotMeta,
}

/// State for the "Detach snapshot?" confirmation modal. Captured at the
/// moment the user pressed `D` so the modal can show stable values even
/// if the active file selection moves while the modal is open.
#[derive(Debug, Clone)]
pub struct DetachConfirm {
    pub file_idx: usize,
    pub file_name: String,
}

/// A loaded .http file with parsed suite and optional results.
pub struct LoadedFile {
    pub id: u64,
    pub path: Option<PathBuf>,
    pub name: String,
    pub content: String,
    pub suite: TestSuite,
    pub results: Option<TestRunResults>,
    pub expanded: bool,
    pub group_expanded: HashMap<String, bool>,
    /// When `Some`, this file was reconstituted from a recorded session
    /// (via `SessionStore::load_as_snapshot`) and should be treated as
    /// read-only. Mirrors desktop's `snapshot?` field.
    pub snapshot: Option<SnapshotMeta>,
    /// When true, edits/saves should be rejected. Set together with
    /// `snapshot` for snapshot-loaded files.
    pub locked: bool,
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
    /// Persisted list of loaded .env files and currently-active index. Kept
    /// in sync with `<config_dir>/request-pilot/env_config.json`.
    pub env_config: request_pilot_core::env_config::EnvConfig,
    /// Env picker overlay state.
    pub env_picker_open: bool,
    pub env_picker_cursor: usize,
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
    pub help_scroll: u16,
    pub run_progress_lines: Vec<(String, String)>,
    pub run_start_time: Option<std::time::Instant>,

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
    pub progress_scroll: usize,
    pub progress_expanded: bool,

    // History mode state
    pub history_cursor: usize,
    pub filter_text: String,
    pub filtered_history_len: usize,

    // Toolbar: Extra Headers
    pub extra_headers: Vec<(String, String, bool)>,
    pub extra_headers_open: bool,
    pub extra_headers_cursor: usize,
    pub extra_headers_edit_mode: HeaderEditMode,

    // Toolbar: Azure Auth
    pub azure_state: AzureAuthState,
    pub azure_token: Option<AzureToken>,
    pub azure_popup_open: bool,
    pub azure_loading: bool,
    pub az_cli_available: Option<bool>,
    pub dev_mode: bool,

    // Toolbar: OTEL
    pub otel_enabled: bool,
    pub otel_stats: Option<TelemetryStats>,
    pub otel_popup_open: bool,

    // Toolbar: Live Capture (Extension Connector)
    pub live_capture_mode: String,
    pub live_capture_connected: bool,
    pub live_capture_popup_open: bool,
    pub live_capture_count: u64,
    pub live_capture_state: Option<std::sync::Arc<crate::live_capture::LiveCaptureState>>,
    pub live_capture_file_idx: Option<usize>,

    // JSON tree view state (response panel)
    pub json_tree_nodes: Vec<crate::components::response::JsonTreeNode>,
    pub json_expanded: HashSet<String>,
    pub json_cursor: usize,
    pub body_fully_loaded: bool,
    pub response_body_full: Option<String>,

    // Code editor state
    pub code_editor_content: String,
    pub code_editor_cursor_line: usize,
    pub code_editor_cursor_col: usize,
    pub code_editor_scroll: u16,
    pub code_editor_modified: bool,
    pub code_editor_editing: bool,

    // History enhanced state
    pub history_group_by: HistoryGroupBy,
    pub history_method_filter: Option<String>,
    pub history_status_filter: Option<String>,
    pub history_popup: Option<HistoryPopup>,
    pub history_groups_collapsed: HashSet<String>,
    pub history_selected_seqs: Vec<u64>,
    pub history_detail_scroll: u16,
    pub history_detail_tab: u8, // 0 = Request, 1 = Response
    pub history_json_nodes: Vec<crate::components::response::JsonTreeNode>,
    pub history_json_cursor: usize,
    pub history_json_built: bool, // true when tree was built for current body
    pub history_json_expanded: HashSet<String>,
    pub history_detail_content_height: u16, // visible content area height set by renderer

    // Diff viewer state
    pub diff_viewer_open: bool,
    pub diff_viewer_data: Option<DiffViewerData>,

    // Inspector popup state
    pub inspect_open: bool,
    pub inspect_scroll: u16,

    // Variable detail popup state
    pub var_detail_open: bool,
    pub var_detail_scroll: u16,

    // Builder state
    pub builder_focus: BuilderFocus,
    pub builder_url_cursor: usize,
    pub builder_body_scroll: u16,
    pub builder_header_cursor: usize,
    pub builder_assert_cursor: usize,
    pub builder_extract_cursor: usize,

    // Logs state
    pub log_entries: Vec<LogEntry>,
    pub log_scroll: usize,
    pub log_filter: LogFilter,
    pub log_auto_scroll: bool,

    // File autocomplete state
    pub autocomplete_suggestions: Vec<String>,
    pub autocomplete_idx: Option<usize>,

    // Code editor search state
    pub editor_search_query: String,
    pub editor_search_active: bool,
    pub editor_search_matches: Vec<(usize, usize)>,
    pub editor_search_idx: usize,
    pub editor_goto_active: bool,
    pub editor_goto_buffer: String,

    // Selection state for code editor (line, col)
    pub editor_selection_anchor: Option<(usize, usize)>,

    // Auto-run state
    pub auto_run_interval: Option<String>,
    pub auto_run_next_due: Option<std::time::Instant>,
    pub auto_run_popup_open: bool,
    pub auto_run_popup_cursor: usize,

    // Variable autocomplete state
    pub var_ac_open: bool,
    pub var_ac_cursor: usize,
    pub var_ac_prefix: String,
    pub var_ac_filtered: Vec<(String, String)>, // (name, value)

    // Channel receiver (set up in run())
    runner_rx: Option<mpsc::UnboundedReceiver<RunnerMessage>>,
    runner_tx: mpsc::UnboundedSender<RunnerMessage>,
    next_file_id: u64,

    // Sessions tab state (shared config with desktop app).
    pub sessions_tab: crate::sessions_tab::SessionsTabState,

    /// When `Some`, a snapshot replay confirmation modal is open. All input
    /// is routed to the modal until the user confirms (`y`/`Y`/Enter) or
    /// cancels (`n`/`N`/Esc).
    pub pending_replay: Option<ReplayConfirm>,

    /// When `Some`, a "Detach snapshot?" confirmation modal is open. All
    /// input is routed to the modal until the user confirms (`y`/`Y`/Enter)
    /// or cancels (`n`/`N`/Esc).
    pub pending_detach: Option<DetachConfirm>,
}

impl App {
    pub fn runner_tx(&self) -> mpsc::UnboundedSender<RunnerMessage> {
        self.runner_tx.clone()
    }

    /// Build extra variables for test runner, including Azure tokens when in dev mode.
    /// Scans @mode app blocks for @dev_auth scopes and @extract variables,
    /// then maps tokens to the correct extract variable names.
    fn build_extra_vars(&self) -> Vec<(String, String)> {
        let vars: Vec<(String, String)> = self.env_vars.iter()
            .map(|(k, v)| (k.clone(), v.clone())).collect();
        vars
    }

    /// Returns the names of `@prompt` variables in the given file that are not
    /// yet resolved (no value in suite.variables or in env_vars). Used to warn
    /// the user before spawning a run so they can supply values via the
    /// variables editor (`V` key).
    fn unresolved_prompts(&self, fi: usize) -> Vec<String> {
        let Some(file) = self.loaded_files.get(fi) else { return Vec::new(); };
        file.suite.prompts.iter().filter_map(|p| {
            let in_vars = file.suite.variables.iter().any(|(k, v)| k == &p.name && !v.is_empty());
            let in_env = self.env_vars.iter().any(|(k, v)| k == &p.name && !v.is_empty());
            if in_vars || in_env { None } else { Some(p.name.clone()) }
        }).collect()
    }

    /// Collect dev_auth token mappings from the active file's @mode app blocks.
    /// Returns Vec<(scope, extract_var_name)> for each token that needs fetching.
    fn collect_dev_auth_mappings(&self) -> Vec<(String, Vec<String>)> {
        let mut mappings: Vec<(String, Vec<String>)> = Vec::new();
        if let Some(fi) = self.active_file_idx {
            if let Some(file) = self.loaded_files.get(fi) {
                for block in &file.suite.blocks {
                    if block.mode.as_deref() == Some("app") {
                        if let Some(ref scope) = block.dev_auth {
                            let var_names: Vec<String> = block.extracts.iter()
                                .map(|e| e.variable_name.clone())
                                .collect();
                            if !var_names.is_empty() {
                                mappings.push((scope.clone(), var_names));
                            }
                        }
                    }
                }
            }
        }
        mappings
    }

    /// Get run_mode string for test runner based on dev_mode flag.
    fn run_mode(&self) -> Option<&str> {
        if self.dev_mode { Some("dev") } else { None }
    }
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let env_config = request_pilot_core::env_config::load().unwrap_or_default();
        // If a previously-active .env exists, load its values up front so the
        // user's first render already reflects the active env.
        let mut env_vars: HashMap<String, String> = HashMap::new();
        if let Some(entry) = env_config.active_entry() {
            if let Ok((_n, vars)) =
                request_pilot_core::env_file::read_env_named_from_path(&entry.path)
            {
                env_vars = vars;
            }
        }
        Self {
            mode: Mode::Files,
            focus: Focus::FileTree,
            sidebar_tab: SidebarTab::Files,
            loaded_files: Vec::new(),
            env_vars,
            env_path: None,
            env_config,
            env_picker_open: false,
            env_picker_cursor: 0,
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
            help_scroll: 0,
            run_progress_lines: Vec::new(),
            run_start_time: None,
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
            progress_scroll: 0,
            progress_expanded: false,
            history_cursor: 0,
            filter_text: String::new(),
            filtered_history_len: 0,
            extra_headers: Vec::new(),
            extra_headers_open: false,
            extra_headers_cursor: 0,
            extra_headers_edit_mode: HeaderEditMode::Browse,
            azure_state: AzureAuthState::Off,
            azure_token: None,
            azure_popup_open: false,
            azure_loading: false,
            az_cli_available: None,
            dev_mode: false,
            otel_enabled: false,
            otel_stats: None,
            otel_popup_open: false,
            live_capture_mode: "off".to_string(),
            live_capture_connected: false,
            live_capture_popup_open: false,
            live_capture_count: 0,
            live_capture_state: None,
            live_capture_file_idx: None,
            json_tree_nodes: Vec::new(),
            json_expanded: HashSet::new(),
            json_cursor: 0,
            body_fully_loaded: false,
            response_body_full: None,
            code_editor_content: String::new(),
            code_editor_cursor_line: 0,
            code_editor_cursor_col: 0,
            code_editor_scroll: 0,
            code_editor_modified: false,
            code_editor_editing: false,
            history_group_by: HistoryGroupBy::Flat,
            history_method_filter: None,
            history_status_filter: None,
            history_popup: None,
            history_groups_collapsed: HashSet::new(),
            history_selected_seqs: Vec::new(),
            history_detail_scroll: 0,
            history_detail_tab: 0,
            history_json_nodes: Vec::new(),
            history_json_cursor: 0,
            history_json_built: false,
            history_json_expanded: HashSet::new(),
            history_detail_content_height: 0,
            diff_viewer_open: false,
            diff_viewer_data: None,
            inspect_open: false,
            inspect_scroll: 0,
            var_detail_open: false,
            var_detail_scroll: 0,
            builder_focus: BuilderFocus::Method,
            builder_url_cursor: 0,
            builder_body_scroll: 0,
            builder_header_cursor: 0,
            builder_assert_cursor: 0,
            builder_extract_cursor: 0,
            log_entries: Vec::new(),
            log_scroll: 0,
            log_filter: LogFilter::All,
            log_auto_scroll: true,
            autocomplete_suggestions: Vec::new(),
            autocomplete_idx: None,
            editor_search_query: String::new(),
            editor_search_active: false,
            editor_search_matches: Vec::new(),
            editor_search_idx: 0,
            editor_goto_active: false,
            editor_goto_buffer: String::new(),
            editor_selection_anchor: None,
            auto_run_interval: None,
            auto_run_next_due: None,
            auto_run_popup_open: false,
            auto_run_popup_cursor: 0,
            var_ac_open: false,
            var_ac_cursor: 0,
            var_ac_prefix: String::new(),
            var_ac_filtered: Vec::new(),
            runner_rx: Some(rx),
            runner_tx: tx,
            next_file_id: 0,
            sessions_tab: crate::sessions_tab::SessionsTabState::from_disk(),
            pending_replay: None,
            pending_detach: None,
        }
    }

    pub fn load_env(&mut self, path: &Path) -> color_eyre::Result<()> {
        let path = if path.is_relative() {
            std::env::current_dir()?.join(path)
        } else {
            path.to_path_buf()
        };
        let path_str = path.to_string_lossy().to_string();
        let (directive_name, vars) =
            request_pilot_core::env_file::read_env_named_from_path(&path_str)
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        // Derive display name: directive > filename stem > "env".
        let name = directive_name
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "env".to_string());
        let was_empty = self.env_config.entries.is_empty();
        let idx = self
            .env_config
            .add(request_pilot_core::env_config::EnvEntry {
                path: path_str.clone(),
                name: name.clone(),
            });
        let _ = request_pilot_core::env_config::save(&self.env_config);
        // An explicit "load .env" is a direct user action — always merge its
        // values into the runtime env_vars map. The persisted active_index
        // only controls which entry auto-applies at startup.
        self.apply_env_vars(&vars);
        // First-ever entry auto-activates (so it sticks across restarts).
        let _ = was_empty;
        let _ = idx;
        self.env_path = Some(path.clone());
        self.set_status(format!("Loaded env '{}': {}", name, path.display()));
        Ok(())
    }

    /// Merge resolved env file vars into the runtime `env_vars` map. Existing
    /// runtime edits are preserved for keys the env file doesn't define.
    fn apply_env_vars(&mut self, vars: &HashMap<String, String>) {
        for (k, v) in vars {
            self.env_vars.insert(k.clone(), v.clone());
        }
    }

    /// Activate (or deactivate with `None`) an entry in the persisted env list
    /// by index. Re-reads the file and merges its vars into `env_vars`.
    pub fn activate_env_index(&mut self, index: Option<usize>) {
        self.env_config.set_active(index);
        let _ = request_pilot_core::env_config::save(&self.env_config);
        if let Some(entry) = self.env_config.active_entry().cloned() {
            match request_pilot_core::env_file::read_env_named_from_path(&entry.path) {
                Ok((_n, vars)) => {
                    self.apply_env_vars(&vars);
                    self.set_status(format!("Activated env: {}", entry.name));
                }
                Err(e) => self.set_status(format!("env read error: {}", e)),
            }
        } else {
            self.set_status("No env active".to_string());
        }
    }

    /// Remove an entry from the persisted env list.
    pub fn remove_env_index(&mut self, index: usize) {
        self.env_config.remove(index);
        let _ = request_pilot_core::env_config::save(&self.env_config);
        // Re-apply whatever is active now (may be None).
        let active = self.env_config.active_entry().cloned();
        if let Some(entry) = active {
            if let Ok((_n, vars)) =
                request_pilot_core::env_file::read_env_named_from_path(&entry.path)
            {
                self.apply_env_vars(&vars);
            }
        }
    }

    pub fn load_file(&mut self, path: &Path) -> color_eyre::Result<()> {
        let path = if path.is_relative() {
            std::env::current_dir()?.join(path)
        } else {
            path.to_path_buf()
        };
        let path = path.as_path();
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

        let file_id = self.next_file_id;
        self.next_file_id += 1;
        self.loaded_files.push(LoadedFile {
            id: file_id,
            path: Some(path.to_path_buf()),
            name,
            content,
            suite,
            results: None,
            expanded: true,
            group_expanded: HashMap::new(),
            snapshot: None,
            locked: false,
        });

        self.rebuild_tree();
        if self.active_file_idx.is_none() {
            self.active_file_idx = Some(0);
            self.active_block_idx = Some(0);
        }

        // Initialize auto-run from file directive if present
        if let Some(ref interval) = self.loaded_files.last().unwrap().suite.auto_run {
            if self.auto_run_interval.is_none() {
                self.auto_run_interval = Some(interval.clone());
                if let Some(secs) = request_pilot_core::duration::parse_duration_secs(interval) {
                    self.auto_run_next_due = Some(std::time::Instant::now() + std::time::Duration::from_secs(secs));
                }
            }
        }

        self.set_status(format!("Loaded: {}", path.display()));
        Ok(())
    }

    pub fn queue_run_all(&mut self) {
        if !self.is_running {
            self.run_queued = true;
            // Reset auto-run timer to avoid immediate re-trigger after manual run
            if let Some(ref interval) = self.auto_run_interval {
                if let Some(secs) = request_pilot_core::duration::parse_duration_secs(interval) {
                    self.auto_run_next_due = Some(std::time::Instant::now() + std::time::Duration::from_secs(secs));
                }
            }
        }
    }

    pub fn queue_run_single(&mut self, file_idx: usize, block_idx: usize) {
        if !self.is_running {
            self.run_single_queued = Some((file_idx, block_idx));
        }
    }

    /// Run the block the cursor is currently on in the code editor.
    /// If the buffer has unsaved changes, parses into memory first (no disk
    /// write). If parsing fails or no block maps to the cursor, status is
    /// updated and no run is queued.
    pub fn run_code_block_at_cursor(&mut self) {
        let fi = match self.active_file_idx {
            Some(i) => i,
            None => {
                self.set_status("No file loaded".into());
                return;
            }
        };

        // If modified, flush parsed content into the in-memory suite so the
        // block index we compute matches what we'll actually run. We don't
        // write to disk — this is a "run" action, not a "save" action.
        if self.code_editor_modified {
            let content = self.code_editor_content.clone();
            let suite = parse_test_suite(&content);
            if let Some(file) = self.loaded_files.get_mut(fi) {
                file.content = content;
                file.suite = suite;
            }
            self.rebuild_tree();
        }

        let file = match self.loaded_files.get(fi) {
            Some(f) => f,
            None => {
                self.set_status("No file loaded".into());
                return;
            }
        };

        if file.suite.blocks.is_empty() {
            self.set_status("No runnable blocks in file".into());
            return;
        }

        let starts = request_pilot_core::http_parser::block_start_lines(&self.code_editor_content);
        if starts.is_empty() {
            self.set_status("No runnable blocks in file".into());
            return;
        }

        let cursor_line = self.code_editor_cursor_line;
        let mut block_idx = 0usize;
        for (i, &start) in starts.iter().enumerate() {
            if start <= cursor_line {
                block_idx = i;
            } else {
                break;
            }
        }

        if block_idx >= file.suite.blocks.len() {
            self.set_status("Block mapping mismatch — try saving first".into());
            return;
        }

        self.queue_run_single(fi, block_idx);
    }

    pub fn queue_run_group(&mut self, file_idx: usize, block_indices: Vec<usize>) {
        if !self.is_running && !block_indices.is_empty() {
            self.run_group_queued = Some((file_idx, block_indices));
        }
    }

    pub fn set_status(&mut self, msg: String) {
        self.status_message = Some((msg, std::time::Instant::now()));
    }

    /// Returns a reference to the currently active `LoadedFile`, if any.
    pub fn active_loaded_file(&self) -> Option<&LoadedFile> {
        self.active_file_idx.and_then(|fi| self.loaded_files.get(fi))
    }

    /// True when the active file was loaded from a snapshot and is locked
    /// against edits. Convenience helper for the editor key handler and UI.
    pub fn active_file_locked(&self) -> bool {
        self.active_loaded_file().map(|f| f.locked).unwrap_or(false)
    }

    /// Load a recorded session as a read-only snapshot into `loaded_files`
    /// and switch to the Files mode. The new entry has `snapshot: Some(..)`
    /// and `locked: true`. Returns an error string on failure (no store
    /// configured, missing run, parse error, etc.) and leaves state
    /// unchanged in that case.
    pub fn load_snapshot_from_session(
        &mut self,
        file_id: &str,
        sha256: &str,
        run_id: &str,
        display_name: &str,
    ) -> Result<(), String> {
        let store = self
            .sessions_tab
            .store
            .as_ref()
            .ok_or_else(|| "No sessions store configured".to_string())?;
        let (source, suite, record) = store
            .load_as_snapshot(file_id, sha256, run_id)
            .map_err(|e| e.to_string())?;

        let short_sha = &sha256[..sha256.len().min(8)];
        let base = if display_name.is_empty() {
            file_id
        } else {
            display_name
        };
        let name = format!("{} @ {}", base, short_sha);

        let new_id = self.next_file_id;
        self.next_file_id += 1;
        let results = record.results.clone();
        self.loaded_files.push(LoadedFile {
            id: new_id,
            path: None,
            name,
            content: source,
            suite,
            results: Some(results),
            expanded: true,
            group_expanded: HashMap::new(),
            snapshot: Some(SnapshotMeta {
                file_id: file_id.to_string(),
                sha256: sha256.to_string(),
                run_id: run_id.to_string(),
                recorded_at: record.started_at,
            }),
            locked: true,
        });
        self.rebuild_tree();
        let new_idx = self.loaded_files.len() - 1;
        self.active_file_idx = Some(new_idx);
        self.active_block_idx = if self.loaded_files[new_idx].suite.blocks.is_empty() {
            None
        } else {
            Some(0)
        };
        self.mode = Mode::Files;
        self.focus = Focus::FileTree;
        Ok(())
    }

    /// Set or clear auto-run interval. Updates timer accordingly.
    pub fn set_auto_run(&mut self, interval: Option<String>) {
        self.auto_run_interval = interval.clone();
        if let Some(ref iv) = interval {
            if let Some(secs) = request_pilot_core::duration::parse_duration_secs(iv) {
                self.auto_run_next_due = Some(std::time::Instant::now() + std::time::Duration::from_secs(secs));
                self.set_status(format!("Auto-run: every {}", iv));
            } else {
                self.auto_run_next_due = None;
                self.auto_run_interval = None;
                self.set_status("Auto-run: invalid interval".to_string());
            }
        } else {
            self.auto_run_next_due = None;
            self.set_status("Auto-run: off".to_string());
        }
    }

    /// Recompute auto-run from loaded files. Uses first file with `auto_run` directive.
    /// Clears auto-run if no loaded file declares it (unless user set it via UI popup).
    fn recompute_auto_run(&mut self) {
        // Check if any remaining file has auto_run
        let file_interval = self.loaded_files.iter()
            .find_map(|f| f.suite.auto_run.clone());
        if file_interval.is_some() {
            if self.auto_run_interval != file_interval {
                self.set_auto_run(file_interval);
            }
        } else {
            // No file declares auto_run — clear it
            self.auto_run_interval = None;
            self.auto_run_next_due = None;
        }
    }

    /// Reset scroll positions when switching blocks.
    pub fn select_block(&mut self, file_idx: usize, block_idx: usize) {
        self.active_file_idx = Some(file_idx);
        self.active_block_idx = Some(block_idx);
        self.code_scroll = 0;
        self.response_scroll = 0;
        self.json_cursor = 0;
        self.body_fully_loaded = false;
        self.response_body_full = None;
        self.json_expanded.clear();
        crate::components::response::try_rebuild_json_tree(self);
    }

    /// Record block results into history.
    fn record_history(&mut self, file_idx: usize, results: &TestRunResults) {
        let file_name = self.loaded_files.get(file_idx).map(|f| f.name.clone());
        let run_id = request_pilot_core::uuid::Uuid::new_v4().to_string();
        for br in &results.block_results {
            // For compare blocks, record each step as a separate history entry
            if !br.step_results.is_empty() {
                for step in &br.step_results {
                    if let Some(ref resp) = step.response {
                        let seq = self.history.next_seq();
                        let entry = HistoryEntry {
                            seq,
                            id: request_pilot_core::uuid::Uuid::new_v4().to_string(),
                            run_id: Some(run_id.clone()),
                            source: "tui".to_string(),
                            file_name: file_name.clone(),
                            group: br.group.clone(),
                            block_name: Some(br.name.clone()),
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
                            timestamp: request_pilot_core::chrono::Utc::now()
                                .to_rfc3339(),
                            result_status: br.status.clone(),
                        };
                        self.history.add(entry);
                    }
                }
            } else if let Some(ref resp) = br.response {
                let seq = self.history.next_seq();
                let entry = HistoryEntry {
                    seq,
                    id: request_pilot_core::uuid::Uuid::new_v4().to_string(),
                    run_id: Some(run_id.clone()),
                    source: "tui".to_string(),
                    file_name: file_name.clone(),
                    group: br.group.clone(),
                    block_name: Some(br.name.clone()),
                    compare_step: None,
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
                    result_status: br.status.clone(),
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

    fn build_history_json_tree(&mut self) {
        use request_pilot_core::history::HistoryFilter;
        self.history_json_built = true;
        self.history_json_nodes.clear();

        let idx = match self.history_detail_idx {
            Some(i) => i,
            None => return,
        };
        let filter = if self.filter_text.is_empty() {
            HistoryFilter::default()
        } else {
            HistoryFilter {
                url_contains: Some(self.filter_text.clone()),
                ..Default::default()
            }
        };
        let mut filtered = self.history.filter(&filter);
        filtered.reverse();
        let body = match filtered.get(idx).and_then(|e| e.response_body.as_ref()) {
            Some(b) if !b.is_empty() => b.clone(),
            _ => return,
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            crate::components::response::build_json_tree(
                &value,
                "root",
                "root",
                0,
                &self.history_json_expanded,
                &mut self.history_json_nodes,
            );
        }
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
        let file_id = self.next_file_id;
        self.next_file_id += 1;
        self.loaded_files.push(LoadedFile {
            id: file_id,
            path: None,
            name: "untitled.http".to_string(),
            content: template.to_string(),
            suite,
            results: None,
            expanded: true,
            group_expanded: HashMap::new(),
            snapshot: None,
            locked: false,
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

    /// Append a new empty test block to the current file (creating a scratch
    /// file if none is loaded) and focus it in the builder so the user can
    /// start typing immediately.
    pub fn new_test_block(&mut self) {
        if self.active_file_idx.is_none() || self.loaded_files.is_empty() {
            // Create a scratch file with no blocks so the new test is the first.
            let file_id = self.next_file_id;
            self.next_file_id += 1;
            self.loaded_files.push(LoadedFile {
                id: file_id,
                path: None,
                name: "untitled.http".to_string(),
                content: String::new(),
                suite: TestSuite::default(),
                results: None,
                expanded: true,
                group_expanded: HashMap::new(),
                snapshot: None,
                locked: false,
            });
            self.active_file_idx = Some(self.loaded_files.len() - 1);
        }

        let fi = match self.active_file_idx {
            Some(i) => i,
            None => return,
        };
        let blank = TestBlock {
            block_type: "test".to_string(),
            name: String::new(),
            description: String::new(),
            disabled: false,
            mode: None,
            dev_auth: None,
            group: None,
            depends: Vec::new(),
            request: ParsedRequest {
                name: None,
                method: "GET".to_string(),
                url: String::new(),
                headers: Vec::new(),
                body: None,
            },
            assertions: Vec::new(),
            extracts: Vec::new(),
            compare: false,
            steps: Vec::new(),
            diff: None,
            diffs: Vec::new(),
            errors: Default::default(),
            request_id_header: None,
            request_id_disabled: false,
            redact_body_rules: Vec::new(),
        };
        if let Some(file) = self.loaded_files.get_mut(fi) {
            file.suite.blocks.push(blank);
            self.active_block_idx = Some(file.suite.blocks.len() - 1);
        }
        self.builder_focus = BuilderFocus::Url;
        self.flush_builder_to_file();
        self.rebuild_tree();
        self.set_status("New test block — type URL to begin".to_string());
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
                    self.input_mode = InputMode::Input {
                        prompt: "Save as: ".to_string(),
                        purpose: InputPurpose::SaveFile,
                        buffer: String::new(),
                    };
                }
            }
        }
    }

    /// Regenerate file.content from file.suite after builder edits.
    /// Clears stale results and marks code editor as modified.
    pub fn flush_builder_to_file(&mut self) {
        if let Some(fi) = self.active_file_idx {
            if let Some(file) = self.loaded_files.get_mut(fi) {
                let content = generate_http_content(&file.suite);
                file.content = content.clone();
                file.results = None;
                self.code_editor_content = content;
                self.code_editor_modified = true;
            }
        }
    }

    /// Live sync from the code editor buffer into the in-memory suite.
    /// Parses silently — does NOT write to disk, does NOT touch the edit
    /// cursor, and does NOT clear the modified flag. Intended to be called
    /// after every code-editor mutation so that switching to builder mode
    /// reflects the latest edits without requiring a save. On parse failure,
    /// the previous suite is kept (errors surface on save / Ctrl+Enter).
    pub fn flush_code_to_builder(&mut self) {
        if let Some(fi) = self.active_file_idx {
            let content = self.code_editor_content.clone();
            let suite = parse_test_suite(&content);
            if let Some(file) = self.loaded_files.get_mut(fi) {
                file.content = content;
                file.suite = suite;
                file.results = None;
            }
            self.rebuild_tree();
        }
    }

    /// Collect all variable names and values for autocomplete.
    pub fn collect_all_vars(&self) -> Vec<(String, String)> {
        let mut vars: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        // Suite variables from all loaded files
        for file in &self.loaded_files {
            for (k, v) in &file.suite.variables {
                vars.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        // Env vars override
        for (k, v) in &self.env_vars {
            vars.insert(k.clone(), v.clone());
        }
        // Built-ins
        vars.insert("$timestamp".into(), "(Unix timestamp)".into());
        vars.insert("$uuid".into(), "(Random UUID v4)".into());
        vars.insert("$randomInt".into(), "(Random 0–9999)".into());
        vars.into_iter().collect()
    }

    /// Open variable autocomplete with current prefix for filtering.
    pub fn open_var_autocomplete(&mut self, prefix: &str) {
        self.var_ac_prefix = prefix.to_string();
        let all = self.collect_all_vars();
        let q = prefix.to_lowercase();
        self.var_ac_filtered = if q.is_empty() {
            all
        } else {
            all.into_iter().filter(|(n, _)| n.to_lowercase().contains(&q)).collect()
        };
        self.var_ac_cursor = 0;
        self.var_ac_open = !self.var_ac_filtered.is_empty();
    }

    /// Update autocomplete filter with new prefix.
    pub fn update_var_autocomplete(&mut self, prefix: &str) {
        self.var_ac_prefix = prefix.to_string();
        let all = self.collect_all_vars();
        let q = prefix.to_lowercase();
        self.var_ac_filtered = if q.is_empty() {
            all
        } else {
            all.into_iter().filter(|(n, _)| n.to_lowercase().contains(&q)).collect()
        };
        if self.var_ac_filtered.is_empty() {
            self.var_ac_open = false;
        }
        self.var_ac_cursor = self.var_ac_cursor.min(self.var_ac_filtered.len().saturating_sub(1));
    }

    /// Close variable autocomplete.
    pub fn close_var_autocomplete(&mut self) {
        self.var_ac_open = false;
        self.var_ac_prefix.clear();
        self.var_ac_filtered.clear();
    }

    pub fn stop_live_capture(&mut self) {
        if let Some(ref state) = self.live_capture_state {
            let state = state.clone();
            tokio::spawn(async move {
                crate::live_capture::stop_server(&state).await;
            });
            self.live_capture_state = None;
            self.live_capture_connected = false;
        }
    }

    pub fn cycle_live_capture_mode(&mut self) {
        let new_mode = match self.live_capture_mode.as_str() {
            "off" => "all",
            "all" => "filtered",
            "filtered" => "off",
            _ => "off",
        };
        self.live_capture_mode = new_mode.to_string();

        if new_mode == "off" {
            self.stop_live_capture();
            self.set_status("Extension connector: off".to_string());
        } else {
            if self.live_capture_state.is_none() {
                // Set initial mode before starting server so newly connected
                // clients receive the correct mode immediately
                self.start_live_capture_with_mode(new_mode);
            } else {
                // Server already running, just update mode
                if let Some(ref state) = self.live_capture_state {
                    let state = state.clone();
                    let mode = new_mode.to_string();
                    tokio::spawn(async move {
                        crate::live_capture::set_mode(&state, &mode).await;
                    });
                }
            }
            // Ensure we have a capture file
            self.ensure_capture_file();
            let label = if new_mode == "all" { "all requests" } else { "filtered requests" };
            self.set_status(format!("Extension connector: capturing {}", label));
        }
    }

    fn start_live_capture_with_mode(&mut self, mode: &str) {
        if self.live_capture_state.is_some() { return; }
        let state = std::sync::Arc::new(crate::live_capture::LiveCaptureState::new_with_mode(mode));
        self.live_capture_state = Some(state.clone());
        let tx = self.runner_tx();
        tokio::spawn(async move {
            if let Err(e) = crate::live_capture::start_server(state, tx).await {
                log::warn!("Live capture server error: {}", e);
            }
        });
    }

    fn ensure_capture_file(&mut self) {
        // If we already have a capture file that exists, reuse it
        if let Some(idx) = self.live_capture_file_idx {
            if idx < self.loaded_files.len() && self.loaded_files[idx].name == "📡 live-capture.http" {
                return;
            }
        }
        // Create a new virtual capture file
        let template = "# Live Capture — requests from browser extension\n\n";
        let suite = parse_test_suite(template);
        let file_id = self.next_file_id;
        self.next_file_id += 1;
        self.loaded_files.push(LoadedFile {
            id: file_id,
            path: None,
            name: "📡 live-capture.http".to_string(),
            content: template.to_string(),
            suite,
            results: None,
            expanded: true,
            group_expanded: HashMap::new(),
            snapshot: None,
            locked: false,
        });
        let idx = self.loaded_files.len() - 1;
        self.live_capture_file_idx = Some(idx);
        self.active_file_idx = Some(idx);
        self.active_block_idx = None;
        self.rebuild_tree();
    }

    pub fn append_captured_request(&mut self, req: &crate::live_capture::CapturedRequest) {
        self.ensure_capture_file();
        let idx = match self.live_capture_file_idx {
            Some(i) if i < self.loaded_files.len() => i,
            _ => return,
        };

        // Record in history
        let seq = self.history.next_seq();
        let req_headers: Vec<(String, String)> = req.request_headers.iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect();
        let resp_headers: Vec<(String, String)> = req.response_headers.iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect();
        let entry = HistoryEntry {
            seq,
            id: request_pilot_core::uuid::Uuid::new_v4().to_string(),
            run_id: None,
            source: "live-capture".to_string(),
            file_name: Some("📡 live-capture.http".to_string()),
            group: None,
            block_name: None,
            compare_step: None,
            method: req.method.clone(),
            url: req.url.clone(),
            request_headers: req_headers,
            request_body: req.request_body.clone(),
            status: req.status_code.unwrap_or(0),
            response_headers: resp_headers,
            response_body: req.response_body.clone(),
            response_time_ms: req.duration.unwrap_or(0),
            response_size_bytes: req.response_body.as_ref().map(|b| b.len()).unwrap_or(0),
            timestamp: request_pilot_core::chrono::Utc::now().to_rfc3339(),
            result_status: String::new(),
        };
        self.history.add(entry);

        // Build .http block from captured request (same logic as desktop)
        let label = if let Some(pos) = req.url.find("://") {
            let after = &req.url[pos + 3..];
            let host_path = after.split('?').next().unwrap_or(after);
            format!("{} {}", req.method, host_path)
        } else {
            format!("{} request", req.method)
        };

        let mut block = format!("###\n@name {}\n{} {}\n", label, req.method, req.url);

        // Filter out browser-internal and sensitive headers
        let skip_prefixes = [":", "sec-ch-", "sec-fetch-"];
        let skip_names: HashSet<&str> = [
            "host", "connection", "accept-encoding", "accept-language",
            "upgrade-insecure-requests", "priority", "pragma", "cache-control",
            "user-agent", "dnt", "origin", "referer",
            "authorization", "cookie", "set-cookie", "proxy-authorization",
            "x-api-key", "x-auth-token",
        ].into_iter().collect();

        for h in &req.request_headers {
            let lower = h.name.to_lowercase();
            if skip_names.contains(lower.as_str()) { continue; }
            if skip_prefixes.iter().any(|p| lower.starts_with(p)) { continue; }
            block.push_str(&format!("{}: {}\n", h.name, h.value));
        }

        if let Some(ref body) = req.request_body {
            if !body.is_empty() {
                block.push_str(&format!("\n{}\n", body));
            }
        }
        block.push('\n');

        // Append to file content and re-parse
        {
            let file = &mut self.loaded_files[idx];
            file.content.push_str(&block);
            file.suite = parse_test_suite(&file.content);
        }
        self.rebuild_tree();

        // Select the newly added block
        let block_count = self.loaded_files[idx].suite.blocks.len();
        if block_count > 0 {
            self.active_file_idx = Some(idx);
            self.active_block_idx = Some(block_count - 1);
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
            // Recompute auto-run from remaining files
            self.recompute_auto_run();
            self.set_status("File closed".to_string());
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

    pub fn clear_all(&mut self) {
        self.history.clear();
        self.history_cursor = 0;
        for f in &mut self.loaded_files {
            f.results = None;
        }
        self.progress_current = 0;
        self.progress_total = 0;
        self.set_status("Cleared all results and history".to_string());
    }

    pub fn enter_code_editor(&mut self) {
        if let Some(fi) = self.active_file_idx {
            if let Some(file) = self.loaded_files.get(fi) {
                self.code_editor_content = file.content.clone();
                self.code_editor_cursor_line = 0;
                self.code_editor_cursor_col = 0;
                self.code_editor_scroll = 0;
                self.code_editor_modified = false;
                self.code_editor_editing = false;
                self.mode = Mode::Code;
            }
        }
    }

    /// Enter code editor and jump cursor to the line where block_idx starts.
    pub fn enter_code_editor_at_block(&mut self, block_idx: usize) {
        if let Some(fi) = self.active_file_idx {
            if let Some(file) = self.loaded_files.get(fi) {
                self.code_editor_content = file.content.clone();
                self.code_editor_modified = false;
                self.code_editor_editing = false;
                self.mode = Mode::Code;

                // Find the line number for this block by counting ### separators
                let target_line = self.find_block_start_line(block_idx);
                let total_lines = self.code_editor_content.split('\n').count();
                self.code_editor_cursor_line = target_line.min(total_lines.saturating_sub(1));
                self.code_editor_cursor_col = 0;
                // Center the block in view
                self.code_editor_scroll = self.code_editor_cursor_line.saturating_sub(5) as u16;
            }
        }
    }

    /// Find the starting line number of the Nth block (0-indexed).
    fn find_block_start_line(&self, block_idx: usize) -> usize {
        let mut sep_count = 0;
        for (i, line) in self.code_editor_content.split('\n').enumerate() {
            if line.trim_start().starts_with("###") {
                if sep_count == block_idx {
                    return i;
                }
                sep_count += 1;
            }
        }
        0
    }

    pub fn save_code_editor(&mut self) {
        if let Some(fi) = self.active_file_idx {
            let content = self.code_editor_content.clone();
            let suite = parse_test_suite(&content);
            if let Some(file) = self.loaded_files.get_mut(fi) {
                file.content = content.clone();
                file.suite = suite;
                if let Some(ref path) = file.path {
                    let display = path.display().to_string();
                    match std::fs::write(path, &content) {
                        Ok(_) => self.set_status(format!("Saved: {}", display)),
                        Err(e) => {
                            self.set_status(format!("Save failed: {}", e));
                            return;
                        }
                    }
                } else {
                    self.set_status("Updated in memory (no file path)".to_string());
                }
            }
            self.code_editor_modified = false;
            self.rebuild_tree();
        }
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

    pub fn compute_file_completions(&mut self, partial: &str) {
        use std::path::Path;
        let path = Path::new(partial);
        let (dir, prefix) = if partial.ends_with('/') || partial.ends_with('\\') {
            (path.to_path_buf(), "")
        } else {
            (
                path.parent().unwrap_or(Path::new(".")).to_path_buf(),
                path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            )
        };

        self.autocomplete_suggestions.clear();
        self.autocomplete_idx = None;

        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.to_lowercase().starts_with(&prefix.to_lowercase()) {
                    let full = if dir == Path::new(".") {
                        name.clone()
                    } else {
                        dir.join(&name).to_string_lossy().to_string()
                    };
                    let display = if entry.path().is_dir() {
                        format!("{}/", full)
                    } else {
                        full
                    };
                    if entry.path().is_dir()
                        || name.ends_with(".http")
                        || name.ends_with(".env")
                    {
                        self.autocomplete_suggestions.push(display);
                    }
                }
            }
        }
        self.autocomplete_suggestions.sort();
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
            InputPurpose::SaveFile => {
                let path = PathBuf::from(&value);
                if let Some(fi) = self.active_file_idx {
                    let content = generate_http_content(&self.loaded_files[fi].suite);
                    let display = path.display().to_string();
                    match std::fs::write(&path, &content) {
                        Ok(_) => {
                            self.loaded_files[fi].path = Some(path);
                            self.loaded_files[fi].content = content;
                            self.set_status(format!("Saved: {}", display));
                        }
                        Err(e) => self.set_status(format!("Save failed: {}", e)),
                    }
                }
            }
            // --- Assertion CRUD ---
            InputPurpose::AssertLeft => {
                if !value.is_empty() {
                    self.input_mode = InputMode::Input {
                        prompt: "Operator (==, !=, >, <, >=, <=, contains): ".to_string(),
                        purpose: InputPurpose::AssertOperator { left: value },
                        buffer: String::new(),
                    };
                    return;
                }
            }
            InputPurpose::AssertOperator { left } => {
                if !value.is_empty() {
                    self.input_mode = InputMode::Input {
                        prompt: "Right operand: ".to_string(),
                        purpose: InputPurpose::AssertRight { left, operator: value },
                        buffer: String::new(),
                    };
                    return;
                }
            }
            InputPurpose::AssertRight { left, operator } => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        blk.assertions.push(request_pilot_core::http_parser::Assertion {
                            left,
                            operator,
                            right: value,
                        });
                        self.builder_assert_cursor = blk.assertions.len().saturating_sub(1);
                        self.flush_builder_to_file();
                        self.set_status("Assertion added".to_string());
                    }
                }
            }
            InputPurpose::EditAssertLeft { index } => {
                if !value.is_empty() {
                    // Pre-fill operator from existing assertion
                    let existing_op = self.active_file_idx.and_then(|fi| {
                        self.active_block_idx.and_then(|bi| {
                            self.loaded_files.get(fi)
                                .and_then(|f| f.suite.blocks.get(bi))
                                .and_then(|b| b.assertions.get(index))
                                .map(|a| a.operator.clone())
                        })
                    }).unwrap_or_default();
                    self.input_mode = InputMode::Input {
                        prompt: "Operator (==, !=, >, <, >=, <=, contains): ".to_string(),
                        purpose: InputPurpose::EditAssertOperator { index, left: value },
                        buffer: existing_op,
                    };
                    return;
                }
            }
            InputPurpose::EditAssertOperator { index, left } => {
                if !value.is_empty() {
                    // Pre-fill right operand from existing assertion
                    let existing_right = self.active_file_idx.and_then(|fi| {
                        self.active_block_idx.and_then(|bi| {
                            self.loaded_files.get(fi)
                                .and_then(|f| f.suite.blocks.get(bi))
                                .and_then(|b| b.assertions.get(index))
                                .map(|a| a.right.clone())
                        })
                    }).unwrap_or_default();
                    self.input_mode = InputMode::Input {
                        prompt: "Right operand: ".to_string(),
                        purpose: InputPurpose::EditAssertRight { index, left, operator: value },
                        buffer: existing_right,
                    };
                    return;
                }
            }
            InputPurpose::EditAssertRight { index, left, operator } => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        if let Some(a) = blk.assertions.get_mut(index) {
                            a.left = left;
                            a.operator = operator;
                            a.right = value;
                            self.flush_builder_to_file();
                            self.set_status("Assertion updated".to_string());
                        }
                    }
                }
            }
            // --- Extract CRUD ---
            InputPurpose::ExtractVarName => {
                if !value.is_empty() {
                    self.input_mode = InputMode::Input {
                        prompt: "Source path (e.g. $.data.id): ".to_string(),
                        purpose: InputPurpose::ExtractSourcePath { variable_name: value },
                        buffer: String::new(),
                    };
                    return;
                }
            }
            InputPurpose::ExtractSourcePath { variable_name } => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        blk.extracts.push(request_pilot_core::http_parser::Extract {
                            variable_name,
                            source_path: value,
                        });
                        self.builder_extract_cursor = blk.extracts.len().saturating_sub(1);
                        self.flush_builder_to_file();
                        self.set_status("Extract added".to_string());
                    }
                }
            }
            InputPurpose::EditExtractVarName { index } => {
                if !value.is_empty() {
                    // Pre-fill source path from existing extract
                    let existing_path = self.active_file_idx.and_then(|fi| {
                        self.active_block_idx.and_then(|bi| {
                            self.loaded_files.get(fi)
                                .and_then(|f| f.suite.blocks.get(bi))
                                .and_then(|b| b.extracts.get(index))
                                .map(|e| e.source_path.clone())
                        })
                    }).unwrap_or_default();
                    self.input_mode = InputMode::Input {
                        prompt: "Source path (e.g. $.data.id): ".to_string(),
                        purpose: InputPurpose::EditExtractSourcePath { index, variable_name: value },
                        buffer: existing_path,
                    };
                    return;
                }
            }
            InputPurpose::EditExtractSourcePath { index, variable_name } => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        if let Some(e) = blk.extracts.get_mut(index) {
                            e.variable_name = variable_name;
                            e.source_path = value;
                            self.flush_builder_to_file();
                            self.set_status("Extract updated".to_string());
                        }
                    }
                }
            }
            // --- Metadata editing ---
            InputPurpose::EditBlockName => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        blk.name = value;
                        self.flush_builder_to_file();
                        self.set_status("Block name updated".to_string());
                    }
                }
            }
            InputPurpose::EditBlockDescription => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        blk.description = value;
                        self.flush_builder_to_file();
                        self.set_status("Description updated".to_string());
                    }
                }
            }
            InputPurpose::EditBlockGroup => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        blk.group = if value.is_empty() { None } else { Some(value) };
                        self.flush_builder_to_file();
                        self.set_status("Group updated".to_string());
                    }
                }
            }
            InputPurpose::EditBlockDepends => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        blk.depends = value.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        self.flush_builder_to_file();
                        self.set_status("Dependencies updated".to_string());
                    }
                }
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
            ConfirmPurpose::DeleteAssertion { index } => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        if index < blk.assertions.len() {
                            blk.assertions.remove(index);
                            if self.builder_assert_cursor >= blk.assertions.len() && !blk.assertions.is_empty() {
                                self.builder_assert_cursor = blk.assertions.len() - 1;
                            }
                            self.flush_builder_to_file();
                            self.set_status("Assertion deleted".to_string());
                        }
                    }
                }
            }
            ConfirmPurpose::DeleteExtract { index } => {
                if let (Some(fi), Some(bi)) = (self.active_file_idx, self.active_block_idx) {
                    if let Some(blk) = self.loaded_files[fi].suite.blocks.get_mut(bi) {
                        if index < blk.extracts.len() {
                            blk.extracts.remove(index);
                            if self.builder_extract_cursor >= blk.extracts.len() && !blk.extracts.is_empty() {
                                self.builder_extract_cursor = blk.extracts.len() - 1;
                            }
                            self.flush_builder_to_file();
                            self.set_status("Extract deleted".to_string());
                        }
                    }
                }
            }
        }
    }

    // --- Run helpers ---

    fn spawn_suite_run(&mut self, fi: usize) {
        let missing = self.unresolved_prompts(fi);
        if !missing.is_empty() {
            self.set_status(format!(
                "⚠ @prompt vars unresolved: {} — press V to set values, then re-run",
                missing.join(", ")
            ));
            return;
        }
        self.is_running = true;
        self.progress_current = 0;
        self.progress_total = self.loaded_files[fi].suite.blocks.len();
        self.spinner_tick = 0;
        self.run_progress_lines.clear();
        self.progress_scroll = 0;
        self.progress_expanded = false;
        self.run_start_time = Some(std::time::Instant::now());

        let fid = self.loaded_files[fi].id;
        let mut suite = self.loaded_files[fi].suite.clone();
        let otel_on = self.otel_enabled;
        // When OTEL is disabled, strip telemetry variables so runner skips init
        if !otel_on {
            suite.variables.retain(|(k, _)| !k.starts_with("telemetry_"));
        }
        let mut extra_vars = self.build_extra_vars();
        if otel_on {
            let file_name = self.loaded_files[fi].name.clone();
            extra_vars.push(("__telemetry_file".to_string(), file_name));
        }
        let run_mode = self.run_mode().map(|s| s.to_string());
        let env_file = self
            .env_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());
        let dev_auth_mappings = if self.dev_mode { self.collect_dev_auth_mappings() } else { Vec::new() };
        let tx = self.runner_tx.clone();
        let handler = Arc::new(TuiProgress::new(tx.clone()));
        let started_at = Utc::now();
        let run_mode_for_ctx = run_mode.clone();
        let file_id_override_for_ctx = self.loaded_files[fi]
            .snapshot
            .as_ref()
            .map(|s| s.file_id.clone());

        tokio::spawn(async move {
            // In dev mode, fetch tokens for each @dev_auth scope and inject as extra vars
            if !dev_auth_mappings.is_empty() {
                for (scope, var_names) in &dev_auth_mappings {
                    // Strip .default suffix for az CLI --resource flag
                    let resource = scope.trim_end_matches("/.default");
                    let token_result = tokio::task::spawn_blocking({
                        let resource = resource.to_string();
                        move || request_pilot_core::azure_auth::fetch_token(&resource)
                    }).await;
                    if let Ok(Ok(token)) = token_result {
                        for var in var_names {
                            extra_vars.push((var.clone(), token.access_token.clone()));
                        }
                    }
                }
            }
            let results = request_pilot_core::test_runner::run_suite(
                &suite, &extra_vars, Some(handler), run_mode.as_deref(),
            ).await;
            let record_ctx = Some(AutoRecordContext {
                started_at,
                run_mode: run_mode_for_ctx,
                env_file,
                variables: extra_vars,
                file_id_override: file_id_override_for_ctx,
            });
            let _ = tx.send(RunnerMessage::SuiteComplete {
                file_idx: fi,
                file_id: fid,
                results,
                record_ctx,
            });
        });
        self.set_status("Running all tests...".to_string());
    }

    fn spawn_single_block_run(&mut self, fi: usize, bi: usize) {
        let missing = self.unresolved_prompts(fi);
        if !missing.is_empty() {
            self.set_status(format!(
                "⚠ @prompt vars unresolved: {} — press V to set values, then re-run",
                missing.join(", ")
            ));
            return;
        }
        let otel_on = self.otel_enabled;
        let suite_data = self.loaded_files.get(fi).and_then(|file| {
            file.suite.blocks.get(bi).map(|block| {
                let mut vars = file.suite.variables.clone();
                if !otel_on {
                    vars.retain(|(k, _)| !k.starts_with("telemetry_"));
                }
                let single_suite = TestSuite {
                    variables: vars,
                    blocks: vec![block.clone()],
                    ..Default::default()
                };
                let total_blocks = file.suite.blocks.len();
                (single_suite, block.name.clone(), total_blocks, file.id, file.name.clone())
            })
        });

        if let Some((single_suite, block_name, total_blocks, fid, file_name)) = suite_data {
            self.is_running = true;
            self.progress_current = 0;
            self.progress_total = 1;
            self.spinner_tick = 0;
            self.run_progress_lines.clear();
            self.progress_scroll = 0;
            self.progress_expanded = false;
            self.run_start_time = Some(std::time::Instant::now());
            self.set_status(format!("Running: {}...", block_name));

            let mut extra_vars = self.build_extra_vars();
            if otel_on {
                extra_vars.push(("__telemetry_file".to_string(), file_name));
            }
            let run_mode = self.run_mode().map(|s| s.to_string());
            let env_file = self
                .env_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            let dev_auth_mappings = if self.dev_mode { self.collect_dev_auth_mappings() } else { Vec::new() };
            let tx = self.runner_tx.clone();
            let handler = Arc::new(TuiProgress::new(tx.clone()));
            let started_at = Utc::now();
            let run_mode_for_ctx = run_mode.clone();
            let file_id_override_for_ctx = self.loaded_files[fi]
                .snapshot
                .as_ref()
                .map(|s| s.file_id.clone());

            tokio::spawn(async move {
                if !dev_auth_mappings.is_empty() {
                    for (scope, var_names) in &dev_auth_mappings {
                        let resource = scope.trim_end_matches("/.default");
                        let token_result = tokio::task::spawn_blocking({
                            let resource = resource.to_string();
                            move || request_pilot_core::azure_auth::fetch_token(&resource)
                        }).await;
                        if let Ok(Ok(token)) = token_result {
                            for var in var_names {
                                extra_vars.push((var.clone(), token.access_token.clone()));
                            }
                        }
                    }
                }
                let single_results = request_pilot_core::test_runner::run_suite(
                    &single_suite, &extra_vars, Some(handler), run_mode.as_deref(),
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
                        group: None,
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
                        step_results: Vec::new(),
                        diff_result: None,
                    diff_results: Vec::new(),
                    });
                }
                if let Some(single_br) = single_results.block_results.into_iter().next() {
                    if bi < full_results.block_results.len() {
                        full_results.block_results[bi] = single_br;
                    }
                }
                let record_ctx = Some(AutoRecordContext {
                    started_at,
                    run_mode: run_mode_for_ctx,
                    env_file,
                    variables: extra_vars,
                    file_id_override: file_id_override_for_ctx,
                });
                let _ = tx.send(RunnerMessage::SuiteComplete {
                    file_idx: fi, file_id: fid, results: full_results, record_ctx,
                });
            });
        }
    }

    /// Compute the first request host across the suite of `loaded_files[fi]`,
    /// for display in the replay confirmation modal. Returns `None` if no
    /// block carries a parseable URL.
    fn first_host_for_file(&self, fi: usize) -> Option<String> {
        let file = self.loaded_files.get(fi)?;
        for block in &file.suite.blocks {
            let url = block.request.url.as_str();
            if url.is_empty() {
                continue;
            }
            // Take the chunk between "://" and the next '/'. Good enough for
            // a one-line modal hint.
            if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
                let host = rest.split('/').next().unwrap_or("");
                if !host.is_empty() {
                    return Some(host.to_string());
                }
            }
        }
        None
    }

    /// Build a `ReplayConfirm` for a single-block replay of the snapshot at
    /// `file_idx`. Returns `None` if the file is not a snapshot or `block_idx`
    /// is out of range.
    pub fn build_block_replay_confirm(
        &self,
        file_idx: usize,
        block_idx: usize,
    ) -> Option<ReplayConfirm> {
        let file = self.loaded_files.get(file_idx)?;
        let snapshot = file.snapshot.clone()?;
        let block = file.suite.blocks.get(block_idx)?;
        Some(ReplayConfirm {
            kind: ReplayKind::Block(block.name.clone()),
            file_idx,
            file_name: file.name.clone(),
            mode: self.run_mode().map(|s| s.to_string()),
            env_file: self
                .env_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            first_host: self.first_host_for_file(file_idx),
            snapshot_meta: snapshot,
        })
    }

    /// Spawn a single-block run triggered by a snapshot replay confirmation.
    /// Locates the named block in the snapshot's suite, then defers to the
    /// existing `spawn_single_block_run` path. The auto-record step in the
    /// `SuiteComplete` handler already pins the new session under the
    /// snapshot's `file_id` whenever the active file is a snapshot, so no
    /// extra plumbing is needed here.
    pub fn spawn_single_block_run_for_replay(&mut self, replay: ReplayConfirm) {
        let block_name = match &replay.kind {
            ReplayKind::Block(name) => name.clone(),
            ReplayKind::All => {
                self.set_status(
                    "Internal: spawn_single_block_run_for_replay called with ReplayKind::All"
                        .to_string(),
                );
                return;
            }
        };
        let fi = replay.file_idx;
        let Some(file) = self.loaded_files.get(fi) else {
            self.set_status("Replay aborted: file no longer loaded".to_string());
            return;
        };
        if file.snapshot.as_ref().map(|s| &s.file_id) != Some(&replay.snapshot_meta.file_id) {
            self.set_status("Replay aborted: snapshot mismatch".to_string());
            return;
        }
        let Some(bi) = file.suite.blocks.iter().position(|b| b.name == block_name) else {
            self.set_status(format!(
                "Replay aborted: block '{}' not found in snapshot",
                block_name
            ));
            return;
        };
        self.active_file_idx = Some(fi);
        self.active_block_idx = Some(bi);
        self.spawn_single_block_run(fi, bi);
    }

    /// Build a `ReplayConfirm` for a full-suite replay of the snapshot at
    /// `file_idx`. Returns `None` if the file is not a snapshot.
    pub fn build_all_replay_confirm(&self, file_idx: usize) -> Option<ReplayConfirm> {
        let file = self.loaded_files.get(file_idx)?;
        let snapshot = file.snapshot.clone()?;
        Some(ReplayConfirm {
            kind: ReplayKind::All,
            file_idx,
            file_name: file.name.clone(),
            mode: self.run_mode().map(|s| s.to_string()),
            env_file: self
                .env_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            first_host: self.first_host_for_file(file_idx),
            snapshot_meta: snapshot,
        })
    }

    /// Spawn a full-suite run triggered by a snapshot replay confirmation.
    /// Defers to the existing `spawn_suite_run` path. The auto-record step
    /// in the `SuiteComplete` handler already pins the new session under the
    /// snapshot's `file_id` whenever the active file is a snapshot, so the
    /// new run lands beside the original in the Sessions tab.
    pub fn spawn_suite_run_for_replay(&mut self, replay: ReplayConfirm) {
        if !matches!(replay.kind, ReplayKind::All) {
            self.set_status(
                "Internal: spawn_suite_run_for_replay called with non-All kind".to_string(),
            );
            return;
        }
        let fi = replay.file_idx;
        let Some(file) = self.loaded_files.get(fi) else {
            self.set_status("Replay aborted: file no longer loaded".to_string());
            return;
        };
        if file.snapshot.as_ref().map(|s| &s.file_id) != Some(&replay.snapshot_meta.file_id) {
            self.set_status("Replay aborted: snapshot mismatch".to_string());
            return;
        }
        self.active_file_idx = Some(fi);
        self.spawn_suite_run(fi);
    }

    /// Build a `DetachConfirm` for the snapshot at `file_idx`. Returns
    /// `None` if the file is not loaded or is not a snapshot.
    pub fn confirm_detach(&self, file_idx: usize) -> Option<DetachConfirm> {
        let file = self.loaded_files.get(file_idx)?;
        if !file.locked || file.snapshot.is_none() {
            return None;
        }
        Some(DetachConfirm {
            file_idx,
            file_name: file.name.clone(),
        })
    }

    /// Apply a pending detach: clear the snapshot marker and unlock the
    /// file so the user can edit and save it as a fresh buffer. The
    /// recorded session on disk is untouched and still visible in the
    /// Sessions tab.
    pub fn apply_detach(&mut self) {
        let Some(detach) = self.pending_detach.take() else { return; };
        if let Some(file) = self.loaded_files.get_mut(detach.file_idx) {
            file.snapshot = None;
            file.locked = false;
        }
        self.set_status(
            "Snapshot detached. Original session is still in Sessions tab.".to_string(),
        );
    }

    fn spawn_group_run(&mut self, fi: usize, block_indices: Vec<usize>) {
        let missing = self.unresolved_prompts(fi);
        if !missing.is_empty() {
            self.set_status(format!(
                "⚠ @prompt vars unresolved: {} — press V to set values, then re-run",
                missing.join(", ")
            ));
            return;
        }
        let otel_on = self.otel_enabled;
        let suite_data = self.loaded_files.get(fi).map(|file| {
            let blocks: Vec<_> = block_indices.iter()
                .filter_map(|&bi| file.suite.blocks.get(bi).cloned())
                .collect();
            let mut vars = file.suite.variables.clone();
            if !otel_on {
                vars.retain(|(k, _)| !k.starts_with("telemetry_"));
            }
            let group_suite = TestSuite {
                variables: vars,
                blocks,
                ..Default::default()
            };
            let total_blocks = file.suite.blocks.len();
            (group_suite, total_blocks, file.id, file.name.clone())
        });

        if let Some((group_suite, total_blocks, fid, file_name)) = suite_data {
            if group_suite.blocks.is_empty() { return; }
            self.is_running = true;
            self.progress_current = 0;
            self.progress_total = group_suite.blocks.len();
            self.spinner_tick = 0;
            self.run_progress_lines.clear();
            self.progress_scroll = 0;
            self.progress_expanded = false;
            self.run_start_time = Some(std::time::Instant::now());
            self.set_status("Running group...".to_string());

            let mut extra_vars = self.build_extra_vars();
            if otel_on {
                extra_vars.push(("__telemetry_file".to_string(), file_name));
            }
            let run_mode = self.run_mode().map(|s| s.to_string());
            let env_file = self
                .env_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            let dev_auth_mappings = if self.dev_mode { self.collect_dev_auth_mappings() } else { Vec::new() };
            let tx = self.runner_tx.clone();
            let handler = Arc::new(TuiProgress::new(tx.clone()));
            let indices = block_indices;
            let started_at = Utc::now();
            let run_mode_for_ctx = run_mode.clone();
            let file_id_override_for_ctx = self.loaded_files[fi]
                .snapshot
                .as_ref()
                .map(|s| s.file_id.clone());

            tokio::spawn(async move {
                if !dev_auth_mappings.is_empty() {
                    for (scope, var_names) in &dev_auth_mappings {
                        let resource = scope.trim_end_matches("/.default");
                        let token_result = tokio::task::spawn_blocking({
                            let resource = resource.to_string();
                            move || request_pilot_core::azure_auth::fetch_token(&resource)
                        }).await;
                        if let Ok(Ok(token)) = token_result {
                            for var in var_names {
                                extra_vars.push((var.clone(), token.access_token.clone()));
                            }
                        }
                    }
                }
                let group_results = request_pilot_core::test_runner::run_suite(
                    &group_suite, &extra_vars, Some(handler), run_mode.as_deref(),
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
                    group: None,
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
                    step_results: Vec::new(),
                    diff_result: None,
                    diff_results: Vec::new(),
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
                let record_ctx = Some(AutoRecordContext {
                    started_at,
                    run_mode: run_mode_for_ctx,
                    env_file,
                    variables: extra_vars,
                    file_id_override: file_id_override_for_ctx,
                });
                let _ = tx.send(RunnerMessage::SuiteComplete {
                    file_idx: fi, file_id: fid, results: full_results, record_ctx,
                });
            });
        }
    }

    /// Main event loop.
    pub async fn run(&mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        let mut rx = self.runner_rx.take().expect("runner_rx already taken");

        loop {
            self.update_filtered_history_len();

            // Build history JSON tree if needed (lazy — only when response tab shown)
            if self.history_detail_idx.is_some() && self.history_detail_tab == 1 && !self.history_json_built {
                self.build_history_json_tree();
            }

            // Update history detail content height from terminal size for auto-scroll
            if self.history_detail_idx.is_some() {
                let term_h = terminal.size()?.height;
                // popup_height = min(term_h - 4, 40), content = popup_height - header(4) - tabs(1) - border(1)
                let popup_h = (term_h.saturating_sub(4)).min(40);
                self.history_detail_content_height = popup_h.saturating_sub(6);
            }

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
                        self.run_progress_lines.push((icon.to_string(), progress.name.clone()));
                        self.set_status(format!(
                            "{} {} ({}/{})",
                            icon, progress.name, self.progress_current, self.progress_total
                        ));
                    }
                    RunnerMessage::SuiteComplete { file_idx, file_id, results, record_ctx } => {
                        let resolved_idx = if self.loaded_files.get(file_idx).map_or(false, |f| f.id == file_id) {
                            Some(file_idx)
                        } else {
                            self.loaded_files.iter().position(|f| f.id == file_id)
                        };

                        if let Some(idx) = resolved_idx {
                            self.record_history(idx, &results);
                            // Capture telemetry stats for OTEL popup
                            if let Some(ref stats) = results.telemetry {
                                self.otel_stats = Some(stats.clone());
                            }
                            self.loaded_files[idx].results = Some(results.clone());

                            if let Some(ref r) = self.loaded_files[idx].results {
                                self.set_status(format!(
                                    "\u{2713} {} passed  \u{2717} {} failed  \u{2298} {} skipped  \u{00b7} {}ms",
                                    r.passed, r.failed, r.skipped, r.total_time_ms
                                ));
                            }

                            // Best-effort auto-record into Sessions store. Never
                            // surfaces failures to the user (the run already
                            // produced its results).
                            if let Some(ctx) = record_ctx {
                                let cfg = sessions_config::load_from(&default_config_path())
                                    .unwrap_or_default();
                                let file = &self.loaded_files[idx];
                                let recorded = auto_record_run(
                                    &cfg,
                                    file.path.as_deref(),
                                    &file.content,
                                    &results,
                                    ctx.started_at,
                                    Utc::now(),
                                    ctx.run_mode.as_deref(),
                                    ctx.env_file.as_deref(),
                                    ctx.variables,
                                    ctx.file_id_override.as_deref(),
                                );
                                match recorded {
                                    Ok(Some(_)) => {
                                        self.sessions_tab.refresh();
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        log::warn!("[sessions] auto-record failed: {}", e);
                                    }
                                }
                            }
                        }
                        self.is_running = false;
                        self.progress_current = 0;
                        self.progress_total = 0;
                        self.run_start_time = None;

                        // Reset auto-run timer for next cycle
                        if let Some(ref interval) = self.auto_run_interval {
                            if let Some(secs) = request_pilot_core::duration::parse_duration_secs(interval) {
                                self.auto_run_next_due = Some(std::time::Instant::now() + std::time::Duration::from_secs(secs));
                            }
                        }
                    }
                    RunnerMessage::AzureAuthResult(result) => {
                        self.azure_loading = false;
                        match result {
                            Ok(token) => {
                                self.azure_state = AzureAuthState::Authenticated;
                                self.azure_token = Some(token);
                                self.dev_mode = true;
                                self.set_status("Azure: authenticated ✓ (dev mode ON)".to_string());
                            }
                            Err(e) => {
                                self.azure_state = AzureAuthState::Expired;
                                self.dev_mode = false;
                                self.set_status(format!("Azure auth failed: {}", e));
                            }
                        }
                    }
                    RunnerMessage::AzureCliCheck(available) => {
                        self.az_cli_available = Some(available);
                    }
                    RunnerMessage::LiveCaptureRequest(req) => {
                        self.live_capture_count += 1;
                        log::info!("Captured: {} {}", req.method, req.url);
                        let summary = format!("📡 {} {} (total: {})", req.method, req.url, self.live_capture_count);
                        self.append_captured_request(&req);
                        self.set_status(summary);
                    }
                    RunnerMessage::LiveCaptureStatus { connected } => {
                        self.live_capture_connected = connected;
                        if connected {
                            self.set_status("📡 Extension connected".to_string());
                        } else {
                            self.set_status("📡 Extension disconnected".to_string());
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

            // Auto-run timer check
            if let Some(due) = self.auto_run_next_due {
                if !self.is_running && std::time::Instant::now() >= due {
                    self.auto_run_next_due = None; // will be reset on SuiteComplete
                    self.queue_run_all();
                    self.set_status("Auto-run triggered".to_string());
                }
            }

            // Update spinner tick
            if self.is_running {
                self.spinner_tick = self.spinner_tick.wrapping_add(1);
            }

            // Poll events with 100ms timeout
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        events::handle_key(self, key);
                    }
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

#[cfg(test)]
mod auto_record_tests {
    use super::*;
    use request_pilot_core::http_client::HttpResponse;
    use request_pilot_core::assertions::AssertionResult;
    use request_pilot_core::test_runner::{BlockResult, ExtractResult};
    use request_pilot_core::sessions::CapturePolicy;
    use std::collections::HashMap;

    fn tmp_dir(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rp-tui-autorecord-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        p
    }

    fn dummy_results() -> TestRunResults {
        TestRunResults {
            passed: 1,
            failed: 0,
            skipped: 0,
            total_time_ms: 1,
            block_results: vec![BlockResult {
                seq: Some(1),
                name: "ping".to_string(),
                block_type: "test".to_string(),
                group: None,
                request_method: "GET".to_string(),
                request_url: "https://example.com/ping".to_string(),
                request_headers: vec![],
                request_body: None,
                status: "passed".to_string(),
                response: Some(HttpResponse {
                    status: 200,
                    status_text: "OK".to_string(),
                    headers: vec![],
                    body: "{\"ok\":true}".to_string(),
                    time_ms: 1,
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
                    value: Some("1".to_string()),
                    success: true,
                }],
                error: None,
                time_ms: 1,
                step_results: vec![],
                diff_result: None,
                    diff_results: Vec::new(),
            }],
            final_variables: HashMap::new(),
            telemetry: None,
        }
    }

    fn find_run_json(root: &Path) -> Option<PathBuf> {
        let files_dir = root.join("files");
        if !files_dir.is_dir() {
            return None;
        }
        for f_entry in std::fs::read_dir(&files_dir).ok()?.flatten() {
            let versions = f_entry.path().join("versions");
            for v_entry in std::fs::read_dir(&versions).ok()?.flatten() {
                let sessions = v_entry.path().join("sessions");
                for s_entry in std::fs::read_dir(&sessions).ok()?.flatten() {
                    let candidate = s_entry.path().join("run.json");
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    #[test]
    fn tui_auto_record_writes_run_when_enabled() {
        let root = tmp_dir("enabled");
        let cfg = SessionsConfig {
            root: Some(root.to_string_lossy().to_string()),
            auto_record: true,
            capture_policy: CapturePolicy::default(),
            capture_preset: "snapshot".to_string(),
            retention: None,
            body_redaction_paths: Vec::new(),
        };
        let now = Utc::now();
        let recorded = auto_record_run(
            &cfg,
            None,
            "GET https://example.com/ping\n",
            &dummy_results(),
            now,
            now,
            None,
            None,
            vec![],
            None,
        )
        .expect("auto_record_run should succeed");
        assert!(recorded.is_some(), "expected a recorded session");
        let run_json = find_run_json(&root)
            .expect("expected a run.json under <root>/files/<file_id>/versions/<sha>/sessions/run-*");
        assert!(run_json.is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tui_auto_record_disabled_skips() {
        let root = tmp_dir("disabled");
        let cfg = SessionsConfig {
            root: Some(root.to_string_lossy().to_string()),
            auto_record: false,
            capture_policy: CapturePolicy::default(),
            capture_preset: "snapshot".to_string(),
            retention: None,
            body_redaction_paths: Vec::new(),
        };
        let now = Utc::now();
        let recorded = auto_record_run(
            &cfg,
            None,
            "GET https://example.com/ping\n",
            &dummy_results(),
            now,
            now,
            None,
            None,
            vec![],
            None,
        )
        .expect("auto_record_run should succeed");
        assert!(recorded.is_none(), "expected no session when disabled");
        assert!(
            find_run_json(&root).is_none(),
            "no run.json should be written when auto_record=false"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tui_auto_record_no_root_skips() {
        let cfg = SessionsConfig {
            root: None,
            auto_record: true,
            capture_policy: CapturePolicy::default(),
            capture_preset: "snapshot".to_string(),
            retention: None,
            body_redaction_paths: Vec::new(),
        };
        let now = Utc::now();
        let recorded = auto_record_run(
            &cfg,
            None,
            "GET https://example.com/ping\n",
            &dummy_results(),
            now,
            now,
            None,
            None,
            vec![],
            None,
        )
        .expect("auto_record_run should not error when root is None");
        assert!(recorded.is_none());
    }
}
#[cfg(test)]
mod snapshot_load_tests {
    use super::*;
    use request_pilot_core::http_client::HttpResponse;
    use request_pilot_core::assertions::AssertionResult;
    use request_pilot_core::test_runner::{BlockResult, ExtractResult};
    use request_pilot_core::sessions::{
        CapturePolicy, Component, FileIdentity, RecordInput, SessionStore, SessionTrigger,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    fn tmp_root(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rp-tui-snapshot-load-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        p
    }

    fn dummy_results() -> TestRunResults {
        TestRunResults {
            passed: 1,
            failed: 0,
            skipped: 0,
            total_time_ms: 1,
            block_results: vec![BlockResult {
                seq: Some(1),
                name: "ping".to_string(),
                block_type: "test".to_string(),
                group: None,
                request_method: "GET".to_string(),
                request_url: "https://example.com/ping".to_string(),
                request_headers: vec![],
                request_body: None,
                status: "passed".to_string(),
                response: Some(HttpResponse {
                    status: 200,
                    status_text: "OK".to_string(),
                    headers: vec![],
                    body: "{\"ok\":true}".to_string(),
                    time_ms: 1,
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
                    value: Some("1".to_string()),
                    success: true,
                }],
                error: None,
                time_ms: 1,
                step_results: vec![],
                diff_result: None,
                    diff_results: Vec::new(),
            }],
            final_variables: HashMap::new(),
            telemetry: None,
        }
    }

    #[test]
    fn load_snapshot_populates_loaded_files() {
        let root = tmp_root("populates");
        let store = SessionStore::open(&root).unwrap();
        let now = Utc::now();
        let recorded = store
            .record_run(RecordInput {
                identity: FileIdentity::Alias("checkout".into()),
                source_path: None,
                source_content: "GET https://example.com/ping\n",
                policy: CapturePolicy::default(),
                trigger: SessionTrigger::Manual,
                component: Component::Cli,
                started_at: now,
                finished_at: now,
                mode: None,
                env_file: None,
                env_identity: None,
                variables: vec![],
                results: dummy_results(),
                file_id_override: None,
                redact_body_rules: Vec::new(),
                block_redact_body_rules: std::collections::HashMap::new(),
            })
            .expect("record_run");

        let mut app = App::new();
        app.sessions_tab.store = Some(Arc::new(store));
        let before = app.loaded_files.len();

        app.load_snapshot_from_session(
            &recorded.file_id,
            &recorded.sha256,
            &recorded.run_id,
            "checkout",
        )
        .expect("load_snapshot_from_session should succeed");

        assert_eq!(app.loaded_files.len(), before + 1);
        let last = app.loaded_files.last().unwrap();
        assert!(last.snapshot.is_some(), "snapshot meta should be set");
        assert!(last.locked, "snapshot-loaded files must be locked");
        let meta = last.snapshot.as_ref().unwrap();
        assert_eq!(meta.file_id, recorded.file_id);
        assert_eq!(meta.sha256, recorded.sha256);
        assert_eq!(meta.run_id, recorded.run_id);
        assert_eq!(app.mode, Mode::Files);
        assert_eq!(app.active_file_idx, Some(app.loaded_files.len() - 1));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_snapshot_missing_returns_err() {
        let root = tmp_root("missing");
        let store = SessionStore::open(&root).unwrap();

        let mut app = App::new();
        app.sessions_tab.store = Some(Arc::new(store));
        let before = app.loaded_files.len();
        let prev_mode = app.mode;

        let res = app.load_snapshot_from_session(
            "no-such-file",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "no-such-run",
            "fake",
        );
        assert!(res.is_err(), "expected Err for missing snapshot");
        assert_eq!(app.loaded_files.len(), before, "loaded_files unchanged");
        assert_eq!(app.mode, prev_mode, "mode unchanged on error");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_snapshot_without_store_returns_err() {
        let mut app = App::new();
        app.sessions_tab.store = None;
        let before = app.loaded_files.len();
        let res = app.load_snapshot_from_session("f", "abcdef", "r", "n");
        assert!(res.is_err());
        assert_eq!(app.loaded_files.len(), before);
    }
}

#[cfg(test)]
mod replay_block_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use request_pilot_core::assertions::AssertionResult;
    use request_pilot_core::http_client::HttpResponse;
    use request_pilot_core::sessions::{
        CapturePolicy, Component, FileIdentity, RecordInput, SessionStore, SessionTrigger,
    };
    use request_pilot_core::test_runner::{BlockResult, ExtractResult};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn tmp_root(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rp-tui-replay-block-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        p
    }

    fn block_result(name: &str, url: &str) -> BlockResult {
        BlockResult {
            seq: Some(1),
            name: name.to_string(),
            block_type: "test".to_string(),
            group: None,
            request_method: "GET".to_string(),
            request_url: url.to_string(),
            request_headers: vec![],
            request_body: None,
            status: "passed".to_string(),
            response: Some(HttpResponse {
                status: 200,
                status_text: "OK".to_string(),
                headers: vec![],
                body: "{}".to_string(),
                time_ms: 1,
                size_bytes: 2,
            }),
            assertion_results: vec![AssertionResult {
                assertion: "status == 200".to_string(),
                expected: Some("200".to_string()),
                actual: Some("200".to_string()),
                passed: true,
            }],
            extract_results: vec![ExtractResult {
                variable: "id".to_string(),
                value: Some("1".to_string()),
                success: true,
            }],
            error: None,
            time_ms: 1,
            step_results: vec![],
            diff_result: None,
                    diff_results: Vec::new(),
        }
    }

    fn two_block_results() -> TestRunResults {
        TestRunResults {
            passed: 2,
            failed: 0,
            skipped: 0,
            total_time_ms: 2,
            block_results: vec![
                block_result("first", "https://example.com/first"),
                block_result("second", "https://example.com/second"),
            ],
            final_variables: HashMap::new(),
            telemetry: None,
        }
    }

    /// End-to-end check for the snapshot block-replay confirmation flow:
    /// 1. Record an original 2-block session.
    /// 2. Load it as a snapshot (no path on disk).
    /// 3. Build a `ReplayConfirm` for just the first block.
    /// 4. Auto-record a synthetic single-block result with
    ///    `file_id_override = snapshot.file_id`, mirroring what
    ///    `spawn_single_block_run_for_replay` causes once the run completes.
    /// 5. Assert the new run.json lives under the *original* file_id and
    ///    contains exactly one block result.
    #[test]
    fn replay_block_confirm_records_single_block() {
        let root = tmp_root("records");
        let store = SessionStore::open(&root).unwrap();
        let now = Utc::now();
        let source = "# @name first\nGET https://example.com/first\n\n###\n\n# @name second\nGET https://example.com/second\n";
        let recorded = store
            .record_run(RecordInput {
                identity: FileIdentity::Alias("replay-target".into()),
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
                variables: vec![],
                results: two_block_results(),
                file_id_override: None,
                redact_body_rules: Vec::new(),
                block_redact_body_rules: std::collections::HashMap::new(),
            })
            .expect("record_run");

        // Step 2 — load as snapshot into a fresh App.
        let mut app = App::new();
        app.sessions_tab.store = Some(Arc::new(store));
        app.load_snapshot_from_session(
            &recorded.file_id,
            &recorded.sha256,
            &recorded.run_id,
            "replay-target",
        )
        .expect("load_snapshot_from_session");
        let fi = app.active_file_idx.expect("active file after snapshot load");
        assert_eq!(app.loaded_files[fi].suite.blocks.len(), 2);

        // Step 3 — build the per-block replay confirm.
        let confirm = app
            .build_block_replay_confirm(fi, 0)
            .expect("snapshot file with focused block must produce a confirm");
        assert!(matches!(confirm.kind, ReplayKind::Block(ref n) if n == "first"));
        assert_eq!(confirm.snapshot_meta.file_id, recorded.file_id);

        // Step 4 — simulate the post-run auto-record. The runtime path goes
        // through `SuiteComplete`, which forwards `file.snapshot.file_id` as
        // the override; we mirror that contract here.
        let cfg = SessionsConfig {
            root: Some(root.to_string_lossy().to_string()),
            auto_record: true,
            capture_policy: CapturePolicy::default(),
            capture_preset: "snapshot".to_string(),
            retention: None,
            body_redaction_paths: Vec::new(),
        };
        let single = TestRunResults {
            passed: 1,
            failed: 0,
            skipped: 0,
            total_time_ms: 1,
            block_results: vec![block_result("first", "https://example.com/first")],
            final_variables: HashMap::new(),
            telemetry: None,
        };
        let recorded_replay = auto_record_run(
            &cfg,
            None,
            source,
            &single,
            now,
            now,
            None,
            None,
            vec![],
            Some(confirm.snapshot_meta.file_id.as_str()),
        )
        .expect("auto_record_run")
        .expect("expected recorded session");

        // Step 5 — assertions on the new session.
        assert_eq!(
            recorded_replay.file_id, recorded.file_id,
            "replayed run must record under the snapshot's file_id"
        );
        assert_ne!(
            recorded_replay.run_id, recorded.run_id,
            "replay must mint a new run_id distinct from the original"
        );

        // Disk layout: <root>/files/<file_id>/versions/<sha>/sessions/<run_id>/run.json
        let run_json = root
            .join("files")
            .join(&recorded.file_id)
            .join("versions")
            .join(&recorded_replay.sha256)
            .join("sessions")
            .join(&recorded_replay.run_id)
            .join("run.json");
        assert!(
            run_json.is_file(),
            "expected replay run.json at {}",
            run_json.display()
        );

        // Cross-check the persisted block_results length: a single-block
        // replay must produce exactly one block result on disk.
        let body = std::fs::read_to_string(&run_json).expect("read run.json");
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("run.json is valid JSON");
        let blocks = parsed
            .get("results")
            .and_then(|r| r.get("block_results"))
            .and_then(|b| b.as_array())
            .expect("run.json has results.block_results array");
        assert_eq!(
            blocks.len(),
            1,
            "single-block replay should record exactly one block result"
        );
        assert_eq!(
            blocks[0].get("name").and_then(|n| n.as_str()),
            Some("first")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Lowercase `r` while focused on a non-Block tree node (e.g. the
    /// snapshot's file row) must NOT pop the replay-confirm modal. The key
    /// falls through to the normal "run all" path, which is harmless in a
    /// test (it only sets a queue flag).
    #[test]
    fn lowercase_r_without_focused_block_falls_through() {
        let root = tmp_root("nofocus");
        let store = SessionStore::open(&root).unwrap();
        let now = Utc::now();
        let source = "# @name first\nGET https://example.com/first\n\n###\n\n# @name second\nGET https://example.com/second\n";

        let recorded = store
            .record_run(RecordInput {
                identity: FileIdentity::Alias("replay-target".into()),
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
                variables: vec![],
                results: two_block_results(),
                file_id_override: None,
                redact_body_rules: Vec::new(),
                block_redact_body_rules: std::collections::HashMap::new(),
            })
            .expect("record_run");

        let mut app = App::new();
        app.sessions_tab.store = Some(Arc::new(store));
        app.load_snapshot_from_session(
            &recorded.file_id,
            &recorded.sha256,
            &recorded.run_id,
            "replay-target",
        )
        .expect("load_snapshot_from_session");

        // Place the cursor on the File node (index 0 in the rebuilt tree).
        // No block is focused.
        app.tree_cursor = 0;
        assert!(matches!(app.tree_nodes[0], TreeNode::File { .. }));

        // Press lowercase 'r'. Route through the real `handle_key` so we
        // exercise the same code path as the live event loop.
        crate::events::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        );

        assert!(
            app.pending_replay.is_none(),
            "no replay modal when focused node is not a Block"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
