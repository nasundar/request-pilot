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
    let nav = resolve_response_value_multi(&assertion.left, status, headers, body);
    let right_clean = assertion.right.trim_matches('"').to_string();
    let right_is_null = right_clean == "null";
    let op = assertion.operator.as_str();

    let (passed, actual) = if !nav.had_wildcard {
        // Single-value path — collapse the (necessarily-singleton) branches
        // Vec into the legacy `Option<String>` shape and run the original
        // comparator.
        let single = match nav.branches.into_iter().next() {
            Some(BranchValue::Value(s)) => Some(s),
            _ => None,
        };
        let p = compare_one(&single, &right_clean, right_is_null, op);
        (p, single)
    } else {
        // Wildcard path — universal quantification with vacuous truth on
        // empty arrays. The runner expanded the wildcard into N branches;
        // EVERY branch must individually satisfy the comparator.
        if nav.branches.is_empty() {
            // Empty array at wildcard → vacuously true. Common pattern:
            // `$.data.result[*].field != null` passes when result is [],
            // which is the "skip when no data" behavior.
            (true, Some("[empty array]".to_string()))
        } else {
            let mut all_passed = true;
            let mut first_fail_actual: Option<String> = None;
            for (i, branch) in nav.branches.iter().enumerate() {
                let single = match branch {
                    BranchValue::Value(s) => Some(s.clone()),
                    BranchValue::Missing | BranchValue::NotArray => None,
                };
                let p = compare_one(&single, &right_clean, right_is_null, op);
                if !p && first_fail_actual.is_none() {
                    let label = match branch {
                        BranchValue::Value(s) => s.clone(),
                        BranchValue::Missing => "<missing>".to_string(),
                        BranchValue::NotArray => "<not an array>".to_string(),
                    };
                    first_fail_actual = Some(format!("[{}] {}", i, label));
                }
                if !p {
                    all_passed = false;
                }
            }
            let actual_summary = if all_passed {
                Some(format!("[{} values]", nav.branches.len()))
            } else {
                first_fail_actual
            };
            (all_passed, actual_summary)
        }
    };

    AssertionResult {
        assertion: format!("{} {} {}", assertion.left, assertion.operator, assertion.right),
        passed,
        actual,
        expected: Some(assertion.right.clone()),
    }
}

/// Apply a comparator op to a single (possibly-missing) value. Extracted
/// from `evaluate` so wildcard branches and single-value paths share the
/// same comparison semantics.
fn compare_one(left_value: &Option<String>, right_clean: &str, right_is_null: bool, op: &str) -> bool {
    match op {
        "==" => {
            if right_is_null {
                left_value.is_none() || left_value.as_deref() == Some("null")
            } else {
                match left_value {
                    Some(l) => l == right_clean || numeric_eq(l, right_clean),
                    None => false,
                }
            }
        }
        "!=" => {
            if right_is_null {
                left_value.is_some() && left_value.as_deref() != Some("null")
            } else {
                match left_value {
                    Some(l) => l != right_clean && !numeric_eq(l, right_clean),
                    None => true,
                }
            }
        }
        ">" => numeric_cmp(left_value, right_clean, |a, b| a > b),
        "<" => numeric_cmp(left_value, right_clean, |a, b| a < b),
        ">=" => numeric_cmp(left_value, right_clean, |a, b| a >= b),
        "<=" => numeric_cmp(left_value, right_clean, |a, b| a <= b),
        "contains" => match left_value {
            Some(l) => l.contains(right_clean),
            None => false,
        },
        _ => false,
    }
}

/// Resolve a dotted path against a response (status, headers, body).
/// Public so the test runner can reuse it for extract evaluation.
///
/// Single-value path resolver. Wildcards are NOT supported here — they
/// only make sense in assertion contexts where universal quantification
/// has a defined meaning. If a wildcard path is passed, returns `None`
/// (consistent with "value can't be resolved to one thing").
pub fn resolve_response_value(
    path: &str,
    status: u16,
    headers: &[(String, String)],
    body: &str,
) -> Option<String> {
    let nav = resolve_response_value_multi(path, status, headers, body);
    if nav.had_wildcard {
        // Wildcard paths can't collapse to a single value. Extract /
        // response-chain consumers should use a non-wildcard path or
        // pre-extract a specific index.
        return None;
    }
    match nav.branches.into_iter().next() {
        Some(BranchValue::Value(s)) => Some(s),
        _ => None,
    }
}

/// Same as `resolve_response_value` but exposes wildcard branches. Used
/// internally by `evaluate` to apply universal quantification.
fn resolve_response_value_multi(
    path: &str,
    status: u16,
    headers: &[(String, String)],
    body: &str,
) -> NavigationResult {
    // Status / header / response.body shorthands have no wildcard semantics
    // — they always resolve to a single value (or nothing). Wrap each
    // result in a singleton NavigationResult.
    if path == "status" || path == "response.status" {
        return NavigationResult::single(BranchValue::Value(status.to_string()));
    }

    if let Some(header_name) = path.strip_prefix("$.headers.") {
        for (k, v) in headers {
            if k.to_lowercase() == header_name.to_lowercase() {
                return NavigationResult::single(BranchValue::Value(v.clone()));
            }
        }
        return NavigationResult::single(BranchValue::Missing);
    }
    if let Some(header_name) = path.strip_prefix("response.headers.") {
        for (k, v) in headers {
            if k.to_lowercase() == header_name.to_lowercase() {
                return NavigationResult::single(BranchValue::Value(v.clone()));
            }
        }
        return NavigationResult::single(BranchValue::Missing);
    }
    // Bracket-key forms for header names containing `.`, `-`, or other
    // chars not allowed in bare identifiers, e.g.
    // `$.headers["x-ms-azure-implicit-rollup-applied"]`. We reuse the
    // existing path-ops parser to get the same quoting/escaping rules as
    // JSON bodies. Without this branch the path falls through to JSON
    // body navigation and silently resolves to Missing.
    if let Some(rest) = path
        .strip_prefix("$.headers")
        .or_else(|| path.strip_prefix("response.headers"))
    {
        if rest.starts_with('[') {
            let ops = parse_path_ops(rest);
            if let Some(PathOp::Field(name)) = ops.first() {
                for (k, v) in headers {
                    if k.to_lowercase() == name.to_lowercase() {
                        return NavigationResult::single(BranchValue::Value(v.clone()));
                    }
                }
                return NavigationResult::single(BranchValue::Missing);
            }
        }
    }

    if path == "response.body" {
        return NavigationResult::single(BranchValue::Value(body.to_string()));
    }

    // JSON body navigation. `$.…` and `response.body.…` both delegate to
    // `navigate_json_all`, which handles wildcards and bracket-keys.
    if let Some(json_path) = path.strip_prefix("$.") {
        let json: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => return NavigationResult::single(BranchValue::Missing),
        };
        return navigate_json_all(&json, json_path);
    }

    if let Some(json_path) = path.strip_prefix("response.body") {
        let json: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => return NavigationResult::single(BranchValue::Missing),
        };
        let remaining = json_path.strip_prefix('.').unwrap_or(json_path);
        return navigate_json_all(&json, remaining);
    }

    NavigationResult::single(BranchValue::Missing)
}

// ── JSON navigation ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum PathOp {
    Field(String),
    Index(usize),
    /// `[*]` — fan out across array elements with universal quantification
    /// at the assertion layer.
    Wildcard,
}

/// Per-branch outcome of multi-value navigation.
#[derive(Debug, Clone, PartialEq)]
enum BranchValue {
    /// Path resolved to a value (JSON null is `Value("null")`).
    Value(String),
    /// A field/index along the path was absent.
    Missing,
    /// `[*]` was applied to a non-array (object/scalar/null).
    NotArray,
}

/// Result of a (possibly wildcarded) JSON path navigation.
#[derive(Debug, Clone)]
struct NavigationResult {
    /// One entry per branch produced by wildcard fan-out. For non-wildcard
    /// paths this is always exactly one entry. For `[*]` over an empty
    /// array this is empty Vec — the vacuous-truth case.
    branches: Vec<BranchValue>,
    had_wildcard: bool,
}

impl NavigationResult {
    fn single(b: BranchValue) -> Self {
        Self { branches: vec![b], had_wildcard: false }
    }
}

fn parse_path_ops(path: &str) -> Vec<PathOp> {
    let mut ops = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];
        if c == b'[' {
            // Three forms inside brackets:
            //   1. `[*]`             — wildcard
            //   2. `[<digits>]`      — numeric index
            //   3. `["…"]` / `['…']` — quoted bracket-key (field name with
            //      dots/spaces/special chars). `\` escapes the active
            //      delimiter and itself; `]` inside the quoted segment is
            //      treated literally.
            if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                // [*] — must be exactly that
                if i + 2 < bytes.len() && bytes[i + 2] == b']' {
                    ops.push(PathOp::Wildcard);
                    i += 3;
                    if i < bytes.len() && bytes[i] == b'.' {
                        i += 1;
                    }
                    continue;
                }
                // Malformed `[*…` — bail to avoid loop
                break;
            }
            if i + 1 < bytes.len() && (bytes[i + 1] == b'"' || bytes[i + 1] == b'\'') {
                let delim = bytes[i + 1];
                let mut j = i + 2;
                let mut field = String::new();
                let mut closed = false;
                while j < bytes.len() {
                    let ch = bytes[j];
                    if ch == b'\\' && j + 1 < bytes.len() {
                        let next = bytes[j + 1];
                        // Only the active delimiter and `\` are escaped;
                        // anything else passes through literally so users
                        // don't have to remember a long escape table.
                        if next == delim || next == b'\\' {
                            field.push(next as char);
                            j += 2;
                            continue;
                        }
                        field.push(ch as char);
                        j += 1;
                        continue;
                    }
                    if ch == delim {
                        closed = true;
                        j += 1;
                        break;
                    }
                    field.push(ch as char);
                    j += 1;
                }
                if !closed || j >= bytes.len() || bytes[j] != b']' {
                    // Malformed — bail rather than emit garbage.
                    break;
                }
                ops.push(PathOp::Field(field));
                i = j + 1;
                if i < bytes.len() && bytes[i] == b'.' {
                    i += 1;
                }
                continue;
            }
            // Numeric index
            let close = match path[i + 1..].find(']') {
                Some(rel) => i + 1 + rel,
                None => break,
            };
            if let Ok(idx) = path[i + 1..close].parse::<usize>() {
                ops.push(PathOp::Index(idx));
            }
            i = close + 1;
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
            }
            continue;
        }
        // Bare field name terminated by `.` or `[`
        let end = path[i..]
            .find(|c: char| c == '.' || c == '[')
            .map(|rel| i + rel)
            .unwrap_or(bytes.len());
        if end > i {
            ops.push(PathOp::Field(path[i..end].to_string()));
        }
        i = end;
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
        }
    }

    ops
}

/// Recursive multi-branch navigation. Each `PathOp::Wildcard` step fans
/// out into N child branches (one per array element); subsequent steps
/// run independently on each branch. `Missing` and `NotArray` branches
/// short-circuit further navigation in that branch — they can only
/// produce themselves.
fn navigate_json_all(value: &serde_json::Value, path: &str) -> NavigationResult {
    let ops = parse_path_ops(path);
    let had_wildcard = ops.iter().any(|op| matches!(op, PathOp::Wildcard));
    let branches = navigate_recurse(value, &ops);
    NavigationResult { branches, had_wildcard }
}

fn navigate_recurse(value: &serde_json::Value, ops: &[PathOp]) -> Vec<BranchValue> {
    if ops.is_empty() {
        return vec![BranchValue::Value(json_value_to_string(value))];
    }
    let (op, rest) = ops.split_first().unwrap();
    match op {
        // `length` is a terminal pseudo-field — only meaningful at the
        // very end of the path. If the user writes `.length.foo` we treat
        // length as a literal field name (which won't exist) and fail
        // navigation there, matching legacy behavior.
        PathOp::Field(name) if name == "length" && rest.is_empty() => {
            vec![BranchValue::Value(get_length(value).to_string())]
        }
        PathOp::Field(name) => match value.get(name.as_str()) {
            Some(child) => navigate_recurse(child, rest),
            None => vec![BranchValue::Missing],
        },
        PathOp::Index(idx) => match value.get(*idx) {
            Some(child) => navigate_recurse(child, rest),
            None => vec![BranchValue::Missing],
        },
        PathOp::Wildcard => match value.as_array() {
            Some(arr) if arr.is_empty() => Vec::new(),
            Some(arr) => arr
                .iter()
                .flat_map(|elem| navigate_recurse(elem, rest))
                .collect(),
            None => vec![BranchValue::NotArray],
        },
    }
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
    // ── Weighted comparison fields (V1.4) ────────────────────────────
    // All defaulted so older serialized DiffResult JSON (history /
    // run.json) still deserializes cleanly.
    /// Paths that hit a `# @@diff_strict` rule and actually differed.
    /// Non-empty ⇒ the pair fails regardless of tolerance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strict_violations: Vec<String>,
    /// One entry per differing path that survived strict matching,
    /// carrying the weight charged to `weighted_score`. Truncated at
    /// `MAX_DIFF_PATHS` for display; the score itself is computed from
    /// the full uncapped enumeration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weighted_paths: Vec<WeightedDiffPath>,
    /// Sum of weights across every non-strict differing path.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub weighted_score: f64,
    /// Sum of weights across every non-strict path on EITHER side.
    /// Defines the denominator of `weighted_ratio` so the metric is
    /// comparable across runs of the same suite.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub weighted_total: f64,
    /// `weighted_score / weighted_total` (0.0 when total is 0).
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub weighted_ratio: f64,
    /// Echo of the configured tolerance for display / round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<DiffTolerance>,
    /// `None` when no tolerance was configured. `Some(true)` when the
    /// weighted score stayed within budget. `Some(false)` when it
    /// exceeded budget (or rules could not be applied — see
    /// `rules_applied`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance_passed: Option<bool>,
    /// `None` when no rules were supplied. `Some(true)` when rules ran
    /// against valid JSON on both sides. `Some(false)` when one or both
    /// bodies were non-JSON — runner treats this as a hard failure so a
    /// silent text-mode pass is impossible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_applied: Option<bool>,
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChangedField {
    pub path: String,
    pub left: String,
    pub right: String,
}

/// Tolerance budget for a weighted diff. Absolute is a raw weight cap;
/// Percent is `0.0..=100.0` of `weighted_total`.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(tag = "kind", content = "value")]
pub enum DiffTolerance {
    Absolute(f64),
    Percent(f64),
}

/// Per-pair weighted comparison rules attached to a `# @@diff a b`
/// directive. An empty `DiffRules` means "no rules" — the runner
/// falls back to the existing `match_exact || allow_mismatch` gate.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct DiffRules {
    /// Paths whose values must match exactly. Any difference at one of
    /// these paths records a strict violation; the pair fails.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strict_paths: Vec<String>,
    /// `(path, weight)` pairs. First match wins; later rules with the
    /// same path are ignored. Weights must be `>= 0`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weighted_paths: Vec<(String, f64)>,
    /// Fallback weight for paths that match neither a strict nor a
    /// weighted rule. `None` ⇒ 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_weight: Option<f64>,
    /// Aggregate budget. `None` ⇒ no tolerance check (any non-strict
    /// diff still scores, but `tolerance_passed` stays `None` so the
    /// runner does not fail on score alone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<DiffTolerance>,
}

impl DiffRules {
    /// True when no directive populated this struct. Runner uses this
    /// to decide whether to invoke the weighted code path.
    pub fn is_empty(&self) -> bool {
        self.strict_paths.is_empty()
            && self.weighted_paths.is_empty()
            && self.default_weight.is_none()
            && self.tolerance.is_none()
    }

    fn effective_default_weight(&self) -> f64 {
        self.default_weight.unwrap_or(1.0)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
pub enum WeightedDiffKind {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WeightedDiffPath {
    pub path: String,
    pub weight: f64,
    pub kind: WeightedDiffKind,
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
        raw_to_diff_result(compute_raw_json_diff(&na, &nb))
    } else if body_a.trim().starts_with('<') && body_b.trim().starts_with('<') {
        // Both look like XML — normalize before text comparison
        let norm_a = normalize_xml(body_a);
        let norm_b = normalize_xml(body_b);
        compute_text_diff(&norm_a, &norm_b)
    } else {
        compute_text_diff(body_a, body_b)
    }
}

/// Same shape as `compute_diff` but layers per-path weighted rules on
/// top. When `rules.is_empty()`, returns the unmodified `compute_diff`
/// output (zero behavior change for callers without rules). When either
/// body is non-JSON, sets `rules_applied = Some(false)` and
/// `tolerance_passed = Some(false)` so the runner fails the pair
/// rather than silently passing on a text-mode diff.
pub fn compute_diff_with_rules(
    body_a: &str,
    body_b: &str,
    rules: &DiffRules,
) -> DiffResult {
    if rules.is_empty() {
        return compute_diff(body_a, body_b);
    }

    let json_a = serde_json::from_str::<serde_json::Value>(body_a);
    let json_b = serde_json::from_str::<serde_json::Value>(body_b);
    let (va, vb) = match (json_a, json_b) {
        (Ok(a), Ok(b)) => (a, b),
        _ => {
            // Non-JSON body with rules → rules cannot be applied. We
            // still emit a baseline text-similarity DiffResult so the UI
            // has something to render, but flag the failure modes so
            // the runner refuses to pass.
            let mut base = compute_diff(body_a, body_b);
            base.rules_applied = Some(false);
            base.tolerance_passed = Some(false);
            base.tolerance = rules.tolerance;
            return base;
        }
    };

    let na = normalize_json(&va);
    let nb = normalize_json(&vb);
    let raw = compute_raw_json_diff(&na, &nb);
    let mut result = raw_to_diff_result_capped(&raw);

    // Compile rules into PathOp vectors once.
    let strict_ops: Vec<Vec<PathOp>> = rules
        .strict_paths
        .iter()
        .map(|p| parse_path_ops(strip_dollar_prefix(p)))
        .collect();
    let weighted_ops: Vec<(Vec<PathOp>, f64)> = rules
        .weighted_paths
        .iter()
        .map(|(p, w)| (parse_path_ops(strip_dollar_prefix(p)), *w))
        .collect();
    let default_w = rules.effective_default_weight();

    // ── Score: walk every differing path in the uncapped raw enum. ──
    let mut strict_violations: Vec<String> = Vec::new();
    let mut weighted_paths: Vec<WeightedDiffPath> = Vec::new();
    let mut weighted_score: f64 = 0.0;

    let mut process = |path: &str, kind: WeightedDiffKind| {
        let path_ops = parse_path_ops(path);
        if strict_ops.iter().any(|r| rule_matches_path(r, &path_ops)) {
            strict_violations.push(path.to_string());
            return;
        }
        let weight = weighted_ops
            .iter()
            .find(|(r, _)| rule_matches_path(r, &path_ops))
            .map(|(_, w)| *w)
            .unwrap_or(default_w);
        weighted_score += weight;
        if weighted_paths.len() < MAX_DIFF_PATHS {
            weighted_paths.push(WeightedDiffPath {
                path: path.to_string(),
                weight,
                kind,
            });
        }
    };
    for p in &raw.added {
        process(p, WeightedDiffKind::Added);
    }
    for p in &raw.removed {
        process(p, WeightedDiffKind::Removed);
    }
    for cf in &raw.changed {
        process(&cf.path, WeightedDiffKind::Changed);
    }

    // ── Total: weighted surface across union(A, B), skipping strict. ──
    // Strict paths contribute 0 because they're hard-fail when violated
    // and irrelevant to the budget when satisfied — keeping them out of
    // the denominator means `weighted_ratio` measures "of the diff-able
    // surface, how much actually differs".
    let mut weighted_total: f64 = 0.0;
    let mut seen_paths: std::collections::HashSet<&str> =
        std::collections::HashSet::with_capacity(raw.paths_a.len() + raw.paths_b.len());
    for (p, _) in raw.paths_a.iter().chain(raw.paths_b.iter()) {
        if !seen_paths.insert(p.as_str()) {
            continue;
        }
        let path_ops = parse_path_ops(p);
        if strict_ops.iter().any(|r| rule_matches_path(r, &path_ops)) {
            continue;
        }
        let weight = weighted_ops
            .iter()
            .find(|(r, _)| rule_matches_path(r, &path_ops))
            .map(|(_, w)| *w)
            .unwrap_or(default_w);
        weighted_total += weight;
    }

    let weighted_ratio = if weighted_total > 0.0 {
        weighted_score / weighted_total
    } else {
        0.0
    };

    let tolerance_passed = rules.tolerance.map(|tol| match tol {
        DiffTolerance::Absolute(cap) => weighted_score <= cap,
        DiffTolerance::Percent(pct) => weighted_ratio * 100.0 <= pct,
    });

    result.strict_violations = strict_violations;
    result.weighted_paths = weighted_paths;
    result.weighted_score = weighted_score;
    result.weighted_total = weighted_total;
    result.weighted_ratio = weighted_ratio;
    result.tolerance = rules.tolerance;
    result.tolerance_passed = tolerance_passed;
    result.rules_applied = Some(true);
    result
}

fn strip_dollar_prefix(path: &str) -> &str {
    path.strip_prefix("$.")
        .or_else(|| path.strip_prefix('$'))
        .unwrap_or(path)
}

/// Match a compiled rule against a compiled diff path. The rule must
/// be a prefix of the path (rule.len() <= path.len()), with wildcards:
///   * `PathOp::Wildcard` matches any `Index(N)` or another `Wildcard`
///   * `PathOp::Field("*")` matches any `Field(name)` segment
///   * everything else requires structural equality
/// Prefix-matching means `data.result[*].metric` matches every leaf path
/// under `metric` (e.g. `data.result[0].metric.namespace`).
fn rule_matches_path(rule: &[PathOp], path: &[PathOp]) -> bool {
    if rule.len() > path.len() {
        return false;
    }
    rule.iter()
        .zip(path.iter())
        .all(|(r, p)| match (r, p) {
            (PathOp::Field(rn), PathOp::Field(pn)) => rn == "*" || rn == pn,
            (PathOp::Wildcard, PathOp::Index(_)) => true,
            (PathOp::Wildcard, PathOp::Wildcard) => true,
            (PathOp::Index(ri), PathOp::Index(pi)) => ri == pi,
            _ => false,
        })
}

/// Uncapped enumeration of differing leaf paths plus the flattened
/// leaf-path lists from both sides. The weighted scorer needs the
/// FULL set; display capping happens separately in
/// `raw_to_diff_result_capped` / `raw_to_diff_result`.
struct RawJsonDiff {
    added: Vec<String>,
    removed: Vec<String>,
    changed: Vec<ChangedField>,
    paths_a: Vec<(String, String)>,
    paths_b: Vec<(String, String)>,
}

fn compute_raw_json_diff(va: &serde_json::Value, vb: &serde_json::Value) -> RawJsonDiff {
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

    for (path, val_a) in &map_a {
        match map_b.get(path) {
            Some(val_b) if val_a != val_b => {
                changed.push(ChangedField {
                    path: path.to_string(),
                    left: val_a.to_string(),
                    right: val_b.to_string(),
                });
            }
            None => {
                removed.push(path.to_string());
            }
            _ => {}
        }
    }
    for path in map_b.keys() {
        if !map_a.contains_key(path) {
            added.push(path.to_string());
        }
    }
    added.sort();
    removed.sort();
    changed.sort_by(|a, b| a.path.cmp(&b.path));

    RawJsonDiff {
        added,
        removed,
        changed,
        paths_a,
        paths_b,
    }
}

/// Build a `DiffResult` from a `RawJsonDiff`, truncating the display
/// vectors at `MAX_DIFF_PATHS` while keeping counts at the uncapped
/// totals. Identical to `raw_to_diff_result` but without consuming
/// the raw enumeration — used when the caller still needs `raw` for
/// weighted scoring.
fn raw_to_diff_result_capped(raw: &RawJsonDiff) -> DiffResult {
    let added_count = raw.added.len();
    let removed_count = raw.removed.len();
    let changed_count = raw.changed.len();

    let added: Vec<String> = raw.added.iter().take(MAX_DIFF_PATHS).cloned().collect();
    let removed: Vec<String> = raw.removed.iter().take(MAX_DIFF_PATHS).cloned().collect();
    let changed: Vec<ChangedField> = raw.changed.iter().take(MAX_DIFF_PATHS).cloned().collect();

    let total_max = raw.paths_a.len().max(raw.paths_b.len()).max(1) as f64;
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
        ..Default::default()
    }
}

fn raw_to_diff_result(raw: RawJsonDiff) -> DiffResult {
    raw_to_diff_result_capped(&raw)
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
        // ── Weighted comparison fields (V1.4) ────────────────────
        "strict_violations" => {
            Some(serde_json::to_string(&diff.strict_violations).unwrap_or_default())
        }
        "strict_violations.length" => Some(diff.strict_violations.len().to_string()),
        "weighted_score" => Some(format!("{:.4}", diff.weighted_score)),
        "weighted_total" => Some(format!("{:.4}", diff.weighted_total)),
        "weighted_ratio" => Some(format!("{:.4}", diff.weighted_ratio)),
        "tolerance_passed" => diff.tolerance_passed.map(|b| b.to_string()),
        "rules_applied" => diff.rules_applied.map(|b| b.to_string()),
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
            ..Default::default()
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

    // ──────────────────────────────────────────────────────────────────
    // DSL extensions: bracket-key syntax `["foo.bar"]`
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn bracket_key_with_dot_in_name() {
        // Prometheus label names like `microsoft.resourceid` would otherwise
        // mis-tokenize as 3 nested fields. Bracket form rescues them.
        let body = r#"{"metric":{"microsoft.resourceid":"/subs/x/rg/foo"}}"#;
        let a = make_assertion(r#"$.metric["microsoft.resourceid"]"#, "==", "/subs/x/rg/foo");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed, "actual={:?}", r.actual);
    }

    #[test]
    fn bracket_key_with_space_in_name() {
        let body = r#"{"obj":{"hello world":42}}"#;
        let a = make_assertion(r#"$.obj["hello world"]"#, "==", "42");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed, "actual={:?}", r.actual);
    }

    #[test]
    fn bracket_key_with_brackets_in_name() {
        // `]` inside the quoted segment is literal — only the closing
        // `]` after the matching quote ends the bracket.
        let body = r#"{"obj":{"foo[bar]":"present"}}"#;
        let a = make_assertion(r#"$.obj["foo[bar]"]"#, "==", "present");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    #[test]
    fn bracket_key_with_escaped_quote() {
        let body = r#"{"obj":{"it\"s":"yes"}}"#;
        let a = make_assertion(r#"$.obj["it\"s"]"#, "==", "yes");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed, "actual={:?}", r.actual);
    }

    #[test]
    fn bracket_key_single_quote_form() {
        let body = r#"{"obj":{"a.b":1}}"#;
        let a = make_assertion(r#"$.obj['a.b']"#, "==", "1");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    #[test]
    fn bracket_key_missing_field_fails() {
        let body = r#"{"obj":{"a.b":1}}"#;
        let a = make_assertion(r#"$.obj["nope.thing"]"#, "==", "1");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(!r.passed);
    }

    #[test]
    fn numeric_index_still_works_alongside_quoted_keys() {
        // Mixed forms — make sure adding bracket-key didn't break numeric
        // indices.
        let body = r#"{"arr":[{"a.b":99}]}"#;
        let a = make_assertion(r#"$.arr[0]["a.b"]"#, "==", "99");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    // ──────────────────────────────────────────────────────────────────
    // DSL extensions: wildcard `[*]` with universal quantification
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn wildcard_passes_when_all_elements_satisfy() {
        let body = r#"{"users":[{"id":"u1"},{"id":"u2"},{"id":"u3"}]}"#;
        let a = make_assertion("$.users[*].id", "!=", "null");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed, "actual={:?}", r.actual);
        // Pass-actual reports branch count.
        assert_eq!(r.actual.as_deref(), Some("[3 values]"));
    }

    #[test]
    fn wildcard_fails_when_one_branch_fails() {
        let body = r#"{"users":[{"id":"u1"},{"id":""},{"id":"u3"}]}"#;
        let a = make_assertion("$.users[*].id.length", ">", "0");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(!r.passed);
        // Failure-actual identifies which branch failed.
        assert!(
            r.actual.as_deref().is_some_and(|s| s.starts_with("[1] ")),
            "expected failing branch label, got {:?}",
            r.actual
        );
    }

    #[test]
    fn wildcard_empty_array_is_vacuously_true() {
        // The headline use-case: "skip when no data". An empty result array
        // means the assertion has nothing to check, so it passes.
        let body = r#"{"data":{"result":[]}}"#;
        let a = make_assertion("$.data.result[*].metric.cluster", "!=", "null");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
        assert_eq!(r.actual.as_deref(), Some("[empty array]"));
    }

    #[test]
    fn wildcard_missing_path_before_wildcard_fails() {
        // CRITICAL: a typo / missing field BEFORE the wildcard must NOT be
        // mistaken for vacuous-true. Distinguishes "no data" from "bug".
        let body = r#"{"data":{"other":[]}}"#;
        let a = make_assertion("$.data.result[*].cluster", "!=", "null");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(!r.passed, "missing field before wildcard must fail");
    }

    #[test]
    fn wildcard_missing_field_after_wildcard_fails() {
        let body = r#"{"users":[{"id":"u1"},{"name":"u2"},{"id":"u3"}]}"#;
        let a = make_assertion("$.users[*].id", "!=", "null");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(!r.passed, "missing 'id' on element 1 must fail");
    }

    #[test]
    fn wildcard_on_non_array_fails() {
        let body = r#"{"data":{"result":"not an array"}}"#;
        let a = make_assertion("$.data.result[*].x", "!=", "null");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(!r.passed, "wildcard on non-array must fail (catches typos)");
    }

    #[test]
    fn wildcard_on_null_fails() {
        let body = r#"{"data":{"result":null}}"#;
        let a = make_assertion("$.data.result[*].x", "!=", "null");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(!r.passed);
    }

    #[test]
    fn wildcard_with_bracket_key_universal_check() {
        // The user's actual scenario: every Prometheus result must have all
        // six labels present and non-empty.
        let body = r#"{"data":{"result":[
            {"metric":{"cluster":"east","microsoft.resourceid":"/r/1"}},
            {"metric":{"cluster":"west","microsoft.resourceid":"/r/2"}}
        ]}}"#;
        let a = make_assertion(
            r#"$.data.result[*].metric["microsoft.resourceid"].length"#,
            ">",
            "0",
        );
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed, "actual={:?}", r.actual);
    }

    #[test]
    fn wildcard_with_empty_string_label_fails() {
        let body = r#"{"data":{"result":[
            {"metric":{"cluster":"east"}},
            {"metric":{"cluster":""}}
        ]}}"#;
        let a = make_assertion("$.data.result[*].metric.cluster.length", ">", "0");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(!r.passed, "empty label string must fail length > 0 check");
    }

    #[test]
    fn nested_wildcards_fan_out_correctly() {
        // [*][*] over a 2D array: all 6 inner values must satisfy.
        let body = r#"{"matrix":[[1,2,3],[4,5,6]]}"#;
        let a = make_assertion("$.matrix[*][*]", ">", "0");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed, "actual={:?}", r.actual);
        assert_eq!(r.actual.as_deref(), Some("[6 values]"));
    }

    #[test]
    fn nested_wildcards_one_inner_empty_still_universal() {
        // [*][*] where one inner array is empty contributes 0 branches —
        // still a valid universal check over the non-empty inner arrays.
        let body = r#"{"matrix":[[1,2,3],[]]}"#;
        let a = make_assertion("$.matrix[*][*]", ">", "0");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed, "actual={:?}", r.actual);
        // 3 values from first inner + 0 from second.
        assert_eq!(r.actual.as_deref(), Some("[3 values]"));
    }

    #[test]
    fn wildcard_per_branch_numeric_coercion() {
        // Each branch yields a string ("6"), but `> 5` should still
        // numeric-coerce per branch.
        let body = r#"{"counts":[6,7,8]}"#;
        let a = make_assertion("$.counts[*]", ">", "5");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    #[test]
    fn wildcard_eq_null_passes_when_all_explicitly_null() {
        let body = r#"{"items":[{"x":null},{"x":null}]}"#;
        let a = make_assertion("$.items[*].x", "==", "null");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    #[test]
    fn wildcard_eq_null_passes_when_all_missing() {
        // For "== null", a missing field is conventionally null-equivalent
        // (mirrors single-value behavior).
        let body = r#"{"items":[{"y":1},{"y":2}]}"#;
        let a = make_assertion("$.items[*].x", "==", "null");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    // ──────────────────────────────────────────────────────────────────
    // DSL extensions: empty-string literal `!= ""`
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn empty_string_neq_passes_for_nonempty() {
        let body = r#"{"x":"hello"}"#;
        let a = make_assertion("$.x", "!=", "\"\"");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    #[test]
    fn empty_string_eq_passes_for_empty_value() {
        let body = r#"{"x":""}"#;
        let a = make_assertion("$.x", "==", "\"\"");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(r.passed);
    }

    #[test]
    fn empty_string_neq_fails_for_empty_value() {
        let body = r#"{"x":""}"#;
        let a = make_assertion("$.x", "!=", "\"\"");
        let r = evaluate(&a, 200, &empty_headers(), body);
        assert!(!r.passed);
    }

    // ──────────────────────────────────────────────────────────────────
    // Single-value resolver rejects wildcard paths
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn resolve_single_value_rejects_wildcard_paths() {
        // resolve_response_value (used by extracts / response-chaining) is
        // single-value only. Wildcard paths must yield None there so users
        // get a clear "this didn't resolve" rather than nondeterministic
        // collapse.
        let body = r#"{"items":[1,2,3]}"#;
        let v = resolve_response_value("$.items[*]", 200, &empty_headers(), body);
        assert!(v.is_none(), "wildcard path must not resolve to a single value");
    }

    // ──────────────────────────────────────────────────────────────────
    // V1.4: weighted-diff rules
    // ──────────────────────────────────────────────────────────────────

    fn make_rules(
        strict: &[&str],
        weighted: &[(&str, f64)],
        default_weight: Option<f64>,
        tolerance: Option<DiffTolerance>,
    ) -> DiffRules {
        DiffRules {
            strict_paths: strict.iter().map(|s| s.to_string()).collect(),
            weighted_paths: weighted.iter().map(|(p, w)| (p.to_string(), *w)).collect(),
            default_weight,
            tolerance,
        }
    }

    #[test]
    fn weighted_no_rules_is_passthrough() {
        // compute_diff_with_rules with empty rules MUST equal compute_diff.
        let a = r#"{"x":1,"y":2}"#;
        let b = r#"{"x":1,"y":3}"#;
        let plain = compute_diff(a, b);
        let ruled = compute_diff_with_rules(a, b, &DiffRules::default());
        assert_eq!(plain.changed_count, ruled.changed_count);
        assert_eq!(plain.match_exact, ruled.match_exact);
        assert!(ruled.rules_applied.is_none(), "no rules ⇒ rules_applied=None");
        assert!(ruled.tolerance_passed.is_none());
        assert!(ruled.strict_violations.is_empty());
        assert_eq!(ruled.weighted_score, 0.0);
    }

    #[test]
    fn weighted_strict_violation_recorded() {
        // Strict rule on a changed field ⇒ strict_violations populated.
        let a = r#"{"id":"alice","ts":1}"#;
        let b = r#"{"id":"bob","ts":2}"#;
        let rules = make_rules(&["$.id"], &[("$.ts", 0.0)], None, None);
        let r = compute_diff_with_rules(a, b, &rules);
        assert_eq!(r.strict_violations, vec!["id".to_string()]);
        assert_eq!(r.rules_applied, Some(true));
        // ts difference contributes 0 to score because weight is 0
        assert_eq!(r.weighted_score, 0.0);
    }

    #[test]
    fn weighted_strict_beats_weighted_when_both_match() {
        // Precedence: strict rule wins over weighted rule for the same path.
        let a = r#"{"id":"alice"}"#;
        let b = r#"{"id":"bob"}"#;
        let rules = make_rules(&["$.id"], &[("$.id", 0.5)], None, None);
        let r = compute_diff_with_rules(a, b, &rules);
        assert_eq!(r.strict_violations.len(), 1);
        assert_eq!(r.weighted_score, 0.0, "weighted rule must not also charge");
    }

    #[test]
    fn weighted_default_weight_fallback() {
        // Path matching no rule gets default_weight.
        let a = r#"{"a":1,"b":2,"c":3}"#;
        let b = r#"{"a":9,"b":9,"c":9}"#;
        let rules = make_rules(&[], &[("$.a", 0.1)], Some(2.0), None);
        let r = compute_diff_with_rules(a, b, &rules);
        // a=0.1, b=2.0, c=2.0 = 4.1
        assert!((r.weighted_score - 4.1).abs() < 1e-9, "got {}", r.weighted_score);
    }

    #[test]
    fn weighted_absolute_tolerance_pass_and_fail() {
        let a = r#"{"x":1,"y":2}"#;
        let b = r#"{"x":1,"y":99}"#;
        let pass = compute_diff_with_rules(
            a, b,
            &make_rules(&[], &[("$.y", 0.3)], Some(1.0), Some(DiffTolerance::Absolute(0.5))),
        );
        assert_eq!(pass.tolerance_passed, Some(true));
        let fail = compute_diff_with_rules(
            a, b,
            &make_rules(&[], &[("$.y", 0.3)], Some(1.0), Some(DiffTolerance::Absolute(0.1))),
        );
        assert_eq!(fail.tolerance_passed, Some(false));
    }

    #[test]
    fn weighted_percent_tolerance() {
        // 4 leaf paths; 2 differ; weights all 1.0 → ratio 50%.
        let a = r#"{"a":1,"b":2,"c":3,"d":4}"#;
        let b = r#"{"a":1,"b":2,"c":99,"d":99}"#;
        let r = compute_diff_with_rules(
            a, b,
            &make_rules(&[], &[], None, Some(DiffTolerance::Percent(60.0))),
        );
        assert!((r.weighted_ratio - 0.5).abs() < 1e-9);
        assert_eq!(r.tolerance_passed, Some(true));
        let r2 = compute_diff_with_rules(
            a, b,
            &make_rules(&[], &[], None, Some(DiffTolerance::Percent(40.0))),
        );
        assert_eq!(r2.tolerance_passed, Some(false));
    }

    #[test]
    fn weighted_array_wildcard_matches_every_element() {
        // $.items[*].price matches both items[0].price and items[1].price.
        let a = r#"{"items":[{"price":10},{"price":20}]}"#;
        let b = r#"{"items":[{"price":11},{"price":99}]}"#;
        let r = compute_diff_with_rules(
            a, b,
            &make_rules(&[], &[("$.items[*].price", 0.5)], Some(10.0), None),
        );
        // Both diffs charged at 0.5 each ⇒ score 1.0
        assert_eq!(r.weighted_score, 1.0);
    }

    #[test]
    fn weighted_prefix_match_covers_subtree() {
        // Strict on `$.data.result[*].metric` should catch a violation at
        // `data.result[0].metric.namespace`.
        let a = r#"{"data":{"result":[{"metric":{"ns":"a"}}]}}"#;
        let b = r#"{"data":{"result":[{"metric":{"ns":"b"}}]}}"#;
        let r = compute_diff_with_rules(
            a, b,
            &make_rules(&["$.data.result[*].metric"], &[], None, None),
        );
        assert_eq!(r.strict_violations.len(), 1);
    }

    #[test]
    fn weighted_strict_on_added_path() {
        // A path present only in B (added) triggers a strict rule, not just
        // a "changed" path. Important so partial responses are caught.
        let a = r#"{"x":1}"#;
        let b = r#"{"x":1,"id":"new"}"#;
        let r = compute_diff_with_rules(
            a, b,
            &make_rules(&["$.id"], &[], None, None),
        );
        assert_eq!(r.strict_violations, vec!["id".to_string()]);
    }

    #[test]
    fn weighted_non_json_body_fails_with_rules() {
        // Non-JSON bodies cannot be path-rule-scored. Runner must see
        // rules_applied=false + tolerance_passed=false so the pair fails
        // instead of silently passing on text similarity.
        let a = "plain text body A";
        let b = "plain text body B";
        let r = compute_diff_with_rules(
            a, b,
            &make_rules(&[], &[("$.foo", 0.1)], None, Some(DiffTolerance::Percent(50.0))),
        );
        assert_eq!(r.rules_applied, Some(false));
        assert_eq!(r.tolerance_passed, Some(false));
    }

    #[test]
    fn weighted_total_excludes_strict_paths() {
        // Strict paths contribute 0 to denominator, so ratio focuses on
        // the "diff-able surface" only. Bodies have 3 paths; one is strict.
        let a = r#"{"id":"a","x":1,"y":2}"#;
        let b = r#"{"id":"a","x":1,"y":3}"#;
        let r = compute_diff_with_rules(
            a, b,
            &make_rules(&["$.id"], &[("$.x", 0.0)], Some(1.0), None),
        );
        // weighted_total: x(0) + y(1) = 1.0. Score: y(1). Ratio 1.0.
        assert!((r.weighted_total - 1.0).abs() < 1e-9);
        assert!((r.weighted_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn weighted_diff_result_round_trips_serde_back_compat() {
        // Old DiffResult JSON without any of the new fields must still
        // deserialize cleanly thanks to #[serde(default)].
        let legacy = r#"{
            "match_exact": false, "similarity": 0.5, "is_json": true,
            "added_paths": [], "removed_paths": [], "changed_paths": [],
            "added_count": 0, "removed_count": 0, "changed_count": 1
        }"#;
        let d: DiffResult = serde_json::from_str(legacy)
            .expect("old DiffResult JSON must deserialize");
        assert!(d.strict_violations.is_empty());
        assert!(d.weighted_paths.is_empty());
        assert_eq!(d.weighted_score, 0.0);
        assert!(d.tolerance_passed.is_none());
        assert!(d.rules_applied.is_none());
    }

    #[test]
    fn weighted_large_diff_scores_beyond_max_display() {
        // Regression: scoring must walk the UNCAPPED diff list, not the
        // display-capped vectors. Build two objects with > MAX_DIFF_PATHS
        // (1000) leaf differences and verify the score reflects all of them.
        let mut a = String::from("{");
        let mut b = String::from("{");
        for i in 0..1500 {
            if i > 0 { a.push(','); b.push(','); }
            a.push_str(&format!(r#""k{}":1"#, i));
            b.push_str(&format!(r#""k{}":2"#, i));
        }
        a.push('}'); b.push('}');
        let r = compute_diff_with_rules(
            &a, &b,
            &make_rules(&[], &[], Some(1.0), None),
        );
        assert_eq!(r.changed_count, 1500);
        assert!(r.changed_paths.len() <= MAX_DIFF_PATHS, "display still capped");
        assert!((r.weighted_score - 1500.0).abs() < 1e-9, "score includes ALL diffs");
    }

    #[test]
    fn weighted_strip_dollar_prefix() {
        // `$.foo` and `foo` and `$foo` should all behave identically.
        let a = r#"{"foo":1}"#;
        let b = r#"{"foo":2}"#;
        let r1 = compute_diff_with_rules(a, b, &make_rules(&["$.foo"], &[], None, None));
        let r2 = compute_diff_with_rules(a, b, &make_rules(&["foo"], &[], None, None));
        let r3 = compute_diff_with_rules(a, b, &make_rules(&["$foo"], &[], None, None));
        assert_eq!(r1.strict_violations.len(), 1);
        assert_eq!(r2.strict_violations.len(), 1);
        assert_eq!(r3.strict_violations.len(), 1);
    }

    // ── Header bracket-key syntax for names with dashes/dots ─────

    fn header(name: &str, value: &str) -> Vec<(String, String)> {
        vec![(name.to_string(), value.to_string())]
    }

    #[test]
    fn header_bracket_key_resolves_when_dot_syntax_cannot() {
        // Header names with dashes can't be addressed via dot syntax;
        // bracket-key syntax must work. Regression test for the
        // `$.headers["..."]` silent-Missing bug.
        let hdrs = header(
            "x-ms-azure-implicit-rollup-applied",
            r#"[{"tier":1,"B":60}]"#,
        );
        let a = make_assertion(
            r#"$.headers["x-ms-azure-implicit-rollup-applied"]"#,
            "contains",
            r#""tier":1"#,
        );
        let r = evaluate(&a, 200, &hdrs, "");
        assert!(
            r.passed,
            "bracket-key header lookup must work; got actual={:?}",
            r.actual
        );
    }

    #[test]
    fn header_bracket_key_not_null_pass() {
        // The `!= null` assertion from the user's repro should pass
        // when the header IS present, not fail because the resolver
        // silently returned Missing.
        let hdrs = header("x-ms-azure-implicit-rollup-applied", "[{}]");
        let a = make_assertion(
            r#"$.headers["x-ms-azure-implicit-rollup-applied"]"#,
            "!=",
            "null",
        );
        let r = evaluate(&a, 200, &hdrs, "");
        assert!(r.passed);
    }

    #[test]
    fn header_bracket_key_case_insensitive() {
        // Same case-insensitivity as the dot-syntax path.
        let hdrs = header("X-Ms-Foo", "bar");
        let a = make_assertion(r#"$.headers["x-ms-foo"]"#, "==", "bar");
        let r = evaluate(&a, 200, &hdrs, "");
        assert!(r.passed);
    }

    #[test]
    fn header_bracket_key_missing_returns_missing_for_neq_null() {
        // When the header is genuinely absent, `!= null` must fail
        // (matching dot-syntax semantics).
        let hdrs: Vec<(String, String)> = Vec::new();
        let a = make_assertion(r#"$.headers["not-there"]"#, "!=", "null");
        let r = evaluate(&a, 200, &hdrs, "");
        assert!(!r.passed);
    }

    #[test]
    fn header_bracket_key_response_prefix_form() {
        // `response.headers["..."]` (no leading `$`) must also work.
        let hdrs = header("x-trace-id", "abc-123");
        let a = make_assertion(
            r#"response.headers["x-trace-id"]"#,
            "==",
            "abc-123",
        );
        let r = evaluate(&a, 200, &hdrs, "");
        assert!(r.passed);
    }

    #[test]
    fn weighted_resolve_diff_value_new_keys() {
        let a = r#"{"id":"a","x":1}"#;
        let b = r#"{"id":"b","x":2}"#;
        let diff = compute_diff_with_rules(
            a, b,
            &make_rules(&["$.id"], &[("$.x", 0.3)], None, Some(DiffTolerance::Absolute(1.0))),
        );
        assert_eq!(resolve_diff_value("$diff.strict_violations.length", &diff), Some("1".into()));
        assert_eq!(resolve_diff_value("$diff.weighted_score", &diff), Some("0.3000".into()));
        assert_eq!(resolve_diff_value("$diff.tolerance_passed", &diff), Some("true".into()));
        assert_eq!(resolve_diff_value("$diff.rules_applied", &diff), Some("true".into()));
        // tolerance_passed key returns None when no rules were configured
        let plain = compute_diff(a, b);
        assert_eq!(resolve_diff_value("$diff.tolerance_passed", &plain), None);
        assert_eq!(resolve_diff_value("$diff.rules_applied", &plain), None);
    }
}
