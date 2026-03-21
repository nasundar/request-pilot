use std::collections::HashMap;

use regex::Regex;

#[derive(Clone)]
pub struct VariableStore {
    variables: HashMap<String, String>,
}

impl VariableStore {
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
        }
    }

    pub fn from_pairs(pairs: &[(String, String)]) -> Self {
        let mut store = Self::new();
        for (k, v) in pairs {
            store.set(k, v);
        }
        store
    }

    pub fn set(&mut self, name: &str, value: &str) {
        self.variables.insert(name.to_string(), value.to_string());
    }

    #[allow(dead_code)]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(|s| s.as_str())
    }

    /// Replace all `{{varName}}` patterns with their values.
    /// Supports built-in variables: `{{$timestamp}}`, `{{$uuid}}`, `{{$randomInt}}`.
    /// Leaves unresolved variables as-is.
    pub fn interpolate(&self, text: &str) -> String {
        let re = Regex::new(r"\{\{(\$?[a-zA-Z_][a-zA-Z0-9_]*)\}\}").unwrap();
        re.replace_all(text, |caps: &regex::Captures| {
            let var_name = &caps[1];
            match var_name {
                "$timestamp" => chrono::Utc::now().timestamp().to_string(),
                "$uuid" => uuid::Uuid::new_v4().to_string(),
                "$randomInt" => {
                    use rand::Rng;
                    rand::thread_rng().gen_range(0..10000).to_string()
                }
                _ => self
                    .variables
                    .get(var_name)
                    .cloned()
                    // Leave as-is if not found: produce {{varName}}
                    .unwrap_or_else(|| format!("{{{{{}}}}}", var_name)),
            }
        })
        .to_string()
    }

    pub fn to_map(&self) -> HashMap<String, String> {
        self.variables.clone()
    }

    pub fn merge(&mut self, other: &[(String, String)]) {
        for (k, v) in other {
            self.set(k, v);
        }
    }

    /// Returns a list of variable names that are referenced in text via {{name}} but not defined in the store.
    /// Excludes built-in variables ($timestamp, $uuid, $randomInt).
    pub fn find_unresolved(&self, text: &str) -> Vec<String> {
        let re = Regex::new(r"\{\{(\$?[a-zA-Z_][a-zA-Z0-9_]*)\}\}").unwrap();
        let mut unresolved = Vec::new();
        for caps in re.captures_iter(text) {
            let var_name = caps[1].to_string();
            // Skip built-ins
            if matches!(var_name.as_str(), "$timestamp" | "$uuid" | "$randomInt") {
                continue;
            }
            if !self.variables.contains_key(&var_name) && !unresolved.contains(&var_name) {
                unresolved.push(var_name);
            }
        }
        unresolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_store_is_empty() {
        let store = VariableStore::new();
        assert!(store.to_map().is_empty());
    }

    #[test]
    fn from_pairs_populates_store() {
        let pairs = vec![
            ("host".to_string(), "localhost".to_string()),
            ("port".to_string(), "8080".to_string()),
        ];
        let store = VariableStore::from_pairs(&pairs);
        assert_eq!(store.get("host"), Some("localhost"));
        assert_eq!(store.get("port"), Some("8080"));
    }

    #[test]
    fn set_and_get() {
        let mut store = VariableStore::new();
        store.set("key", "value");
        assert_eq!(store.get("key"), Some("value"));
    }

    #[test]
    fn get_missing_returns_none() {
        let store = VariableStore::new();
        assert_eq!(store.get("nope"), None);
    }

    #[test]
    fn interpolate_simple_variable() {
        let mut store = VariableStore::new();
        store.set("name", "world");
        assert_eq!(store.interpolate("hello {{name}}"), "hello world");
    }

    #[test]
    fn interpolate_multiple_variables() {
        let mut store = VariableStore::new();
        store.set("host", "example.com");
        store.set("port", "3000");
        let result = store.interpolate("https://{{host}}:{{port}}/api");
        assert_eq!(result, "https://example.com:3000/api");
    }

    #[test]
    fn unresolved_variable_left_as_is() {
        let store = VariableStore::new();
        let result = store.interpolate("{{missing}}");
        assert_eq!(result, "{{missing}}");
    }

    #[test]
    fn builtin_timestamp_is_numeric() {
        let store = VariableStore::new();
        let result = store.interpolate("{{$timestamp}}");
        assert!(result.parse::<i64>().is_ok(), "timestamp should be a number: {}", result);
    }

    #[test]
    fn builtin_uuid_format() {
        let store = VariableStore::new();
        let result = store.interpolate("{{$uuid}}");
        assert_eq!(result.len(), 36, "UUID should be 36 chars: {}", result);
        assert_eq!(result.matches('-').count(), 4, "UUID should have 4 dashes");
    }

    #[test]
    fn builtin_random_int_range() {
        let store = VariableStore::new();
        let result = store.interpolate("{{$randomInt}}");
        let num: i64 = result.parse().expect("randomInt should be a number");
        assert!((0..10000).contains(&num), "randomInt should be 0-9999: {}", num);
    }

    #[test]
    fn merge_overrides_existing() {
        let mut store = VariableStore::new();
        store.set("key", "old");
        store.merge(&[("key".to_string(), "new".to_string())]);
        assert_eq!(store.get("key"), Some("new"));
    }

    #[test]
    fn merge_adds_new_keys() {
        let mut store = VariableStore::new();
        store.set("a", "1");
        store.merge(&[("b".to_string(), "2".to_string())]);
        assert_eq!(store.get("a"), Some("1"));
        assert_eq!(store.get("b"), Some("2"));
    }

    #[test]
    fn to_map_returns_all() {
        let mut store = VariableStore::new();
        store.set("x", "1");
        store.set("y", "2");
        let map = store.to_map();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("x").unwrap(), "1");
        assert_eq!(map.get("y").unwrap(), "2");
    }

    #[test]
    fn set_overwrites_value() {
        let mut store = VariableStore::new();
        store.set("key", "first");
        store.set("key", "second");
        assert_eq!(store.get("key"), Some("second"));
    }

    #[test]
    fn find_unresolved_returns_missing() {
        let mut store = VariableStore::new();
        store.set("host", "localhost");
        let unresolved = store.find_unresolved("{{host}}:{{port}}/{{path}}");
        assert_eq!(unresolved, vec!["port".to_string(), "path".to_string()]);
    }

    #[test]
    fn find_unresolved_skips_builtins() {
        let store = VariableStore::new();
        let unresolved = store.find_unresolved("{{$timestamp}} {{$uuid}} {{missing}}");
        assert_eq!(unresolved, vec!["missing".to_string()]);
    }

    #[test]
    fn find_unresolved_empty_when_all_resolved() {
        let mut store = VariableStore::new();
        store.set("a", "1");
        store.set("b", "2");
        let unresolved = store.find_unresolved("{{a}} {{b}}");
        assert!(unresolved.is_empty());
    }
}
