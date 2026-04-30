//! `request-pilot sessions export <run-id> [--format json|md] [-o <path>]`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

use request_pilot_core::sessions::SessionStore;
use request_pilot_core::test_runner::BlockResult;

use crate::find::{find_run_by_id, FoundRun};

/// Maximum number of bytes of any single request/response payload echoed
/// in the markdown export. Anything larger is truncated and the marker
/// `... [truncated]` is appended.
const MD_PAYLOAD_CAP: usize = 4 * 1024;

/// Resolve the run, render in the requested format, and emit it either to
/// `output` (when set) or to `writer`.
pub fn run_with_store(
    store: &SessionStore,
    run_id: &str,
    format: &str,
    output: Option<&PathBuf>,
    writer: &mut dyn Write,
) -> Result<()> {
    let run = find_run_by_id(store, run_id)?;

    let rendered = match format {
        "json" => serde_json::to_string_pretty(&run.record)
            .context("failed to serialize SessionRecord as JSON")?,
        "md" | "markdown" => render_md(&run),
        other => {
            return Err(anyhow!(
                "unsupported --format `{}` (expected `json` or `md`)",
                other
            ));
        }
    };

    match output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create output directory {}", parent.display())
                    })?;
                }
            }
            fs::write(path, rendered.as_bytes())
                .with_context(|| format!("failed to write export to {}", path.display()))?;
        }
        None => {
            writer.write_all(rendered.as_bytes())?;
            if !rendered.ends_with('\n') {
                writer.write_all(b"\n")?;
            }
        }
    }
    Ok(())
}

fn render_md(run: &FoundRun) -> String {
    use std::fmt::Write as _;

    let rec = &run.record;
    let mut out = String::new();

    let duration_ms = rec
        .finished_at
        .signed_duration_since(rec.started_at)
        .num_milliseconds()
        .max(0) as u64;

    let _ = writeln!(out, "# Run {}", rec.run_id);
    out.push('\n');
    let _ = writeln!(out, "- **File**: {}", md_escape(&run.file_alias));
    let _ = writeln!(out, "- **SHA-256**: `{}`", run.sha256);
    let _ = writeln!(out, "- **Started**: {}", rec.started_at.to_rfc3339());
    let _ = writeln!(out, "- **Finished**: {}", rec.finished_at.to_rfc3339());
    let _ = writeln!(
        out,
        "- **Duration**: {} ms (run total {} ms)",
        duration_ms, rec.results.total_time_ms
    );
    let _ = writeln!(
        out,
        "- **Results**: {} passed, {} failed, {} skipped",
        rec.results.passed, rec.results.failed, rec.results.skipped
    );
    out.push('\n');

    // ── Block results table ───────────────────────────────────────────
    out.push_str("## Block results\n\n");
    out.push_str("| # | Name | Status | Duration (ms) | HTTP |\n");
    out.push_str("|---|------|--------|---------------|------|\n");
    for (i, b) in rec.results.block_results.iter().enumerate() {
        let seq = b
            .seq
            .map(|n| n.to_string())
            .unwrap_or_else(|| (i + 1).to_string());
        let http = b
            .response
            .as_ref()
            .map(|r| r.status.to_string())
            .unwrap_or_else(|| "-".to_string());
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            seq,
            md_escape(&b.name),
            md_escape(&b.status),
            b.time_ms,
            http,
        );
    }
    out.push('\n');

    // ── Redaction report ──────────────────────────────────────────────
    let r = &rec.redaction_report;
    out.push_str("## Redaction report\n\n");
    let _ = writeln!(out, "- Headers redacted: {}", r.headers_redacted);
    let _ = writeln!(out, "- Query params redacted: {}", r.query_params_redacted);
    let _ = writeln!(out, "- Variables dropped: {}", r.variables_dropped);
    let _ = writeln!(out, "- Bodies dropped: {}", r.bodies_dropped);
    let _ = writeln!(out, "- Bodies truncated: {}", r.bodies_truncated);
    out.push('\n');

    // ── Per-block request/response ────────────────────────────────────
    out.push_str("## Block details\n\n");
    for (i, b) in rec.results.block_results.iter().enumerate() {
        let seq = b
            .seq
            .map(|n| n.to_string())
            .unwrap_or_else(|| (i + 1).to_string());
        let _ = writeln!(
            out,
            "### {}. {} ({})\n",
            seq,
            md_escape(&b.name),
            md_escape(&b.status)
        );
        out.push_str("#### Request\n\n");
        out.push_str("```http\n");
        out.push_str(&truncate(&format_request(b), MD_PAYLOAD_CAP));
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");

        out.push_str("#### Response\n\n");
        out.push_str("```http\n");
        out.push_str(&truncate(&format_response(b), MD_PAYLOAD_CAP));
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    // ── Raw source.http ───────────────────────────────────────────────
    out.push_str("## source.http\n\n");
    out.push_str("```http\n");
    out.push_str(&run.source);
    if !run.source.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("```\n");

    out
}

/// Minimal escape for markdown table cells: just escape pipes and backticks
/// so the table renders correctly. (Defined here to keep cmd_export
/// compiling; the dedicated export todo can replace this if it wants a
/// fancier implementation.)
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
        .replace('`', "\\`")
        .replace('\n', " ")
}

fn format_request(b: &BlockResult) -> String {
    let mut s = String::new();
    s.push_str(&format!("{} {}\n", b.request_method, b.request_url));
    for (k, v) in &b.request_headers {
        s.push_str(&format!("{}: {}\n", k, v));
    }
    if let Some(body) = &b.request_body {
        s.push('\n');
        s.push_str(body);
    }
    s
}

fn format_response(b: &BlockResult) -> String {
    let mut s = String::new();
    match &b.response {
        Some(r) => {
            s.push_str(&format!("HTTP/1.1 {} {}\n", r.status, r.status_text));
            for (k, v) in &r.headers {
                s.push_str(&format!("{}: {}\n", k, v));
            }
            s.push('\n');
            s.push_str(&r.body);
        }
        None => {
            if let Some(err) = &b.error {
                s.push_str("(no response)\n");
                s.push_str(err);
            } else {
                s.push_str("(no response)");
            }
        }
    }
    s
}

fn truncate(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    // Truncate on a UTF-8 char boundary.
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 32);
    out.push_str(&s[..end]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("... [truncated]");
    out
}
