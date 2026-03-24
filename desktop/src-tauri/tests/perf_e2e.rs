//! End-to-end / integration tests for the `perf` module.
//!
//! These exercise the full pipeline: format → tree → diff with realistic
//! payloads, edge cases, and basic performance benchmarks.

use request_pilot_desktop::perf::{
    build_tree_node, compute_diff_inner, format_body_inner, navigate_json,
    sort_and_normalize_inner, DiffOp,
};
use serde_json::{json, Value};
use std::time::Instant;

// ─── Helpers ───────────────────────────────────────────────────────────────────

/// Generate a realistic JSON array of `n` objects (simulating an API response).
fn generate_large_json_array(n: usize) -> String {
    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        items.push(json!({
            "id": i,
            "name": format!("User {}", i),
            "email": format!("user{}@example.com", i),
            "address": {
                "street": format!("{} Main St", i),
                "city": "Metropolis",
                "zip": format!("{:05}", i % 100_000)
            },
            "created_at": "2024-01-15T10:30:00Z",
            "active": i % 3 != 0
        }));
    }
    serde_json::to_string(&Value::Array(items)).unwrap()
}

/// Generate a deeply nested JSON object.
fn generate_nested_json(depth: usize) -> String {
    let mut val = json!("leaf");
    for i in (0..depth).rev() {
        val = json!({ format!("level_{}", i): val });
    }
    serde_json::to_string(&val).unwrap()
}

/// Generate a simple JSON object with `n` top-level keys.
fn generate_wide_json(n: usize) -> String {
    let mut map = serde_json::Map::new();
    for i in 0..n {
        map.insert(format!("key_{:05}", i), json!(format!("value_{}", i)));
    }
    serde_json::to_string(&Value::Object(map)).unwrap()
}

/// Count DiffOp variants in a slice.
#[allow(dead_code)]
fn count_ops(ops: &[DiffOp]) -> (usize, usize, usize, usize) {
    let (mut same, mut add, mut rem, mut chg) = (0, 0, 0, 0);
    for op in ops {
        match op {
            DiffOp::Same { .. } => same += 1,
            DiffOp::Add { .. } => add += 1,
            DiffOp::Remove { .. } => rem += 1,
            DiffOp::Change { .. } => chg += 1,
        }
    }
    (same, add, rem, chg)
}

// ─── Realistic Large Payload Tests ─────────────────────────────────────────────

#[test]
fn e2e_format_2mb_json_response() {
    let body = generate_large_json_array(10_000);
    assert!(body.len() > 1_500_000, "payload should be roughly 2MB");

    let start = Instant::now();
    let result = format_body_inner(&body, "application/json").unwrap();
    let elapsed = start.elapsed();

    println!(
        "format_body 2MB JSON: {} lines in {:.2?}",
        result.len(),
        elapsed
    );
    assert!(!result.is_empty());
    // Pretty-printed 10k objects → many lines
    assert!(result.len() > 10_000);
    assert!(elapsed.as_secs() < 10, "should complete in < 10 seconds");
}

#[test]
fn e2e_build_tree_2mb_json() {
    let body = generate_large_json_array(10_000);

    let start = Instant::now();
    let val: Value = serde_json::from_str(&body).unwrap();
    let tree = build_tree_node(&val, None, 0, 3, 100, vec![], true, true);
    let elapsed = start.elapsed();

    println!("build_json_tree 2MB: {:.2?}", elapsed);
    assert_eq!(tree.node_type, "array");
    assert_eq!(tree.child_count, 10_000);
    // max_children=100 → 100 expanded + 1 truncated sentinel
    assert_eq!(tree.children.len(), 101);
    assert_eq!(tree.children.last().unwrap().node_type, "truncated");
    assert!(elapsed.as_secs() < 10, "should complete in < 10 seconds");
}

#[test]
fn e2e_diff_large_similar_jsons() {
    // Generate two JSON arrays that are ~95% identical.
    let mut items_a = Vec::with_capacity(10_000);
    let mut items_b = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        let name_a = format!("User {}", i);
        let name_b = if i % 20 == 0 {
            format!("Modified User {}", i)
        } else {
            format!("User {}", i)
        };
        items_a.push(json!({"id": i, "name": name_a}));
        items_b.push(json!({"id": i, "name": name_b}));
    }
    let json_a = serde_json::to_string(&Value::Array(items_a)).unwrap();
    let json_b = serde_json::to_string(&Value::Array(items_b)).unwrap();

    let start = Instant::now();
    let result = compute_diff_inner(&json_a, &json_b, "application/json").unwrap();
    let elapsed = start.elapsed();

    println!(
        "diff 95% similar: sim={:.3}, changed={}, elapsed={:.2?}",
        result.similarity, result.changed_count, elapsed
    );
    assert!(result.similarity > 0.80, "similarity should be high");
    assert!(result.changed_count > 0, "some lines should differ");
    assert!(elapsed.as_secs() < 30, "should complete in < 30 seconds");
}

#[test]
fn e2e_diff_large_different_jsons() {
    let json_a = generate_wide_json(5_000);
    let json_b = generate_large_json_array(2_000);

    let start = Instant::now();
    let result = compute_diff_inner(&json_a, &json_b, "application/json").unwrap();
    let elapsed = start.elapsed();

    println!(
        "diff completely different: sim={:.3}, elapsed={:.2?}",
        result.similarity, elapsed
    );
    // Should not panic or hang
    assert!(result.similarity < 0.5);
    assert!(elapsed.as_secs() < 30, "should complete in < 30 seconds");
}

// ─── Format → Tree → Expand Pipeline ──────────────────────────────────────────

#[test]
fn e2e_json_tree_lazy_expansion() {
    let body = json!({
        "users": [
            {"name": "Alice", "age": 30},
            {"name": "Bob", "age": 25}
        ],
        "meta": {"total": 2}
    });
    let json_str = serde_json::to_string(&body).unwrap();
    let val: Value = serde_json::from_str(&json_str).unwrap();

    // Build with max_depth=1 → children of "users" should be truncated
    let tree = build_tree_node(&val, None, 0, 1, 100, vec![], true, false);
    assert_eq!(tree.node_type, "object");
    assert!(!tree.children.is_empty());

    // Find a child that was truncated (has child_count > 0 but empty children)
    let users_node = tree.children.iter().find(|n| n.key.as_deref() == Some("users")).unwrap();
    assert_eq!(users_node.child_count, 2);
    assert!(users_node.children.is_empty(), "depth-limited → no children expanded");

    // Expand it
    let path = vec!["users".to_string()];
    let target = navigate_json(&val, &path).unwrap();
    let expanded = build_tree_node(target, Some("users".to_string()), 0, 3, 100, path, true, false);
    assert_eq!(expanded.node_type, "array");
    assert_eq!(expanded.children.len(), 2);
    assert_eq!(expanded.children[0].node_type, "object");
}

#[test]
fn e2e_tree_expand_all_levels() {
    let json_str = generate_nested_json(5);
    let val: Value = serde_json::from_str(&json_str).unwrap();

    // Build depth=1
    let tree = build_tree_node(&val, None, 0, 1, 100, vec![], true, false);
    assert_eq!(tree.node_type, "object");
    assert!(!tree.children.is_empty());

    // Expand level by level down to leaf
    let mut current_path: Vec<String> = Vec::new();
    for level in 0..5 {
        let key = format!("level_{}", level);
        current_path.push(key.clone());
        let target = navigate_json(&val, &current_path).unwrap();
        let node = build_tree_node(
            target,
            Some(key),
            0,
            1,
            100,
            current_path.clone(),
            true,
            false,
        );
        if level < 4 {
            assert_eq!(node.node_type, "object", "level {} should be object", level);
        } else {
            assert_eq!(node.node_type, "string", "leaf should be string");
            assert_eq!(node.value_preview.as_deref(), Some("\"leaf\""));
        }
    }
}

// ─── Normalize → Diff Pipeline ─────────────────────────────────────────────────

#[test]
fn e2e_json_different_key_order_same_content() {
    let a = r#"{"b":1,"a":2}"#;
    let b = r#"{"a":2,"b":1}"#;

    let norm_a = sort_and_normalize_inner(a, "application/json").unwrap();
    let norm_b = sort_and_normalize_inner(b, "application/json").unwrap();
    assert_eq!(norm_a, norm_b, "normalized forms should be identical");

    let diff = compute_diff_inner(a, b, "application/json").unwrap();
    assert!(
        (diff.similarity - 1.0).abs() < f64::EPSILON,
        "similarity should be 1.0, got {}",
        diff.similarity
    );
    assert_eq!(diff.changed_count, 0);
    assert_eq!(diff.added_count, 0);
    assert_eq!(diff.removed_count, 0);
}

#[test]
fn e2e_json_normalize_then_diff_large() {
    // Large JSON with shuffled keys but same data
    let mut map_a = serde_json::Map::new();
    let mut map_b = serde_json::Map::new();
    for i in 0..1_000 {
        map_a.insert(format!("key_{:04}", i), json!(i));
        // Insert in reverse order
        map_b.insert(format!("key_{:04}", 999 - i), json!(999 - i));
    }
    let json_a = serde_json::to_string(&Value::Object(map_a)).unwrap();
    let json_b = serde_json::to_string(&Value::Object(map_b)).unwrap();

    let norm_a = sort_and_normalize_inner(&json_a, "application/json").unwrap();
    let norm_b = sort_and_normalize_inner(&json_b, "application/json").unwrap();
    assert_eq!(norm_a, norm_b);

    let diff = compute_diff_inner(&json_a, &json_b, "application/json").unwrap();
    assert!(
        (diff.similarity - 1.0).abs() < f64::EPSILON,
        "same data → similarity 1.0, got {}",
        diff.similarity
    );
}

#[test]
fn e2e_xml_normalize_then_diff() {
    let xml_a = r#"<root><item id="1" name="foo" /><item id="2" name="bar" /></root>"#;
    let xml_b = r#"<root><item name="foo" id="1" /><item name="bar" id="2" /></root>"#;

    let norm_a = sort_and_normalize_inner(xml_a, "application/xml").unwrap();
    let norm_b = sort_and_normalize_inner(xml_b, "application/xml").unwrap();
    assert_eq!(norm_a, norm_b, "XML with different attr order should normalize identically");

    let diff = compute_diff_inner(xml_a, xml_b, "application/xml").unwrap();
    assert!(
        (diff.similarity - 1.0).abs() < f64::EPSILON,
        "same XML → similarity 1.0, got {}",
        diff.similarity
    );
}

// ─── Edge Cases ────────────────────────────────────────────────────────────────

#[test]
fn e2e_empty_json_handling() {
    // Empty string
    let result = format_body_inner("", "application/json").unwrap();
    // Empty or fallback to raw (single empty line or zero lines)
    assert!(result.len() <= 1);

    // Empty object
    let val: Value = serde_json::from_str("{}").unwrap();
    let tree = build_tree_node(&val, None, 0, 3, 100, vec![], true, false);
    assert_eq!(tree.node_type, "object");
    assert_eq!(tree.child_count, 0);
    assert!(tree.children.is_empty());

    // Diff empty strings
    let diff = compute_diff_inner("", "", "text/plain").unwrap();
    assert!((diff.similarity - 1.0).abs() < f64::EPSILON);
    assert_eq!(diff.changed_count, 0);
}

#[test]
fn e2e_unicode_payload() {
    let body = json!({
        "emoji": "Hello 🌍🚀✨",
        "cjk": "你好世界",
        "rtl": "مرحبا",
        "mixed": "Café résumé naïve"
    });
    let json_str = serde_json::to_string(&body).unwrap();

    // Format
    let lines = format_body_inner(&json_str, "application/json").unwrap();
    let full_text: String = lines.iter().map(|l| l.raw.as_str()).collect::<Vec<_>>().join("\n");
    assert!(full_text.contains("🌍"));
    assert!(full_text.contains("你好世界"));
    assert!(full_text.contains("مرحبا"));
    assert!(full_text.contains("Café"));

    // Tree
    let val: Value = serde_json::from_str(&json_str).unwrap();
    let tree = build_tree_node(&val, None, 0, 3, 100, vec![], true, false);
    assert_eq!(tree.child_count, 4);

    // Diff identical → no corruption
    let diff = compute_diff_inner(&json_str, &json_str, "application/json").unwrap();
    assert!((diff.similarity - 1.0).abs() < f64::EPSILON);
}

#[test]
fn e2e_deeply_nested_100_levels() {
    let json_str = generate_nested_json(100);

    // format_body should not stack overflow
    let lines = format_body_inner(&json_str, "application/json").unwrap();
    assert!(lines.len() >= 100, "at least one line per nesting level");

    // build tree with max_depth=3 → handles without issue
    let val: Value = serde_json::from_str(&json_str).unwrap();
    let tree = build_tree_node(&val, None, 0, 3, 100, vec![], true, false);
    assert_eq!(tree.node_type, "object");
    assert_eq!(tree.depth, 0);
}

#[test]
fn e2e_array_10000_items() {
    let items: Vec<Value> = (0..10_000).map(|i| json!(i)).collect();
    let json_str = serde_json::to_string(&items).unwrap();

    // Tree with max_children=100 → truncated
    let val: Value = serde_json::from_str(&json_str).unwrap();
    let tree = build_tree_node(&val, None, 0, 3, 100, vec![], true, false);
    assert_eq!(tree.node_type, "array");
    assert_eq!(tree.child_count, 10_000);
    // 100 real children + 1 "truncated" sentinel
    assert_eq!(tree.children.len(), 101);
    let last = tree.children.last().unwrap();
    assert_eq!(last.node_type, "truncated");
    assert!(last.value_preview.as_ref().unwrap().contains("9900 more"));

    // format_body → all items present
    let lines = format_body_inner(&json_str, "application/json").unwrap();
    assert!(lines.len() > 10_000);
}

#[test]
fn e2e_malformed_json_graceful() {
    let bad_json = r#"{"broken: true, nope"#;

    // format_body falls back to raw text lines (no panic)
    let lines = format_body_inner(bad_json, "application/json").unwrap();
    assert!(!lines.is_empty());
    assert_eq!(lines[0].raw, bad_json);

    // build_tree_node requires valid parsed JSON → we test via the parse step
    let parse_result: Result<Value, _> = serde_json::from_str(bad_json);
    assert!(parse_result.is_err());

    // compute_diff with JSON content_type falls back to text diff on invalid JSON
    let diff = compute_diff_inner(bad_json, "something else", "application/json").unwrap();
    assert!(diff.changed_count > 0 || diff.added_count > 0 || diff.removed_count > 0);
}

#[test]
fn e2e_binary_content() {
    // Simulate binary-looking content
    let binary_str: String = (0..=255u8).map(|b| b as char).collect();

    let lines = format_body_inner(&binary_str, "application/octet-stream").unwrap();
    assert!(!lines.is_empty());

    // Diff binary as text
    let other = "completely different content";
    let diff = compute_diff_inner(&binary_str, other, "application/octet-stream").unwrap();
    assert!(diff.similarity < 1.0);
}

#[test]
fn e2e_csv_large() {
    let mut csv = String::from("id,name,email\n");
    for i in 0..10_000 {
        csv.push_str(&format!("{},User {},user{}@example.com\n", i, i, i));
    }

    let lines = format_body_inner(&csv, "text/csv").unwrap();
    // Header + 10,000 data rows (last newline may add an empty line)
    assert!(
        lines.len() >= 10_001,
        "expected >= 10001 lines, got {}",
        lines.len()
    );
}

// ─── Performance Benchmark Tests ───────────────────────────────────────────────

#[test]
fn e2e_benchmark_format_body_sizes() {
    let sizes = [
        ("1KB", 10),
        ("10KB", 100),
        ("100KB", 1_000),
        ("1MB", 8_000),
        ("2MB", 15_000),
    ];

    println!("\n--- format_body benchmark ---");
    for (label, count) in &sizes {
        let body = generate_large_json_array(*count);
        let size_kb = body.len() / 1024;
        let start = Instant::now();
        let result = format_body_inner(&body, "application/json").unwrap();
        let elapsed = start.elapsed();
        println!(
            "  {}: {}KB payload → {} lines in {:.2?}",
            label,
            size_kb,
            result.len(),
            elapsed
        );
        assert!(
            elapsed.as_secs() < 15,
            "{} took too long: {:.2?}",
            label,
            elapsed
        );
    }
}

#[test]
fn e2e_benchmark_diff_sizes() {
    let sizes = [
        ("100 lines", 100),
        ("1000 lines", 1_000),
        ("5000 lines", 5_000),
    ];

    println!("\n--- compute_diff benchmark ---");
    for (label, line_count) in &sizes {
        let mut lines_a = Vec::with_capacity(*line_count);
        let mut lines_b = Vec::with_capacity(*line_count);
        for i in 0..*line_count {
            lines_a.push(format!("line {} content alpha", i));
            if i % 10 == 0 {
                lines_b.push(format!("line {} content MODIFIED", i));
            } else {
                lines_b.push(format!("line {} content alpha", i));
            }
        }
        let text_a = lines_a.join("\n");
        let text_b = lines_b.join("\n");

        let start = Instant::now();
        let result = compute_diff_inner(&text_a, &text_b, "text/plain").unwrap();
        let elapsed = start.elapsed();
        println!(
            "  {}: sim={:.3}, changes={}, elapsed={:.2?}",
            label, result.similarity, result.changed_count, elapsed
        );
        assert!(
            elapsed.as_secs() < 30,
            "{} took too long: {:.2?}",
            label,
            elapsed
        );
    }
}

#[test]
fn e2e_benchmark_tree_build() {
    let sizes = [
        ("100 keys", 100),
        ("1000 keys", 1_000),
        ("5000 keys", 5_000),
        ("10000 keys", 10_000),
    ];

    println!("\n--- build_json_tree benchmark ---");
    for (label, key_count) in &sizes {
        let body = generate_wide_json(*key_count);
        let val: Value = serde_json::from_str(&body).unwrap();
        let start = Instant::now();
        let tree = build_tree_node(&val, None, 0, 3, 100, vec![], true, true);
        let elapsed = start.elapsed();
        println!(
            "  {}: child_count={}, children={}, elapsed={:.2?}",
            label,
            tree.child_count,
            tree.children.len(),
            elapsed
        );
        assert_eq!(tree.child_count, *key_count);
        assert!(
            elapsed.as_secs() < 10,
            "{} took too long: {:.2?}",
            label,
            elapsed
        );
    }
}

// ─── Additional Pipeline Tests ─────────────────────────────────────────────────

#[test]
fn e2e_format_then_diff_xml() {
    let xml_a = r#"<root><a>1</a><b>2</b></root>"#;
    let xml_b = r#"<root><a>1</a><b>3</b></root>"#;

    // Format both
    let lines_a = format_body_inner(xml_a, "application/xml").unwrap();
    let lines_b = format_body_inner(xml_b, "application/xml").unwrap();
    assert!(!lines_a.is_empty());
    assert!(!lines_b.is_empty());

    // Diff
    let diff = compute_diff_inner(xml_a, xml_b, "application/xml").unwrap();
    assert!(diff.similarity > 0.5, "mostly similar XML");
    assert!(diff.changed_count > 0, "value '2' vs '3' should differ");
}

#[test]
fn e2e_normalize_idempotent() {
    let body = json!({"z": 1, "a": [3, 2, 1], "m": {"x": true, "b": null}});
    let json_str = serde_json::to_string(&body).unwrap();

    let norm1 = sort_and_normalize_inner(&json_str, "application/json").unwrap();
    let norm2 = sort_and_normalize_inner(&norm1, "application/json").unwrap();
    assert_eq!(norm1, norm2, "normalizing twice should be idempotent");
}

#[test]
fn e2e_diff_identical_large_json() {
    let body = generate_large_json_array(1_000);
    let diff = compute_diff_inner(&body, &body, "application/json").unwrap();
    assert!(
        (diff.similarity - 1.0).abs() < f64::EPSILON,
        "identical inputs → similarity 1.0"
    );
    assert_eq!(diff.changed_count, 0);
    assert_eq!(diff.added_count, 0);
    assert_eq!(diff.removed_count, 0);
}
