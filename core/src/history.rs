use crate::url_trie::UrlTrie;
use serde::{Deserialize, Serialize};

/// Filter criteria for querying history entries.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct HistoryFilter {
    pub method: Option<String>,
    pub status_min: Option<u16>,
    pub status_max: Option<u16>,
    pub url_contains: Option<String>,
    pub source: Option<String>,
    pub run_id: Option<String>,
    pub file_name: Option<String>,
    pub group: Option<String>,
    pub block_name: Option<String>,
    pub compare_step: Option<String>,
    pub limit: Option<usize>,
}

/// A single recorded HTTP request/response with metadata.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryEntry {
    pub seq: u64,
    pub id: String,
    pub run_id: Option<String>,
    pub source: String,
    pub file_name: Option<String>,
    pub group: Option<String>,
    pub block_name: Option<String>,
    /// For @compare blocks, the step name within the comparison (e.g., "baseline", "candidate").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_step: Option<String>,
    pub method: String,
    pub url: String,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<String>,
    pub status: u16,
    pub response_headers: Vec<(String, String)>,
    pub response_body: Option<String>,
    pub response_time_ms: u64,
    pub response_size_bytes: usize,
    pub timestamp: String,
    /// Test result status: "passed", "failed", "skipped", "error". Empty for non-test entries (e.g. live capture).
    #[serde(default)]
    pub result_status: String,
}

/// In-memory store of HTTP request history entries.
pub struct HistoryStore {
    pub entries: Vec<HistoryEntry>,
    max_entries: usize,
    next_seq: u64,
    pub url_trie: UrlTrie,
}

impl HistoryStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 1000,
            next_seq: 1,
            url_trie: UrlTrie::new(),
        }
    }

    pub fn next_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    pub fn add(&mut self, entry: HistoryEntry) {
        self.url_trie.insert(&entry.url);
        self.entries.insert(0, entry);
        if self.entries.len() > self.max_entries {
            self.entries.truncate(self.max_entries);
        }
    }

    /// Update an existing entry by id, replacing its fields.
    pub fn update(&mut self, entry: HistoryEntry) -> bool {
        if let Some(existing) = self.entries.iter_mut().find(|e| e.id == entry.id) {
            let old_url = existing.url.clone();
            let seq = existing.seq; // preserve original seq
            *existing = entry;
            existing.seq = seq;
            // Keep url_trie in sync when URL changes
            if existing.url != old_url {
                self.url_trie.insert(&existing.url);
            }
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.url_trie.clear();
        // Don't reset next_seq — keep counting
    }

    pub fn filter(&self, f: &HistoryFilter) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .filter(|e| {
                if let Some(ref m) = f.method {
                    if !e.method.eq_ignore_ascii_case(m) {
                        return false;
                    }
                }
                if let Some(min) = f.status_min {
                    if e.status < min {
                        return false;
                    }
                }
                if let Some(max) = f.status_max {
                    if e.status >= max {
                        return false;
                    }
                }
                if let Some(ref url) = f.url_contains {
                    if !e.url.to_lowercase().contains(&url.to_lowercase()) {
                        return false;
                    }
                }
                if let Some(ref src) = f.source {
                    if e.source != *src {
                        return false;
                    }
                }
                if let Some(ref rid) = f.run_id {
                    match &e.run_id {
                        Some(r) if r == rid => {}
                        _ => return false,
                    }
                }
                if let Some(ref fname) = f.file_name {
                    match &e.file_name {
                        Some(f) if f == fname => {}
                        _ => return false,
                    }
                }
                if let Some(ref grp) = f.group {
                    match &e.group {
                        Some(g) if g == grp => {}
                        _ => return false,
                    }
                }
                if let Some(ref bn) = f.block_name {
                    match &e.block_name {
                        Some(b) if b == bn => {}
                        _ => return false,
                    }
                }
                if let Some(ref cs) = f.compare_step {
                    match &e.compare_step {
                        Some(s) if s == cs => {}
                        _ => return false,
                    }
                }
                true
            })
            .take(f.limit.unwrap_or(usize::MAX))
            .collect()
    }

    /// Return distinct values for a field, useful for populating filter dropdowns.
    pub fn distinct_file_names(&self) -> Vec<String> {
        self.entries.iter()
            .filter_map(|e| e.file_name.as_deref())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(String::from)
            .collect()
    }

    pub fn distinct_groups(&self) -> Vec<String> {
        self.entries.iter()
            .filter_map(|e| e.group.as_deref())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(String::from)
            .collect()
    }

    pub fn distinct_block_names(&self) -> Vec<String> {
        self.entries.iter()
            .filter_map(|e| e.block_name.as_deref())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(String::from)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str) -> HistoryEntry {
        HistoryEntry {
            seq: 0,
            id: id.to_string(),
            run_id: None,
            source: "manual".to_string(),
            file_name: None,
            group: None,
            block_name: None,
            compare_step: None,
            method: "GET".to_string(),
            url: "https://example.com".to_string(),
            request_headers: Vec::new(),
            request_body: None,
            status: 200,
            response_headers: Vec::new(),
            response_body: None,
            response_time_ms: 50,
            response_size_bytes: 0,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            result_status: String::new(),
        }
    }

    #[test]
    fn new_store_is_empty() {
        let store = HistoryStore::new();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn add_entry_increases_count() {
        let mut store = HistoryStore::new();
        store.add(make_entry("1"));
        assert_eq!(store.entries.len(), 1);
        store.add(make_entry("2"));
        assert_eq!(store.entries.len(), 2);
    }

    #[test]
    fn entries_in_reverse_order() {
        let mut store = HistoryStore::new();
        store.add(make_entry("first"));
        store.add(make_entry("second"));
        store.add(make_entry("third"));
        assert_eq!(store.entries[0].id, "third");
        assert_eq!(store.entries[1].id, "second");
        assert_eq!(store.entries[2].id, "first");
    }

    #[test]
    fn max_entries_cap() {
        let mut store = HistoryStore::new();
        for i in 0..1001 {
            store.add(make_entry(&i.to_string()));
        }
        assert_eq!(store.entries.len(), 1000);
        // newest entry (1000) should be first
        assert_eq!(store.entries[0].id, "1000");
    }

    #[test]
    fn clear_empties_store() {
        let mut store = HistoryStore::new();
        store.add(make_entry("1"));
        store.add(make_entry("2"));
        assert_eq!(store.entries.len(), 2);
        store.clear();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn entry_fields_preserved() {
        let mut store = HistoryStore::new();
        let entry = HistoryEntry {
            seq: 1,
            id: "test-id".to_string(),
            run_id: Some("run-1".to_string()),
            source: "test-run".to_string(),
            file_name: Some("api-tests.http".to_string()),
            group: Some("auth".to_string()),
            block_name: Some("create user".to_string()),
            compare_step: None,
            method: "POST".to_string(),
            url: "https://api.test.com/data".to_string(),
            request_headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            request_body: Some("{\"key\":\"value\"}".to_string()),
            status: 201,
            response_headers: vec![("X-Request-Id".to_string(), "abc".to_string())],
            response_body: Some("{\"id\":1}".to_string()),
            response_time_ms: 150,
            response_size_bytes: 8,
            timestamp: "2024-06-15T12:00:00Z".to_string(),
            result_status: String::new(),
        };
        store.add(entry);
        let stored = &store.entries[0];
        assert_eq!(stored.seq, 1);
        assert_eq!(stored.method, "POST");
        assert_eq!(stored.status, 201);
        assert_eq!(stored.request_headers.len(), 1);
        assert_eq!(stored.request_body.as_deref(), Some("{\"key\":\"value\"}"));
        assert_eq!(stored.response_headers.len(), 1);
        assert_eq!(stored.response_body.as_deref(), Some("{\"id\":1}"));
        assert_eq!(stored.response_size_bytes, 8);
        assert_eq!(stored.run_id.as_deref(), Some("run-1"));
        assert_eq!(stored.source, "test-run");
        assert_eq!(stored.file_name.as_deref(), Some("api-tests.http"));
        assert_eq!(stored.group.as_deref(), Some("auth"));
        assert_eq!(stored.block_name.as_deref(), Some("create user"));
    }

    #[test]
    fn next_seq_auto_increments() {
        let mut store = HistoryStore::new();
        assert_eq!(store.next_seq(), 1);
        assert_eq!(store.next_seq(), 2);
        assert_eq!(store.next_seq(), 3);
    }

    #[test]
    fn clear_does_not_reset_seq() {
        let mut store = HistoryStore::new();
        assert_eq!(store.next_seq(), 1);
        assert_eq!(store.next_seq(), 2);
        store.clear();
        assert!(store.entries.is_empty());
        assert_eq!(store.next_seq(), 3);
    }

    // --- filter tests ---

    fn make_entry_with(
        seq: u64,
        method: &str,
        url: &str,
        status: u16,
        source: &str,
        run_id: Option<&str>,
    ) -> HistoryEntry {
        HistoryEntry {
            seq,
            id: format!("id-{}", seq),
            run_id: run_id.map(|s| s.to_string()),
            source: source.to_string(),
            file_name: None,
            group: None,
            block_name: None,
            compare_step: None,
            method: method.to_string(),
            url: url.to_string(),
            request_headers: Vec::new(),
            request_body: None,
            status,
            response_headers: Vec::new(),
            response_body: None,
            response_time_ms: 50,
            response_size_bytes: 100,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            result_status: String::new(),
        }
    }

    fn sample_store() -> HistoryStore {
        let mut store = HistoryStore::new();
        store.add(make_entry_with(1, "GET", "https://api.example.com/users", 200, "manual", None));
        store.add(make_entry_with(2, "POST", "https://api.example.com/users", 201, "manual", None));
        store.add(make_entry_with(3, "GET", "https://api.example.com/orders", 404, "test-run", Some("run-1")));
        store.add(make_entry_with(4, "DELETE", "https://api.example.com/users/1", 204, "test-run", Some("run-1")));
        store.add(make_entry_with(5, "POST", "https://api.example.com/login", 401, "manual", None));
        store.add(make_entry_with(6, "GET", "https://OTHER.example.com/health", 500, "run-all", Some("run-2")));
        store
    }

    #[test]
    fn filter_empty_returns_all() {
        let store = sample_store();
        let results = store.filter(&HistoryFilter::default());
        assert_eq!(results.len(), 6);
    }

    #[test]
    fn filter_by_method_get() {
        let store = sample_store();
        let f = HistoryFilter {
            method: Some("GET".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|e| e.method == "GET"));
    }

    #[test]
    fn filter_by_method_post() {
        let store = sample_store();
        let f = HistoryFilter {
            method: Some("post".to_string()), // case-insensitive
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.method == "POST"));
    }

    #[test]
    fn filter_by_status_success_range() {
        let store = sample_store();
        let f = HistoryFilter {
            status_min: Some(200),
            status_max: Some(300),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|e| e.status >= 200 && e.status < 300));
    }

    #[test]
    fn filter_by_status_client_error() {
        let store = sample_store();
        let f = HistoryFilter {
            status_min: Some(400),
            status_max: Some(500),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.status >= 400 && e.status < 500));
    }

    #[test]
    fn filter_by_url_case_insensitive() {
        let store = sample_store();
        let f = HistoryFilter {
            url_contains: Some("USERS".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 3);
        assert!(results
            .iter()
            .all(|e| e.url.to_lowercase().contains("users")));
    }

    #[test]
    fn filter_by_source() {
        let store = sample_store();
        let f = HistoryFilter {
            source: Some("test-run".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.source == "test-run"));
    }

    #[test]
    fn filter_by_run_id() {
        let store = sample_store();
        let f = HistoryFilter {
            run_id: Some("run-1".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .all(|e| e.run_id.as_deref() == Some("run-1")));
    }

    #[test]
    fn filter_combined() {
        let store = sample_store();
        let f = HistoryFilter {
            method: Some("GET".to_string()),
            status_min: Some(200),
            status_max: Some(300),
            url_contains: Some("users".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://api.example.com/users");
        assert_eq!(results[0].status, 200);
    }

    #[test]
    fn filter_with_limit() {
        let store = sample_store();
        let f = HistoryFilter {
            limit: Some(2),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn filter_empty_store() {
        let store = HistoryStore::new();
        let f = HistoryFilter {
            method: Some("GET".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert!(results.is_empty());
    }

    #[test]
    fn filter_no_matches() {
        let store = sample_store();
        let f = HistoryFilter {
            method: Some("PATCH".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert!(results.is_empty());
    }

    fn make_entry_full(
        seq: u64,
        method: &str,
        url: &str,
        status: u16,
        source: &str,
        file_name: Option<&str>,
        group: Option<&str>,
        block_name: Option<&str>,
    ) -> HistoryEntry {
        HistoryEntry {
            seq,
            id: format!("id-{}", seq),
            run_id: None,
            source: source.to_string(),
            file_name: file_name.map(|s| s.to_string()),
            group: group.map(|s| s.to_string()),
            block_name: block_name.map(|s| s.to_string()),
            compare_step: None,
            method: method.to_string(),
            url: url.to_string(),
            request_headers: Vec::new(),
            request_body: None,
            status,
            response_headers: Vec::new(),
            response_body: None,
            response_time_ms: 50,
            response_size_bytes: 100,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            result_status: String::new(),
        }
    }

    fn enriched_store() -> HistoryStore {
        let mut store = HistoryStore::new();
        store.add(make_entry_full(1, "GET", "https://api.test.com/a", 200, "test-run", Some("auth.http"), Some("login"), Some("Login Test")));
        store.add(make_entry_full(2, "POST", "https://api.test.com/b", 201, "test-run", Some("auth.http"), Some("login"), Some("Create Token")));
        store.add(make_entry_full(3, "GET", "https://api.test.com/c", 200, "test-run", Some("users.http"), None, Some("List Users")));
        store.add(make_entry_full(4, "DELETE", "https://api.test.com/d", 204, "test-run", Some("users.http"), Some("cleanup"), Some("Delete User")));
        store.add(make_entry_full(5, "GET", "https://api.test.com/e", 200, "manual", None, None, None));
        store
    }

    #[test]
    fn filter_by_file_name() {
        let store = enriched_store();
        let f = HistoryFilter { file_name: Some("auth.http".to_string()), ..Default::default() };
        let results = store.filter(&f);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.file_name.as_deref() == Some("auth.http")));
    }

    #[test]
    fn filter_by_group() {
        let store = enriched_store();
        let f = HistoryFilter { group: Some("login".to_string()), ..Default::default() };
        let results = store.filter(&f);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.group.as_deref() == Some("login")));
    }

    #[test]
    fn filter_by_block_name() {
        let store = enriched_store();
        let f = HistoryFilter { block_name: Some("List Users".to_string()), ..Default::default() };
        let results = store.filter(&f);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].block_name.as_deref(), Some("List Users"));
    }

    #[test]
    fn filter_by_file_and_group() {
        let store = enriched_store();
        let f = HistoryFilter {
            file_name: Some("users.http".to_string()),
            group: Some("cleanup".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].block_name.as_deref(), Some("Delete User"));
    }

    #[test]
    fn filter_no_file_name_excludes() {
        let store = enriched_store();
        let f = HistoryFilter { file_name: Some("nonexistent.http".to_string()), ..Default::default() };
        let results = store.filter(&f);
        assert!(results.is_empty());
    }

    #[test]
    fn distinct_file_names() {
        let store = enriched_store();
        let names = store.distinct_file_names();
        assert_eq!(names, vec!["auth.http", "users.http"]);
    }

    #[test]
    fn distinct_groups() {
        let store = enriched_store();
        let groups = store.distinct_groups();
        assert_eq!(groups, vec!["cleanup", "login"]);
    }

    #[test]
    fn distinct_block_names() {
        let store = enriched_store();
        let names = store.distinct_block_names();
        assert_eq!(names, vec!["Create Token", "Delete User", "List Users", "Login Test"]);
    }

    // --- update tests ---

    #[test]
    fn update_existing_entry() {
        let mut store = HistoryStore::new();
        let mut entry = make_entry("update-me");
        entry.seq = 42;
        entry.status = 200;
        entry.method = "GET".to_string();
        store.add(entry);

        let mut updated = make_entry("update-me");
        updated.seq = 999; // should be overwritten with original seq
        updated.status = 201;
        updated.method = "POST".to_string();
        updated.response_body = Some("new body".to_string());

        assert!(store.update(updated));
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].seq, 42); // preserved
        assert_eq!(store.entries[0].status, 201); // updated
        assert_eq!(store.entries[0].method, "POST"); // updated
        assert_eq!(store.entries[0].response_body.as_deref(), Some("new body"));
    }

    #[test]
    fn update_nonexistent_returns_false() {
        let mut store = HistoryStore::new();
        store.add(make_entry("existing"));
        let ghost = make_entry("ghost-id");
        assert!(!store.update(ghost));
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].id, "existing");
    }

    #[test]
    fn update_preserves_seq_not_other_fields() {
        let mut store = HistoryStore::new();
        let mut original = make_entry("e1");
        original.seq = 10;
        original.url = "https://old.com".to_string();
        original.request_body = Some("old body".to_string());
        store.add(original);

        let mut replacement = make_entry("e1");
        replacement.seq = 99;
        replacement.url = "https://new.com".to_string();
        replacement.request_body = None;

        assert!(store.update(replacement));
        let e = &store.entries[0];
        assert_eq!(e.seq, 10); // seq preserved
        assert_eq!(e.url, "https://new.com"); // replaced
        assert!(e.request_body.is_none()); // replaced (was Some, now None)
    }

    #[test]
    fn update_correct_entry_among_many() {
        let mut store = HistoryStore::new();
        store.add(make_entry("a"));
        store.add(make_entry("b"));
        store.add(make_entry("c"));

        let mut updated = make_entry("b");
        updated.status = 500;

        assert!(store.update(updated));
        // Only "b" should be updated
        assert_eq!(store.entries.iter().find(|e| e.id == "a").unwrap().status, 200);
        assert_eq!(store.entries.iter().find(|e| e.id == "b").unwrap().status, 500);
        assert_eq!(store.entries.iter().find(|e| e.id == "c").unwrap().status, 200);
    }

    #[test]
    fn update_empty_store_returns_false() {
        let mut store = HistoryStore::new();
        assert!(!store.update(make_entry("no-one-home")));
    }

    #[test]
    fn update_syncs_url_trie_on_url_change() {
        let mut store = HistoryStore::new();
        let mut e = make_entry("trie-test");
        e.url = "https://old.example.com/api".to_string();
        store.add(e);

        let suggestions = store.url_trie.suggest("https://old", 10);
        assert_eq!(suggestions.len(), 1);

        let mut updated = make_entry("trie-test");
        updated.url = "https://new.example.com/api".to_string();
        assert!(store.update(updated));

        let new_suggestions = store.url_trie.suggest("https://new", 10);
        assert_eq!(new_suggestions.len(), 1);
        assert_eq!(new_suggestions[0].url, "https://new.example.com/api");
    }

    // --- filter by source extension-live ---

    #[test]
    fn filter_by_extension_live_source() {
        let mut store = HistoryStore::new();
        store.add(make_entry_with(1, "GET", "https://api.com/a", 200, "manual", None));
        store.add(make_entry_with(2, "POST", "https://api.com/b", 200, "extension-live", None));
        store.add(make_entry_with(3, "GET", "https://api.com/c", 200, "extension-live", None));
        store.add(make_entry_with(4, "DELETE", "https://api.com/d", 204, "test-run", Some("r1")));

        let f = HistoryFilter { source: Some("extension-live".to_string()), ..Default::default() };
        let results = store.filter(&f);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.source == "extension-live"));
    }

    // --- compare_step tests ---

    fn make_entry_with_compare_step(
        seq: u64,
        compare_step: Option<&str>,
    ) -> HistoryEntry {
        HistoryEntry {
            seq,
            id: format!("id-{}", seq),
            run_id: Some("run-cmp".to_string()),
            source: "test-run".to_string(),
            file_name: Some("compare.http".to_string()),
            group: None,
            block_name: Some("Compare APIs".to_string()),
            compare_step: compare_step.map(|s| s.to_string()),
            method: "GET".to_string(),
            url: format!("https://api.test.com/v{}", seq),
            request_headers: Vec::new(),
            request_body: None,
            status: 200,
            response_headers: Vec::new(),
            response_body: Some(format!("{{\"step\":\"{}\"}}", seq)),
            response_time_ms: 50,
            response_size_bytes: 20,
            timestamp: "2024-06-15T12:00:00Z".to_string(),
            result_status: String::new(),
        }
    }

    #[test]
    fn filter_by_compare_step() {
        let mut store = HistoryStore::new();
        store.add(make_entry_with_compare_step(1, Some("baseline")));
        store.add(make_entry_with_compare_step(2, Some("candidate")));
        store.add(make_entry_with_compare_step(3, Some("baseline")));
        store.add(make_entry_with_compare_step(4, None)); // no compare_step

        let f = HistoryFilter {
            compare_step: Some("baseline".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert_eq!(results.len(), 2, "should match exactly the two baseline entries");
        assert!(
            results.iter().all(|e| e.compare_step.as_deref() == Some("baseline")),
            "all results should have compare_step='baseline'"
        );
    }

    #[test]
    fn compare_step_serde_roundtrip() {
        let entry = make_entry_with_compare_step(1, Some("baseline"));
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: HistoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.compare_step.as_deref(),
            Some("baseline"),
            "compare_step should survive serialization roundtrip"
        );
    }

    #[test]
    fn compare_step_none_excluded_from_json() {
        let entry = make_entry_with_compare_step(1, None);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(
            !json.contains("compare_step"),
            "compare_step=None should be omitted from JSON via skip_serializing_if, got: {}",
            json
        );
    }

    #[test]
    fn filter_compare_step_no_match() {
        let mut store = HistoryStore::new();
        store.add(make_entry_with_compare_step(1, Some("baseline")));
        store.add(make_entry_with_compare_step(2, Some("candidate")));

        let f = HistoryFilter {
            compare_step: Some("nonexistent_step".to_string()),
            ..Default::default()
        };
        let results = store.filter(&f);
        assert!(results.is_empty(), "no entries should match a non-existent compare_step");
    }
}
