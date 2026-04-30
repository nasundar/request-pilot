//! `sessions show <run-id>` — pretty-print a single recorded run, or dump
//! its raw `SessionRecord` JSON when `--json` is set.

use std::io::Write;

use anyhow::{Context, Result};

use request_pilot_core::sessions::{SessionRecord, SessionStore};

use crate::find::find_run_by_id;
use crate::store::open_store;

#[derive(Debug, Default, Clone)]
pub struct ShowArgs {
    pub run_id: String,
    pub json: bool,
}

pub fn run(args: ShowArgs) -> Result<()> {
    let (_cfg, store) = open_store()?;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    run_with_store(&store, &args, &mut handle)
}

pub fn run_with_store<W: Write>(
    store: &SessionStore,
    args: &ShowArgs,
    out: &mut W,
) -> Result<()> {
    let found = find_run_by_id(store, &args.run_id)?;

    if args.json {
        let s = serde_json::to_string_pretty(&found.record)
            .context("failed to serialize SessionRecord as JSON")?;
        writeln!(out, "{}", s)?;
        return Ok(());
    }

    pretty_print(&found.file_id, &found.sha256, &found.record, out)
}

fn pretty_print<W: Write>(
    file_id: &str,
    sha: &str,
    rec: &SessionRecord,
    out: &mut W,
) -> Result<()> {
    let duration_ms = (rec.finished_at - rec.started_at)
        .num_milliseconds()
        .max(0) as u64;

    writeln!(out, "Run:       {}", rec.run_id)?;
    writeln!(out, "File:      {}", file_id)?;
    writeln!(out, "SHA:       {}", sha)?;
    writeln!(
        out,
        "Started:   {}",
        rec.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    )?;
    writeln!(out, "Duration:  {} ms (run total: {} ms)", duration_ms, rec.results.total_time_ms)?;
    writeln!(
        out,
        "Trigger:   {:?}    Component: {:?}",
        rec.trigger, rec.component
    )?;
    if let Some(mode) = &rec.mode {
        writeln!(out, "Mode:      {}", mode)?;
    }
    if let Some(env) = &rec.env_file {
        writeln!(out, "Env file:  {}", env)?;
    }
    writeln!(
        out,
        "Totals:    {} passed, {} failed, {} skipped",
        rec.results.passed, rec.results.failed, rec.results.skipped
    )?;
    writeln!(out)?;

    // Per-block table.
    writeln!(out, "Blocks:")?;
    writeln!(
        out,
        "  {:<4}  {:<32}  {:<8}  {:>8}  {:>10}  {:>10}",
        "SEQ", "NAME", "STATUS", "TIME", "ASSERT", "EXTRACT"
    )?;
    for b in &rec.block_summaries {
        writeln!(
            out,
            "  {:<4}  {:<32}  {:<8}  {:>6}ms  {:>4}/{:<5}  {:>4}/{:<5}",
            b.seq.map(|s| s.to_string()).unwrap_or_else(|| "-".into()),
            truncate(&b.name, 32),
            b.status,
            b.time_ms,
            b.assertion_passed,
            b.assertion_total,
            b.extract_ok,
            b.extract_total,
        )?;
    }
    writeln!(out)?;

    // Redaction report.
    let r = &rec.redaction_report;
    writeln!(out, "Redaction report:")?;
    writeln!(out, "  headers redacted:      {}", r.headers_redacted)?;
    writeln!(out, "  query params redacted: {}", r.query_params_redacted)?;
    writeln!(out, "  variables dropped:     {}", r.variables_dropped)?;
    writeln!(out, "  bodies dropped:        {}", r.bodies_dropped)?;
    writeln!(out, "  bodies truncated:      {}", r.bodies_truncated)?;
    writeln!(out)?;

    // Capture policy.
    writeln!(out, "Capture policy:")?;
    let policy_json = serde_json::to_string_pretty(&rec.capture_policy)
        .unwrap_or_else(|_| "<unprintable>".into());
    for line in policy_json.lines() {
        writeln!(out, "  {}", line)?;
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}
