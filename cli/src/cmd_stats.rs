//! `sessions stats <file-id> [--sha <sha>]` — render markdown stats from
//! the stats rollup, or dump raw JSON when `--json` is set.

use std::io::Write;

use anyhow::{anyhow, Context, Result};

use request_pilot_core::sessions::{render_stats_md, SessionStore};

use crate::store::open_store;

#[derive(Debug, Default, Clone)]
pub struct StatsArgs {
    pub file_id: String,
    pub sha: Option<String>,
    pub json: bool,
}

pub fn run(args: StatsArgs) -> Result<()> {
    let (_cfg, store) = open_store()?;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    run_with_store(&store, &args, &mut handle)
}

pub fn run_with_store<W: Write>(
    store: &SessionStore,
    args: &StatsArgs,
    out: &mut W,
) -> Result<()> {
    let stats = store
        .load_stats(&args.file_id, args.sha.as_deref())
        .with_context(|| format!("failed to load stats for {}", args.file_id))?
        .ok_or_else(|| {
            anyhow!(
                "no stats for file `{}`{}",
                args.file_id,
                args.sha
                    .as_ref()
                    .map(|s| format!(" @ {}", s))
                    .unwrap_or_default()
            )
        })?;

    if args.json {
        let s = serde_json::to_string_pretty(&stats)
            .context("failed to serialize StatsRecord as JSON")?;
        writeln!(out, "{}", s)?;
    } else {
        write!(out, "{}", render_stats_md(&stats))?;
    }
    Ok(())
}
