use anyhow::Result;
use clap::{Parser, Subcommand};

mod cmd_export;
mod cmd_list;
mod cmd_prune;
mod cmd_replay;
mod cmd_show;
mod cmd_stats;
mod find;
mod store;

#[derive(Parser)]
#[command(name = "request-pilot", version, about = "Request Pilot CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage recorded test sessions
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
}

#[derive(Subcommand)]
enum SessionsCommand {
    /// List recorded sessions
    List {
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        since: Option<String>,
    },
    /// Show details of one run
    Show {
        run_id: String,
        /// Dump the raw `SessionRecord` as JSON for scripting.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Replay a recorded run
    Replay {
        run_id: String,
        #[arg(long)]
        block: Option<String>,
        #[arg(long)]
        env: Option<std::path::PathBuf>,
    },
    /// Export a run as JSON or Markdown
    Export {
        run_id: String,
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },
    /// Show stats for a file
    Stats {
        file_id: String,
        /// Per-version stats for this sha. Defaults to the file-level rollup.
        #[arg(long)]
        sha: Option<String>,
        /// Dump the raw `StatsRecord` as JSON for scripting.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Prune old sessions
    Prune {
        /// Drop sessions older than this duration (e.g. `30d`, `12h`).
        #[arg(long)]
        older_than: Option<String>,
        /// Keep only the N most-recent sessions per file/version.
        #[arg(long)]
        keep_last: Option<u32>,
        /// Cap total store size (suffixes: `gb` default, `mb`).
        #[arg(long)]
        max_size: Option<String>,
        /// Show what would be removed without touching the disk.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Skip the confirmation prompt (for unattended / CI runs).
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sessions { command } => match command {
            SessionsCommand::List { file, since } => {
                cmd_list::run(cmd_list::ListArgs { file, since })?;
            }
            SessionsCommand::Show { run_id, json } => {
                cmd_show::run(cmd_show::ShowArgs { run_id, json })?;
            }
            SessionsCommand::Replay {
                run_id,
                block,
                env,
            } => {
                let (config, store) = store::open_store()?;
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                cmd_replay::run_with_store(
                    &store,
                    &config,
                    &run_id,
                    block.as_deref(),
                    env.as_deref(),
                    &mut handle,
                )
                .await?;
            }
            SessionsCommand::Export {
                run_id,
                format,
                output,
            } => {
                let (_, store) = store::open_store()?;
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                cmd_export::run_with_store(
                    &store,
                    &run_id,
                    &format,
                    output.as_ref(),
                    &mut handle,
                )?;
            }
            SessionsCommand::Stats { file_id, sha, json } => {
                cmd_stats::run(cmd_stats::StatsArgs { file_id, sha, json })?;
            }
            SessionsCommand::Prune {
                older_than,
                keep_last,
                max_size,
                dry_run,
                force,
            } => {
                let (_, store) = store::open_store()?;
                let args = cmd_prune::PruneArgs {
                    older_than,
                    keep_last,
                    max_size,
                    dry_run,
                    force,
                };
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                cmd_prune::run_with_store(&store, args, &mut handle)?;
            }
        },
    }
    Ok(())
}

