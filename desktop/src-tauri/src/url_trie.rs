use serde::Serialize;
use std::collections::HashMap;

/// A segment-aware trie for URL autocomplete.
/// URLs are split by separators (/ . : ? & =) into tokens.
/// Each node stores frequency (how many URLs pass through it).
pub struct UrlTrie {
    root: TrieNode,
    unique_count: usize,
}

struct TrieNode {
    children: HashMap<String, TrieNode>,
    /// Number of times this URL was inserted (only meaningful at terminal nodes)
    frequency: usize,
    /// Whether this node represents a complete URL endpoint
    is_terminal: bool,
    /// The original (cased) URL stored at terminal nodes
    original_url: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct DomainPathSuggestion {
    /// The domain + path portion (no query string)
    pub domain_path: String,
    /// Aggregated frequency across all URLs with this domain+path
    pub frequency: usize,
    /// Number of unique full URLs under this domain+path
    pub url_count: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct Suggestion {
    /// The full URL suggestion
    pub url: String,
    /// How many times this URL appears in history
    pub frequency: usize,
}

const SEPARATORS: [char; 6] = ['/', '.', ':', '?', '&', '='];

/// Split URL into tokens preserving separators as separate single-char tokens.
fn tokenize_url(url: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in url.chars() {
        if SEPARATORS.contains(&ch) {
            if !current.is_empty() {
                tokens.push(current.clone());
                current.clear();
            }
            tokens.push(ch.to_string());
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

impl TrieNode {
    fn new() -> Self {
        Self {
            children: HashMap::new(),
            frequency: 0,
            is_terminal: false,
            original_url: None,
        }
    }
}

impl UrlTrie {
    pub fn new() -> Self {
        Self {
            root: TrieNode::new(),
            unique_count: 0,
        }
    }

    /// Insert a URL into the trie. If URL already exists, increment frequency.
    pub fn insert(&mut self, url: &str) {
        let tokens: Vec<String> = tokenize_url(url)
            .into_iter()
            .map(|t| t.to_lowercase())
            .collect();
        let mut current = &mut self.root;

        for token in &tokens {
            current = current
                .children
                .entry(token.clone())
                .or_insert_with(TrieNode::new);
        }

        if !current.is_terminal {
            current.is_terminal = true;
            current.original_url = Some(url.to_string());
            self.unique_count += 1;
        }
        current.frequency += 1;
    }

    /// Remove a URL from the trie. Decrement frequency, prune dead branches.
    pub fn remove(&mut self, url: &str) {
        let tokens: Vec<String> = tokenize_url(url)
            .into_iter()
            .map(|t| t.to_lowercase())
            .collect();
        if Self::remove_recursive(&mut self.root, &tokens, 0) {
            self.unique_count -= 1;
        }
    }

    fn remove_recursive(node: &mut TrieNode, tokens: &[String], index: usize) -> bool {
        if index == tokens.len() {
            if node.is_terminal && node.frequency > 0 {
                node.frequency -= 1;
                if node.frequency == 0 {
                    node.is_terminal = false;
                    node.original_url = None;
                    return true;
                }
            }
            return false;
        }

        let token = tokens[index].clone();

        let (removed, should_prune) = if let Some(child) = node.children.get_mut(&token) {
            let removed = Self::remove_recursive(child, tokens, index + 1);
            let should_prune = !child.is_terminal && child.children.is_empty();
            (removed, should_prune)
        } else {
            return false;
        };

        if should_prune {
            node.children.remove(&token);
        }

        removed
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.root = TrieNode::new();
        self.unique_count = 0;
    }

    /// Get autocomplete suggestions for a prefix.
    /// Splits the prefix by separators, walks the trie matching segments,
    /// then collects all completions from that point.
    /// Returns suggestions sorted by frequency (descending), limited to `max_results`.
    pub fn suggest(&self, prefix: &str, max_results: usize) -> Vec<Suggestion> {
        let tokens: Vec<String> = tokenize_url(prefix)
            .into_iter()
            .map(|t| t.to_lowercase())
            .collect();

        if tokens.is_empty() {
            let mut results = Vec::new();
            Self::collect_completions(&self.root, &mut results);
            results.sort_by(|a, b| b.frequency.cmp(&a.frequency));
            results.truncate(max_results);
            return results;
        }

        let mut current = &self.root;

        // Walk all tokens except the last one, matching exactly
        for token in &tokens[..tokens.len() - 1] {
            match current.children.get(token.as_str()) {
                Some(child) => current = child,
                None => return Vec::new(),
            }
        }

        let last_token = &tokens[tokens.len() - 1];
        let mut results = Vec::new();

        // Find all children whose key starts with the last token (fuzzy segment match)
        for (key, child) in &current.children {
            if key.starts_with(last_token.as_str()) {
                Self::collect_completions(child, &mut results);
            }
        }

        results.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        results.truncate(max_results);
        results
    }

    fn collect_completions(node: &TrieNode, results: &mut Vec<Suggestion>) {
        if node.is_terminal {
            if let Some(ref url) = node.original_url {
                results.push(Suggestion {
                    url: url.clone(),
                    frequency: node.frequency,
                });
            }
        }
        for child in node.children.values() {
            Self::collect_completions(child, results);
        }
    }

    /// Suggest domain+path pairs (without query strings) matching a prefix.
    /// Aggregates frequencies from all URLs sharing the same base path.
    pub fn suggest_domain_paths(&self, prefix: &str, max_results: usize) -> Vec<DomainPathSuggestion> {
        // Get all matching full URL suggestions (use a generous limit)
        let all = self.suggest(prefix, 500);

        // Group by domain+path (strip query string)
        let mut groups: HashMap<String, (usize, usize)> = HashMap::new(); // (total_freq, url_count)
        for s in &all {
            let base = match s.url.find('?') {
                Some(idx) => &s.url[..idx],
                None => &s.url,
            };
            let entry = groups.entry(base.to_string()).or_insert((0, 0));
            entry.0 += s.frequency;
            entry.1 += 1;
        }

        let mut results: Vec<DomainPathSuggestion> = groups.into_iter()
            .map(|(dp, (freq, count))| DomainPathSuggestion {
                domain_path: dp,
                frequency: freq,
                url_count: count,
            })
            .collect();

        results.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        results.truncate(max_results);
        results
    }

    /// Total number of unique URLs in the trie.
    pub fn len(&self) -> usize {
        self.unique_count
    }

    pub fn is_empty(&self) -> bool {
        self.unique_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_trie_no_suggestions() {
        let trie = UrlTrie::new();
        assert!(trie.suggest("https://", 10).is_empty());
        assert!(trie.is_empty());
        assert_eq!(trie.len(), 0);
    }

    #[test]
    fn insert_and_suggest_exact() {
        let mut trie = UrlTrie::new();
        trie.insert("https://api.example.com/users");
        let s = trie.suggest("https://api", 10);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].url, "https://api.example.com/users");
    }

    #[test]
    fn suggest_partial_segment() {
        // "ex" should match "example" and "external"
        let mut trie = UrlTrie::new();
        trie.insert("https://api.example.com/users");
        trie.insert("https://api.external.com/data");
        let s = trie.suggest("https://api.ex", 10);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn suggest_after_separator() {
        // After typing "https://api.example.com/", suggest all paths
        let mut trie = UrlTrie::new();
        trie.insert("https://api.example.com/users");
        trie.insert("https://api.example.com/products");
        trie.insert("https://api.example.com/orders");
        let s = trie.suggest("https://api.example.com/", 10);
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn frequency_ordering() {
        let mut trie = UrlTrie::new();
        trie.insert("https://api.example.com/users");
        trie.insert("https://api.example.com/products");
        trie.insert("https://api.example.com/users"); // 2nd time
        trie.insert("https://api.example.com/users"); // 3rd time
        let s = trie.suggest("https://api.example.com/", 10);
        assert_eq!(s[0].url, "https://api.example.com/users"); // most frequent first
        assert_eq!(s[0].frequency, 3);
    }

    #[test]
    fn remove_decrements_frequency() {
        let mut trie = UrlTrie::new();
        trie.insert("https://api.example.com/users");
        trie.insert("https://api.example.com/users");
        trie.insert("https://api.example.com/users");
        assert_eq!(trie.len(), 1);

        trie.remove("https://api.example.com/users");
        let s = trie.suggest("https://api.example.com/", 10);
        assert_eq!(s[0].frequency, 2);
        assert_eq!(trie.len(), 1); // still 1 unique URL

        trie.remove("https://api.example.com/users");
        trie.remove("https://api.example.com/users");
        let s = trie.suggest("https://api.example.com/", 10);
        assert!(s.is_empty());
        assert_eq!(trie.len(), 0);
    }

    #[test]
    fn clear_empties_trie() {
        let mut trie = UrlTrie::new();
        trie.insert("https://a.com");
        trie.insert("https://b.com");
        assert_eq!(trie.len(), 2);

        trie.clear();
        assert!(trie.is_empty());
        assert_eq!(trie.len(), 0);
        assert!(trie.suggest("https://", 10).is_empty());
    }

    #[test]
    fn case_insensitive_matching() {
        // Suggest should match case-insensitively
        let mut trie = UrlTrie::new();
        trie.insert("https://API.Example.COM/users");
        let s = trie.suggest("https://api.ex", 10);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn query_parameters() {
        let mut trie = UrlTrie::new();
        trie.insert("https://api.example.com/users?page=1&size=10");
        trie.insert("https://api.example.com/users?page=2&size=10");
        let s = trie.suggest("https://api.example.com/users?page=", 10);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn max_results_limit() {
        let mut trie = UrlTrie::new();
        for i in 0..20 {
            trie.insert(&format!("https://api.example.com/item/{}", i));
        }
        let s = trie.suggest("https://api.example.com/item/", 5);
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn tokenize_url_basic() {
        let tokens = tokenize_url("https://api.example.com/v1/users");
        assert_eq!(
            tokens,
            vec![
                "https", ":", "/", "/", "api", ".", "example", ".", "com", "/", "v1", "/", "users"
            ]
        );
    }

    #[test]
    fn suggest_domain_paths_aggregates() {
        let mut trie = UrlTrie::new();
        trie.insert("https://api.example.com/users?page=1");
        trie.insert("https://api.example.com/users?page=2");
        trie.insert("https://api.example.com/users?page=3");
        trie.insert("https://api.example.com/products?page=1");
        let dp = trie.suggest_domain_paths("https://api", 5);
        assert_eq!(dp.len(), 2);
        assert_eq!(dp[0].domain_path, "https://api.example.com/users");
        assert_eq!(dp[0].frequency, 3);
        assert_eq!(dp[0].url_count, 3);
        assert_eq!(dp[1].domain_path, "https://api.example.com/products");
        assert_eq!(dp[1].frequency, 1);
    }

    #[test]
    fn suggest_domain_paths_no_query() {
        let mut trie = UrlTrie::new();
        trie.insert("https://api.example.com/health");
        let dp = trie.suggest_domain_paths("https://api", 5);
        assert_eq!(dp.len(), 1);
        assert_eq!(dp[0].domain_path, "https://api.example.com/health");
        assert_eq!(dp[0].url_count, 1);
    }

    #[test]
    fn suggest_domain_paths_empty_prefix() {
        let mut trie = UrlTrie::new();
        trie.insert("https://a.com/x?q=1");
        trie.insert("https://b.com/y?q=2");
        let dp = trie.suggest_domain_paths("", 5);
        assert_eq!(dp.len(), 2);
    }

    #[test]
    fn len_counts_unique_urls() {
        let mut trie = UrlTrie::new();
        trie.insert("https://a.com");
        trie.insert("https://b.com");
        trie.insert("https://a.com"); // duplicate
        assert_eq!(trie.len(), 2);
    }
}
