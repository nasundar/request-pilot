use std::collections::HashMap;

use regex::Regex;

/// A captured response from a previously-executed block, referenced via
/// `{{blockName.response.status}}`, `{{blockName.response.headers.name}}`,
/// and `{{blockName.response.body.$.jsonpath}}`.
#[derive(Clone, Debug)]
pub struct CapturedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone)]
pub struct VariableStore {
    variables: HashMap<String, String>,
    responses: HashMap<String, CapturedResponse>,
}

impl VariableStore {
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            responses: HashMap::new(),
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

    /// Record a response under `name` for later reference via
    /// `{{name.response.*}}` interpolation.
    pub fn set_response(&mut self, name: &str, resp: CapturedResponse) {
        if name.is_empty() {
            return;
        }
        self.responses.insert(name.to_string(), resp);
    }

    /// Merge all captured responses from another store into this one.
    pub fn merge_responses(&mut self, other: &VariableStore) {
        for (k, v) in &other.responses {
            self.responses.insert(k.clone(), v.clone());
        }
    }

    /// Replace all `{{varName}}` patterns with their values.
    ///
    /// Supports user variables, the following REST Client–compatible
    /// built-ins, and response-chain references of the form
    /// `{{blockName.response.status}}`, `{{blockName.response.headers.name}}`,
    /// and `{{blockName.response.body.$.jsonpath}}` (for JSON bodies).
    ///
    /// | Built-in                         | Value                                   |
    /// |----------------------------------|-----------------------------------------|
    /// | `{{$timestamp}}`                 | Unix epoch seconds                      |
    /// | `{{$datetime}}`                  | Current UTC time, ISO 8601              |
    /// | `{{$uuid}}` / `{{$guid}}`        | UUIDv4                                  |
    /// | `{{$randomInt}}`                 | Random int 0..9999                      |
    /// | `{{$randomInt min max}}`         | Random int min..max (inclusive low,     |
    /// |                                  | exclusive high — REST Client semantics) |
    /// | `{{$processEnv VARNAME}}`        | Value of OS environment variable        |
    /// | `{{$localHostname}}`             | Local machine hostname                  |
    ///
    /// Leaves unresolved variables as-is (e.g. `{{missing}}` stays `{{missing}}`).
    pub fn interpolate(&self, text: &str) -> String {
        let re = Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").unwrap();
        re.replace_all(text, |caps: &regex::Captures| {
            let inner = caps[1].trim();
            resolve_token(inner, &self.variables, &self.responses)
                .unwrap_or_else(|| format!("{{{{{}}}}}", inner))
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

    /// Returns a list of variable names that are referenced in text via
    /// `{{name}}` but not defined in the store. Excludes built-in variables
    /// (`$timestamp`, `$datetime`, `$uuid`, `$guid`, `$randomInt`,
    /// `$processEnv`, `$localHostname`).
    pub fn find_unresolved(&self, text: &str) -> Vec<String> {
        let re = Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").unwrap();
        let mut unresolved = Vec::new();
        for caps in re.captures_iter(text) {
            let inner = caps[1].trim();
            if resolve_token(inner, &self.variables, &self.responses).is_some() {
                continue;
            }
            let head = inner.split_whitespace().next().unwrap_or("");
            if matches!(
                head,
                "$timestamp"
                    | "$datetime"
                    | "$uuid"
                    | "$guid"
                    | "$randomInt"
                    | "$processEnv"
                    | "$localHostname"
            ) {
                continue;
            }
            // Treat response-chain refs whose block hasn't run yet as expected
            // late-bound references — still report them so users see what's missing.
            let name = inner.to_string();
            if !unresolved.contains(&name) {
                unresolved.push(name);
            }
        }
        unresolved
    }
}

/// Resolve a single `{{...}}` token's inner text. Returns `None` if the token
/// is unknown / unresolvable.
fn resolve_token(
    inner: &str,
    vars: &HashMap<String, String>,
    responses: &HashMap<String, CapturedResponse>,
) -> Option<String> {
    // Response-chain reference: `<name>.response.<kind>[.<path>]`
    if let Some(resolved) = resolve_response_chain(inner, responses) {
        return Some(resolved);
    }

    // Built-in handlers.
    let mut parts = inner.split_whitespace();
    let head = parts.next()?;
    match head {
        "$timestamp" => Some(chrono::Utc::now().timestamp().to_string()),
        "$datetime" => Some(chrono::Utc::now().to_rfc3339()),
        "$uuid" | "$guid" => Some(uuid::Uuid::new_v4().to_string()),
        "$randomInt" => {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let min: i64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let max: i64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10_000);
            if max <= min {
                return Some(min.to_string());
            }
            Some(rng.gen_range(min..max).to_string())
        }
        "$processEnv" => {
            let name = parts.next()?;
            std::env::var(name).ok()
        }
        "$localHostname" => hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok()),
        _ => {
            // User variable: must be a single identifier (no spaces/args).
            if parts.next().is_some() {
                return None;
            }
            vars.get(head).cloned()
        }
    }
}

/// Resolve `<blockName>.response.<status|headers|body>[.<path>]`.
fn resolve_response_chain(
    inner: &str,
    responses: &HashMap<String, CapturedResponse>,
) -> Option<String> {
    // Require ".response." somewhere in the token; whitespace would indicate a
    // built-in with args, not a response-chain ref.
    if inner.contains(char::is_whitespace) {
        return None;
    }
    let marker = ".response";
    let idx = inner.find(marker)?;
    let block_name = &inner[..idx];
    if block_name.is_empty() {
        return None;
    }
    let tail = &inner[idx + marker.len()..];
    let resp = responses.get(block_name)?;

    // tail is one of: "", ".status", ".headers.<name>", ".body[.<path>]"
    let tail = match tail.strip_prefix('.') {
        Some(t) => t,
        None if tail.is_empty() => return Some(resp.body.clone()),
        None => return None,
    };

    if tail == "status" {
        return Some(resp.status.to_string());
    }
    if let Some(header_name) = tail.strip_prefix("headers.") {
        let lower = header_name.to_ascii_lowercase();
        for (k, v) in &resp.headers {
            if k.to_ascii_lowercase() == lower {
                return Some(v.clone());
            }
        }
        return None;
    }
    if tail == "body" {
        return Some(resp.body.clone());
    }
    if let Some(path) = tail.strip_prefix("body.") {
        // Path may start with "$" (JSONPath-lite) or be a dotted path.
        let json: serde_json::Value = serde_json::from_str(&resp.body).ok()?;
        return resolve_json_path(&json, path);
    }
    None
}

/// Minimal JSONPath-lite: supports `$.a.b`, `a.b`, `$[0].b`, `a.b[0].c`.
fn resolve_json_path(root: &serde_json::Value, path: &str) -> Option<String> {
    let mut cur = root;
    let path = path.strip_prefix('$').unwrap_or(path);
    let path = path.strip_prefix('.').unwrap_or(path);
    if path.is_empty() {
        return Some(json_to_string(cur));
    }
    // Split on `.` and handle `[N]` index segments inline.
    for segment in path.split('.') {
        let mut seg = segment;
        // Extract bracket indices e.g. "items[0]"
        while let Some(open) = seg.find('[') {
            let key = &seg[..open];
            if !key.is_empty() {
                cur = cur.get(key)?;
            }
            let close = seg.find(']')?;
            let idx: usize = seg[open + 1..close].parse().ok()?;
            cur = cur.get(idx)?;
            seg = &seg[close + 1..];
            seg = seg.strip_prefix('.').unwrap_or(seg);
        }
        if !seg.is_empty() {
            cur = cur.get(seg)?;
        }
    }
    Some(json_to_string(cur))
}

fn json_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
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

    // ── Extended built-ins (REST Client compatible) ──

    #[test]
    fn builtin_guid_is_uuid_alias() {
        let store = VariableStore::new();
        let result = store.interpolate("{{$guid}}");
        assert_eq!(result.len(), 36, "guid should be uuid-shaped: {}", result);
        assert_eq!(result.matches('-').count(), 4);
    }

    #[test]
    fn builtin_datetime_is_iso8601() {
        let store = VariableStore::new();
        let result = store.interpolate("{{$datetime}}");
        // RFC 3339 sample: "2026-04-23T16:55:29.147Z" or "...+00:00"
        assert!(result.contains('T'), "should contain T separator: {}", result);
        assert!(result.starts_with("20"), "should start with current century: {}", result);
    }

    #[test]
    fn builtin_random_int_with_bounds() {
        let store = VariableStore::new();
        for _ in 0..20 {
            let result = store.interpolate("{{$randomInt 5 10}}");
            let n: i64 = result.parse().unwrap();
            assert!((5..10).contains(&n), "out of range: {}", n);
        }
    }

    #[test]
    fn builtin_process_env_resolves() {
        std::env::set_var("REQUEST_PILOT_TEST_VAR", "test_value_123");
        let store = VariableStore::new();
        let result = store.interpolate("{{$processEnv REQUEST_PILOT_TEST_VAR}}");
        assert_eq!(result, "test_value_123");
        std::env::remove_var("REQUEST_PILOT_TEST_VAR");
    }

    #[test]
    fn builtin_process_env_missing_left_as_is() {
        let store = VariableStore::new();
        let result = store.interpolate("{{$processEnv DEFINITELY_MISSING_VAR_XYZ}}");
        assert!(result.contains("$processEnv"), "unresolved should be preserved: {}", result);
    }

    #[test]
    fn builtin_local_hostname_returns_value() {
        let store = VariableStore::new();
        let result = store.interpolate("{{$localHostname}}");
        assert!(!result.is_empty());
        assert_ne!(result, "{{$localHostname}}", "should resolve hostname");
    }

    #[test]
    fn find_unresolved_skips_extended_builtins() {
        let store = VariableStore::new();
        let unresolved = store.find_unresolved(
            "{{$datetime}} {{$guid}} {{$processEnv X}} {{$localHostname}} {{$randomInt 1 5}} {{missing}}",
        );
        assert_eq!(unresolved, vec!["missing".to_string()]);
    }

    // ── Response-chain refs ──

    fn sample_resp() -> CapturedResponse {
        CapturedResponse {
            status: 201,
            headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("X-Request-Id".to_string(), "abc123".to_string()),
            ],
            body: r#"{"id":42,"user":{"name":"alice","roles":["admin","ops"]}}"#.to_string(),
        }
    }

    #[test]
    fn response_chain_status() {
        let mut store = VariableStore::new();
        store.set_response("login", sample_resp());
        assert_eq!(store.interpolate("{{login.response.status}}"), "201");
    }

    #[test]
    fn response_chain_header_case_insensitive() {
        let mut store = VariableStore::new();
        store.set_response("login", sample_resp());
        assert_eq!(
            store.interpolate("{{login.response.headers.x-request-id}}"),
            "abc123"
        );
        assert_eq!(
            store.interpolate("{{login.response.headers.Content-Type}}"),
            "application/json"
        );
    }

    #[test]
    fn response_chain_body_jsonpath() {
        let mut store = VariableStore::new();
        store.set_response("login", sample_resp());
        assert_eq!(store.interpolate("{{login.response.body.$.id}}"), "42");
        assert_eq!(
            store.interpolate("{{login.response.body.$.user.name}}"),
            "alice"
        );
        assert_eq!(
            store.interpolate("{{login.response.body.$.user.roles[0]}}"),
            "admin"
        );
        // Without $ prefix also works
        assert_eq!(
            store.interpolate("{{login.response.body.user.roles[1]}}"),
            "ops"
        );
    }

    #[test]
    fn response_chain_whole_body() {
        let mut store = VariableStore::new();
        store.set_response("login", sample_resp());
        let whole = store.interpolate("{{login.response.body}}");
        assert!(whole.contains("\"id\":42"));
    }

    #[test]
    fn response_chain_missing_block_left_literal() {
        let store = VariableStore::new();
        assert_eq!(
            store.interpolate("{{nope.response.status}}"),
            "{{nope.response.status}}"
        );
    }

    #[test]
    fn response_chain_missing_path_left_literal() {
        let mut store = VariableStore::new();
        store.set_response("login", sample_resp());
        assert_eq!(
            store.interpolate("{{login.response.body.$.missing}}"),
            "{{login.response.body.$.missing}}"
        );
    }

    #[test]
    fn response_chain_does_not_clash_with_dotted_var_names() {
        // A normal variable named `user.name` would be weird but should still work —
        // only tokens containing `.response.` are treated as response-chain refs.
        let mut store = VariableStore::new();
        store.set("plain_name", "hello");
        assert_eq!(store.interpolate("{{plain_name}}"), "hello");
    }
}
