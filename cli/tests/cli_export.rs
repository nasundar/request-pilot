//! Integration tests for `cmd_export::run_with_store`.

use std::collections::HashMap;
use std::io::Cursor;

use chrono::Utc;
use tempfile::tempdir;

use request_pilot_core::assertions::AssertionResult;
use request_pilot_core::http_client::HttpResponse;
use request_pilot_core::sessions::{
    CapturePolicy, Component, FileIdentity, RecordInput, RecordedSession, SessionStore,
    SessionTrigger,
};
use request_pilot_core::test_runner::{BlockResult, ExtractResult, TestRunResults};

// We rebuild the cli binary helpers here. Pull in the cli crate's
// internals via the binary target's `path = "src/main.rs"` setup is
// awkward, so we instead `include!` the modules directly.
#[path = "../src/find.rs"]
mod find;
#[path = "../src/cmd_export.rs"]
mod cmd_export;

fn make_block(name: &str, status: &str, http_status: u16) -> BlockResult {
    BlockResult {
        seq: Some(1),
        name: name.to_string(),
        block_type: "test".to_string(),
        group: None,
        request_method: "GET".to_string(),
        request_url: "https://example.com/health".to_string(),
        request_headers: vec![("X-Trace".to_string(), "ok".to_string())],
        request_body: Some("hello".to_string()),
        status: status.to_string(),
        response: Some(HttpResponse {
            status: http_status,
            status_text: "OK".to_string(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: "{\"ok\":true}".to_string(),
            time_ms: 7,
            size_bytes: 11,
        }),
        assertion_results: vec![AssertionResult {
            assertion: "status == 200".to_string(),
            expected: Some("200".to_string()),
            actual: Some(http_status.to_string()),
            passed: status == "passed",
        }],
        extract_results: vec![ExtractResult {
            variable: "id".to_string(),
            value: Some("42".to_string()),
            success: true,
        }],
        error: None,
        time_ms: 7,
        step_results: vec![],
        diff_result: None,
        diff_results: Vec::new(),
    }
}

fn record_one(store: &SessionStore, alias: &str, source: &str) -> RecordedSession {
    let block = make_block("hc", "passed", 200);
    let now = Utc::now();
    let results = TestRunResults {
        passed: 1,
        failed: 0,
        skipped: 0,
        total_time_ms: 7,
        block_results: vec![block],
        final_variables: HashMap::new(),
        telemetry: None,
    };
    store
        .record_run(RecordInput {
            identity: FileIdentity::Alias(alias.into()),
            source_path: None,
            source_content: source,
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
        .expect("record_run should succeed")
}

#[test]
fn export_json_contains_run_id() {
    let dir = tempdir().unwrap();
    let store = SessionStore::open(dir.path()).unwrap();
    let recorded = record_one(&store, "alpha", "GET https://example.com/health\n");

    let mut buf: Vec<u8> = Vec::new();
    cmd_export::run_with_store(&store, &recorded.run_id, "json", None, &mut buf)
        .expect("export json should succeed");
    let out = String::from_utf8(buf).unwrap();

    assert!(
        out.contains(&recorded.run_id),
        "json export should mention run_id, got:\n{}",
        out
    );
    // Round-trip: must be valid JSON with run_id field.
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(v["run_id"].as_str(), Some(recorded.run_id.as_str()));
}

#[test]
fn export_md_contains_block_table() {
    let dir = tempdir().unwrap();
    let store = SessionStore::open(dir.path()).unwrap();
    let recorded = record_one(&store, "beta", "GET https://example.com/health\n");

    let mut buf: Vec<u8> = Vec::new();
    cmd_export::run_with_store(&store, &recorded.run_id, "md", None, &mut buf)
        .expect("export md should succeed");
    let out = String::from_utf8(buf).unwrap();

    assert!(out.contains("# Run "), "md should have header, got:\n{}", out);
    assert!(
        out.contains("## Block results"),
        "md should have block-results section, got:\n{}",
        out
    );
    assert!(
        out.contains("| # | Name | Status | Duration (ms) | HTTP |"),
        "md should contain block table header, got:\n{}",
        out
    );
    assert!(
        out.contains("hc"),
        "md should list the block by name, got:\n{}",
        out
    );
    assert!(
        out.contains("## Redaction report"),
        "md should include redaction report, got:\n{}",
        out
    );
    assert!(
        out.contains("## source.http"),
        "md should embed the source.http, got:\n{}",
        out
    );
}

#[test]
fn export_to_file_writes_disk() {
    let dir = tempdir().unwrap();
    let store = SessionStore::open(dir.path()).unwrap();
    let recorded = record_one(&store, "gamma", "GET https://example.com/health\n");

    let out_path = dir.path().join("nested").join("export.json");
    let mut sink = Cursor::new(Vec::<u8>::new());
    cmd_export::run_with_store(
        &store,
        &recorded.run_id,
        "json",
        Some(&out_path),
        &mut sink,
    )
    .expect("export-to-file should succeed");

    // Nothing went to the writer — the file got it.
    assert!(
        sink.into_inner().is_empty(),
        "writer must not be touched when --output is set"
    );
    let content = std::fs::read_to_string(&out_path).expect("output file written");
    assert!(
        content.contains(&recorded.run_id),
        "file content should mention run_id, got:\n{}",
        content
    );
}
