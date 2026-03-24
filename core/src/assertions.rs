use serde::{Deserialize, Serialize};

use crate::http_parser::Assertion;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssertionResult {
    pub assertion: String,
    pub passed: bool,
    pub actual: Option<String>,
    pub expected: Option<String>,
}

/// Evaluate a single assertion against a response.
pub fn evaluate(
    assertion: &Assertion,
    status: u16,
    headers: &[(String, String)],
    body: &str,
) -> AssertionResult {
    let left_value = resolve_response_value(&assertion.left, status, headers, body);
    let right_clean = assertion.right.trim_matches('"').to_string();
    let right_is_null = right_clean == "null";

    let passed = match assertion.operator.as_str() {
        "==" => {
            if right_is_null {
                left_value.is_none() || left_value.as_deref() == Some("null")
            } else {
                match &left_value {
                    Some(l) => l == &right_clean || numeric_eq(l, &right_clean),
                    None => false,
                }
            }
        }
        "!=" => {
            if right_is_null {
                left_value.is_some() && left_value.as_deref() != Some("null")
            } else {
                match &left_value {
                    Some(l) => l != &right_clean && !numeric_eq(l, &right_clean),
                    None => true,
                }
            }
        }
        ">" => numeric_cmp(&left_value, &right_clean, |a, b| a > b),
        "<" => numeric_cmp(&left_value, &right_clean, |a, b| a < b),
        ">=" => numeric_cmp(&left_value, &right_clean, |a, b| a >= b),
        "<=" => numeric_cmp(&left_value, &right_clean, |a, b| a <= b),
        "contains" => match &left_value {
            Some(l) => l.contains(&right_clean),
            None => false,
        },
        _ => false,
    };

    AssertionResult {
        assertion: format!("{} {} {}", assertion.left, assertion.operator, assertion.right),
        passed,
        actual: left_value,
        expected: Some(assertion.right.clone()),
    }
}

/// Resolve a dotted path against a response (status, headers, body).
/// Public so the test runner can reuse it for extract evaluation.
pub fn resolve_response_value(
    path: &str,
    status: u16,
    headers: &[(String, String)],
    body: &str,
) -> Option<String> {
    // Shorthand: "status" -> response status code
    if path == "status" || path == "response.status" {
        return Some(status.to_string());
    }

    // Shorthand: "$.headers.name" -> response header
    if let Some(header_name) = path.strip_prefix("$.headers.") {
        for (k, v) in headers {
            if k.to_lowercase() == header_name.to_lowercase() {
                return Some(v.clone());
            }
        }
        return None;
    }
    // Long form: "response.headers.name"
    if let Some(header_name) = path.strip_prefix("response.headers.") {
        for (k, v) in headers {
            if k.to_lowercase() == header_name.to_lowercase() {
                return Some(v.clone());
            }
        }
        return None;
    }

    if path == "response.body" {
        return Some(body.to_string());
    }

    // Shorthand: "$.field.path" -> JSON body navigation
    if let Some(json_path) = path.strip_prefix("$.") {
        let json: serde_json::Value = serde_json::from_str(body).ok()?;
        return navigate_json(&json, json_path);
    }

    // Long form: "response.body.field.path"
    if let Some(json_path) = path.strip_prefix("response.body") {
        let json: serde_json::Value = serde_json::from_str(body).ok()?;
        let remaining = json_path.strip_prefix('.').unwrap_or(json_path);
        return navigate_json(&json, remaining);
    }

    None
}

// ── JSON navigation ──────────────────────────────────────────────────

enum PathOp {
    Field(String),
    Index(usize),
}

fn parse_path_ops(path: &str) -> Vec<PathOp> {
    let mut ops = Vec::new();
    let mut remaining = path;

    while !remaining.is_empty() {
        if remaining.starts_with('[') {
            if let Some(end) = remaining.find(']') {
                if let Ok(idx) = remaining[1..end].parse::<usize>() {
                    ops.push(PathOp::Index(idx));
                }
                remaining = &remaining[end + 1..];
                if remaining.starts_with('.') {
                    remaining = &remaining[1..];
                }
            } else {
                break;
            }
        } else {
            let end = remaining
                .find(|c: char| c == '.' || c == '[')
                .unwrap_or(remaining.len());
            let field = &remaining[..end];
            if !field.is_empty() {
                ops.push(PathOp::Field(field.to_string()));
            }
            remaining = &remaining[end..];
            if remaining.starts_with('.') {
                remaining = &remaining[1..];
            }
        }
    }

    ops
}

fn navigate_json(value: &serde_json::Value, path: &str) -> Option<String> {
    if path.is_empty() {
        return Some(json_value_to_string(value));
    }

    let ops = parse_path_ops(path);
    let mut current = value.clone();

    for (i, op) in ops.iter().enumerate() {
        let is_last = i == ops.len() - 1;
        match op {
            PathOp::Field(name) if name == "length" && is_last => {
                return Some(get_length(&current).to_string());
            }
            PathOp::Field(name) => {
                current = current.get(name.as_str())?.clone();
            }
            PathOp::Index(idx) => {
                current = current.get(*idx)?.clone();
            }
        }
    }

    Some(json_value_to_string(&current))
}

fn json_value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => v.to_string(),
    }
}

fn get_length(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Array(arr) => arr.len(),
        serde_json::Value::String(s) => s.len(),
        _ => 0,
    }
}

// ── Comparison helpers ───────────────────────────────────────────────

fn numeric_eq(a: &str, b: &str) -> bool {
    if let (Ok(an), Ok(bn)) = (a.parse::<f64>(), b.parse::<f64>()) {
        (an - bn).abs() < f64::EPSILON
    } else {
        false
    }
}

fn numeric_cmp(left: &Option<String>, right: &str, cmp: fn(f64, f64) -> bool) -> bool {
    if let Some(l) = left {
        if let (Ok(ln), Ok(rn)) = (l.parse::<f64>(), right.parse::<f64>()) {
            return cmp(ln, rn);
        }
    }
    false
}

// ── Diff engine ──────────────────────────────────────────────────────

const MAX_DIFF_PATHS: usize = 1000;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct DiffResult {
    pub match_exact: bool,
    pub similarity: f64,
    pub is_json: bool,
    pub added_paths: Vec<String>,
    pub removed_paths: Vec<String>,
    pub changed_paths: Vec<ChangedField>,
    pub added_count: usize,
    pub removed_count: usize,
    pub changed_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChangedField {
    pub path: String,
    pub left: String,
    pub right: String,
}

/// Recursively collect all leaf paths and their string representations.
pub fn collect_json_paths(
    value: &serde_json::Value,
    prefix: &str,
    paths: &mut Vec<(String, String)>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                paths.push((prefix.to_string(), "{}".to_string()));
            }
            for (k, v) in map {
                let p = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", prefix, k)
                };
                collect_json_paths(v, &p, paths);
            }
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                paths.push((prefix.to_string(), "[]".to_string()));
            }
            for (i, v) in arr.iter().enumerate() {
                let p = format!("{}[{}]", prefix, i);
                collect_json_paths(v, &p, paths);
            }
        }
        _ => {
            paths.push((prefix.to_string(), json_value_to_string(value)));
        }
    }
}

/// Recursively normalize a JSON value for order-independent comparison.
/// Object keys are sorted lexicographically. Array elements are sorted by
/// their canonical JSON representation so `[2,1]` and `[1,2]` compare equal.
fn normalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                sorted.insert(k.clone(), normalize_json(&map[k]));
            }
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(arr) => {
            let mut normalized: Vec<serde_json::Value> =
                arr.iter().map(normalize_json).collect();
            normalized.sort_by(|a, b| {
                let sa = serde_json::to_string(a).unwrap_or_default();
                let sb = serde_json::to_string(b).unwrap_or_default();
                sa.cmp(&sb)
            });
            serde_json::Value::Array(normalized)
        }
        other => other.clone(),
    }
}

/// Normalize XML text for order-independent comparison.
/// Sorts attributes within each tag and trims inter-element whitespace.
fn normalize_xml(text: &str) -> String {
    // Regex to find opening tags with attributes
    let tag_re = regex::Regex::new(r#"<(\w[\w:.-]*)((?:\s+[\w:.-]+\s*=\s*(?:"[^"]*"|'[^']*'))*)\s*(/?)>"#).unwrap();
    let attr_re = regex::Regex::new(r#"([\w:.-]+)\s*=\s*("[^"]*"|'[^']*')"#).unwrap();

    let result = tag_re.replace_all(text, |caps: &regex::Captures| {
        let tag_name = &caps[1];
        let attrs_str = &caps[2];
        let self_close = &caps[3];

        if attrs_str.trim().is_empty() {
            return format!("<{}{}>", tag_name, if self_close.is_empty() { "" } else { " /" });
        }

        let mut attrs: Vec<(String, String)> = attr_re
            .captures_iter(attrs_str)
            .map(|m| (m[1].to_string(), m[2].to_string()))
            .collect();
        attrs.sort_by(|a, b| a.0.cmp(&b.0));

        let attrs_sorted = attrs
            .iter()
            .map(|(k, v)| format!(" {}={}", k, v))
            .collect::<String>();
        format!("<{}{}{}>", tag_name, attrs_sorted, if self_close.is_empty() { "" } else { " /" })
    });

    // Trim whitespace between tags for consistent comparison
    let ws_re = regex::Regex::new(r">\s+<").unwrap();
    ws_re.replace_all(&result, "><").to_string()
}

/// Compare two response bodies and produce a structured diff.
pub fn compute_diff(body_a: &str, body_b: &str) -> DiffResult {
    let json_a = serde_json::from_str::<serde_json::Value>(body_a);
    let json_b = serde_json::from_str::<serde_json::Value>(body_b);

    if let (Ok(va), Ok(vb)) = (json_a, json_b) {
        let na = normalize_json(&va);
        let nb = normalize_json(&vb);
        compute_json_diff(&na, &nb)
    } else if body_a.trim().starts_with('<') && body_b.trim().starts_with('<') {
        // Both look like XML — normalize before text comparison
        let norm_a = normalize_xml(body_a);
        let norm_b = normalize_xml(body_b);
        compute_text_diff(&norm_a, &norm_b)
    } else {
        compute_text_diff(body_a, body_b)
    }
}

fn compute_json_diff(va: &serde_json::Value, vb: &serde_json::Value) -> DiffResult {
    let mut paths_a: Vec<(String, String)> = Vec::new();
    let mut paths_b: Vec<(String, String)> = Vec::new();
    collect_json_paths(va, "", &mut paths_a);
    collect_json_paths(vb, "", &mut paths_b);

    let map_a: std::collections::HashMap<&str, &str> =
        paths_a.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let map_b: std::collections::HashMap<&str, &str> =
        paths_b.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    let mut added: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut changed: Vec<ChangedField> = Vec::new();
    let mut total_added = 0usize;
    let mut total_removed = 0usize;
    let mut total_changed = 0usize;

    for (path, val_a) in &map_a {
        match map_b.get(path) {
            Some(val_b) if val_a != val_b => {
                total_changed += 1;
                if changed.len() < MAX_DIFF_PATHS {
                    changed.push(ChangedField {
                        path: path.to_string(),
                        left: val_a.to_string(),
                        right: val_b.to_string(),
                    });
                }
            }
            None => {
                total_removed += 1;
                if removed.len() < MAX_DIFF_PATHS {
                    removed.push(path.to_string());
                }
            }
            _ => {}
        }
    }

    for path in map_b.keys() {
        if !map_a.contains_key(path) {
            total_added += 1;
            if added.len() < MAX_DIFF_PATHS {
                added.push(path.to_string());
            }
        }
    }

    added.sort();
    removed.sort();
    changed.sort_by(|a, b| a.path.cmp(&b.path));

    let added_count = total_added;
    let removed_count = total_removed;
    let changed_count = total_changed;

    let total_max = paths_a.len().max(paths_b.len()).max(1) as f64;
    let diff_count = (changed_count + added_count + removed_count) as f64;
    let similarity = (1.0 - diff_count / total_max).max(0.0);

    DiffResult {
        match_exact: added_count == 0 && removed_count == 0 && changed_count == 0,
        similarity,
        is_json: true,
        added_paths: added,
        removed_paths: removed,
        changed_paths: changed,
        added_count,
        removed_count,
        changed_count,
    }
}

fn compute_text_diff(body_a: &str, body_b: &str) -> DiffResult {
    let match_exact = body_a == body_b;
    let similarity = if match_exact {
        1.0
    } else {
        let chars_a: std::collections::HashSet<char> = body_a.chars().collect();
        let chars_b: std::collections::HashSet<char> = body_b.chars().collect();
        let intersection = chars_a.intersection(&chars_b).count() as f64;
        let union = chars_a.union(&chars_b).count() as f64;
        if union == 0.0 {
            1.0
        } else {
            intersection / union
        }
    };

    DiffResult {
        match_exact,
        similarity,
        is_json: false,
        ..Default::default()
    }
}

/// Resolve a `$diff.*` path against a DiffResult.
pub fn resolve_diff_value(path: &str, diff: &DiffResult) -> Option<String> {
    let key = path.strip_prefix("$diff.")?;

    match key {
        "match" => Some(diff.match_exact.to_string()),
        "similarity" => Some(format!("{:.4}", diff.similarity)),
        "is_json" => Some(diff.is_json.to_string()),
        "added_count" => Some(diff.added_count.to_string()),
        "removed_count" => Some(diff.removed_count.to_string()),
        "changed_count" => Some(diff.changed_count.to_string()),
        "added_paths" => Some(serde_json::to_string(&diff.added_paths).unwrap_or_default()),
        "removed_paths" => Some(serde_json::to_string(&diff.removed_paths).unwrap_or_default()),
        "changed_paths" => {
            let paths: Vec<&str> = diff.changed_paths.iter().map(|c| c.path.as_str()).collect();
            Some(serde_json::to_string(&paths).unwrap_or_default())
        }
        "added_paths.length" => Some(diff.added_paths.len().to_string()),
        "removed_paths.length" => Some(diff.removed_paths.len().to_string()),
        "changed_paths.length" => Some(diff.changed_paths.len().to_string()),
        _ => resolve_changed_path_index(key, diff),
    }
}

fn resolve_changed_path_index(key: &str, diff: &DiffResult) -> Option<String> {
    let rest = key.strip_prefix("changed_paths[")?;
    let bracket_end = rest.find(']')?;
    let idx: usize = rest[..bracket_end].parse().ok()?;
    let field = &rest[bracket_end + 1..];

    let entry = diff.changed_paths.get(idx)?;
    match field {
        ".path" => Some(entry.path.clone()),
        ".left" => Some(entry.left.clone()),
        ".right" => Some(entry.right.clone()),
        _ => None,
    }
}

/// Evaluate an assertion whose left side references `$diff.*`.
pub fn evaluate_with_diff(assertion: &Assertion, diff: &DiffResult) -> AssertionResult {
    let left_value = resolve_diff_value(&assertion.left, diff);
    let right_clean = assertion.right.trim_matches('"').to_string();
    let right_is_null = right_clean == "null";

    let passed = match assertion.operator.as_str() {
        "==" => {
            if right_is_null {
                left_value.is_none() || left_value.as_deref() == Some("null")
            } else {
                match &left_value {
                    Some(l) => l == &right_clean || numeric_eq(l, &right_clean),
                    None => false,
                }
            }
        }
        "!=" => {
            if right_is_null {
                left_value.is_some() && left_value.as_deref() != Some("null")
            } else {
                match &left_value {
                    Some(l) => l != &right_clean && !numeric_eq(l, &right_clean),
                    None => true,
                }
            }
        }
        ">" => numeric_cmp(&left_value, &right_clean, |a, b| a > b),
        "<" => numeric_cmp(&left_value, &right_clean, |a, b| a < b),
        ">=" => numeric_cmp(&left_value, &right_clean, |a, b| a >= b),
        "<=" => numeric_cmp(&left_value, &right_clean, |a, b| a <= b),
        "contains" => match &left_value {
            Some(l) => l.contains(&right_clean),
            None => false,
        },
        _ => false,
    };

    AssertionResult {
        assertion: format!("{} {} {}", assertion.left, assertion.operator, assertion.right),
        passed,
        actual: left_value,
        expected: Some(assertion.right.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_parser::Assertion;

    fn make_assertion(left: &str, op: &str, right: &str) -> Assertion {
        Assertion {
            left: left.to_string(),
            operator: op.to_string(),
            right: right.to_string(),
        }
    }

    fn empty_headers() -> Vec<(String, String)> {
        Vec::new()
    }

    // ── Status checks ────────────────────────────────────────────────
    #[test]
    fn status_eq_200_pass() {
        let a = make_assertion("response.status", "==", "200");
        let r = evaluate(&a, 200, &empty_headers(), "");
        assert!(r.passed);
    }

    #[test]
    fn status_eq_200_fail_on_404() {
        let a = make_assertion("response.status", "==", "200");
        let r = evaluate(&a, 404, &empty_headers(), "");
        assert!(!r.passed);
    }

    #[test]
    fn status_ne_404_pass() {
        let a = make_assertion("response.status", "!=", "404");
        let r = evaluate(&a, 200, &empty_headers(), "");
        assert!(r.passed);
    }

    #[test]
    fn status_ne_404_fail() {
        let a = make_assertion("response.status", "!=", "404");
        let r = evaluate(&a, 404, &empty_headers(), "");
        assert!(!r.passed);
    }

    #[test]
    fn status_gt_199_pass() {
        let a = make_assertion("response.status", ">", "199");
        let r = evaluate(&a, 200, &empty_headers(), "");
        assert!(r.passed);
    }

    #[test]
    fn status_lt_300_pass() {
        let a = make_assertion("response.status", "<", "300");
        let r = evaluate(&a, 200, &empty_headers(), "");
        assert!(r.passed);
    }

    #[test]
    fn status_gte_200_pass() {
        let a = make_assertion("response.status", ">=", "200");
        let r = evaluate(&a, 200, &empty_headers(), "");
        assert!(r.passed);
    }

    #[test]
    fn status_lte_200_pass() {
        let a = make_assertion("response.status", "<=", "200");
        let r = evaluate(&a, 200, &empty_headers(), "");
        assert!(r.passed);
    }

    // ── Body JSON access ─────────────────────────────────────────────
    #[test]
    fn body_token_ne_null_pass() {
        let a = make_assertion("response.body.token", "!=", "null");
        let r = evaluate(&a, 200, &empty_headers(), r#"{"token":"abc"}"#);
        assert!(r.passed);
    }

    #[test]
    fn body_token_eq_null_when_missing() {
        let a = make_assertion("response.body.token", "==", "null");
        let r = evaluate(&a, 200, &empty_headers(), r#"{"other":"val"}"#);
        assert!(r.passed);
    }

    #[test]
    fn body_array_length_gt_0() {
        let a = make_assertion("response.body.items.length", ">", "0");
        let r = evaluate(&a, 200, &empty_headers(), r#"{"items":[1,2,3]}"#);
        assert!(r.passed);
    }

    #[test]
    fn body_array_index_access() {
        let a = make_assertion("response.body[0].name", "==", "\"test\"");
        let r = evaluate(&a, 200, &empty_headers(), r#"[{"name":"test"}]"#);
        assert!(r.passed);
    }

    // ── contains operator ────────────────────────────────────────────
    #[test]
    fn body_contains_pass() {
        let a = make_assertion("response.body", "contains", "hello");
        let r = evaluate(&a, 200, &empty_headers(), "hello world");
        assert!(r.passed);
    }

    #[test]
    fn body_contains_fail() {
        let a = make_assertion("response.body", "contains", "missing");
        let r = evaluate(&a, 200, &empty_headers(), "hello world");
        assert!(!r.passed);
    }

    // ── Header checks ────────────────────────────────────────────────
    #[test]
    fn header_contains_json() {
        let headers = vec![("content-type".to_string(), "application/json; charset=utf-8".to_string())];
        let a = make_assertion("response.headers.content-type", "contains", "json");
        let r = evaluate(&a, 200, &headers, "");
        assert!(r.passed);
    }

    // ── Nested JSON ──────────────────────────────────────────────────
    #[test]
    fn nested_json_access() {
        let body = r#"{"user":{"profile":{"name":"John"}}}"#;
        let a = make_assertion("response.body.user.profile.name", "==", "\"John\"");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    // ── Numeric comparison with string status ────────────────────────
    #[test]
    fn numeric_comparison_status_string() {
        let a = make_assertion("response.status", ">", "100");
        let r = evaluate(&a, 200, &empty_headers(), "");
        assert!(r.passed);
        assert_eq!(r.actual.as_deref(), Some("200"));
    }

    // ── body == null when body is "null" ─────────────────────────────
    #[test]
    fn body_eq_null_when_body_is_null_string() {
        let a = make_assertion("response.body", "==", "null");
        let r = evaluate(&a, 200, &empty_headers(), "null");
        assert!(r.passed);
    }

    // ── resolve_response_value for status ────────────────────────────
    #[test]
    fn resolve_status() {
        let val = resolve_response_value("response.status", 404, &[], "");
        assert_eq!(val, Some("404".to_string()));
    }

    // ── resolve_response_value for missing header ────────────────────
    #[test]
    fn resolve_missing_header() {
        let val = resolve_response_value("response.headers.x-missing", 200, &[], "");
        assert_eq!(val, None);
    }

    // ── resolve_response_value for body ──────────────────────────────
    #[test]
    fn resolve_body_plain() {
        let val = resolve_response_value("response.body", 200, &[], "hello");
        assert_eq!(val, Some("hello".to_string()));
    }

    // ── Shorthand $ syntax for JSON body paths ──────────────────────
    #[test]
    fn shorthand_status() {
        let a = make_assertion("status", "==", "200");
        let r = evaluate(&a, 200, &empty_headers(), "");
        assert!(r.passed);
    }

    #[test]
    fn shorthand_body_field() {
        let body = r#"{"name":"test","count":42}"#;
        let a = make_assertion("$.name", "==", "test");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    #[test]
    fn shorthand_nested_body() {
        let body = r#"{"properties":{"metrics":{"endpoint":"https://example.com"}}}"#;
        let val = resolve_response_value("$.properties.metrics.endpoint", 200, &[], body);
        assert_eq!(val, Some("https://example.com".to_string()));
    }

    #[test]
    fn shorthand_header() {
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        let a = make_assertion("$.headers.content-type", "contains", "json");
        let r = evaluate(&a, 200, &headers, "");
        assert!(r.passed);
    }

    #[test]
    fn shorthand_array_length() {
        let body = r#"{"items":[1,2,3]}"#;
        let a = make_assertion("$.items.length", ">", "0");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    #[test]
    fn shorthand_array_index() {
        let body = r#"{"items":[{"id":"a"},{"id":"b"}]}"#;
        let val = resolve_response_value("$.items[0].id", 200, &[], body);
        assert_eq!(val, Some("a".to_string()));
    }

    // ── compute_diff tests ──────────────────────────

    #[test]
    fn diff_identical_json() {
        let body = r#"{"name":"Alice","age":30}"#;
        let result = compute_diff(body, body);
        assert!(result.match_exact);
        assert!(result.is_json);
        assert_eq!(result.similarity, 1.0);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.changed_count, 0);
    }

    #[test]
    fn diff_json_with_added_field() {
        let a = r#"{"name":"Alice"}"#;
        let b = r#"{"name":"Alice","age":30}"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert!(result.is_json);
        assert_eq!(result.added_count, 1);
        assert!(result.added_paths.contains(&"age".to_string()));
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.changed_count, 0);
    }

    #[test]
    fn diff_json_with_removed_field() {
        let a = r#"{"name":"Alice","age":30}"#;
        let b = r#"{"name":"Alice"}"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert_eq!(result.removed_count, 1);
        assert!(result.removed_paths.contains(&"age".to_string()));
        assert_eq!(result.added_count, 0);
    }

    #[test]
    fn diff_json_with_changed_value() {
        let a = r#"{"name":"Alice","age":30}"#;
        let b = r#"{"name":"Bob","age":30}"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert_eq!(result.changed_count, 1);
        assert_eq!(result.changed_paths[0].path, "name");
        assert_eq!(result.changed_paths[0].left, "Alice");
        assert_eq!(result.changed_paths[0].right, "Bob");
    }

    #[test]
    fn diff_nested_json() {
        let a = r#"{"user":{"name":"Alice","address":{"city":"NYC"}}}"#;
        let b = r#"{"user":{"name":"Alice","address":{"city":"LA"}}}"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert_eq!(result.changed_count, 1);
        assert_eq!(result.changed_paths[0].path, "user.address.city");
    }

    #[test]
    fn diff_json_arrays_order_independent() {
        // Arrays with same elements in different order should match after normalization
        let a = r#"{"items":[1,2,3]}"#;
        let b = r#"{"items":[1,3,2]}"#;
        let result = compute_diff(a, b);
        assert!(result.match_exact, "arrays with same elements in different order should match");
        assert_eq!(result.changed_count, 0);
    }

    #[test]
    fn diff_json_arrays_different_elements() {
        // Arrays with genuinely different elements should NOT match
        let a = r#"{"items":[1,2,3]}"#;
        let b = r#"{"items":[1,4,3]}"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert!(result.changed_count >= 1);
    }

    #[test]
    fn diff_identical_text() {
        let body = "Hello World";
        let result = compute_diff(body, body);
        assert!(result.match_exact);
        assert!(!result.is_json);
        assert_eq!(result.similarity, 1.0);
    }

    #[test]
    fn diff_different_text() {
        let a = "Hello World";
        let b = "Goodbye World";
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert!(!result.is_json);
        assert!(result.similarity > 0.0);
        assert!(result.similarity < 1.0);
    }

    #[test]
    fn diff_empty_bodies() {
        let result = compute_diff("", "");
        assert!(result.match_exact);
        assert_eq!(result.similarity, 1.0);
    }

    #[test]
    fn diff_counts_accurate_beyond_max_diff_paths() {
        // Build two JSON objects where every key has a different value,
        // plus object B has extra keys — totals exceed MAX_DIFF_PATHS.
        let n = MAX_DIFF_PATHS + 500; // changed paths
        let extra = 200; // added paths (only in B)

        let mut obj_a = serde_json::Map::new();
        let mut obj_b = serde_json::Map::new();

        for i in 0..n {
            let key = format!("key_{i}");
            obj_a.insert(key.clone(), serde_json::Value::String(format!("a_{i}")));
            obj_b.insert(key, serde_json::Value::String(format!("b_{i}")));
        }
        for i in 0..extra {
            obj_b.insert(
                format!("extra_{i}"),
                serde_json::Value::String(format!("new_{i}")),
            );
        }

        let body_a = serde_json::to_string(&serde_json::Value::Object(obj_a)).unwrap();
        let body_b = serde_json::to_string(&serde_json::Value::Object(obj_b)).unwrap();

        let result = compute_diff(&body_a, &body_b);

        // Counts must reflect the true totals, not the capped vectors.
        assert_eq!(result.changed_count, n, "changed_count should be exact");
        assert_eq!(result.added_count, extra, "added_count should be exact");
        assert_eq!(result.removed_count, 0);

        // Preview vectors are capped at MAX_DIFF_PATHS.
        assert!(result.changed_paths.len() <= MAX_DIFF_PATHS);
        assert!(result.added_paths.len() <= MAX_DIFF_PATHS);

        // Similarity must use the accurate totals.
        let total_max = (n as f64).max((n + extra) as f64).max(1.0);
        let expected_sim = (1.0 - (n + extra) as f64 / total_max).max(0.0);
        assert!(
            (result.similarity - expected_sim).abs() < 1e-9,
            "similarity should use accurate counters: got {} expected {}",
            result.similarity,
            expected_sim
        );
    }

    // ── resolve_diff_value tests ────────────────────

    #[test]
    fn resolve_diff_match_true() {
        let diff = DiffResult {
            match_exact: true,
            similarity: 1.0,
            is_json: true,
            ..Default::default()
        };
        assert_eq!(resolve_diff_value("$diff.match", &diff), Some("true".to_string()));
        assert_eq!(resolve_diff_value("$diff.similarity", &diff), Some("1.0000".to_string()));
        assert_eq!(resolve_diff_value("$diff.is_json", &diff), Some("true".to_string()));
    }

    #[test]
    fn resolve_diff_counts() {
        let diff = DiffResult {
            added_count: 2,
            removed_count: 1,
            changed_count: 3,
            added_paths: vec!["a".into(), "b".into()],
            removed_paths: vec!["c".into()],
            changed_paths: vec![
                ChangedField { path: "x".into(), left: "1".into(), right: "2".into() },
                ChangedField { path: "y".into(), left: "a".into(), right: "b".into() },
                ChangedField { path: "z".into(), left: "old".into(), right: "new".into() },
            ],
            ..Default::default()
        };
        assert_eq!(resolve_diff_value("$diff.added_count", &diff), Some("2".to_string()));
        assert_eq!(resolve_diff_value("$diff.removed_count", &diff), Some("1".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_count", &diff), Some("3".to_string()));
        assert_eq!(resolve_diff_value("$diff.added_paths.length", &diff), Some("2".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_paths.length", &diff), Some("3".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_paths[0].path", &diff), Some("x".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_paths[1].left", &diff), Some("a".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_paths[2].right", &diff), Some("new".to_string()));
    }

    #[test]
    fn resolve_diff_unknown_path_returns_none() {
        let diff = DiffResult::default();
        assert_eq!(resolve_diff_value("$diff.nonexistent", &diff), None);
        assert_eq!(resolve_diff_value("$something.else", &diff), None);
    }

    // ── evaluate_with_diff tests ────────────────────

    #[test]
    fn evaluate_diff_assertion_match_true() {
        let diff = DiffResult { match_exact: true, ..Default::default() };
        let assertion = Assertion {
            left: "$diff.match".to_string(),
            operator: "==".to_string(),
            right: "true".to_string(),
        };
        let result = evaluate_with_diff(&assertion, &diff);
        assert!(result.passed);
    }

    #[test]
    fn evaluate_diff_assertion_changed_count() {
        let diff = DiffResult { changed_count: 3, ..Default::default() };
        let assertion = Assertion {
            left: "$diff.changed_count".to_string(),
            operator: "==".to_string(),
            right: "3".to_string(),
        };
        let result = evaluate_with_diff(&assertion, &diff);
        assert!(result.passed);
    }

    #[test]
    fn evaluate_diff_assertion_similarity_gte() {
        let diff = DiffResult { similarity: 0.95, ..Default::default() };
        let assertion = Assertion {
            left: "$diff.similarity".to_string(),
            operator: ">=".to_string(),
            right: "0.9".to_string(),
        };
        let result = evaluate_with_diff(&assertion, &diff);
        assert!(result.passed);
    }

    // ── Diff engine edge cases ──────────────────────

    #[test]
    fn diff_empty_json_objects() {
        let result = compute_diff("{}", "{}");
        assert!(result.match_exact, "two empty objects should be exact match");
        assert!(result.is_json);
        assert_eq!(result.similarity, 1.0);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.changed_count, 0);
    }

    #[test]
    fn diff_empty_vs_nonempty() {
        let result = compute_diff("{}", r#"{"a":1}"#);
        assert!(!result.match_exact);
        assert!(result.is_json);
        // Empty object has a sentinel leaf ("", "{}"), so it counts as removed
        // while "a" counts as added — net result: 1 added + 1 removed
        assert_eq!(result.added_count, 1, "one field was added");
        assert_eq!(result.removed_count, 1, "empty-object sentinel was removed");
        assert_eq!(result.changed_count, 0);
        assert!(result.added_paths.contains(&"a".to_string()));
    }

    #[test]
    fn diff_nested_array_diff() {
        let a = r#"{"items":[1,2,3]}"#;
        let b = r#"{"items":[1,2]}"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert!(result.is_json);
        // items[2] exists in a but not b → removed
        assert!(result.removed_count >= 1, "shorter array should have removed paths");
    }

    #[test]
    fn diff_deeply_nested_paths() {
        let a = r#"{"l1":{"l2":{"l3":{"l4":{"l5":"deep_a"}}}}}"#;
        let b = r#"{"l1":{"l2":{"l3":{"l4":{"l5":"deep_b"}}}}}"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert_eq!(result.changed_count, 1);
        assert_eq!(result.changed_paths[0].path, "l1.l2.l3.l4.l5");
        assert_eq!(result.changed_paths[0].left, "deep_a");
        assert_eq!(result.changed_paths[0].right, "deep_b");
    }

    #[test]
    fn diff_non_json_text() {
        let a = "plain text response A";
        let b = "plain text response B";
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert!(!result.is_json, "non-JSON should have is_json=false");
        assert!(result.similarity > 0.0, "similar text should have positive similarity");
        assert!(result.similarity < 1.0, "different text should have similarity < 1");
        // Text diff should not have path-level info
        assert!(result.added_paths.is_empty());
        assert!(result.removed_paths.is_empty());
        assert!(result.changed_paths.is_empty());
    }

    #[test]
    fn diff_mixed_types() {
        let a = r#"{"val":"hello"}"#;
        let b = r#"{"val":42}"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert_eq!(result.changed_count, 1, "same key with different type should be a change");
        assert_eq!(result.changed_paths[0].path, "val");
        assert_eq!(result.changed_paths[0].left, "hello");
        assert_eq!(result.changed_paths[0].right, "42");
    }

    #[test]
    fn diff_null_values() {
        // null vs value
        let a = r#"{"x":null}"#;
        let b = r#"{"x":"present"}"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact);
        assert_eq!(result.changed_count, 1);
        assert_eq!(result.changed_paths[0].path, "x");

        // value vs null
        let result2 = compute_diff(b, a);
        assert!(!result2.match_exact);
        assert_eq!(result2.changed_count, 1);
    }

    #[test]
    fn resolve_diff_value_all_paths() {
        let diff = DiffResult {
            match_exact: false,
            similarity: 0.8765,
            is_json: true,
            added_count: 2,
            removed_count: 1,
            changed_count: 1,
            added_paths: vec!["new_a".into(), "new_b".into()],
            removed_paths: vec!["old_c".into()],
            changed_paths: vec![
                ChangedField {
                    path: "name".into(),
                    left: "Alice".into(),
                    right: "Bob".into(),
                },
            ],
        };

        assert_eq!(resolve_diff_value("$diff.match", &diff), Some("false".to_string()));
        assert_eq!(resolve_diff_value("$diff.similarity", &diff), Some("0.8765".to_string()));
        assert_eq!(resolve_diff_value("$diff.is_json", &diff), Some("true".to_string()));
        assert_eq!(resolve_diff_value("$diff.added_count", &diff), Some("2".to_string()));
        assert_eq!(resolve_diff_value("$diff.removed_count", &diff), Some("1".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_count", &diff), Some("1".to_string()));
        assert_eq!(resolve_diff_value("$diff.added_paths.length", &diff), Some("2".to_string()));
        assert_eq!(resolve_diff_value("$diff.removed_paths.length", &diff), Some("1".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_paths.length", &diff), Some("1".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_paths[0].path", &diff), Some("name".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_paths[0].left", &diff), Some("Alice".to_string()));
        assert_eq!(resolve_diff_value("$diff.changed_paths[0].right", &diff), Some("Bob".to_string()));

        // Verify JSON array serialization for paths
        let added_json = resolve_diff_value("$diff.added_paths", &diff).unwrap();
        assert!(added_json.contains("new_a"));
        assert!(added_json.contains("new_b"));
    }

    #[test]
    fn resolve_diff_value_invalid_path() {
        let diff = DiffResult::default();
        assert_eq!(resolve_diff_value("$diff.nonexistent_field", &diff), None);
        assert_eq!(resolve_diff_value("$something.else", &diff), None);
        assert_eq!(resolve_diff_value("no_prefix", &diff), None);
        assert_eq!(resolve_diff_value("$diff.changed_paths[99].path", &diff), None);
        assert_eq!(resolve_diff_value("$diff.changed_paths[0].unknown", &diff), None);
    }

    // --- Normalization tests ---

    #[test]
    fn diff_json_object_key_order_independent() {
        let a = r#"{"z":3,"a":1,"m":2}"#;
        let b = r#"{"a":1,"m":2,"z":3}"#;
        let result = compute_diff(a, b);
        assert!(result.match_exact, "same keys in different order should match");
        assert_eq!(result.changed_count, 0);
    }

    #[test]
    fn diff_json_nested_key_order_independent() {
        let a = r#"{"outer":{"z":1,"a":2},"list":[{"b":2,"a":1}]}"#;
        let b = r#"{"list":[{"a":1,"b":2}],"outer":{"a":2,"z":1}}"#;
        let result = compute_diff(a, b);
        assert!(result.match_exact, "deeply nested reordered keys should match");
    }

    #[test]
    fn diff_json_array_of_objects_order_independent() {
        let a = r#"[{"id":2,"name":"b"},{"id":1,"name":"a"}]"#;
        let b = r#"[{"id":1,"name":"a"},{"id":2,"name":"b"}]"#;
        let result = compute_diff(a, b);
        assert!(result.match_exact, "array of objects in different order should match");
    }

    #[test]
    fn diff_json_array_duplicate_elements() {
        let a = r#"[1,1,2]"#;
        let b = r#"[1,2,1]"#;
        let result = compute_diff(a, b);
        assert!(result.match_exact, "same multiset in different order should match");
    }

    #[test]
    fn diff_json_array_different_counts() {
        let a = r#"[1,1,2]"#;
        let b = r#"[1,2,2]"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact, "different element counts should not match");
    }

    #[test]
    fn diff_xml_attribute_order_independent() {
        let a = r#"<root><item id="1" name="test" /></root>"#;
        let b = r#"<root><item name="test" id="1" /></root>"#;
        let result = compute_diff(a, b);
        assert!(result.match_exact, "XML with reordered attributes should match");
        assert!(!result.is_json);
    }

    #[test]
    fn diff_xml_whitespace_independent() {
        let a = "<root>\n  <item>hello</item>\n</root>";
        let b = "<root><item>hello</item></root>";
        let result = compute_diff(a, b);
        assert!(result.match_exact, "XML with different whitespace should match");
    }

    #[test]
    fn diff_xml_different_content() {
        let a = r#"<root><item id="1">hello</item></root>"#;
        let b = r#"<root><item id="2">world</item></root>"#;
        let result = compute_diff(a, b);
        assert!(!result.match_exact, "XML with different content should not match");
    }

    #[test]
    fn normalize_json_preserves_values() {
        let a = r#"{"name":"Alice","age":30,"active":true,"score":null}"#;
        let b = r#"{"score":null,"active":true,"name":"Alice","age":30}"#;
        let result = compute_diff(a, b);
        assert!(result.match_exact);
        assert_eq!(result.similarity, 1.0);
    }
}
