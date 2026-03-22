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
}
