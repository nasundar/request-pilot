mod app;
mod code_editor;
mod components;
mod events;
mod live_capture;
mod migrate;
mod sessions_tab;
mod toolbar;
#[cfg(test)]
mod tests;
mod ui;

use app::App;
use clap::Parser;
use std::path::PathBuf;
use simplelog::{WriteLogger, LevelFilter, ConfigBuilder};
use std::fs::File;

#[derive(Parser, Debug)]
#[command(
    name = "request-pilot",
    about = "Request Pilot — HTTP testing from the terminal",
    long_about = "Request Pilot — HTTP testing from the terminal\n\n\
        Load .http test files, attach environment variables, and run API tests \
        with an interactive TUI.\n\n\
        Examples:\n  \
        request-pilot --file tests/api.http\n  \
        request-pilot -f api.http -f auth.http --env .env\n  \
        request-pilot -f suite.http -e prod.env --run\n  \
        request-pilot migrate tests/*.http",
    after_help = "Use Ctrl+O to open files interactively, or pass them via --file."
)]
struct Cli {
    /// .http test file(s) to load on startup (repeatable)
    #[arg(short, long = "file", value_name = "FILE", num_args = 1)]
    files: Vec<PathBuf>,

    /// .env file to load variables from
    #[arg(short, long, value_name = "ENV")]
    env: Option<PathBuf>,

    /// Run all tests immediately after loading
    #[arg(short, long)]
    run: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Migrate a .http file from legacy Pilot syntax (# @x, @variables block, ### @type)
    /// to the new REST Client–compatible syntax (# @@x, top-level @name = value, ### @@type).
    ///
    /// By default prints a unified diff; pass --write to update files in place.
    Migrate {
        /// .http files to migrate
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Write changes to files in place (default: dry-run, show diff only)
        #[arg(short, long)]
        write: bool,
    },
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    // All log output goes to a file — never to stdout/stderr (would corrupt the TUI)
    let log_path = std::env::temp_dir().join("request-pilot.log");
    if let Ok(file) = File::create(&log_path) {
        let _ = WriteLogger::init(
            LevelFilter::Info,
            ConfigBuilder::new().set_time_format_rfc3339().build(),
            file,
        );
    }

    let cli = Cli::parse();

    // Handle `migrate` subcommand — runs headless (no TUI)
    if let Some(Command::Migrate { files, write }) = cli.command {
        return migrate::run(files, write);
    }

    let mut app = App::new();

    // Load .env file if provided
    if let Some(env_path) = &cli.env {
        app.load_env(env_path)?;
    }

    // Load .http files
    for path in &cli.files {
        app.load_file(path)?;
    }

    // Auto-run if requested
    if cli.run && !app.loaded_files.is_empty() {
        app.queue_run_all();
    }

    // Enter TUI
    let mut terminal = ui::init_terminal()?;

    // Animated splash screen
    ui::draw_splash(&mut terminal).await?;

    let result = app.run(terminal).await;
    ui::restore_terminal()?;
    result
}
