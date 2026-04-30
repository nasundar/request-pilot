//! `request-pilot sessions prune` — apply a retention policy to the on-disk
//! session store. The work happens in `request_pilot_core::sessions::SessionStore::prune`;
//! this module just translates CLI flags into a `RetentionPolicy`, handles
//! the interactive confirmation prompt, and prints the resulting report.

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{anyhow, bail, Context, Result};

use request_pilot_core::sessions::SessionStore;
use request_pilot_core::sessions_config::RetentionPolicy;

/// Parsed CLI inputs for `sessions prune`.
#[derive(Debug, Default, Clone)]
pub struct PruneArgs {
    /// Drop sessions older than this duration (e.g. `30d`, `12h`, `2weeks`).
    pub older_than: Option<String>,
    /// Keep at most N most-recent sessions per `(file_id, sha)`.
    pub keep_last: Option<u32>,
    /// Cap total store size. Suffixes: `gb` (default), `mb`.
    pub max_size: Option<String>,
    /// Compute what would be removed without touching the disk.
    pub dry_run: bool,
    /// Skip the interactive confirmation prompt (for CI / unattended runs).
    pub force: bool,
}

/// Apply the retention policy described by `args` to `store`, writing a
/// human-readable summary to `writer`. When stdin is a TTY and neither
/// `--dry-run` nor `--force` is set, prompts the user for confirmation
/// before deleting anything.
pub fn run_with_store<W: Write>(
    store: &SessionStore,
    args: PruneArgs,
    writer: &mut W,
) -> Result<()> {
    let policy = build_policy(&args)?;

    if !args.dry_run && !args.force && io::stdin().is_terminal() {
        write!(
            writer,
            "About to delete sessions matching: {}. Continue? [y/N] ",
            describe_policy(&policy)
        )?;
        writer.flush().ok();
        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let trimmed = input.trim();
        if !(trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")) {
            writeln!(writer, "Aborted.")?;
            return Ok(());
        }
    }

    let report = store
        .prune(&policy, args.dry_run)
        .map_err(|e| anyhow!("prune failed: {}", e))?;

    let mb = (report.bytes_freed as f64) / (1024.0 * 1024.0);
    if report.dry_run {
        writeln!(
            writer,
            "[DRY RUN] would remove {} sessions, {:.1} MB freed ({} kept)",
            report.removed_count, mb, report.kept_count
        )?;
    } else {
        writeln!(
            writer,
            "Removed {} sessions, {:.1} MB freed ({} kept)",
            report.removed_count, mb, report.kept_count
        )?;
    }
    Ok(())
}

fn build_policy(args: &PruneArgs) -> Result<RetentionPolicy> {
    let max_age_days = args
        .older_than
        .as_deref()
        .map(parse_older_than)
        .transpose()?;
    let max_total_size_gb = args.max_size.as_deref().map(parse_max_size).transpose()?;
    let max_sessions_per_version = args.keep_last;

    if max_age_days.is_none()
        && max_total_size_gb.is_none()
        && max_sessions_per_version.is_none()
    {
        bail!(
            "no retention cap specified: pass at least one of \
             --older-than, --keep-last, or --max-size"
        );
    }

    Ok(RetentionPolicy {
        max_age_days,
        max_sessions_per_version,
        max_total_size_gb,
    })
}

/// Parse a `--older-than` value into whole days. Sub-day inputs (e.g. `12h`)
/// are rounded to the nearest day; anything that rounds to 0 is rejected.
fn parse_older_than(s: &str) -> Result<u32> {
    let dur = humantime::parse_duration(s)
        .with_context(|| format!("invalid --older-than value: {}", s))?;
    let secs = dur.as_secs();
    if secs == 0 {
        bail!("--older-than must be greater than 0");
    }
    let days = (secs as f64) / 86_400.0;
    let rounded = days.round() as i64;
    if rounded < 1 {
        bail!(
            "--older-than must be at least 1 day (got {}, ~{:.2} days)",
            s,
            days
        );
    }
    Ok(rounded as u32)
}

/// Parse a `--max-size` value into GB. Suffixes recognised (case-insensitive):
/// `gb` / `g` (default) and `mb` / `m`.
fn parse_max_size(s: &str) -> Result<f64> {
    let lower = s.trim().to_ascii_lowercase();
    let (num_str, multiplier_gb) = if let Some(stripped) = lower.strip_suffix("gb") {
        (stripped.trim().to_string(), 1.0_f64)
    } else if let Some(stripped) = lower.strip_suffix("mb") {
        (stripped.trim().to_string(), 1.0_f64 / 1024.0)
    } else if let Some(stripped) = lower.strip_suffix('g') {
        (stripped.trim().to_string(), 1.0)
    } else if let Some(stripped) = lower.strip_suffix('m') {
        (stripped.trim().to_string(), 1.0 / 1024.0)
    } else {
        (lower.clone(), 1.0)
    };

    let val: f64 = num_str
        .parse()
        .with_context(|| format!("invalid --max-size value: {}", s))?;
    if !(val > 0.0) {
        bail!("--max-size must be greater than 0 (got {})", s);
    }
    Ok(val * multiplier_gb)
}

fn describe_policy(p: &RetentionPolicy) -> String {
    let mut parts = Vec::new();
    if let Some(d) = p.max_age_days {
        parts.push(format!("older than {} day(s)", d));
    }
    if let Some(n) = p.max_sessions_per_version {
        parts.push(format!("more than {} per version", n));
    }
    if let Some(g) = p.max_total_size_gb {
        parts.push(format!("over {:.2} GB total", g));
    }
    if parts.is_empty() {
        "<none>".to_string()
    } else {
        parts.join(" / ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_older_than_days() {
        assert_eq!(parse_older_than("30d").unwrap(), 30);
        assert_eq!(parse_older_than("1day").unwrap(), 1);
    }

    #[test]
    fn parse_older_than_rounds_hours() {
        // 36h rounds to 2 days.
        assert_eq!(parse_older_than("36h").unwrap(), 2);
        // 24h is exactly 1 day.
        assert_eq!(parse_older_than("24h").unwrap(), 1);
    }

    #[test]
    fn parse_older_than_rejects_sub_day() {
        assert!(parse_older_than("1h").is_err());
        assert!(parse_older_than("11h").is_err());
    }

    #[test]
    fn parse_max_size_units() {
        assert!((parse_max_size("5gb").unwrap() - 5.0).abs() < 1e-9);
        assert!((parse_max_size("5GB").unwrap() - 5.0).abs() < 1e-9);
        assert!((parse_max_size("5").unwrap() - 5.0).abs() < 1e-9);
        assert!((parse_max_size("512mb").unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn parse_max_size_rejects_zero_and_garbage() {
        assert!(parse_max_size("0gb").is_err());
        assert!(parse_max_size("xx").is_err());
    }

    #[test]
    fn build_policy_requires_one_cap() {
        let err = build_policy(&PruneArgs::default()).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("--older-than"));
    }
}
