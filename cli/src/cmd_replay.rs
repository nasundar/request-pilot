//! `request-pilot sessions replay <run-id>` — locate a recorded run, parse
//! the captured `source.http`, optionally filter to a single block and/or
//! merge an env file, re-execute via the core test runner, and record the
//! new run beside the original by reusing its `file_id`.

use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;

use request_pilot_core::env_file::read_env_from_path;
use request_pilot_core::http_parser::TestSuite;
use request_pilot_core::sessions::{
    build_body_redact_rules, Component, FileIdentity, RecordInput, SessionStore, SessionTrigger,
};
use request_pilot_core::sessions_config::SessionsConfig;
use request_pilot_core::test_runner::run_suite_with_headers;

use crate::find::find_run_by_id;

/// Return a `TestSuite` containing only the block whose `name` matches.
/// Errors when no block in the suite has that name.
pub fn filter_suite_to_block(suite: &TestSuite, block_name: &str) -> Result<TestSuite> {
    let block = suite
        .blocks
        .iter()
        .find(|b| b.name == block_name)
        .ok_or_else(|| anyhow!("no block named `{}` in recorded suite", block_name))?
        .clone();
    let mut filtered = suite.clone();
    filtered.blocks = vec![block];
    Ok(filtered)
}

/// Run the replay end-to-end. Pulls the source from `store`, optionally
/// filters to one block, optionally merges an env file's KEY=VALUE entries
/// into the suite-level variables, executes via the core runner, and
/// records the new run with `file_id_override = Some(original_file_id)`.
pub async fn run_with_store<W: Write>(
    store: &SessionStore,
    config: &SessionsConfig,
    run_id: &str,
    block_name: Option<&str>,
    env_path: Option<&Path>,
    writer: &mut W,
) -> Result<()> {
    let found = find_run_by_id(store, run_id)?;
    let file_id = found.file_id.clone();
    let sha = found.sha256.clone();
    let (source, suite, _orig) = store
        .load_as_snapshot(&file_id, &sha, &found.run_id)
        .map_err(|e| anyhow!("failed to load snapshot: {}", e))?;

    let suite = match block_name {
        Some(name) => filter_suite_to_block(&suite, name)?,
        None => suite,
    };

    let mut extra_vars: Vec<(String, String)> = Vec::new();
    let mut env_file_label: Option<String> = None;
    if let Some(p) = env_path {
        let path_str = p.to_string_lossy().to_string();
        let map = read_env_from_path(&path_str)
            .map_err(|e| anyhow!("failed to read env file {}: {}", path_str, e))?;
        extra_vars.extend(map.into_iter());
        env_file_label = Some(path_str);
    }

    let started_at = Utc::now();
    let results = run_suite_with_headers(&suite, &extra_vars, &[], None, Some("replay")).await;
    let finished_at = Utc::now();

    let passed = results.passed;
    let failed = results.failed;
    let skipped = results.skipped;

    let identity = FileIdentity::from_path_and_alias(None, None, "cli-replay");
    let (suite_redact_rules, block_redact_rules) =
        build_body_redact_rules(&suite, &config.body_redaction_paths);
    let input = RecordInput {
        identity,
        source_path: None,
        source_content: &source,
        policy: config.capture_policy.clone(),
        trigger: SessionTrigger::Manual,
        component: Component::Cli,
        started_at,
        finished_at,
        mode: Some("replay".to_string()),
        env_file: env_file_label,
        env_identity: None,
        variables: extra_vars,
        results,
        file_id_override: Some(file_id.clone()),
        redact_body_rules: suite_redact_rules,
        block_redact_body_rules: block_redact_rules,
    };
    let recorded = store
        .record_run(input)
        .with_context(|| format!("recording replay run for {}", file_id))?;

    writeln!(
        writer,
        "Replay complete: passed={} failed={} skipped={}",
        passed, failed, skipped
    )?;
    writeln!(
        writer,
        "New run: {} (file_id={}, sha={})",
        recorded.run_id, recorded.file_id, recorded.sha256
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use request_pilot_core::http_parser::{TestBlock, TestSuite};
    use request_pilot_core::sessions::{CapturePolicy, RecordInput};
    use request_pilot_core::test_runner::TestRunResults;
    use std::collections::HashMap;

    fn block(name: &str) -> TestBlock {
        // Construct via JSON to remain robust against TestBlock field changes.
        let json = serde_json::json!({
            "block_type": "test",
            "name": name,
            "description": "",
            "disabled": false,
            "mode": null,
            "dev_auth": null,
            "group": null,
            "depends": [],
            "request": {
                "name": null,
                "method": "GET",
                "url": "http://127.0.0.1:0/",
                "headers": [],
                "body": null
            },
            "assertions": [],
            "extracts": []
        });
        serde_json::from_value(json).expect("synthetic TestBlock should deserialize")
    }

    #[test]
    fn filter_suite_to_block_returns_only_named_block() {
        let mut suite = TestSuite::default();
        suite.blocks = vec![block("alpha"), block("beta"), block("gamma")];
        let filtered = filter_suite_to_block(&suite, "beta").expect("beta exists");
        assert_eq!(filtered.blocks.len(), 1);
        assert_eq!(filtered.blocks[0].name, "beta");
    }

    #[test]
    fn filter_suite_to_block_errors_on_missing() {
        let mut suite = TestSuite::default();
        suite.blocks = vec![block("alpha")];
        let err = filter_suite_to_block(&suite, "ghost").unwrap_err();
        assert!(
            err.to_string().contains("ghost"),
            "error should mention missing block name, got: {}",
            err
        );
    }

    #[test]
    fn find_run_by_id_locates_recorded_run() {
        // Record a synthetic run, then ensure find_run_by_id returns the
        // same (file_id, sha256). No network is exercised — we record the
        // run directly via SessionStore.
        let tmp = tempdir_unique();
        let store = SessionStore::open(&tmp).expect("open store");
        let identity = FileIdentity::Alias("replay-locate".into());
        let started = Utc::now();
        let results = TestRunResults {
            passed: 0,
            failed: 0,
            skipped: 0,
            total_time_ms: 0,
            block_results: vec![],
            final_variables: HashMap::new(),
            telemetry: None,
        };
        let input = RecordInput {
            identity,
            source_path: None,
            source_content: "GET http://example.com\n",
            policy: CapturePolicy::default(),
            trigger: SessionTrigger::Manual,
            component: Component::Cli,
            started_at: started,
            finished_at: started,
            mode: None,
            env_file: None,
            env_identity: None,
            variables: vec![],
            results,
            file_id_override: None,
            redact_body_rules: Vec::new(),
            block_redact_body_rules: std::collections::HashMap::new(),
        };
        let recorded = store.record_run(input).expect("record_run");
        let found = find_run_by_id(&store, &recorded.run_id).expect("find");
        assert_eq!(found.file_id, recorded.file_id);
        assert_eq!(found.sha256, recorded.sha256);
        assert_eq!(found.record.run_id, recorded.run_id);

        let missing = find_run_by_id(&store, "no-such-run");
        assert!(missing.is_err(), "no-such-run should not be found");
        assert!(missing.err().unwrap().to_string().contains("not found"));
    }

    fn tempdir_unique() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!("rp-cli-replay-test-{}-{}", std::process::id(), nanos));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    // NOTE: A full end-to-end "replay records under original file_id" test
    // would require either a live HTTP endpoint or a mock server (httpmock,
    // wiremock). Both add a heavy dev-dependency for marginal coverage in
    // this CLI crate. The behaviour is exercised by:
    //   * `find_run_by_id_locates_recorded_run` (locator logic)
    //   * `filter_suite_to_block_returns_only_named_block` (filter logic)
    //   * `core::sessions` tests covering `file_id_override` round-trip
    // so we intentionally skip the network-dependent integration test here.
}
