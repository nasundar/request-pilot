//! `sessions list` — table of recorded runs, optionally filtered by file
//! and/or recency. Designed to be grep-friendly: plain ASCII, no colors.

use std::io::Write;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};

use request_pilot_core::sessions::{SessionStore, SessionSummary};

use crate::store::open_store;

#[derive(Debug, Default, Clone)]
pub struct ListArgs {
    pub file: Option<String>,
    pub since: Option<String>,
}

#[derive(Debug, Clone)]
struct Row {
    run_id: String,
    file_id: String,
    sha: String,
    started_at: DateTime<Utc>,
    duration_ms: u64,
    passed: usize,
    failed: usize,
    skipped: usize,
}

impl Row {
    fn status(&self) -> &'static str {
        if self.failed > 0 {
            "FAILED"
        } else if self.passed > 0 {
            "PASSED"
        } else if self.skipped > 0 {
            "SKIPPED"
        } else {
            "EMPTY"
        }
    }
}

pub fn run(args: ListArgs) -> Result<()> {
    let (_cfg, store) = open_store()?;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    run_with_store(&store, &args, &mut handle)
}

pub fn run_with_store<W: Write>(
    store: &SessionStore,
    args: &ListArgs,
    out: &mut W,
) -> Result<()> {
    let cutoff: Option<DateTime<Utc>> = match args.since.as_deref() {
        Some(s) => {
            let dur: Duration = humantime::parse_duration(s)
                .map_err(|e| anyhow!("invalid --since `{}`: {}", s, e))?;
            let chrono_dur = chrono::Duration::from_std(dur)
                .map_err(|e| anyhow!("--since out of range: {}", e))?;
            Some(Utc::now() - chrono_dur)
        }
        None => None,
    };

    let files = store.list_files().context("failed to list files")?;
    let mut rows: Vec<Row> = Vec::new();

    for f in &files {
        if let Some(filter) = &args.file {
            if &f.file_id != filter {
                continue;
            }
        }
        let versions = store
            .list_versions(&f.file_id)
            .with_context(|| format!("failed to list versions for {}", f.file_id))?;
        for v in &versions {
            let sessions: Vec<SessionSummary> = store
                .list_sessions(&f.file_id, &v.sha256)
                .with_context(|| {
                    format!("failed to list sessions for {} @ {}", f.file_id, v.sha256)
                })?;
            for s in sessions {
                if let Some(cut) = cutoff {
                    if s.started_at < cut {
                        continue;
                    }
                }
                rows.push(Row {
                    run_id: s.run_id,
                    file_id: f.file_id.clone(),
                    sha: v.sha256.clone(),
                    started_at: s.started_at,
                    duration_ms: s.total_time_ms,
                    passed: s.passed,
                    failed: s.failed,
                    skipped: s.skipped,
                });
            }
        }
    }

    rows.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    // Header. Fixed widths chosen to be readable but tight; run_id and
    // file_id are intentionally not truncated so they remain greppable.
    writeln!(
        out,
        "{:<38}  {:<28}  {:<10}  {:<19}  {:>10}  {:<10}  {}",
        "RUN_ID", "FILE", "SHA", "STARTED", "DURATION", "P/F/S", "STATUS"
    )?;
    for r in &rows {
        let sha_short: String = r.sha.chars().take(8).collect();
        let started = r.started_at.format("%Y-%m-%d %H:%M:%S").to_string();
        let duration = format_duration(r.duration_ms);
        let pfs = format!("{}/{}/{}", r.passed, r.failed, r.skipped);
        writeln!(
            out,
            "{:<38}  {:<28}  {:<10}  {:<19}  {:>10}  {:<10}  {}",
            r.run_id,
            truncate(&r.file_id, 28),
            sha_short,
            started,
            duration,
            pfs,
            r.status()
        )?;
    }

    if rows.is_empty() {
        writeln!(out, "(no sessions recorded)")?;
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

fn format_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.2}s", ms as f64 / 1_000.0)
    } else {
        let secs = ms / 1_000;
        format!("{}m{}s", secs / 60, secs % 60)
    }
}
