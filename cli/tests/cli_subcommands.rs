//! Integration tests for `sessions list`, `sessions show`, and `sessions stats`.
//!
//! Drives the subcommand handlers directly against a real `SessionStore`
//! rooted in a temp directory so we don't need to override the global
//! `SessionsConfig` path.

use std::collections::HashMap;

use chrono::Utc;
use tempfile::TempDir;

use request_pilot_core::assertions::AssertionResult;
use request_pilot_core::http_client::HttpResponse;
use request_pilot_core::sessions::{
    CapturePolicy, Component, FileIdentity, RecordInput, SessionStore, SessionTrigger,
};
use request_pilot_core::test_runner::{BlockResult, ExtractResult, TestRunResults};

use request_pilot_cli::cmd_list::{run_with_store as run_list, ListArgs};
use request_pilot_cli::cmd_show::{run_with_store as run_show, ShowArgs};
use request_pilot_cli::cmd_stats::{run_with_store as run_stats, StatsArgs};

// ─── Fixtures ────────────────────────────────────────────────────────────

fn dummy_block(name: &str, status: &str) -> BlockResult {
    BlockResult {
        seq: Some(1),
        name: name.to_string(),
        block_type: "test".into(),
        group: None,
        request_method: "GET".into(),
        request_url: "https://example.com/api".into(),
        request_headers: vec![("Authorization".into(), "Bearer secret".into())],
        request_body: None,
        status: status.to_string(),
        response: Some(HttpResponse {
            status: 200,
            status_text: "OK".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: "{\"ok\":true}".into(),
            time_ms: 12,
            size_bytes: 11,
        }),
        assertion_results: vec![AssertionResult {
            assertion: "status == 200".into(),
            expected: Some("200".into()),
            actual: Some("200".into()),
            passed: true,
        }],
        extract_results: vec![ExtractResult {
            variable: "id".into(),
            value: Some("42".into()),
            success: true,
        }],
        error: None,
        time_ms: 12,
        step_results: vec![],
        diff_result: None,
        diff_results: Vec::new(),
        iterations: Vec::new(),
    }
}

fn dummy_results(blocks: Vec<BlockResult>) -> TestRunResults {
    let passed = blocks.iter().filter(|b| b.status == "passed").count();
    let failed = blocks.iter().filter(|b| b.status == "failed").count();
    let skipped = blocks.iter().filter(|b| b.status == "skipped").count();
    TestRunResults {
        passed,
        failed,
        skipped,
        total_time_ms: 12 * blocks.len() as u64,
        block_results: blocks,
        final_variables: HashMap::new(),
        telemetry: None,
    }
}

fn record_input<'a>(
    identity: FileIdentity,
    source: &'a str,
    results: TestRunResults,
) -> RecordInput<'a> {
    let now = Utc::now();
    RecordInput {
        identity,
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
    }
}

fn fresh_store() -> (TempDir, SessionStore) {
    let tmp = TempDir::new().expect("tempdir");
    let store = SessionStore::open(tmp.path()).expect("open store");
    (tmp, store)
}

// ─── list ─────────────────────────────────────────────────────────────────

#[test]
fn list_emits_header_when_empty() {
    let (_tmp, store) = fresh_store();
    let mut buf = Vec::<u8>::new();
    run_list(&store, &ListArgs::default(), &mut buf).unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("RUN_ID"), "missing RUN_ID header in:\n{}", s);
    assert!(s.contains("STATUS"), "missing STATUS header in:\n{}", s);
    assert!(s.contains("(no sessions recorded)"), "expected empty marker in:\n{}", s);
}

#[test]
fn list_shows_recorded_run() {
    let (_tmp, store) = fresh_store();
    let id = FileIdentity::Alias("checkout".into());
    let src = "GET https://example.com/health\n";
    let results = dummy_results(vec![dummy_block("hc", "passed")]);
    let recorded = store.record_run(record_input(id, src, results)).unwrap();

    let mut buf = Vec::<u8>::new();
    run_list(&store, &ListArgs::default(), &mut buf).unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains(&recorded.run_id), "list output missing run id:\n{}", s);
    assert!(s.contains("PASSED"), "expected PASSED status in:\n{}", s);
    let sha8: String = recorded.sha256.chars().take(8).collect();
    assert!(s.contains(&sha8), "expected truncated sha {} in:\n{}", sha8, s);
}

#[test]
fn list_filters_by_file_id() {
    let (_tmp, store) = fresh_store();
    let id_a = FileIdentity::Alias("alpha".into());
    let id_b = FileIdentity::Alias("beta".into());
    let recorded_a = store
        .record_run(record_input(
            id_a,
            "GET https://a/\n",
            dummy_results(vec![dummy_block("a", "passed")]),
        ))
        .unwrap();
    let recorded_b = store
        .record_run(record_input(
            id_b,
            "GET https://b/\n",
            dummy_results(vec![dummy_block("b", "passed")]),
        ))
        .unwrap();

    let args = ListArgs {
        file: Some(recorded_a.file_id.clone()),
        since: None,
    };
    let mut buf = Vec::<u8>::new();
    run_list(&store, &args, &mut buf).unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains(&recorded_a.run_id));
    assert!(!s.contains(&recorded_b.run_id), "filter should hide other file");
}

// ─── show ────────────────────────────────────────────────────────────────

#[test]
fn show_pretty_prints_run() {
    let (_tmp, store) = fresh_store();
    let id = FileIdentity::Alias("checkout".into());
    let src = "GET https://example.com/health\n";
    let recorded = store
        .record_run(record_input(
            id,
            src,
            dummy_results(vec![dummy_block("hc", "passed")]),
        ))
        .unwrap();

    let mut buf = Vec::<u8>::new();
    run_show(
        &store,
        &ShowArgs {
            run_id: recorded.run_id.clone(),
            json: false,
        },
        &mut buf,
    )
    .unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains(&recorded.run_id), "missing run id in pretty output");
    assert!(s.contains("Blocks:"), "missing blocks header");
    assert!(s.contains("Redaction report:"), "missing redaction report");
    assert!(s.contains("Capture policy:"), "missing capture policy");
}

#[test]
fn show_json_dumps_session_record() {
    let (_tmp, store) = fresh_store();
    let id = FileIdentity::Alias("checkout".into());
    let recorded = store
        .record_run(record_input(
            id,
            "GET https://x/\n",
            dummy_results(vec![dummy_block("hc", "passed")]),
        ))
        .unwrap();

    let mut buf = Vec::<u8>::new();
    run_show(
        &store,
        &ShowArgs {
            run_id: recorded.run_id.clone(),
            json: true,
        },
        &mut buf,
    )
    .unwrap();
    let s = String::from_utf8(buf).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(s.trim()).expect("show --json must emit valid JSON");
    assert_eq!(
        v.get("run_id").and_then(|r| r.as_str()),
        Some(recorded.run_id.as_str())
    );
}

#[test]
fn show_unknown_run_id_errors() {
    let (_tmp, store) = fresh_store();
    let mut buf = Vec::<u8>::new();
    let err = run_show(
        &store,
        &ShowArgs {
            run_id: "does-not-exist".into(),
            json: false,
        },
        &mut buf,
    )
    .unwrap_err();
    assert!(
        format!("{}", err).contains("not found"),
        "unexpected error: {err}"
    );
}

// ─── stats ───────────────────────────────────────────────────────────────

#[test]
fn stats_renders_markdown_for_recorded_file() {
    let (_tmp, store) = fresh_store();
    let id = FileIdentity::Alias("checkout".into());
    let recorded = store
        .record_run(record_input(
            id,
            "GET https://example.com/health\n",
            dummy_results(vec![dummy_block("hc", "passed")]),
        ))
        .unwrap();

    // Trigger a stats fold so files/<file>/stats.json exists.
    let (_src, rec) = store
        .load_session(&recorded.file_id, &recorded.sha256, &recorded.run_id)
        .unwrap();
    store
        .update_stats(&recorded.file_id, &recorded.sha256, &rec)
        .unwrap();

    let mut buf = Vec::<u8>::new();
    run_stats(
        &store,
        &StatsArgs {
            file_id: recorded.file_id.clone(),
            sha: None,
            json: false,
        },
        &mut buf,
    )
    .unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(!s.is_empty(), "stats markdown should be non-empty");
}

#[test]
fn stats_json_emits_valid_json() {
    let (_tmp, store) = fresh_store();
    let id = FileIdentity::Alias("checkout".into());
    let recorded = store
        .record_run(record_input(
            id,
            "GET https://example.com/health\n",
            dummy_results(vec![dummy_block("hc", "passed")]),
        ))
        .unwrap();
    let (_src, rec) = store
        .load_session(&recorded.file_id, &recorded.sha256, &recorded.run_id)
        .unwrap();
    store
        .update_stats(&recorded.file_id, &recorded.sha256, &rec)
        .unwrap();

    let mut buf = Vec::<u8>::new();
    run_stats(
        &store,
        &StatsArgs {
            file_id: recorded.file_id.clone(),
            sha: None,
            json: true,
        },
        &mut buf,
    )
    .unwrap();
    let s = String::from_utf8(buf).unwrap();
    let v: serde_json::Value = serde_json::from_str(s.trim()).expect("valid json");
    assert_eq!(
        v.get("file_id").and_then(|x| x.as_str()),
        Some(recorded.file_id.as_str())
    );
}

#[test]
fn stats_missing_file_errors() {
    let (_tmp, store) = fresh_store();
    let mut buf = Vec::<u8>::new();
    let err = run_stats(
        &store,
        &StatsArgs {
            file_id: "no-such-file".into(),
            sha: None,
            json: false,
        },
        &mut buf,
    )
    .unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("no stats"), "unexpected error: {msg}");
}
