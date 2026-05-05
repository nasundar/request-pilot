//! Sessions tab — browser for recorded test sessions.
//!
//! This module owns the TUI-side state for the Sessions feature, mirroring
//! the desktop app's `SessionsState`. Configuration is read from the same
//! `sessions_config.json` file desktop writes to, so the user only configures
//! the sessions root once.
//!
//! Layout: groups (left) | sessions (center) | detail (right).
#![allow(dead_code)]

use std::cell::RefCell;
use std::sync::Arc;

use request_pilot_core::sessions::{
    SessionRecord, SessionResult, SessionStore, SessionSummary, TrackedFile, VersionEntry,
};
use request_pilot_core::sessions_config::{self, default_config_path, SessionsConfig};

/// Cached row used by the Sessions list. One entry per (file, version, session).
#[derive(Clone)]
pub struct SessionRow {
    pub file: TrackedFile,
    pub version: VersionEntry,
    pub session: SessionSummary,
}

impl SessionRow {
    /// Returns "passed" / "failed" / "skipped" / "mixed" based on counts.
    pub fn status_label(&self) -> &'static str {
        let s = &self.session;
        if s.failed > 0 && (s.passed > 0 || s.skipped > 0) {
            "mixed"
        } else if s.failed > 0 {
            "failed"
        } else if s.skipped > 0 && s.passed == 0 {
            "skipped"
        } else if s.passed > 0 {
            "passed"
        } else {
            "skipped"
        }
    }

    pub fn status_icon(&self) -> &'static str {
        match self.status_label() {
            "passed" => "\u{2713}",
            "failed" => "\u{2717}",
            "skipped" => "\u{26a0}",
            _ => "\u{26a0}",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GroupBy {
    File,
    Date,
    Version,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StatusFilter {
    All,
    Passed,
    Failed,
    /// Skipped sessions (also retained as `Mixed` historically).
    Mixed,
}

/// State backing the Sessions tab.
pub struct SessionsTabState {
    pub config: SessionsConfig,
    pub store: Option<Arc<SessionStore>>,
    /// Cached rows: (file, version, session). Populated by `refresh()`.
    pub rows: Vec<SessionRow>,
    pub group_by: GroupBy,
    pub status_filter: StatusFilter,
    pub search: String,
    /// Whether `/`-search input mode is currently active.
    pub searching: bool,
    /// Selected group index in the left pane.
    pub selected_group: usize,
    /// Selected row index within the active group's filtered rows.
    pub selected_row: usize,
    /// Legacy flat-row focus index, retained for back-compat.
    pub focused: usize,
    pub last_error: Option<String>,
    /// Cache of the last loaded `(run_id, source, record)` triple. Avoids
    /// re-reading `run.json` and `source.http` on every redraw. Invalidated
    /// when the selected `run_id` changes (or on `refresh()`).
    cache: RefCell<Option<(String, String, SessionRecord)>>,
}

impl Default for SessionsTabState {
    fn default() -> Self {
        Self {
            config: SessionsConfig::default(),
            store: None,
            rows: Vec::new(),
            group_by: GroupBy::File,
            status_filter: StatusFilter::All,
            search: String::new(),
            searching: false,
            selected_group: 0,
            selected_row: 0,
            focused: 0,
            last_error: None,
            cache: RefCell::new(None),
        }
    }
}

impl SessionsTabState {
    /// Load the persisted sessions config from disk and, if a root is
    /// configured, open the on-disk store. Never panics — a missing or
    /// unreadable config yields the default state with `store = None`.
    pub fn from_disk() -> Self {
        let config = sessions_config::load_from(&default_config_path()).unwrap_or_default();
        let store = config
            .root
            .as_ref()
            .and_then(|r| SessionStore::open(std::path::PathBuf::from(r)).ok())
            .map(Arc::new);
        Self {
            config,
            store,
            ..Self::default()
        }
    }

    /// Eagerly load files → versions → sessions. Idempotent. Used on
    /// tab-enter and after recording new sessions.
    pub fn refresh(&mut self) {
        self.rows.clear();
        self.last_error = None;
        // Selection may now point at a different run; drop stale cache.
        self.cache.borrow_mut().take();
        let Some(store) = self.store.clone() else {
            return;
        };
        match store.list_files() {
            Ok(files) => {
                for f in files {
                    if let Ok(versions) = store.list_versions(&f.file_id) {
                        for v in versions {
                            if let Ok(sessions) = store.list_sessions(&f.file_id, &v.sha256) {
                                for s in sessions {
                                    self.rows.push(SessionRow {
                                        file: f.clone(),
                                        version: v.clone(),
                                        session: s,
                                    });
                                }
                            }
                        }
                    }
                }
                self.rows
                    .sort_by(|a, b| b.session.started_at.cmp(&a.session.started_at));
                self.clamp_selection();
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    /// Number of unique tracked files among the cached rows.
    pub fn unique_file_count(&self) -> usize {
        let mut ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for r in &self.rows {
            ids.insert(r.file.file_id.as_str());
        }
        ids.len()
    }

    // ─── Filter / search ──────────────────────────────────────────────────

    /// Returns the rows matching the current `status_filter` and `search`.
    pub fn apply_filter_and_search(&self) -> Vec<&SessionRow> {
        let needle = self.search.trim().to_lowercase();
        self.rows
            .iter()
            .filter(|r| match self.status_filter {
                StatusFilter::All => true,
                StatusFilter::Passed => r.status_label() == "passed",
                StatusFilter::Failed => {
                    let s = r.status_label();
                    s == "failed" || s == "mixed"
                }
                StatusFilter::Mixed => r.status_label() == "skipped",
            })
            .filter(|r| {
                if needle.is_empty() {
                    return true;
                }
                let hay = format!(
                    "{} {} {} {}",
                    r.file.display_name,
                    r.version.sha256,
                    r.session.run_id,
                    r.status_label()
                )
                .to_lowercase();
                hay.contains(&needle)
            })
            .collect()
    }

    /// Group filtered rows by the current `group_by`. Groups are sorted
    /// alphabetically by key (descending for dates so newest first).
    pub fn grouped(&self) -> Vec<(String, Vec<&SessionRow>)> {
        let filtered = self.apply_filter_and_search();
        let mut map: std::collections::BTreeMap<String, Vec<&SessionRow>> =
            std::collections::BTreeMap::new();
        for r in filtered {
            let key = match self.group_by {
                GroupBy::File => r.file.display_name.clone(),
                GroupBy::Date => r.session.started_at.format("%Y-%m-%d").to_string(),
                GroupBy::Version => r
                    .version
                    .sha256
                    .chars()
                    .take(8)
                    .collect::<String>(),
            };
            map.entry(key).or_default().push(r);
        }
        let mut out: Vec<(String, Vec<&SessionRow>)> = map.into_iter().collect();
        if matches!(self.group_by, GroupBy::Date) {
            out.reverse();
        }
        out
    }

    /// Returns the currently selected row, if any.
    pub fn selected_row_ref(&self) -> Option<&SessionRow> {
        let groups = self.grouped();
        let g = groups.get(self.selected_group)?;
        g.1.get(self.selected_row).copied()
    }

    /// Clamp `selected_group` and `selected_row` to currently valid ranges.
    pub fn clamp_selection(&mut self) {
        let group_count;
        let group_len;
        {
            let groups = self.grouped();
            group_count = groups.len();
            if group_count == 0 {
                self.selected_group = 0;
                self.selected_row = 0;
                return;
            }
            let sg = self.selected_group.min(group_count - 1);
            group_len = groups[sg].1.len();
        }
        if self.selected_group >= group_count {
            self.selected_group = group_count - 1;
        }
        if group_len == 0 {
            self.selected_row = 0;
        } else if self.selected_row >= group_len {
            self.selected_row = group_len - 1;
        }
    }

    // ─── Navigation ───────────────────────────────────────────────────────

    pub fn move_down(&mut self) {
        let groups = self.grouped();
        if let Some(g) = groups.get(self.selected_group) {
            if self.selected_row + 1 < g.1.len() {
                self.selected_row += 1;
            }
        }
    }

    pub fn move_up(&mut self) {
        if self.selected_row > 0 {
            self.selected_row -= 1;
        }
    }

    pub fn next_group(&mut self) {
        let groups = self.grouped();
        if groups.is_empty() {
            return;
        }
        if self.selected_group + 1 < groups.len() {
            self.selected_group += 1;
            self.selected_row = 0;
        }
    }

    pub fn prev_group(&mut self) {
        if self.selected_group > 0 {
            self.selected_group -= 1;
            self.selected_row = 0;
        }
    }

    pub fn toggle_group_by(&mut self) {
        self.group_by = match self.group_by {
            GroupBy::File => GroupBy::Date,
            GroupBy::Date => GroupBy::Version,
            GroupBy::Version => GroupBy::File,
        };
        self.selected_group = 0;
        self.selected_row = 0;
    }

    pub fn cycle_status_filter(&mut self) {
        self.status_filter = match self.status_filter {
            StatusFilter::All => StatusFilter::Passed,
            StatusFilter::Passed => StatusFilter::Failed,
            StatusFilter::Failed => StatusFilter::Mixed,
            StatusFilter::Mixed => StatusFilter::All,
        };
        self.selected_group = 0;
        self.selected_row = 0;
    }

    // ─── Detail loading ──────────────────────────────────────────────────

    /// Load the full `SessionRecord` for the currently selected row.
    /// Returns `None` if there is no store configured or no row is selected.
    /// Result is cached by `run_id` so repeated calls during redraw do not
    /// re-read `run.json` from disk.
    pub fn load_selected_record(&self) -> Option<SessionResult<SessionRecord>> {
        self.load_selected_full().map(|r| r.map(|(_, rec)| rec))
    }

    /// Like `load_selected_record` but also returns the on-disk source
    /// (`source.http` content) — used by the detail renderer's footer to
    /// show a snippet of the test that produced the run.
    pub fn load_selected_full(&self) -> Option<SessionResult<(String, SessionRecord)>> {
        let store = self.store.as_ref()?;
        let row = self.selected_row_ref()?;
        let run_id = row.session.run_id.clone();
        let file_id = row.file.file_id.clone();
        let sha = row.version.sha256.clone();

        if let Some((cached_run, src, rec)) = self.cache.borrow().as_ref() {
            if cached_run == &run_id {
                return Some(Ok((src.clone(), rec.clone())));
            }
        }

        match store.load_session(&file_id, &sha, &run_id) {
            Ok((source, rec)) => {
                *self.cache.borrow_mut() = Some((run_id, source.clone(), rec.clone()));
                Some(Ok((source, rec)))
            }
            Err(e) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use request_pilot_core::http_client::HttpResponse;
    use request_pilot_core::sessions::{
        Component, FileIdentity, RecordInput, SessionStore, SessionSummary, SessionTrigger,
        TrackedFile, VersionEntry,
    };
    use request_pilot_core::sessions::CapturePolicy;
    use request_pilot_core::assertions::AssertionResult;
    use request_pilot_core::test_runner::{
        BlockResult, ExtractResult, TestRunResults,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn tmp_root(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rp-tui-sessions-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        p
    }

    fn dummy_block(name: &str, status: &str) -> BlockResult {
        BlockResult {
            seq: Some(1),
            name: name.to_string(),
            block_type: "test".to_string(),
            group: None,
            request_method: "GET".to_string(),
            request_url: "https://example.com/x".to_string(),
            request_headers: vec![],
            request_body: None,
            status: status.to_string(),
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
            iterations: Vec::new(),
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
            total_time_ms: 1,
            block_results: blocks,
            final_variables: HashMap::new(),
            telemetry: None,
        }
    }

    /// Helper: synthesize a SessionRow without a store.
    fn mk_row(
        file_name: &str,
        sha: &str,
        passed: usize,
        failed: usize,
        skipped: usize,
        date: chrono::DateTime<Utc>,
    ) -> SessionRow {
        SessionRow {
            file: TrackedFile {
                file_id: file_name.to_string(),
                display_name: file_name.to_string(),
                source_paths: vec![],
                last_seen: date,
            },
            version: VersionEntry {
                sha256: sha.to_string(),
                byte_size: 0,
                first_seen: date,
                last_seen: date,
                session_count: 1,
            },
            session: SessionSummary {
                run_id: format!("run-{}-{}", file_name, sha),
                started_at: date,
                finished_at: date,
                trigger: SessionTrigger::Manual,
                passed,
                failed,
                skipped,
                total_time_ms: 1,
                mode: None,
                env_file: None,
            },
        }
    }

    fn mk_state(rows: Vec<SessionRow>) -> SessionsTabState {
        let mut s = SessionsTabState::default();
        s.rows = rows;
        s
    }

    #[test]
    fn sessions_tab_state_loads_default_when_no_config() {
        let _ = SessionsTabState::from_disk();
    }

    #[test]
    fn sessions_tab_state_default_has_no_store() {
        let s = SessionsTabState::default();
        assert!(s.store.is_none());
        assert!(s.rows.is_empty());
        assert_eq!(s.group_by, GroupBy::File);
        assert_eq!(s.status_filter, StatusFilter::All);
        assert!(!s.searching);
    }

    #[test]
    fn sessions_tab_state_refresh_with_no_store_is_noop() {
        let mut s = SessionsTabState::default();
        s.refresh();
        assert!(s.rows.is_empty());
        assert!(s.last_error.is_none());
    }

    #[test]
    fn refresh_populates_rows_from_recorded_run() {
        let root = tmp_root("refresh");
        let store = SessionStore::open(&root).unwrap();
        let now = Utc::now();
        let results = dummy_results(vec![dummy_block("hc", "passed")]);
        let _ = store
            .record_run(RecordInput {
                identity: FileIdentity::Alias("checkout".into()),
                source_path: None,
                source_content: "GET https://example.com/health\n",
                policy: CapturePolicy::default(),
                trigger: SessionTrigger::Manual,
                component: Component::Cli,
                started_at: now,
                finished_at: now,
                mode: None,
                env_file: None,
                env_identity: None,
                variables: vec![],
                results,
                file_id_override: None,
                redact_body_rules: Vec::new(),
                block_redact_body_rules: std::collections::HashMap::new(),
            })
            .unwrap();

        let mut s = SessionsTabState {
            store: Some(Arc::new(store)),
            ..Default::default()
        };
        s.refresh();
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.unique_file_count(), 1);
        assert!(s.last_error.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn grouped_by_file_returns_sorted_groups() {
        let d = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let rows = vec![
            mk_row("zeta.http", "aaaaaaaa", 1, 0, 0, d),
            mk_row("alpha.http", "bbbbbbbb", 1, 0, 0, d),
            mk_row("alpha.http", "cccccccc", 1, 0, 0, d),
        ];
        let s = mk_state(rows);
        let g = s.grouped();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, "alpha.http");
        assert_eq!(g[0].1.len(), 2);
        assert_eq!(g[1].0, "zeta.http");
    }

    #[test]
    fn grouped_by_date_uses_ymd() {
        let d1 = Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap();
        let d2 = Utc.with_ymd_and_hms(2024, 5, 2, 8, 30, 0).unwrap();
        let rows = vec![
            mk_row("a.http", "11111111", 1, 0, 0, d1),
            mk_row("a.http", "22222222", 1, 0, 0, d2),
        ];
        let mut s = mk_state(rows);
        s.group_by = GroupBy::Date;
        let g = s.grouped();
        assert_eq!(g.len(), 2);
        // Newest first
        assert_eq!(g[0].0, "2024-05-02");
        assert_eq!(g[1].0, "2024-05-01");
    }

    #[test]
    fn filter_by_status_fail_only() {
        let d = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let rows = vec![
            mk_row("a.http", "aa", 1, 0, 0, d), // passed
            mk_row("b.http", "bb", 0, 1, 0, d), // failed
            mk_row("c.http", "cc", 1, 1, 0, d), // mixed
            mk_row("d.http", "dd", 0, 0, 1, d), // skipped
        ];
        let mut s = mk_state(rows);
        s.status_filter = StatusFilter::Failed;
        let f = s.apply_filter_and_search();
        // failed + mixed counted under Failed filter
        assert_eq!(f.len(), 2);
        assert!(f.iter().all(|r| r.status_label() == "failed" || r.status_label() == "mixed"));
    }

    #[test]
    fn search_matches_file_name_case_insensitive() {
        let d = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let rows = vec![
            mk_row("Checkout.http", "aaaaaaaa", 1, 0, 0, d),
            mk_row("login.http", "bbbbbbbb", 1, 0, 0, d),
        ];
        let mut s = mk_state(rows);
        s.search = "CHECK".to_string();
        let f = s.apply_filter_and_search();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].file.display_name, "Checkout.http");
    }

    #[test]
    fn navigation_wraps_correctly() {
        let d = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let rows = vec![
            mk_row("a.http", "aa", 1, 0, 0, d),
            mk_row("a.http", "bb", 1, 0, 0, d),
            mk_row("a.http", "cc", 1, 0, 0, d),
        ];
        let mut s = mk_state(rows);
        // Single group with 3 rows.
        assert_eq!(s.selected_row, 0);
        s.move_up(); // already at 0
        assert_eq!(s.selected_row, 0);
        s.move_down();
        s.move_down();
        assert_eq!(s.selected_row, 2);
        s.move_down(); // past end → stay
        assert_eq!(s.selected_row, 2);
        s.move_up();
        assert_eq!(s.selected_row, 1);
    }

    #[test]
    fn load_selected_record_returns_none_when_nothing_selected() {
        // No store configured → None.
        let s = SessionsTabState::default();
        assert!(s.load_selected_record().is_none());

        // Store configured but no rows → None.
        let root = tmp_root("none-selected");
        let store = SessionStore::open(&root).unwrap();
        let s = SessionsTabState {
            store: Some(Arc::new(store)),
            ..SessionsTabState::default()
        };
        assert!(s.load_selected_record().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_selected_record_returns_record_when_row_selected() {
        let root = tmp_root("load-selected");
        let store = SessionStore::open(&root).unwrap();
        let now = Utc::now();
        let results = dummy_results(vec![dummy_block("hc", "passed")]);
        let recorded = store
            .record_run(RecordInput {
                identity: FileIdentity::Alias("checkout".into()),
                source_path: None,
                source_content: "GET https://example.com/health\n",
                policy: CapturePolicy::default(),
                trigger: SessionTrigger::Manual,
                component: Component::Cli,
                started_at: now,
                finished_at: now,
                mode: None,
                env_file: None,
                env_identity: None,
                variables: vec![],
                results,
                file_id_override: None,
                redact_body_rules: Vec::new(),
                block_redact_body_rules: std::collections::HashMap::new(),
            })
            .unwrap();

        let mut s = SessionsTabState {
            store: Some(Arc::new(store)),
            ..SessionsTabState::default()
        };
        s.refresh();
        assert_eq!(s.rows.len(), 1);
        s.selected_group = 0;
        s.selected_row = 0;

        let rec = s
            .load_selected_record()
            .expect("Some(_)")
            .expect("Ok(_)");
        assert_eq!(rec.run_id, recorded.run_id);
        assert_eq!(rec.results.passed, 1);
        assert_eq!(rec.results.block_results.len(), 1);
        assert_eq!(rec.results.block_results[0].name, "hc");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_selected_record_caches_last_result() {
        let root = tmp_root("cache");
        let store = SessionStore::open(&root).unwrap();
        let now = Utc::now();
        let results = dummy_results(vec![dummy_block("hc", "passed")]);
        let _ = store
            .record_run(RecordInput {
                identity: FileIdentity::Alias("checkout".into()),
                source_path: None,
                source_content: "GET https://example.com/health\n",
                policy: CapturePolicy::default(),
                trigger: SessionTrigger::Manual,
                component: Component::Cli,
                started_at: now,
                finished_at: now,
                mode: None,
                env_file: None,
                env_identity: None,
                variables: vec![],
                results,
                file_id_override: None,
                redact_body_rules: Vec::new(),
                block_redact_body_rules: std::collections::HashMap::new(),
            })
            .unwrap();

        let mut s = SessionsTabState {
            store: Some(Arc::new(store)),
            ..SessionsTabState::default()
        };
        s.refresh();
        s.selected_group = 0;
        s.selected_row = 0;
        let row = s.selected_row_ref().expect("row").clone();
        let run_id = row.session.run_id.clone();

        // First call populates the cache.
        let rec1 = s
            .load_selected_record()
            .expect("Some")
            .expect("Ok");
        assert_eq!(rec1.run_id, run_id);

        // Corrupt run.json on disk; if the cache is consulted, the second
        // call still returns the original record (no re-read).
        let run_json = PathBuf::from(&root)
            .join("files")
            .join(&row.file.file_id)
            .join("versions")
            .join(&row.version.sha256)
            .join("sessions")
            .join(&run_id)
            .join("run.json");
        std::fs::write(&run_json, "not json").unwrap();

        let rec2 = s
            .load_selected_record()
            .expect("Some")
            .expect("cached Ok");
        assert_eq!(rec2.run_id, run_id);

        let _ = std::fs::remove_dir_all(&root);
    }
}
