//! Integration tests for `request-pilot sessions prune`. Drives the
//! command-handler directly via the `request_pilot_cli` library so we can
//! point it at a tmp `SessionStore` without a full binary spawn.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Duration, Utc};

use request_pilot_cli::cmd_prune::{run_with_store, PruneArgs};
use request_pilot_core::sessions::{
    CapturePolicy, Component, FileIdentity, RecordInput, SessionStore, SessionTrigger,
};
use request_pilot_core::test_runner::TestRunResults;

fn tmp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std::process::id();
    let p = std::env::temp_dir().join(format!("rp-cli-prune-{}-{}-{}", label, pid, nanos));
    fs::create_dir_all(&p).unwrap();
    p
}

fn empty_results() -> TestRunResults {
    TestRunResults {
        passed: 0,
        failed: 0,
        skipped: 0,
        total_time_ms: 0,
        block_results: vec![],
        final_variables: HashMap::new(),
        telemetry: None,
    }
}

fn record_at(
    store: &SessionStore,
    alias: &str,
    src: &str,
    started_at: DateTime<Utc>,
) -> PathBuf {
    let input = RecordInput {
        identity: FileIdentity::Alias(alias.into()),
        source_path: None,
        source_content: src,
        policy: CapturePolicy::default(),
        trigger: SessionTrigger::Manual,
        component: Component::Cli,
        started_at,
        finished_at: started_at,
        mode: None,
        env_file: None,
        env_identity: None,
        variables: vec![],
        results: empty_results(),
        file_id_override: None,
        redact_body_rules: Vec::new(),
        block_redact_body_rules: std::collections::HashMap::new(),
    };
    let recorded = store.record_run(input).unwrap();

    // Force the on-disk `started_at` to match what we asked for so prune's
    // age-based logic sees a deterministic timestamp.
    let run_json = recorded.run_dir.join("run.json");
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(&run_json).unwrap()).unwrap();
    v["started_at"] = serde_json::Value::String(started_at.to_rfc3339());
    v["finished_at"] = serde_json::Value::String(started_at.to_rfc3339());
    fs::write(&run_json, serde_json::to_vec_pretty(&v).unwrap()).unwrap();

    recorded.run_dir
}

fn count_run_dirs(root: &PathBuf) -> usize {
    let files = root.join("files");
    if !files.exists() {
        return 0;
    }
    let mut n = 0;
    for f in fs::read_dir(&files).unwrap().flatten() {
        let versions = f.path().join("versions");
        if !versions.exists() {
            continue;
        }
        for v in fs::read_dir(&versions).unwrap().flatten() {
            let sessions = v.path().join("sessions");
            if !sessions.exists() {
                continue;
            }
            for s in fs::read_dir(&sessions).unwrap().flatten() {
                if s.file_type().unwrap().is_dir() {
                    n += 1;
                }
            }
        }
    }
    n
}

#[test]
fn prune_dry_run_does_not_delete_real_files() {
    let root = tmp_root("dryrun");
    let store = SessionStore::open(&root).unwrap();

    let now = Utc::now();
    let old = record_at(&store, "dry", "GET https://x\n", now - Duration::days(60));
    let recent = record_at(&store, "dry", "GET https://x\n", now - Duration::days(1));
    assert_eq!(count_run_dirs(&root), 2);

    let args = PruneArgs {
        older_than: Some("30d".into()),
        dry_run: true,
        force: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    run_with_store(&store, args, &mut buf).expect("dry-run should succeed");

    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("[DRY RUN]"), "unexpected output: {}", out);
    assert!(out.contains("would remove 1"), "unexpected output: {}", out);

    assert!(old.exists(), "old run dir was deleted by dry-run");
    assert!(recent.exists(), "recent run dir was deleted by dry-run");
    assert_eq!(count_run_dirs(&root), 2);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn prune_with_no_policy_errors() {
    let root = tmp_root("nopolicy");
    let store = SessionStore::open(&root).unwrap();

    let args = PruneArgs::default();
    let mut buf: Vec<u8> = Vec::new();
    let err = run_with_store(&store, args, &mut buf).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("--older-than") && msg.contains("--keep-last") && msg.contains("--max-size"),
        "expected error mentioning all three caps, got: {}",
        msg
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn prune_with_force_skips_prompt() {
    let root = tmp_root("force");
    let store = SessionStore::open(&root).unwrap();

    // Same alias + same source content => same (file_id, sha) version, so
    // `--keep-last 1` will leave exactly one of the three sessions on disk.
    let base = Utc::now() - Duration::hours(10);
    record_at(&store, "force", "GET https://x\n", base);
    record_at(&store, "force", "GET https://x\n", base + Duration::hours(1));
    record_at(&store, "force", "GET https://x\n", base + Duration::hours(2));
    assert_eq!(count_run_dirs(&root), 3);

    let args = PruneArgs {
        keep_last: Some(1),
        force: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    run_with_store(&store, args, &mut buf).expect("prune should succeed");

    let out = String::from_utf8(buf).unwrap();
    assert!(
        out.contains("Removed 2 sessions") && out.contains("(1 kept)"),
        "unexpected output: {}",
        out
    );
    assert_eq!(count_run_dirs(&root), 1);

    let _ = fs::remove_dir_all(&root);
}
