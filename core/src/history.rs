use crate::url_trie::UrlTrie;
use serde::{Deserialize, Serialize};

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
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryEntry {
    pub seq: u64,
    pub id: String,
    pub run_id: Option<String>,
    pub source: String,
    pub file_name: Option<String>,
    pub group: Option<String>,
    pub block_name: Option<String>,
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
}

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
                true
            })
            .take(f.limit.unwrap_or(usize::MAX))
            .collect()
    }

    /// Return distinct values for a field, useful for populating filter dropdowns.
    pub fn distinct_file_names(&self) -> Vec<String> {
        let mut seen = std::collections::BTreeSet::new();
        for e in &self.entries {
            if let Some(ref f) = e.file_name {
                seen.insert(f.clone());
            }
        }
        seen.into_iter().collect()
    }

    pub fn distinct_groups(&self) -> Vec<String> {
        let mut seen = std::collections::BTreeSet::new();
        for e in &self.entries {
            if let Some(ref g) = e.group {
                seen.insert(g.clone());
            }
        }
        seen.into_iter().collect()
    }

    pub fn distinct_block_names(&self) -> Vec<String> {
        let mut seen = std::collections::BTreeSet::new();
        for e in &self.entries {
            if let Some(ref b) = e.block_name {
                seen.insert(b.clone());
            }
        }
        seen.into_iter().collect()
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
}
